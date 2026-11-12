import { useEffect, useMemo, useRef, useState } from "react";
import DelayControls from "./DelayControls";

declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;
import { deviceLabel, useAudioDevices } from "./useAudioDevices";
import { useLiveAudio } from "./useLiveAudio";
import {
  apiRequest,
  errorMessage,
  readStored,
  roleLabel,
  useMixerChannels,
  usePolling,
  type AudioMetrics,
  type GuestLink,
  type LiveSession,
  type MixerChannel,
  type MonitorSettings,
  type Participant,
} from "./api";

const PARTICIPANT_KEY = "cloud-console-live-session";
const PREFS_KEY = "cloud-console-local-prefs";

let creating: Promise<Participant> | null = null;

type Prefs = { name: string; inputDeviceId: string; outputDeviceId: string; active: boolean };

function dbText(values: number[] | null | undefined): string {
  const peak = values && values.length > 0 ? Math.max(...values) : null;
  return peak === null || peak < -100 ? "—" : `${peak.toFixed(0)} dB`;
}

function dbToPercent(db: number | undefined | null): number {
  if (db === undefined || db === null || !Number.isFinite(db)) return 0;
  return Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
}

function Meter({ values, label }: { values: number[] | null | undefined; label?: string }) {
  const peak = values && values.length > 0 ? Math.max(...values) : null;
  const percent = dbToPercent(peak);
  return (
    <div className="meter" role="img" aria-label={`${label ?? "Level"} ${peak === null ? "no signal" : `${peak.toFixed(0)} dBFS`}`}>
      <div className={`meter-fill ${percent > 92 ? "hot" : percent > 75 ? "warm" : ""}`} style={{ height: `${percent}%` }} />
    </div>
  );
}

