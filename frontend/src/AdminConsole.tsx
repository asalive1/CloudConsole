import { useEffect, useState, type FormEvent } from "react";
import {
  apiRequest,
  errorMessage,
  STATION_KEY,
  usePolling,
  type AudioMetrics,
  type DelayStatus,
  type GuestLink,
} from "./api";

type Resource = {
  id: string;
  name: string;
  type?: string;
  direction?: string;
  format?: string;
  enabled: boolean;
  config: Record<string, unknown>;
};
type StationConfig = {
  id: string;
  name: string;
  inputs: Resource[];
  outputs: Resource[];
  metadata: Resource[];
};
type Tab = "automation" | "inputs" | "outputs" | "metadata" | "delay" | "station" | "guests" | "routing";
type Cfg = Record<string, unknown>;
type FieldSpec = {
  key: string;
  label: string;
  kind: "text" | "number" | "password" | "select";
  options?: string[];
  placeholder?: string;
  help?: string;
  source?: "nic" | "alsa";
};
type SystemDevices = {
  network_interfaces: Array<{ name: string; state: string; addresses: string[] }>;
  default_audio_address?: string | null;
  default_metadata_address?: string | null;
  audio_listener_ports?: string | null;
  metadata_listener_ports?: string | null;
  alsa_inputs: Array<{ device: string; name: string; description: string }>;
  alsa_outputs: Array<{ device: string; name: string; description: string }>;
};

const TABS: Array<{ id: Tab; label: string }> = [
  { id: "automation", label: "Automation" },
  { id: "inputs", label: "Inputs" },
  { id: "outputs", label: "Outputs" },
  { id: "metadata", label: "Metadata" },
  { id: "delay", label: "Delay" },
  { id: "routing", label: "Routing" },
  { id: "station", label: "Station" },
  { id: "guests", label: "Guest links" },
];

const SAMPLE_RATES = ["44100", "48000", "96000"];
const CHANNELS = ["1", "2"];

function fieldsFor(direction: "input" | "output", type: string): FieldSpec[] {
  if (type === "alsa") {
    return [
      { key: "device", label: "Server ALSA device", kind: "text", placeholder: "hw:0,0", source: "alsa" },
      { key: "sample_rate", label: "Sample rate", kind: "select", options: SAMPLE_RATES },
      { key: "channels", label: "Channels", kind: "select", options: CHANNELS },
      { key: "bit_depth", label: "Bit depth", kind: "select", options: ["16", "24", "32"] },
    ];
  }
  if (type === "srt") {
    const common: FieldSpec[] = [
      { key: "mode", label: "SRT mode", kind: "select", options: ["listener", "caller"], help: "Listener waits for a connection; caller connects out." },
      { key: "host", label: direction === "input" ? "Host (caller target or bind address)" : "Destination host", kind: "text", placeholder: direction === "input" ? "0.0.0.0" : "srt.example.com" },
      { key: "port", label: "Port", kind: "number", placeholder: "9000" },
      { key: "interface", label: "Network interface", kind: "text", source: "nic", help: "Interface used for the SRT connection." },
      { key: "latency_ms", label: "Latency (ms)", kind: "number", placeholder: "120" },
      { key: "stream_id", label: "Stream ID", kind: "text" },
      { key: "passphrase", label: "Passphrase", kind: "password", help: "10–79 characters; leave empty for no encryption." },
    ];
    if (direction === "output") {
      common.push({ key: "pbkeylen", label: "Key length", kind: "select", options: ["", "16", "24", "32"] });
      common.push({ key: "bitrate", label: "AAC bitrate (bps)", kind: "number", placeholder: "192000" });
    }
    return common;
  }
  if (type === "rtp") {
    if (direction === "input") {
      return [
        { key: "address", label: "Address (multicast group or bind)", kind: "text", placeholder: "239.0.0.1" },
        { key: "port", label: "Port", kind: "number", placeholder: "5004" },
        { key: "interface", label: "Network interface", kind: "text", source: "nic" },
        { key: "encoding", label: "Encoding", kind: "select", options: ["l24", "l16"] },
        { key: "channels", label: "Channels", kind: "select", options: CHANNELS },
        { key: "payload_type", label: "Payload type", kind: "number", placeholder: "96" },
        { key: "jitter_buffer_ms", label: "Jitter buffer (ms)", kind: "number", placeholder: "50" },
      ];
    }
    return [
      { key: "host", label: "Destination (unicast or multicast)", kind: "text", placeholder: "239.0.0.2" },
      { key: "port", label: "Port", kind: "number", placeholder: "5004" },
      { key: "interface", label: "Network interface", kind: "text", source: "nic" },
      { key: "encoding", label: "Encoding", kind: "select", options: ["l24", "l16"] },
      { key: "channels", label: "Channels", kind: "select", options: CHANNELS },
      { key: "payload_type", label: "Payload type", kind: "number", placeholder: "96" },
    ];
  }
  if (type === "test") {
    return [{ key: "frequency", label: "Tone frequency (Hz)", kind: "number", placeholder: "1000" }];
  }
  return [];
}

