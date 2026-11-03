import { useEffect, useState, type FormEvent } from "react";
import DelayControls from "./DelayControls";

type ResourceKind = "inputs" | "outputs" | "metadata" | "routes";
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
  delay: Array<{ id: string; delay_seconds: number; enabled: boolean }>;
  routing_rules: Resource[];
};
type GuestLink = {
  id: string;
  name: string | null;
  expires_at: string;
  invalidated_at: string | null;
  status: "active" | "expired" | "invalidated";
  invitation_token: string | null;
};

const STORAGE_KEY = "cloud-console-station-key";
const sections: Array<{ id: ResourceKind | "station" | "guest-links" | "delay"; label: string }> = [
  { id: "inputs", label: "Audio inputs" },
  { id: "outputs", label: "Audio outputs" },
  { id: "metadata", label: "Metadata" },
  { id: "routes", label: "Routing" },
  { id: "delay", label: "Delay" },
  { id: "station", label: "Station settings" },
  { id: "guest-links", label: "Guest links" },
];

export default function AdminConsole() {
  const [apiKey, setApiKey] = useState(() => sessionStorage.getItem(STORAGE_KEY) ?? "");
  const [authenticated, setAuthenticated] = useState(false);
  const [config, setConfig] = useState<StationConfig | null>(null);
  const [links, setLinks] = useState<GuestLink[]>([]);
  const [section, setSection] = useState<(typeof sections)[number]["id"]>("inputs");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [newLink, setNewLink] = useState<GuestLink | null>(null);

  async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const response = await fetch(`/api${path}`, {
      ...init,
      headers: { "X-API-Key": apiKey, ...init.headers },
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as { error?: string };
      throw new Error(body.error ?? `Request failed (${response.status})`);
    }
    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

  async function load() {
    const [nextConfig, nextLinks] = await Promise.all([
      request<StationConfig>("/station/config"),
      request<GuestLink[]>("/guest-links"),
    ]);
    setConfig(nextConfig);
    setLinks(nextLinks);
    setError("");
  }

  async function signIn(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await request("/auth/login", { method: "POST" });
      setAuthenticated(true);
      sessionStorage.setItem(STORAGE_KEY, apiKey);
      await load();
    } catch (cause) {
      setAuthenticated(false);
      setError(messageOf(cause));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    if (!authenticated) return;
    const timer = window.setInterval(() => {
      void request<GuestLink[]>("/guest-links")
        .then(setLinks)
        .catch((cause: unknown) => setError(messageOf(cause)));
    }, 5000);
    return () => window.clearInterval(timer);
  }, [authenticated, apiKey]);

  return (
    <main className="shell admin-shell">
      <header className="topbar">
        <div className="brand-mark" aria-hidden="true">CC</div>
        <div>
          <p className="eyebrow">STATION ADMINISTRATION</p>
          <h1>Cloud Console</h1>
        </div>
        <a className="admin-console-link" href="/">Main console</a>
      </header>
      <section className="welcome">
        <p className="eyebrow">CONFIGURATION</p>
        <h2>Station administration</h2>
        <p className="intro">Manage station resources, routing, delay, and guest invitations.</p>
      </section>
      {!authenticated ? (
        <section className="panel" aria-labelledby="admin-login-title">
          <h3 id="admin-login-title">Operator sign-in</h3>
          <form onSubmit={signIn}>
            <label htmlFor="admin-api-key">Station API key</label>
            <input
              id="admin-api-key"
              type="password"
              autoComplete="off"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              required
            />
            <button type="submit" disabled={busy}>{busy ? "Connecting..." : "Open station settings"}</button>
          </form>
        </section>
      ) : (
        <>
          <nav className="admin-nav" aria-label="Station administration sections">
            {sections.map((item) => (
              <button
                type="button"
                key={item.id}
                className={section === item.id ? "" : "secondary"}
                aria-current={section === item.id ? "page" : undefined}
                onClick={() => setSection(item.id)}
              >
                {item.label}
              </button>
            ))}
          </nav>
          {section === "station" && config && (
            <StationSettings
              name={config.name}
              onSave={async (name) => {
                await request("/station/config", {
                  method: "PATCH",
                  headers: { "Content-Type": "application/json" },
                  body: JSON.stringify({ name }),
                });
                await load();
              }}
            />
          )}
          {isResourceKind(section) && config && (
            <ResourceSettings
              kind={section}
              resources={resourceList(config, section)}
              busy={busy}
              onBusy={setBusy}
              request={request}
              onRefresh={load}
              onError={setError}
            />
          )}
          {section === "delay" && <DelayControls apiKey={apiKey} />}
          {section === "guest-links" && (
            <GuestLinkSettings
              links={links}
              newLink={newLink}
              busy={busy}
              onBusy={setBusy}
              onCreate={async (name, expiresAt) => {
                const created = await request<GuestLink>("/guest-links", {
                  method: "POST",
                  headers: { "Content-Type": "application/json" },
                  body: JSON.stringify({ name: name.trim() || null, expires_at: expiresAt }),
                });
                setNewLink(created);
                await load();
              }}
              onInvalidate={async (id) => {
                await request(`/guest-links/${id}`, { method: "DELETE" });
                await load();
              }}
              onError={setError}
            />
          )}
          {config && <p className="muted admin-station-id">Station ID: <code>{config.id}</code></p>}
        </>
      )}
      {error && <p className="error" role="alert">{error}</p>}
      <footer><span>Station configuration is saved to the station database.</span></footer>
    </main>
  );
}