export default function Console({ apiKey, onSignOut }: { apiKey: string; onSignOut: () => void }) {
  const operator = useMemo<HeadersInit>(() => ({ "X-API-Key": apiKey }), [apiKey]);
  const json = useMemo<HeadersInit>(() => ({ ...operator, "Content-Type": "application/json" }), [operator]);
  const { channels } = useMixerChannels(apiKey);
  const metrics = usePolling<AudioMetrics>("/audio/metrics", apiKey, 400);
  const monitorPoll = usePolling<MonitorSettings>("/monitor", apiKey, 5000);
  const sessions = usePolling<LiveSession[]>("/live-sessions", apiKey, 3000);
  const links = usePolling<GuestLink[]>("/guest-links", apiKey, 5000);
  const devices = useAudioDevices();
  const station = usePolling<{ name: string }>("/station/config", apiKey, 30000);

  const [prefs, setPrefs] = useState<Prefs>(
    () => readStored<Prefs>(localStorage, PREFS_KEY) ?? { name: "Operator", inputDeviceId: "", outputDeviceId: "", active: false },
  );
  const [participant, setParticipant] = useState<Participant | null>(() => readStored<Participant>(sessionStorage, PARTICIPANT_KEY));
  const [micMuted, setMicMuted] = useState(false);
  const [monitor, setMonitor] = useState<MonitorSettings | null>(null);
  const [localChoice, setLocalChoice] = useState("program_mix");
  const [guestChoice, setGuestChoice] = useState("program_mix");
  const [error, setError] = useState("");
  const [newLink, setNewLink] = useState("");
  const pendingMix = useRef(new Map<string, Partial<Pick<MixerChannel, "fader" | "is_on" | "cue_enabled" | "muted">>>());
  const sendingMix = useRef(new Set<string>());

  useEffect(() => {
    localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
  }, [prefs]);

  useEffect(() => {
    const data = monitorPoll.data;
    if (data && !monitor) {
      setMonitor(data);
      setLocalChoice(data.local_source === "input" && data.local_input_id ? `input:${data.local_input_id}` : data.local_source);
      setGuestChoice(data.guest_source === "input" && data.guest_input_id ? `input:${data.guest_input_id}` : data.guest_source);
    }
  }, [monitorPoll.data, monitor]);

  // Make sure the local user has a host session so the monitor feed can be heard.
  useEffect(() => {
    let cancelled = false;
    async function ensure() {
      if (participant) {
        try {
          await apiRequest("/auth/validate", { headers: { Authorization: `Bearer ${participant.access_token}` } });
          return;
        } catch {
          sessionStorage.removeItem(PARTICIPANT_KEY);
        }
      }
      try {
        creating ??= apiRequest<Participant>("/live-sessions", {
          method: "POST",
          headers: json,
          body: JSON.stringify({ display_name: prefs.name.trim() || "Operator", role: "host" }),
        }).finally(() => {
          creating = null;
        });
        const created = await creating;
        if (cancelled) return;
        sessionStorage.setItem(PARTICIPANT_KEY, JSON.stringify(created));
        setParticipant(created);
      } catch (cause) {
        if (!cancelled) setError(errorMessage(cause));
      }
    }
    void ensure();
    return () => {
      cancelled = true;
    };
  }, [participant?.session_id]);

  // A dropped, expired or rejected host session can never reconnect; start a fresh one.
  const ownStatus = sessions.data?.find((item) => item.id === participant?.session_id)?.status;
  useEffect(() => {
    if (ownStatus && !["approved", "connected", "disconnected"].includes(ownStatus)) {
      sessionStorage.removeItem(PARTICIPANT_KEY);
      setParticipant(null);
    }
  }, [ownStatus]);

  // Keep the shared session name in step with this browser's configured name.
  useEffect(() => {
    if (!participant) return;
    const name = prefs.name.trim() || "Operator";
    const timer = window.setTimeout(() => {
      void apiRequest(`/live-sessions/${participant.session_id}`, {
        method: "PATCH",
        headers: json,
        body: JSON.stringify({ display_name: name }),
      }).catch(() => undefined);
    }, 500);
    return () => window.clearTimeout(timer);
  }, [participant?.session_id, prefs.name]);

  const localMonitorBus = localChoice === "cue" ? "cue" : "program";
  const live = useLiveAudio({
    participant,
    mode: participant ? (prefs.active ? "live" : "listen") : "off",
    inputDeviceId: prefs.inputDeviceId,
    outputDeviceId: prefs.outputDeviceId,
    monitorBus: localMonitorBus,
    monitorVolume: monitor?.local_volume ?? 0.8,
    monitorMuted: monitor?.local_muted ?? false,
    micMuted,
    sidetone: localChoice === "program_mix",
  });

  const ownId = participant?.session_id;
  const sources = channels.filter((channel) => channel.role === "automation" || channel.role === "aux");
  const connectedIds = new Set((sessions.data ?? []).filter((item) => item.status === "connected").map((item) => item.id));
  const sessionNames = new Map((sessions.data ?? []).map((item) => [item.id, item.display_name]));
  const mics = channels
    .filter((channel) => channel.role !== "automation" && channel.role !== "aux" && channel.channel_id !== ownId && connectedIds.has(channel.channel_id))
    .map((channel) => ({ ...channel, display_name: sessionNames.get(channel.channel_id) ?? channel.display_name }));
  const ownChannel = channels.find((channel) => channel.channel_id === ownId);
  const metricFor = (id: string) => metrics.data?.sources.find((source) => source.id === id);

  async function patchChannel(id: string, body: Partial<Pick<MixerChannel, "fader" | "is_on" | "cue_enabled" | "muted">>) {
    pendingMix.current.set(id, { ...pendingMix.current.get(id), ...body });
    if (sendingMix.current.has(id)) return;
    sendingMix.current.add(id);
    try {
      while (pendingMix.current.has(id)) {
        const next = pendingMix.current.get(id);
        pendingMix.current.delete(id);
        try {
          await apiRequest(`/mixer/channels/${id}`, { method: "PATCH", headers: json, body: JSON.stringify(next) });
        } catch (cause) {
          setError(`Mixer update failed: ${errorMessage(cause)}`);
        }
      }
    } finally {
      sendingMix.current.delete(id);
    }
  }

  async function saveMonitor(patch: Record<string, unknown>) {
    try {
      setMonitor(await apiRequest<MonitorSettings>("/monitor", { method: "PUT", headers: json, body: JSON.stringify(patch) }));
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  function monitorPatch(prefix: "local" | "guest", choice: string) {
    if (choice.startsWith("input:")) return { [`${prefix}_source`]: "input", [`${prefix}_input_id`]: choice.slice(6) };
    return { [`${prefix}_source`]: choice };
  }

  async function session(id: string, action: "approve" | "reject" | "disconnect") {
    try {
      await apiRequest(`/live-sessions/${id}/${action}`, { method: "POST", headers: operator });
      sessions.refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function createLink() {
    try {
      const expires = new Date(Date.now() + 8 * 3600 * 1000).toISOString();
      const link = await apiRequest<GuestLink>("/guest-links", {
        method: "POST",
        headers: json,
        body: JSON.stringify({ name: null, expires_at: expires }),
      });
      if (link.invitation_token) {
        const url = `${window.location.origin}/?invite=${encodeURIComponent(link.invitation_token)}`;
        setNewLink(url);
        await navigator.clipboard.writeText(url).catch(() => undefined);
      }
      links.refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function invalidate(id: string) {
    try {
      await apiRequest(`/guest-links/${id}`, { method: "DELETE", headers: operator });
      links.refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  const pending = (sessions.data ?? []).filter((item) => item.role === "guest" && item.status === "pending");
  const activeLinks = (links.data ?? []).filter((link) => link.status === "active");
  const programMeter = metrics.data?.program_peak_dbfs ?? null;

  return (
    <main className="console">
      <header className="bar">
        <strong className="brand">{station.data?.name || "Cloud Console"}</strong>
        <span className={`chip ${metrics.data?.pipeline_state === "playing" ? "chip-live" : ""}`}>
          {metrics.data ? (metrics.data.pipeline_state === "playing" ? "On-Air" : metrics.data.pipeline_state) : metrics.error ? "engine offline" : "…"}
        </span>
        <button type="button" className="btn btn-small" onClick={onSignOut}>Sign out</button>
      </header>

      {error && (
        <p className="error" role="alert" onClick={() => setError("")}>{error}</p>
      )}

      <section className="card program-card" aria-label="Program level">
        <span className="label">Program</span>
        <div className="hmeter">
          <div className="hmeter-fill" style={{ width: `${dbToPercent(programMeter ? Math.max(...programMeter) : null)}%` }} />
        </div>
      </section>
      <div className="live-inputs">
        <DelayControls apiKey={apiKey} />
        <section className="strips" aria-label="Input faders">
          {sources.length === 0 && <p className="muted pad">No audio inputs configured. Add one in Station admin.</p>}
          {sources.map((channel) => (
            <Strip
              key={channel.channel_id}
              channel={channel}
              peak={metricFor(channel.channel_id)?.peak_dbfs}
              state={metricFor(channel.channel_id)?.state}
              onChange={(body) => void patchChannel(channel.channel_id, body)}
            />
          ))}
        </section>
      </div>

      <section className="card mics" aria-label="Connected microphones">
        {pending.map((guest) => (
          <div key={guest.id} className="row">
            <span>{guest.display_name} <span className="chip">waiting</span></span>
            <span className="row-actions">
              <button type="button" className="btn btn-small btn-build" onClick={() => void session(guest.id, "approve")}>Approve</button>
              <button type="button" className="btn btn-small" onClick={() => void session(guest.id, "reject")}>Reject</button>
            </span>
          </div>
        ))}
        <div className="strips inner">
          {prefs.active && ownChannel && (
            <Strip
              channel={{ ...ownChannel, display_name: `${prefs.name || "Me"} (you)` }}
              peak={ownChannel.channel_id in live.levels ? [live.levels[ownChannel.channel_id]] : null}
              onChange={(body) => void patchChannel(ownChannel.channel_id, body)}
            />
          )}
          {mics.map((channel) => (
            <Strip
              key={channel.channel_id}
              channel={channel}
              peak={channel.channel_id in live.levels ? [live.levels[channel.channel_id]] : null}
              onChange={(body) => void patchChannel(channel.channel_id, body)}
              onDisconnect={() => void session(channel.channel_id, "disconnect")}
            />
          ))}
        </div>
        {mics.length === 0 && !(prefs.active && ownChannel) && pending.length === 0 && (
          <p className="muted">No microphones connected.</p>
        )}
      </section>

      <section className="card" aria-label="Monitor">
        <h2>Monitor</h2>
        <div className="grid2">
          <label>
            <span className="label">My monitor</span>
            <select
              value={localChoice}
              onChange={(event) => {
                setLocalChoice(event.target.value);
                void saveMonitor(monitorPatch("local", event.target.value));
              }}
            >
              <SourceOptions sources={sources} />
            </select>
          </label>
          <label>
            <span className="label">Guest monitor</span>
            <select
              value={guestChoice}
              onChange={(event) => {
                setGuestChoice(event.target.value);
                void saveMonitor(monitorPatch("guest", event.target.value));
              }}
            >
              <SourceOptions sources={sources} />
            </select>
          </label>
        </div>
        <div className="row">
          <input
            type="range"
            min="0"
            max="1"
            step="0.05"
            aria-label="Monitor volume"
            value={monitor?.local_volume ?? 0.8}
            onChange={(event) => setMonitor((current) => current && { ...current, local_volume: Number(event.target.value) })}
            onPointerUp={() => void saveMonitor({ local_volume: monitor?.local_volume ?? 0.8 })}
            onKeyUp={() => void saveMonitor({ local_volume: monitor?.local_volume ?? 0.8 })}
          />
          <button
            type="button"
            className={`btn btn-small ${monitor?.local_muted ? "btn-on" : ""}`}
            aria-pressed={monitor?.local_muted ?? false}
            onClick={() => void saveMonitor({ local_muted: !(monitor?.local_muted ?? false) })}
          >
            {monitor?.local_muted ? "Muted" : "Mute"}
          </button>
        </div>
      </section>

      <section className="card" aria-label="Local user">
        <div className="my-mic">
          <label className="switch" title="Activate your microphone on the console">
            <input
              type="checkbox"
              checked={prefs.active}
              onChange={(event) => setPrefs({ ...prefs, active: event.target.checked })}
            />
            <span className="slider" />
            <span>{prefs.active ? "My mic active" : "Activate my mic"}</span>
          </label>
          <span className="muted small my-mic-status" title={prefs.active ? live.status : "Not active: your microphone is not sent."}>
            {prefs.active ? live.status : "Listening only"}
          </span>
          {prefs.active && (
            <button type="button" className={`btn btn-small ${micMuted ? "btn-on" : ""}`} onClick={() => setMicMuted(!micMuted)}>
              {micMuted ? "Unmute" : "Mute"}
            </button>
          )}
        </div>
        {live.error && <p className="error" role="alert">{live.error}</p>}
      </section>

      <details className="card settings" aria-label="Settings">
        <summary>Settings and guests</summary>
        <div className="grid2">
          <label>
            <span className="label">Display name</span>
            <input value={prefs.name} maxLength={100} onChange={(event) => setPrefs({ ...prefs, name: event.target.value })} />
          </label>
          <span />
          <label>
            <span className="label">Input device</span>
            <select value={prefs.inputDeviceId} onChange={(event) => setPrefs({ ...prefs, inputDeviceId: event.target.value })}>
              <option value="">System default</option>
              {devices.inputs.map((device, index) => (
                <option key={device.deviceId} value={device.deviceId}>{deviceLabel(device, index)}</option>
              ))}
            </select>
          </label>
          <label>
            <span className="label">Output device</span>
            <select
              value={prefs.outputDeviceId}
              disabled={!devices.canSelectOutput}
              onChange={(event) => setPrefs({ ...prefs, outputDeviceId: event.target.value })}
            >
              <option value="">System default</option>
              {devices.outputs.map((device, index) => (
                <option key={device.deviceId} value={device.deviceId}>{deviceLabel(device, index)}</option>
              ))}
            </select>
          </label>
        </div>
        {devices.needsPermission && (
          <button type="button" className="btn btn-small" onClick={() => void devices.requestPermission()}>
            Allow microphone access to list devices
          </button>
        )}
        {devices.error && <p className="error">{devices.error}</p>}

        <h3>Guest links</h3>
        <button type="button" className="btn btn-small" onClick={() => void createLink()}>New guest link (8 h)</button>
        {newLink && <input readOnly className="mono" value={newLink} onFocus={(event) => event.currentTarget.select()} />}
        {activeLinks.map((link) => (
          <div key={link.id} className="row">
            <span className="small">{link.name ?? "Guest link"} · expires {new Date(link.expires_at).toLocaleString()}</span>
            <button type="button" className="btn btn-small" onClick={() => void invalidate(link.id)}>Invalidate</button>
          </div>
        ))}
      </details>

      <details className="card stats" aria-label="Audio stream statistics">
        <summary>Audio stream statistics</summary>
        {metrics.error && <p className="error">{metrics.error}</p>}
        <table>
          <thead>
            <tr><th>Stream</th><th>State</th><th>Level</th><th>Jitter</th><th>Loss</th></tr>
          </thead>
          <tbody>
            {(metrics.data?.sources ?? []).map((source) => (
              <tr key={source.id}>
                <td>{channels.find((channel) => channel.channel_id === source.id)?.display_name ?? source.id.slice(0, 8)}</td>
                <td>{source.state}</td>
                <td>{dbText(source.peak_dbfs)}</td>
                <td>{source.jitter_ms != null ? `${source.jitter_ms.toFixed(1)} ms` : "—"}</td>
                <td>{source.packets_lost ?? "—"}</td>
              </tr>
            ))}
            <tr>
              <td>Program out</td>
              <td>{metrics.data?.output_count ?? 0} output(s)</td>
              <td>{dbText(programMeter)}</td>
              <td colSpan={2}>latency {metrics.data?.pipeline_latency_ms != null && metrics.data.pipeline_latency_ms < 10000 ? `${metrics.data.pipeline_latency_ms} ms` : "—"}</td>
            </tr>
          </tbody>
        </table>
        {metrics.data?.last_pipeline_error && <p className="error">{metrics.data.last_pipeline_error}</p>}
      </details>

      <footer className="foot">
        <a href="/admin">Station admin · audio routing</a>
        <a href="/api/docs">API</a>
        <span className="version">v{APP_VERSION}</span>
      </footer>
    </main>
  );
}

function SourceOptions({ sources }: { sources: MixerChannel[] }) {
  return (
    <>
      <option value="program_mix">Program mix</option>
      <option value="mix_minus">Mix-minus (Program − my mic)</option>
      <option value="cue">Cue</option>
      {sources.map((source) => (
        <option key={source.channel_id} value={`input:${source.channel_id}`}>{source.display_name}</option>
      ))}
    </>
  );
}

function Strip({
  channel,
  peak,
  state,
  onChange,
  onDisconnect,
}: {
  channel: MixerChannel;
  peak?: number[] | null;
  state?: string;
  onChange: (body: Partial<Pick<MixerChannel, "fader" | "is_on" | "cue_enabled" | "muted">>) => void;
  onDisconnect?: () => void;
}) {
  const [fader, setFader] = useState(channel.fader);
  const [dragging, setDragging] = useState(false);
  useEffect(() => {
    if (!dragging) setFader(channel.fader);
  }, [channel.fader, dragging]);
  const finish = () => {
    setDragging(false);
  };
  const isMic = channel.role !== "automation" && channel.role !== "aux";
  return (
    <div className={`strip ${channel.is_on && !channel.muted ? "strip-on" : ""}`}>
      <span className="strip-name" title={channel.display_name}>{channel.display_name}</span>
      <span className="small muted">{isMic ? roleLabel(channel.role) : state ?? "—"}</span>
      <div className="strip-body">
        <Meter values={peak} label={channel.display_name} />
        <input
          type="range"
          className="fader"
          min="0"
          max="2"
          step="0.02"
          value={fader}
          aria-label={`${channel.display_name} fader`}
          onChange={(event) => {
            setDragging(true);
            const value = Number(event.target.value);
            setFader(value);
            onChange({ fader: value });
          }}
          onPointerUp={finish}
          onPointerCancel={finish}
          onKeyUp={finish}
          onBlur={finish}
        />
      </div>
      <button
        type="button"
        className={`btn btn-small ${channel.is_on && !channel.muted ? "btn-on-air" : ""}`}
        aria-pressed={channel.is_on}
        onClick={() => onChange({ is_on: !channel.is_on, muted: false })}
      >
        {channel.is_on && !channel.muted ? "ON" : "OFF"}
      </button>
      <button
        type="button"
        className={`btn btn-small ${channel.cue_enabled ? "btn-cue" : ""}`}
        aria-pressed={channel.cue_enabled}
        onClick={() => onChange({ cue_enabled: !channel.cue_enabled })}
      >
        CUE
      </button>
      {onDisconnect && (
        <button type="button" className="btn btn-small" onClick={onDisconnect}>Drop</button>
      )}
    </div>
  );
}