function legacyInterface(config: Cfg): Cfg {
  if (config.multicast_interface && !config.interface) config.interface = config.multicast_interface;
  delete config.multicast_interface;
  return config;
}

function defaultsFor(direction: "input" | "output", type: string): Cfg {
  if (type === "alsa") return { sample_rate: 48000, channels: 2, bit_depth: 24 };
  if (type === "srt") {
    return direction === "input"
      ? { mode: "listener", host: "0.0.0.0", port: 9000, latency_ms: 120 }
      : { mode: "caller", port: 9000, latency_ms: 120, bitrate: 192000 };
  }
  if (type === "rtp") {
    return direction === "input"
      ? { address: "239.0.0.1", port: 5004, encoding: "l24", channels: 2, payload_type: 96, jitter_buffer_ms: 50 }
      : { port: 5004, encoding: "l24", channels: 2, payload_type: 96 };
  }
  return type === "test" ? { frequency: 1000 } : {};
}

export default function AdminConsole() {
  const [apiKey, setApiKey] = useState(() => sessionStorage.getItem(STATION_KEY) ?? "");
  const [authenticated, setAuthenticated] = useState(false);
  const [config, setConfig] = useState<StationConfig | null>(null);
  const [tab, setTab] = useState<Tab>(() => (window.location.hash.slice(1) as Tab) || "automation");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const headers = (extra?: Record<string, string>): HeadersInit => ({ "X-API-Key": apiKey, ...extra });
  const json = { "Content-Type": "application/json" };

  async function load() {
    setConfig(await apiRequest<StationConfig>("/station/config", { headers: headers() }));
  }

  async function signIn(key: string) {
    setBusy(true);
    try {
      await apiRequest("/auth/login", { method: "POST", headers: { "X-API-Key": key } });
      sessionStorage.setItem(STATION_KEY, key);
      setAuthenticated(true);
      setError("");
      setConfig(await apiRequest<StationConfig>("/station/config", { headers: { "X-API-Key": key } }));
    } catch (cause) {
      setAuthenticated(false);
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    if (apiKey) void signIn(apiKey);
  }, []);

  if (!authenticated) {
    return (
      <main className="console admin">
        <header className="bar"><strong className="brand">Station administration</strong><a href="/">Console</a></header>
        <section className="card">
          <h2>Operator sign-in</h2>
          <form onSubmit={(event) => { event.preventDefault(); void signIn(apiKey); }}>
            <label>
              <span className="label">Station API key</span>
              <input type="password" value={apiKey} autoComplete="off" onChange={(event) => setApiKey(event.target.value)} required />
            </label>
            <button type="submit" className="btn btn-build" disabled={busy}>Sign in</button>
          </form>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      </main>
    );
  }

  const api = { headers, json, load, setError, setBusy, busy };

  return (
    <main className="console admin">
      <header className="bar">
        <strong className="brand">Station administration</strong>
        <a href="/">← Console</a>
      </header>
      <nav className="tabs" role="tablist">
        {TABS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={tab === item.id}
            className={`tab ${tab === item.id ? "tab-on" : ""}`}
            onClick={() => { setTab(item.id); window.history.replaceState(null, "", `#${item.id}`); }}
          >
            {item.label}
          </button>
        ))}
      </nav>
      {error && <p className="error" role="alert" onClick={() => setError("")}>{error}</p>}
      {config && tab === "automation" && (
        <Endpoints title="Automation audio" kind="inputs" scope="automation" single config={config} apiKey={apiKey} api={api} />
      )}
      {config && tab === "inputs" && (
        <Endpoints title="Additional audio inputs" kind="inputs" scope="aux" config={config} apiKey={apiKey} api={api} />
      )}
      {config && tab === "outputs" && (
        <Endpoints title="Audio outputs" kind="outputs" scope="output" config={config} apiKey={apiKey} api={api} />
      )}
      {config && tab === "metadata" && <Metadata config={config} api={api} apiKey={apiKey} />}
      {tab === "delay" && <DelaySettings apiKey={apiKey} onError={setError} />}
      {tab === "routing" && (
        <section className="card">
          <h2>Routing</h2>
          <p>
            Routing is automatic. The <strong>Program</strong> output carries the mix of every enabled audio source:
            Automation, additional inputs, and connected microphones. Use the faders and ON/CUE switches on the console to
            control what is on air. Nothing needs to be configured here.
          </p>
        </section>
      )}
      {config && tab === "station" && <StationSettings config={config} api={api} />}
      {tab === "guests" && <GuestLinks apiKey={apiKey} onError={setError} />}
    </main>
  );
}