function ResourceSettings({
  kind,
  resources,
  busy,
  onBusy,
  request,
  onRefresh,
  onError,
}: {
  kind: ResourceKind;
  resources: Resource[];
  busy: boolean;
  onBusy: (busy: boolean) => void;
  request: <T>(path: string, init?: RequestInit) => Promise<T>;
  onRefresh: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const [editing, setEditing] = useState<Resource | null>(null);
  const [name, setName] = useState("");
  const [resourceType, setResourceType] = useState(kind === "inputs" ? "test" : "rtp");
  const [direction, setDirection] = useState("input");
  const [format, setFormat] = useState("json");
  const [enabled, setEnabled] = useState(true);
  const [configText, setConfigText] = useState("{}");
  const label = kind === "routes" ? "Routing rules" : kind === "metadata" ? "Metadata configurations" : kind === "inputs" ? "Audio inputs" : "Audio outputs";

  function beginEdit(resource?: Resource) {
    setEditing(resource ?? null);
    setName(resource?.name ?? "");
    setResourceType(resource?.type ?? (kind === "inputs" ? "test" : "rtp"));
    setDirection(resource?.direction ?? "input");
    setFormat(resource?.format ?? "json");
    setEnabled(resource?.enabled ?? true);
    setConfigText(JSON.stringify(resource?.config ?? {}, null, 2));
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    let parsedConfig: Record<string, unknown>;
    try {
      const parsed = JSON.parse(configText) as unknown;
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
        throw new Error("Configuration must be a JSON object.");
      }
      parsedConfig = parsed as Record<string, unknown>;
    } catch (cause) {
      onError(messageOf(cause));
      return;
    }
    const body: Record<string, unknown> = { name, enabled, config: parsedConfig };
    if (kind === "inputs" || kind === "outputs") body.resource_type = resourceType;
    if (kind === "metadata") {
      body.direction = direction;
      body.format = format;
    }
    onBusy(true);
    onError("");
    try {
      await request(
        `/station/config/${kind}${editing ? `/${editing.id}` : ""}`,
        {
          method: editing ? "PATCH" : "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      );
      beginEdit();
      await onRefresh();
    } catch (cause) {
      onError(messageOf(cause));
    } finally {
      onBusy(false);
    }
  }

  async function remove(resource: Resource) {
    onBusy(true);
    onError("");
    try {
      await request(`/station/config/${kind}/${resource.id}`, { method: "DELETE" });
      if (editing?.id === resource.id) beginEdit();
      await onRefresh();
    } catch (cause) {
      onError(messageOf(cause));
    } finally {
      onBusy(false);
    }
  }

  return (
    <section className="panel admin-panel">
      <div className="panel-heading">
        <div><p className="eyebrow">STATION RESOURCES</p><h3>{label}</h3></div>
        <span className="status-pill">{resources.length} configured</span>
      </div>
      <div className="admin-resource-grid">
        <ul className="item-list admin-resource-list">
          {resources.map((resource) => (
            <li key={resource.id}>
              <div>
                <strong>{resource.name}</strong>
                <span>{resource.type ?? resource.format ?? "Routing rule"} · {resource.enabled ? "Enabled" : "Disabled"}</span>
              </div>
              <div className="inline-actions">
                <button type="button" className="secondary" disabled={busy} onClick={() => beginEdit(resource)}>Edit</button>
                <button type="button" className="secondary" disabled={busy} onClick={() => void remove(resource)}>Delete</button>
              </div>
            </li>
          ))}
          {resources.length === 0 && <li><span className="muted">No resources configured.</span></li>}
        </ul>
        <form className="admin-resource-form" onSubmit={(event) => void save(event)}>
          <h4>{editing ? "Edit resource" : "Add resource"}</h4>
          <label htmlFor="resource-name">Name</label>
          <input id="resource-name" value={name} onChange={(event) => setName(event.target.value)} maxLength={100} required />
          {(kind === "inputs" || kind === "outputs") && (
            <>
              <label htmlFor="resource-type">Type</label>
              <input id="resource-type" value={resourceType} onChange={(event) => setResourceType(event.target.value)} maxLength={40} required />
            </>
          )}
          {kind === "metadata" && (
            <div className="admin-two-column">
              <label>Direction
                <select value={direction} onChange={(event) => setDirection(event.target.value)}>
                  <option value="input">Input</option><option value="output">Output</option>
                </select>
              </label>
              <label>Format
                <input value={format} onChange={(event) => setFormat(event.target.value)} maxLength={40} required />
              </label>
            </div>
          )}
          <label className="admin-enabled"><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /> Enabled</label>
          <label htmlFor="resource-config">Configuration (JSON)</label>
          <textarea id="resource-config" rows={6} value={configText} onChange={(event) => setConfigText(event.target.value)} />
          <div className="inline-actions">
            <button type="submit" disabled={busy}>{editing ? "Save changes" : "Add resource"}</button>
            {editing && <button type="button" className="secondary" disabled={busy} onClick={() => beginEdit()}>Cancel</button>}
          </div>
        </form>
      </div>
    </section>
  );
}

function StationSettings({ name, onSave }: { name: string; onSave: (name: string) => Promise<void> }) {
  const [value, setValue] = useState(name);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => setValue(name), [name]);
  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await onSave(value);
    } catch (cause) {
      setError(messageOf(cause));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="panel admin-panel">
      <p className="eyebrow">STATION PROFILE</p>
      <h3>Station settings</h3>
      <form className="admin-settings-form" onSubmit={(event) => void save(event)}>
        <label htmlFor="station-name">Station name</label>
        <input id="station-name" value={value} onChange={(event) => setValue(event.target.value)} maxLength={100} required />
        <button type="submit" disabled={busy || value === name}>{busy ? "Saving..." : "Save station name"}</button>
      </form>
      {error && <p className="error" role="alert">{error}</p>}
    </section>
  );
}

