import { useEffect, useState } from "react";
import { apiRequest, errorMessage, readStored, type Participant } from "./api";
import { deviceLabel, useAudioDevices } from "./useAudioDevices";
import { useLiveAudio } from "./useLiveAudio";
import { guestMonitor } from "./guestMonitor";

const ENDED = ["rejected", "expired", "timed_out"];

export default function GuestApp({ invitationToken }: { invitationToken: string }) {
  const storageKey = `cloud-console-guest-${invitationToken}`;
  const [name, setName] = useState("");
  const [participant, setParticipant] = useState<Participant | null>(() => readStored<Participant>(sessionStorage, storageKey));
  const [status, setStatus] = useState(participant?.status ?? "");
  const [inStudio, setInStudio] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!participant) return;
    let active = true;
    const check = () => {
      void apiRequest<{ status: string }>(`/guest-session/${participant.session_id}`, {
        headers: { Authorization: `Bearer ${participant.access_token}` },
      })
        .then((response) => {
          if (!active) return;
          setStatus(response.status);
          if (ENDED.includes(response.status)) setInStudio(false);
        })
        .catch((cause: unknown) => {
          if (!active) return;
          const message = errorMessage(cause);
          if (message === "unauthorized") {
            setStatus("expired");
            setInStudio(false);
          } else {
            setError(message);
          }
        });
    };
    check();
    const timer = window.setInterval(check, 2500);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [participant]);

  async function requestToJoin(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const response = await apiRequest<Participant>(`/guest-links/${encodeURIComponent(invitationToken)}/sessions`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      setParticipant(response);
      setStatus(response.status ?? "pending");
      sessionStorage.setItem(storageKey, JSON.stringify(response));
    } catch (cause) {
      const message = errorMessage(cause);
      if (message === "unauthorized") {
        setStatus("expired");
        setError("This invitation is expired or unavailable. Ask the host for a new link.");
      } else {
        setError(message);
      }
    } finally {
      setBusy(false);
    }
  }

  function requestAgain() {
    setParticipant(null);
    setStatus("");
    setInStudio(false);
    sessionStorage.removeItem(storageKey);
  }

  return (
    <main className="console guest">
      <header className="bar">
        <strong className="brand">Cloud Console</strong>
        <span className="chip chip-live">Guest</span>
      </header>

      {inStudio && participant && !ENDED.includes(status) ? (
        <GuestStudio
          participant={participant}
          onLeave={() => {
            setInStudio(false);
            setStatus("disconnected");
          }}
        />
      ) : status === "expired" || status === "timed_out" ? (
        <section className="card">
          <h2>{status === "timed_out" ? "Request timed out" : "Invitation expired"}</h2>
          <p className="muted">Ask the host for a new invitation link.</p>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      ) : !participant ? (
        <section className="card">
          <h2>Request to join</h2>
          <form onSubmit={requestToJoin}>
            <label>
              <span className="label">Your name</span>
              <input value={name} onChange={(event) => setName(event.target.value)} maxLength={100} autoComplete="name" required />
            </label>
            <button type="submit" className="btn btn-build" disabled={busy}>{busy ? "Sending…" : "Connect"}</button>
          </form>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      ) : status === "pending" ? (
        <section className="card">
          <h2>Waiting for the host</h2>
          <p className="muted">Your request is pending. You can connect after approval.</p>
        </section>
      ) : status === "rejected" ? (
        <section className="card">
          <h2>Connection rejected</h2>
          <button type="button" className="btn" onClick={requestAgain}>Request access again</button>
        </section>
      ) : (
        <section className="card">
          <h2>{status === "disconnected" ? "You may reconnect" : "You are approved"}</h2>
          <button type="button" className="btn btn-build" onClick={() => setInStudio(true)}>Join the live audio</button>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      )}
    </main>
  );
}

function GuestStudio({ participant, onLeave }: { participant: Participant; onLeave: () => void }) {
  const devices = useAudioDevices();
  const [inputId, setInputId] = useState("");
  const [outputId, setOutputId] = useState("");
  const [micMuted, setMicMuted] = useState(false);
  const [monitorMuted, setMonitorMuted] = useState(false);
  const [volume, setVolume] = useState(0.8);
  const [guestSource, setGuestSource] = useState<string | null>(null);
  const [monitorError, setMonitorError] = useState("");
  useEffect(() => {
    let active = true;
    let sequence = 0;
    let applied = 0;
    const load = () => {
      const request = ++sequence;
      void apiRequest<{ guest_source: string }>("/guest-monitor", {
        headers: { Authorization: `Bearer ${participant.access_token}` },
      }).then(({ guest_source }) => {
        if (!active || request <= applied) return;
        applied = request;
        if (!guestMonitor(guest_source).available) {
          setGuestSource(null);
          setMonitorError("The selected guest monitor source is unavailable. Ask the host to select Program, Mix-minus, or Cue.");
          return;
        }
        setGuestSource(guest_source);
        setMonitorError("");
      }).catch((cause: unknown) => {
        if (active && request > applied) {
          applied = request;
          setMonitorError(`Could not refresh guest monitor: ${errorMessage(cause)}`);
        }
      });
    };
    load();
    const timer = window.setInterval(load, 2000);
    return () => { active = false; window.clearInterval(timer); };
  }, [participant.access_token]);
  const selectedMonitor = guestMonitor(guestSource);
  const live = useLiveAudio({
    participant,
    mode: "live",
    inputDeviceId: inputId,
    outputDeviceId: outputId,
    monitorBus: selectedMonitor.bus,
    monitorVolume: volume,
    monitorMuted: monitorMuted || !selectedMonitor.available,
    micMuted,
    sidetone: selectedMonitor.sidetone,
  });

  return (
    <section className="card">
      <div className="row">
        <h2>{live.connected ? "You are connected" : live.status}</h2>
        <span className={`chip ${live.connected ? "chip-live" : ""}`}>{live.connected ? "Live" : "…"}</span>
      </div>
      <div className="grid2">
        <button type="button" className={`btn ${micMuted ? "btn-on" : ""}`} aria-pressed={micMuted} onClick={() => setMicMuted(!micMuted)}>
          {micMuted ? "Unmute my mic" : "Mute my mic"}
        </button>
        <button type="button" className={`btn ${monitorMuted ? "btn-on" : ""}`} aria-pressed={monitorMuted} onClick={() => setMonitorMuted(!monitorMuted)}>
          {monitorMuted ? "Unmute monitor" : "Mute monitor"}
        </button>
      </div>
      <label>
        <span className="label">Monitor volume</span>
        <input type="range" min="0" max="1" step="0.05" value={volume} onChange={(event) => setVolume(Number(event.target.value))} />
      </label>
      <div className="grid2">
        <label>
          <span className="label">Input device</span>
          <select value={inputId} onChange={(event) => setInputId(event.target.value)}>
            <option value="">System default</option>
            {devices.inputs.map((device, index) => (
              <option key={device.deviceId} value={device.deviceId}>{deviceLabel(device, index)}</option>
            ))}
          </select>
        </label>
        <label>
          <span className="label">Output device</span>
          <select value={outputId} disabled={!devices.canSelectOutput} onChange={(event) => setOutputId(event.target.value)}>
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
      {live.error && <p className="error" role="alert">{live.error}</p>}
      {monitorError && <p className="error" role="alert">{monitorError}</p>}
      <button type="button" className="btn" onClick={onLeave}>Leave</button>
    </section>
  );
}