type Api = {
  headers: (extra?: Record<string, string>) => HeadersInit;
  json: Record<string, string>;
  load: () => Promise<void>;
  setError: (message: string) => void;
  setBusy: (busy: boolean) => void;
  busy: boolean;
};

function Endpoints({
  title,
  kind,
  scope,
  single,
  config,
  apiKey,
  api,
}: {
  title: string;
  kind: "inputs" | "outputs";
  scope: "automation" | "aux" | "output";
  single?: boolean;
  config: StationConfig;
  apiKey: string;
  api: Api;
}) {
  const direction = kind === "inputs" ? "input" : "output";
  const all = kind === "inputs" ? config.inputs : config.outputs;
  const items = kind === "inputs" ? all.filter((item) => (item.type ?? "aux") === scope) : all;
  const types = kind === "inputs" ? ["srt", "rtp", "alsa", "test"] : ["srt", "rtp", "alsa"];
  const metrics = usePolling<AudioMetrics>("/audio/metrics", apiKey, 2000);
  const system = usePolling<SystemDevices>("/audio/system/devices", apiKey, 15000).data;
  const alsaDevices = (direction === "input" ? system?.alsa_inputs : system?.alsa_outputs) ?? [];

  const [editing, setEditing] = useState<Resource | null>(null);
  const [name, setName] = useState("");
  const [type, setType] = useState("srt");
  const [enabled, setEnabled] = useState(true);
  const [values, setValues] = useState<Cfg>(defaultsFor(direction, "srt"));
  const [open, setOpen] = useState(false);

  function begin(resource: Resource | null) {
    setEditing(resource);
    setOpen(true);
    setName(resource?.name ?? "");
    const resourceType = resource ? String((resource.config.type as string | undefined) ?? resource.type ?? "srt") : "srt";
    setType(resourceType);
    setEnabled(resource?.enabled ?? true);
    setValues(resource ? legacyInterface({ ...resource.config }) : newDefaults(resourceType));
  }

  function newDefaults(next: string): Cfg {
    const defaults = defaultsFor(direction, next);
    if (direction === "input" && (next === "rtp" || next === "srt") && system?.audio_listener_ports) {
      defaults.port = Number(system.audio_listener_ports.split(",")[0].split("-")[0].trim());
    }
    return defaults;
  }

  function changeType(next: string) {
    setType(next);
    setValues(newDefaults(next));
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    const cleaned: Cfg = { type };
    for (const spec of fieldsFor(direction, type)) {
      const raw = values[spec.key];
      if (raw === undefined || raw === "" || raw === null) continue;
      cleaned[spec.key] = spec.kind === "number" || (spec.kind === "select" && /^\d+$/.test(String(raw))) ? Number(raw) : raw;
    }
    const body: Record<string, unknown> = { name, enabled, config: cleaned };
    body.resource_type = kind === "inputs" ? scope : type;
    api.setBusy(true);
    api.setError("");
    try {
      await apiRequest(`/station/config/${kind}${editing ? `/${editing.id}` : ""}`, {
        method: editing ? "PATCH" : "POST",
        headers: api.headers(api.json),
        body: JSON.stringify(body),
      });
      setOpen(false);
      await api.load();
    } catch (cause) {
      const message = errorMessage(cause);
      if (message.startsWith("Configuration was saved but")) {
        setOpen(false);
        await api.load();
      }
      api.setError(message);
    } finally {
      api.setBusy(false);
    }
  }

  async function remove(resource: Resource) {
    if (!window.confirm(`Delete “${resource.name}”?`)) return;
    try {
      await apiRequest(`/station/config/${kind}/${resource.id}`, { method: "DELETE", headers: api.headers() });
      await api.load();
    } catch (cause) {
      const message = errorMessage(cause);
      if (message.startsWith("Configuration was deleted but")) await api.load();
      api.setError(message);
    }
  }

  const stateOf = (id: string) => metrics.data?.sources.find((source) => source.id === id)?.state;
  const canAdd = !(single && items.length >= 1);

  return (
    <section className="card">
      <div className="row">
        <h2>{title}</h2>
        {canAdd && !open && <button type="button" className="btn btn-small btn-build" onClick={() => begin(null)}>Add</button>}
      </div>
      {items.length === 0 && !open && <p className="muted">Nothing configured yet.</p>}
      {items.map((item) => (
        <div key={item.id} className="row item">
          <span>
            <strong>{item.name}</strong>{" "}
            <span className="chip">{String(item.config.type ?? item.type ?? "").toUpperCase()}</span>{" "}
            {!item.enabled && <span className="chip">disabled</span>}
            {kind === "inputs" && stateOf(item.id) && <span className="chip chip-live">{stateOf(item.id)}</span>}
            <br />
            <span className="small muted">{summary(item.config)}</span>
          </span>
          <span className="row-actions">
            <button type="button" className="btn btn-small" onClick={() => begin(item)}>Edit</button>
            <button type="button" className="btn btn-small" onClick={() => void remove(item)}>Delete</button>
          </span>
        </div>
      ))}
      {open && (
        <form onSubmit={save} className="form">
          <h3>{editing ? "Edit" : "New"} {scope === "automation" ? "automation input" : direction}</h3>
          {system?.audio_listener_ports && (type === "rtp" || type === "srt") && (
            <p>
              Station audio listener ports: {system.audio_listener_ports}. Change AUDIO_INPUT_PORTS in the station
              environment file and recreate the station to use other listener ports. Outgoing destination ports are not restricted.
            </p>
          )}
          <label>
            <span className="label">Name</span>
            <input value={name} maxLength={100} required onChange={(event) => setName(event.target.value)} />
          </label>
          <label>
            <span className="label">Type</span>
            <select value={type} onChange={(event) => changeType(event.target.value)}>
              {types.map((option) => (
                <option key={option} value={option}>{option === "test" ? "Test tone" : option.toUpperCase()}</option>
              ))}
            </select>
          </label>
          <div className="grid2">
            {fieldsFor(direction, type).map((spec) => (
              <label key={spec.key}>
                <span className="label">{spec.label}</span>
                {spec.source === "nic" ? (
                  <select value={String(values[spec.key] ?? "")} onChange={(event) => setValues({ ...values, [spec.key]: event.target.value })}>
                    <option value="">{system?.default_audio_address ? `Default audio network (${system.default_audio_address})` : "Default (system routing)"}</option>
                    {(system?.network_interfaces ?? []).map((nic) => (
                      <option key={nic.name} value={nic.name}>{nic.name}{nic.addresses.length > 0 ? ` — ${nic.addresses.join(", ")}` : ""} ({nic.state})</option>
                    ))}
                    {Boolean(values[spec.key]) && !(system?.network_interfaces ?? []).some((nic) => nic.name === values[spec.key]) && (
                      <option value={String(values[spec.key])}>{String(values[spec.key])}</option>
                    )}
                  </select>
                ) : spec.source === "alsa" ? (
                  <>
                    {alsaDevices.length > 0 && (
                      <select
                        aria-label={`Detected ALSA ${direction} devices`}
                        value={alsaDevices.some((device) => device.device === values[spec.key]) ? String(values[spec.key]) : ""}
                        onChange={(event) => {
                          if (event.target.value) setValues({ ...values, [spec.key]: event.target.value });
                        }}
                      >
                        <option value="">Choose a detected device</option>
                        {alsaDevices.map((device) => (
                          <option key={device.device} value={device.device}>
                            {device.device} — {device.name}{device.description && device.description !== device.name ? ` (${device.description})` : ""}
                          </option>
                        ))}
                      </select>
                    )}
                    <input
                      aria-label={spec.label}
                      value={String(values[spec.key] ?? "")}
                      placeholder={spec.placeholder}
                      autoComplete="off"
                      onChange={(event) => setValues({ ...values, [spec.key]: event.target.value })}
                    />
                    {system && alsaDevices.length === 0 && <span className="small muted">No ALSA {direction} devices detected on the server; enter one manually.</span>}
                    <span className="small muted">Choose a detected device or enter an ALSA identifier, including a subdevice if needed (for example hw:0,1,3).</span>
                  </>
                ) : spec.kind === "select" ? (
                  <select value={String(values[spec.key] ?? "")} onChange={(event) => setValues({ ...values, [spec.key]: event.target.value })}>
                    {spec.options?.map((option) => <option key={option} value={option}>{option || "None"}</option>)}
                  </select>
                ) : (
                  <input
                    type={spec.kind === "number" ? "number" : spec.kind}
                    value={String(values[spec.key] ?? "")}
                    placeholder={spec.placeholder}
                    autoComplete="off"
                    onChange={(event) => setValues({ ...values, [spec.key]: event.target.value })}
                  />
                )}
                {spec.help && <span className="small muted">{spec.help}</span>}
              </label>
            ))}
          </div>
          <label className="check"><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /> Enabled</label>
          <div className="row-actions">
            <button type="submit" className="btn btn-build" disabled={api.busy}>Save</button>
            <button type="button" className="btn" onClick={() => setOpen(false)}>Cancel</button>
          </div>
        </form>
      )}
    </section>
  );
}