function GuestLinkSettings({
  links,
  newLink,
  busy,
  onBusy,
  onCreate,
  onInvalidate,
  onError,
}: {
  links: GuestLink[];
  newLink: GuestLink | null;
  busy: boolean;
  onBusy: (busy: boolean) => void;
  onCreate: (name: string, expiresAt: string) => Promise<void>;
  onInvalidate: (id: string) => Promise<void>;
  onError: (message: string) => void;
}) {
  const [name, setName] = useState("");
  const [expiresAt, setExpiresAt] = useState(defaultExpiry());
  const [copyError, setCopyError] = useState("");
  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onBusy(true);
    onError("");
    try {
      await onCreate(name, new Date(expiresAt).toISOString());
      setName("");
    } catch (cause) {
      onError(messageOf(cause));
    } finally {
      onBusy(false);
    }
  }
  async function invalidate(id: string) {
    onBusy(true);
    onError("");
    try {
      await onInvalidate(id);
    } catch (cause) {
      onError(messageOf(cause));
    } finally {
      onBusy(false);
    }
  }
  const inviteUrl = newLink?.invitation_token
    ? `${window.location.origin}/?invite=${encodeURIComponent(newLink.invitation_token)}`
    : "";
  return (
    <section className="panel admin-panel">
      <div className="panel-heading">
        <div><p className="eyebrow">INVITATIONS</p><h3>Guest links</h3></div>
        <span className="status-pill">{links.filter((link) => link.status === "active").length} active</span>
      </div>
      <form className="admin-settings-form" onSubmit={(event) => void create(event)}>
        <label htmlFor="admin-link-name">Invitation name</label>
        <input id="admin-link-name" value={name} onChange={(event) => setName(event.target.value)} maxLength={100} />
        <label htmlFor="admin-link-expiry">Expires</label>
        <input id="admin-link-expiry" type="datetime-local" value={expiresAt} onChange={(event) => setExpiresAt(event.target.value)} required />
        <button type="submit" disabled={busy}>Create guest link</button>
      </form>
      {inviteUrl && (
        <div className="invitation-created">
          <p>Copy this invitation now; its token is shown only once.</p>
          <code>{inviteUrl}</code>
          <button type="button" className="secondary" onClick={() => {
            void navigator.clipboard.writeText(inviteUrl).then(() => setCopyError("")).catch((cause: unknown) => setCopyError(messageOf(cause)));
          }}>Copy invitation</button>
          {copyError && <p className="error" role="alert">{copyError}</p>}
        </div>
      )}
      <ul className="item-list">
        {links.map((link) => (
          <li key={link.id}>
            <div><strong>{link.name || "Guest invitation"}</strong><span>{link.status} · expires {new Date(link.expires_at).toLocaleString()}</span></div>
            {link.status === "active" && <button type="button" className="secondary" disabled={busy} onClick={() => void invalidate(link.id)}>Revoke</button>}
          </li>
        ))}
      </ul>
    </section>
  );
}

function isResourceKind(value: string): value is ResourceKind {
  return value === "inputs" || value === "outputs" || value === "metadata" || value === "routes";
}

function resourceList(config: StationConfig, kind: ResourceKind): Resource[] {
  if (kind === "inputs") return config.inputs;
  if (kind === "outputs") return config.outputs;
  if (kind === "metadata") return config.metadata;
  return config.routing_rules;
}

function defaultExpiry(): string {
  const date = new Date(Date.now() + 24 * 60 * 60 * 1000);
  date.setMinutes(date.getMinutes() - date.getTimezoneOffset());
  return date.toISOString().slice(0, 16);
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred";
}