function summary(config: Cfg): string {
  const parts: string[] = [];
  for (const key of ["device", "mode", "host", "address", "port", "interface"]) {
    if (config[key] !== undefined && config[key] !== "") parts.push(`${key === "interface" ? "nic" : key}: ${String(config[key])}`);
  }
  return parts.join(" · ");
}

function NicSelect({ value, onChange, interfaces, defaultAddress }: { value: string; onChange: (value: string) => void; interfaces: SystemDevices["network_interfaces"]; defaultAddress?: string | null }) {
  return (
    <select value={value} onChange={(event) => onChange(event.target.value)}>
      <option value="">{defaultAddress ? `Default data network (${defaultAddress})` : "Default (system routing)"}</option>
      {interfaces.map((nic) => (
        <option key={nic.name} value={nic.name}>{nic.name}{nic.addresses.length > 0 ? ` — ${nic.addresses.join(", ")}` : ""} ({nic.state})</option>
      ))}
      {value && !interfaces.some((nic) => nic.name === value) && <option value={value}>{value}</option>}
    </select>
  );
}

type MetadataStatus = { id: string; connections: number; bytes: number; error: string | null };

function Metadata({ config, api, apiKey }: { config: StationConfig; api: Api; apiKey: string }) {
  const system = usePolling<SystemDevices>("/audio/system/devices", apiKey, 15000).data;
  const interfaces = system?.network_interfaces ?? [];
  const status = usePolling<MetadataStatus[]>("/audio/metadata", apiKey, 3000).data ?? [];
  return (
    <>
      <MetadataCard direction="input" config={config} api={api} interfaces={interfaces} defaultAddress={system?.default_metadata_address} listenerPorts={system?.metadata_listener_ports} status={status} />
      <MetadataCard direction="output" config={config} api={api} interfaces={interfaces} defaultAddress={system?.default_metadata_address} listenerPorts={system?.metadata_listener_ports} status={status} />
    </>
  );
}

function MetadataCard({ direction, config, api, interfaces, defaultAddress, listenerPorts, status }: { direction: "input" | "output"; config: StationConfig; api: Api; interfaces: SystemDevices["network_interfaces"]; defaultAddress?: string | null; listenerPorts?: string | null; status: MetadataStatus[] }) {
  const existing = config.metadata.find((item) => item.direction === direction) ?? null;
  const [mode, setMode] = useState("server");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [nic, setNic] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [delayed, setDelayed] = useState(true);
  const live = existing ? status.find((item) => item.id === existing.id) : undefined;

  useEffect(() => {
    setDelayed(existing?.config.delayed !== false);
    setMode(String(existing?.config.mode ?? "server"));
    setHost(String(existing?.config.host ?? ""));
    setPort(String(existing?.config.port ?? ""));
    setNic(String(existing?.config.interface ?? ""));
    setEnabled(existing?.enabled ?? true);
  }, [existing?.id, existing?.config.mode, existing?.config.host, existing?.config.port, existing?.config.interface, existing?.enabled]);

  // Input: server accepts incoming data, client pulls. Output: client pushes, server waits for a puller.
  const needsHost = mode === "client";
  const serverLabel = direction === "input" ? "Server — accept incoming data" : "Server — a client connects to pull data";
  const clientLabel = direction === "input" ? "Client — pull data from a source" : "Client — push data to a destination";

  async function save(event: FormEvent) {
    event.preventDefault();
    const body = {
      name: `Metadata ${direction}`,
      direction,
      format: "passthrough",
      enabled,
      config: { mode, port: Number(port), ...(needsHost ? { host } : {}), ...(nic ? { interface: nic } : {}), ...(direction === "output" ? { delayed } : {}) },
    };
    api.setBusy(true);
    api.setError("");
    try {
      await apiRequest(`/station/config/metadata${existing ? `/${existing.id}` : ""}`, {
        method: existing ? "PATCH" : "POST",
        headers: api.headers(api.json),
        body: JSON.stringify(body),
      });
      await api.load();
    } catch (cause) {
      const message = errorMessage(cause);
      if (message.startsWith("Configuration was saved but")) await api.load();
      api.setError(message);
    } finally {
      api.setBusy(false);
    }
  }

  async function remove() {
    if (!existing) return;
    try {
      await apiRequest(`/station/config/metadata/${existing.id}`, { method: "DELETE", headers: api.headers() });
      await api.load();
    } catch (cause) {
      const message = errorMessage(cause);
      if (message.startsWith("Configuration was deleted but")) await api.load();
      api.setError(message);
    }
  }

  return (
    <section className="card">
      <h2>Metadata {direction}</h2>
      <p className="muted small">Data is passed through unchanged; its format does not matter.</p>
      <form onSubmit={save} className="form">
        <label>
          <span className="label">Mode</span>
          <select value={mode} onChange={(event) => setMode(event.target.value)}>
            <option value="server">{serverLabel}</option>
            <option value="client">{clientLabel}</option>
          </select>
        </label>
        <div className="grid2">
          {needsHost && (
            <label>
              <span className="label">{direction === "input" ? "Source IP address" : "Destination IP address"}</span>
              <input value={host} required onChange={(event) => setHost(event.target.value)} placeholder="192.168.1.10" />
            </label>
          )}
          <label>
            <span className="label">{needsHost ? "Port" : "Listen port"}</span>
            <input type="number" min={1} max={65535} required value={port} onChange={(event) => setPort(event.target.value)} />
          </label>
        </div>
        <label>
          <span className="label">Network interface</span>
          <NicSelect value={nic} onChange={setNic} interfaces={interfaces} defaultAddress={defaultAddress} />
        </label>
        {mode === "server" && listenerPorts && <p>Listener ports for this station: {listenerPorts}.</p>}
        <label className="check"><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /> Enabled</label>
        {direction === "output" && (
          <label className="check"><input type="checkbox" checked={delayed} onChange={(event) => setDelayed(event.target.checked)} /> Delay with the program audio</label>
        )}
        <div className="row-actions">
          <button type="submit" className="btn btn-build" disabled={api.busy}>Save</button>
          {existing && <button type="button" className="btn" onClick={() => void remove()}>Remove</button>}
        </div>
      </form>
      {existing && enabled && (
        <p className="muted small">
          {live ? `${live.connections > 0 ? "Connected" : "Waiting for a connection"} · ${live.bytes.toLocaleString()} bytes` : "Starting…"}
          {live?.error ? ` · ${live.error}` : ""}
        </p>
      )}
    </section>
  );
}

function DelaySettings({ apiKey, onError }: { apiKey: string; onError: (message: string) => void }) {
  const status = usePolling<DelayStatus>("/delay", apiKey, 2000);
  const [seconds, setSeconds] = useState("");
  useEffect(() => {
    if (status.data && seconds === "") setSeconds(String(status.data.configured_delay_seconds));
  }, [status.data]);

  async function save(event: FormEvent) {
    event.preventDefault();
    try {
      await apiRequest("/delay", {
        method: "PATCH",
        headers: { "X-API-Key": apiKey, "Content-Type": "application/json" },
        body: JSON.stringify({ delay_seconds: Number(seconds) }),
      });
      status.refresh();
      onError("");
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function setEnabled(enabled: boolean) {
    try {
      await apiRequest("/delay", {
        method: "PATCH",
        headers: { "X-API-Key": apiKey, "Content-Type": "application/json" },
        body: JSON.stringify({ enabled }),
      });
      status.refresh();
      onError("");
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  return (
    <section className="card">
      <h2>Profanity delay</h2>
      <form onSubmit={save} className="form">
        <label>
          <span className="label">Delay time (seconds, 5–60)</span>
          <input type="number" min={5} max={60} required value={seconds} onChange={(event) => setSeconds(event.target.value)} />
        </label>
        <button type="submit" className="btn btn-build">Save delay time</button>
      </form>
      <label className="check" style={{ marginTop: 8 }}>
        <input type="checkbox" checked={status.data?.enabled !== false} onChange={(event) => void setEnabled(event.target.checked)} />
        Enable profanity delay (when off, the delay control is hidden from the console)
      </label>
      <p className="muted small">
        Current buffer {status.data ? `${status.data.buffered_delay_seconds.toFixed(1)}s (${status.data.state})` : "…"}. Use Build
        delay on the console to start building the buffer.
      </p>
    </section>
  );
}

function StationSettings({ config, api }: { config: StationConfig; api: Api }) {
  const [name, setName] = useState(config.name);
  async function save(event: FormEvent) {
    event.preventDefault();
    try {
      await apiRequest("/station/config", { method: "PATCH", headers: api.headers(api.json), body: JSON.stringify({ name }) });
      await api.load();
    } catch (cause) {
      api.setError(errorMessage(cause));
    }
  }
  return (
    <section className="card">
      <h2>Station</h2>
      <form onSubmit={save} className="form">
        <label>
          <span className="label">Station name</span>
          <input value={name} maxLength={100} required onChange={(event) => setName(event.target.value)} />
        </label>
        <button type="submit" className="btn btn-build">Save</button>
      </form>
    </section>
  );
}

function GuestLinks({ apiKey, onError }: { apiKey: string; onError: (message: string) => void }) {
  const links = usePolling<GuestLink[]>("/guest-links", apiKey, 5000);
  const [name, setName] = useState("");
  const [hours, setHours] = useState("8");
  const [created, setCreated] = useState("");

  async function create(event: FormEvent) {
    event.preventDefault();
    try {
      const link = await apiRequest<GuestLink>("/guest-links", {
        method: "POST",
        headers: { "X-API-Key": apiKey, "Content-Type": "application/json" },
        body: JSON.stringify({
          name: name.trim() || null,
          expires_at: new Date(Date.now() + Number(hours) * 3600 * 1000).toISOString(),
        }),
      });
      if (link.invitation_token) setCreated(`${window.location.origin}/?invite=${encodeURIComponent(link.invitation_token)}`);
      setName("");
      links.refresh();
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function invalidate(id: string) {
    try {
      await apiRequest(`/guest-links/${id}`, { method: "DELETE", headers: { "X-API-Key": apiKey } });
      links.refresh();
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  return (
    <section className="card">
      <h2>Guest links</h2>
      <form onSubmit={create} className="form">
        <div className="grid2">
          <label><span className="label">Name (optional)</span><input value={name} maxLength={100} onChange={(event) => setName(event.target.value)} /></label>
          <label><span className="label">Valid for (hours)</span><input type="number" min={1} max={720} value={hours} onChange={(event) => setHours(event.target.value)} required /></label>
        </div>
        <button type="submit" className="btn btn-build">Create link</button>
      </form>
      {created && <input readOnly className="mono" value={created} onFocus={(event) => event.currentTarget.select()} />}
      {(links.data ?? []).map((link) => (
        <div key={link.id} className="row item">
          <span className="small">{link.name ?? "Guest link"} · {link.status} · expires {new Date(link.expires_at).toLocaleString()}</span>
          {link.status === "active" && <button type="button" className="btn btn-small" onClick={() => void invalidate(link.id)}>Invalidate</button>}
        </div>
      ))}
    </section>
  );
}
