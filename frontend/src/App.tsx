import { useEffect, useState } from "react";
import LiveAudioConnection from "./LiveAudioConnection";
import DelayControls from "./DelayControls";

type AuthResponse = {
  authenticated: boolean;
  station_id: string;
  role: string;
};

type Participant = {
  session_id: string;
  display_name: string;
  role: string;
  status: string;
  access_token: string;
  expires_at: string;
};

type GuestLink = {
  id: string;
  name: string | null;
  expires_at: string;
  invalidated_at: string | null;
  invitation_token: string | null;
};

type LiveSession = {
  id: string;
  display_name: string;
  role: string;
  status: string;
  expires_at: string;
};

type Health = { status: string };
type MixerChannel = {
  channel_id: string;
  display_name: string;
  role: string;
  fader: number;
  is_on: boolean;
  cue_enabled: boolean;
  muted: boolean;
  revision: number;
};

const API_BASE = "/api";
const PARTICIPANT_KEY = "cloud-console-live-session";
const STATION_KEY = "cloud-console-station-key";

export default function App() {
  const invite = new URLSearchParams(window.location.search).get("invite");
  return invite ? <GuestApp invitationToken={invite} /> : <StationApp />;
}

function StationApp() {
  const [apiKey, setApiKey] = useState(() => sessionStorage.getItem(STATION_KEY) ?? "");
  const [health, setHealth] = useState<Health | null>(null);
  const [auth, setAuth] = useState<AuthResponse | null>(null);
  const [participant, setParticipant] = useState<Participant | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    const url = new URL(window.location.href);
    const key = url.searchParams.get("api_key");
    if (key) {
      setApiKey(key);
      url.searchParams.delete("api_key");
      window.history.replaceState({}, "", url);
    }
    void apiRequest<Health>("/health")
      .then(setHealth)
      .catch((cause: unknown) => setError(errorMessage(cause)));
  }, []);

  async function signIn(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError("");
    try {
      const response = await apiRequest<AuthResponse>("/auth/login", {
        method: "POST",
        headers: { "X-API-Key": apiKey },
      });
      setAuth(response);
      sessionStorage.setItem(STATION_KEY, apiKey);
      const saved = readParticipant();
      if (saved) {
        try {
          await apiRequest("/auth/validate", {
            headers: { Authorization: `Bearer ${saved.access_token}` },
          });
          setParticipant(saved);
        } catch {
          sessionStorage.removeItem(PARTICIPANT_KEY);
        }
      }
    } catch (cause) {
      setError(errorMessage(cause));
      setAuth(null);
    }
  }

  function signOut() {
    setAuth(null);
    setParticipant(null);
    setApiKey("");
    sessionStorage.removeItem(STATION_KEY);
    sessionStorage.removeItem(PARTICIPANT_KEY);
  }

  function saveParticipant(next: Participant) {
    setParticipant(next);
    sessionStorage.setItem(PARTICIPANT_KEY, JSON.stringify(next));
  }

  return (
    <main className="shell">
      <header className="topbar">
        <div className="brand-mark" aria-hidden="true">CC</div>
        <div>
          <p className="eyebrow">BROADCAST OPERATIONS</p>
          <h1>Cloud Console</h1>
        </div>
        <span className={`service-status ${health?.status === "ok" ? "online" : ""}`}>
          <span /> {health?.status === "ok" ? "Service online" : "Checking service"}
        </span>
      </header>

      <section className="welcome">
        <p className="eyebrow">STATION CONTROL</p>
        <h2>Your studio, wherever you are.</h2>
        <p className="intro">
          Connect a host or co-host microphone, manage guest invitations, and approve callers.
        </p>
      </section>

      {!auth ? (
        <section className="panel" aria-labelledby="connection-title">
          <div className="panel-heading">
            <div>
              <p className="eyebrow">STATION ACCESS</p>
              <h3 id="connection-title">Operator sign-in</h3>
            </div>
            <span className="status-pill">Not connected</span>
          </div>
          <form onSubmit={signIn}>
            <label htmlFor="api-key">Station API key</label>
            <input
              id="api-key"
              type="password"
              autoComplete="off"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder="Enter the station API key"
              required
            />
            <button type="submit" disabled={!apiKey}>Connect to station</button>
          </form>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      ) : (
        <>
          <div className="operator-bar">
            <p>Station <code>{auth.station_id}</code></p>
            <button type="button" className="secondary" onClick={signOut}>Sign out</button>
          </div>
          <DelayControls apiKey={apiKey} />
          <MixerConsole apiKey={apiKey} />
          {participant && (
            <LiveAudioConnection
              participant={participant}
              onLeave={() => {
                setParticipant(null);
                sessionStorage.removeItem(PARTICIPANT_KEY);
              }}
            />
          )}
          <StationWorkspace
            apiKey={apiKey}
            onParticipant={saveParticipant}
            onError={(message) => setError(message)}
          />
          {error && <p className="error" role="alert">{error}</p>}
        </>
      )}

      <footer>
        <span>Cloud Console</span>
        <a href="/api/docs">API documentation</a>
      </footer>
    </main>
  );
}

function MixerConsole({ apiKey }: { apiKey: string }) {
  const [channels, setChannels] = useState<MixerChannel[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    let retryTimer = 0;
    let socket: WebSocket | null = null;
    const connect = () => {
      const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
      socket = new WebSocket(
        `${scheme}//${window.location.host}/api/mixer/ws?api_key=${encodeURIComponent(apiKey)}`,
      );
      socket.onmessage = (event: MessageEvent<string>) => {
        const message = JSON.parse(event.data) as {
          type: "snapshot" | "update";
          channels?: MixerChannel[];
          channel?: MixerChannel;
        };
        if (message.type === "snapshot") {
          setChannels(message.channels ?? []);
        } else if (message.channel) {
          const updated = message.channel;
          setChannels((current) => {
            const existing = current.find((item) => item.channel_id === updated.channel_id);
            return existing
              ? current.map((item) =>
                  item.channel_id === updated.channel_id ? { ...item, ...updated } : item,
                )
              : current;
          });
        }
      };
      socket.onerror = () => {
        if (active) setError("Mixer synchronization is unavailable.");
      };
      socket.onclose = () => {
        if (active) retryTimer = window.setTimeout(connect, 1500);
      };
    };
    connect();
    return () => {
      active = false;
      window.clearTimeout(retryTimer);
      socket?.close();
    };
  }, [apiKey]);

  async function updateChannel(channelId: string, update: Partial<MixerChannel>) {
    setError("");
    try {
      const response = await fetch(`${API_BASE}/mixer/channels/${channelId}`, {
        method: "PATCH",
        headers: {
          "X-API-Key": apiKey,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(update),
      });
      if (!response.ok) {
        const body = (await response.json().catch(() => ({}))) as { error?: string };
        throw new Error(body.error ?? `Mixer update failed (${response.status})`);
      }
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  return (
    <section className="panel mixer-panel" aria-labelledby="mixer-title">
      <div className="panel-heading">
        <div>
          <p className="eyebrow">SERVER-SIDE AUDIO</p>
          <h3 id="mixer-title">Program &amp; Cue mixer</h3>
        </div>
        <span className="status-pill">{channels.length} channels</span>
      </div>
      <p className="muted">
        Program uses a server-side mix-minus. Cue is a separate shared bus and remains independent of Program ON/OFF.
      </p>
      {channels.length === 0 ? (
        <p className="muted mixer-empty">Approved host and guest channels appear here.</p>
      ) : (
        <div className="mixer-grid">
          {channels.map((channel) => (
            <article className="mixer-channel" key={channel.channel_id}>
              <div className="mixer-channel-heading">
                <div>
                  <strong>{channel.display_name}</strong>
                  <span>{channel.role.replace("_", " ")}</span>
                </div>
                <span className={`status-pill ${channel.is_on && !channel.muted ? "active" : ""}`}>
                  {channel.is_on && !channel.muted ? "ON" : "OFF"}
                </span>
              </div>
              <label className="mixer-fader">
                Program fader
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.01"
                  value={channel.fader}
                  onChange={(event) =>
                    void updateChannel(channel.channel_id, { fader: Number(event.target.value) })
                  }
                />
              </label>
              <div className="inline-actions">
                <button
                  type="button"
                  className={channel.is_on ? "secondary" : ""}
                  onClick={() =>
                    void updateChannel(channel.channel_id, { is_on: !channel.is_on })
                  }
                >
                  {channel.is_on ? "Program ON" : "Program OFF"}
                </button>
                <button
                  type="button"
                  className={channel.cue_enabled ? "" : "secondary"}
                  onClick={() =>
                    void updateChannel(channel.channel_id, { cue_enabled: !channel.cue_enabled })
                  }
                >
                  {channel.cue_enabled ? "Cue ON" : "Cue OFF"}
                </button>
                <button
                  type="button"
                  className={channel.muted ? "" : "secondary"}
                  onClick={() =>
                    void updateChannel(channel.channel_id, { muted: !channel.muted })
                  }
                >
                  {channel.muted ? "Unmute" : "Mute"}
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
      {error && <p className="error" role="alert">{error}</p>}
    </section>
  );
}

function StationWorkspace({
  apiKey,
  onParticipant,
  onError,
}: {
  apiKey: string;
  onParticipant: (participant: Participant) => void;
  onError: (message: string) => void;
}) {
  const [displayName, setDisplayName] = useState("");
  const [role, setRole] = useState("host");
  const [guestName, setGuestName] = useState("");
  const [expiresAt, setExpiresAt] = useState(defaultExpiry());
  const [links, setLinks] = useState<GuestLink[]>([]);
  const [newLink, setNewLink] = useState<GuestLink | null>(null);
  const [sessions, setSessions] = useState<LiveSession[]>([]);
  const [busy, setBusy] = useState(false);

  const operatorHeaders: HeadersInit = { "X-API-Key": apiKey };

  async function refresh() {
    const [guestLinks, liveSessions] = await Promise.all([
      apiRequest<GuestLink[]>("/guest-links", { headers: operatorHeaders }),
      apiRequest<LiveSession[]>("/live-sessions", { headers: operatorHeaders }),
    ]);
    setLinks(guestLinks);
    setSessions(liveSessions);
  }

  useEffect(() => {
    let active = true;
    const load = () => {
      void Promise.all([
        apiRequest<GuestLink[]>("/guest-links", { headers: operatorHeaders }),
        apiRequest<LiveSession[]>("/live-sessions", { headers: operatorHeaders }),
      ])
        .then(([guestLinks, liveSessions]) => {
          if (active) {
            setLinks(guestLinks);
            setSessions(liveSessions);
          }
        })
        .catch((cause: unknown) => {
          if (active) onError(errorMessage(cause));
        });
    };
    load();
    const timer = window.setInterval(load, 3000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [apiKey]);

  async function startLiveSession(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    onError("");
    try {
      const session = await apiRequest<Participant>("/live-sessions", {
        method: "POST",
        headers: { ...operatorHeaders, "Content-Type": "application/json" },
        body: JSON.stringify({ display_name: displayName, role }),
      });
      onParticipant(session);
      setDisplayName("");
      await refresh();
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function createGuestLink(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    onError("");
    try {
      const link = await apiRequest<GuestLink>("/guest-links", {
        method: "POST",
        headers: { ...operatorHeaders, "Content-Type": "application/json" },
        body: JSON.stringify({
          name: guestName.trim() || null,
          expires_at: new Date(expiresAt).toISOString(),
        }),
      });
      setNewLink(link);
      setGuestName("");
      await refresh();
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function decideGuest(id: string, decision: "approve" | "reject") {
    setBusy(true);
    onError("");
    try {
      await apiRequest(`/live-sessions/${id}/${decision}`, {
        method: "POST",
        headers: operatorHeaders,
      });
      await refresh();
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function invalidateLink(id: string) {
    setBusy(true);
    onError("");
    try {
      await apiRequest(`/guest-links/${id}`, {
        method: "DELETE",
        headers: operatorHeaders,
      });
      if (newLink?.id === id) setNewLink(null);
      await refresh();
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function copyInvitation(link: GuestLink) {
    if (!link.invitation_token) return;
    const invitationUrl = `${window.location.origin}/?invite=${encodeURIComponent(link.invitation_token)}`;
    try {
      await navigator.clipboard.writeText(invitationUrl);
    } catch (cause) {
      onError(`Could not copy the invitation link: ${errorMessage(cause)}`);
    }
  }

  const pendingGuests = sessions.filter((session) => session.role === "guest" && session.status === "pending");

  return (
    <div className="workspace-grid">
      <section className="panel">
        <div className="panel-heading">
          <div>
            <p className="eyebrow">STUDIO</p>
            <h3>Connect a microphone</h3>
          </div>
        </div>
        <form onSubmit={startLiveSession}>
          <label htmlFor="display-name">Display name</label>
          <input
            id="display-name"
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
            maxLength={100}
            required
          />
          <label htmlFor="participant-role">Studio role</label>
          <select id="participant-role" value={role} onChange={(event) => setRole(event.target.value)}>
            <option value="host">Host</option>
            <option value="co_host">Co-Host</option>
          </select>
          <button type="submit" disabled={busy}>Start live session</button>
        </form>
      </section>

      <section className="panel">
        <div className="panel-heading">
          <div>
            <p className="eyebrow">INVITATIONS</p>
            <h3>Create a guest link</h3>
          </div>
        </div>
        <form onSubmit={createGuestLink}>
          <label htmlFor="guest-link-name">Invitation name</label>
          <input
            id="guest-link-name"
            value={guestName}
            onChange={(event) => setGuestName(event.target.value)}
            placeholder="Optional guest or segment name"
            maxLength={100}
          />
          <label htmlFor="guest-link-expiry">Link expires</label>
          <input
            id="guest-link-expiry"
            type="datetime-local"
            value={expiresAt}
            onChange={(event) => setExpiresAt(event.target.value)}
            required
          />
          <button type="submit" disabled={busy}>Create invitation</button>
        </form>
        {newLink?.invitation_token && (
          <div className="invitation-created">
            <p>New invitation ready. Copy it now; the token is only shown once.</p>
            <code>{`${window.location.origin}/?invite=${newLink.invitation_token}`}</code>
            <button type="button" className="secondary" onClick={() => void copyInvitation(newLink)}>
              Copy invitation link
            </button>
          </div>
        )}
        <ul className="item-list">
          {links.map((link) => (
            <li key={link.id}>
              <div>
                <strong>{link.name || "Guest invitation"}</strong>
                <span>{link.invalidated_at ? "Invalidated" : `Expires ${formatDate(link.expires_at)}`}</span>
              </div>
              {link.invitation_token ? (
                <button type="button" className="secondary" onClick={() => void copyInvitation(link)}>
                  Copy link
                </button>
              ) : (
                !link.invalidated_at && (
                  <button type="button" className="secondary" disabled={busy} onClick={() => void invalidateLink(link.id)}>
                    Revoke
                  </button>
                )
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className="panel session-panel">
        <div className="panel-heading">
          <div>
            <p className="eyebrow">LIVE SESSIONS</p>
            <h3>Guest approval and studio status</h3>
          </div>
          <span className="status-pill">{pendingGuests.length} awaiting approval</span>
        </div>
        {sessions.length === 0 ? (
          <p className="muted">No recent live sessions.</p>
        ) : (
          <ul className="item-list">
            {sessions.map((session) => (
              <li key={session.id}>
                <div>
                  <strong>{session.display_name}</strong>
                  <span>{roleLabel(session.role)} · {session.status}</span>
                </div>
                {session.role === "guest" && session.status === "pending" && (
                  <div className="inline-actions">
                    <button type="button" disabled={busy} onClick={() => void decideGuest(session.id, "approve")}>
                      Approve
                    </button>
                    <button type="button" className="secondary" disabled={busy} onClick={() => void decideGuest(session.id, "reject")}>
                      Reject
                    </button>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function GuestApp({ invitationToken }: { invitationToken: string }) {
  const [name, setName] = useState("");
  const [participant, setParticipant] = useState<Participant | null>(() =>
    readParticipant(`cloud-console-guest-${invitationToken}`),
  );
  const [status, setStatus] = useState(participant?.status ?? "");
  const [inStudio, setInStudio] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const storageKey = `cloud-console-guest-${invitationToken}`;

  useEffect(() => {
    if (!participant) return;
    let active = true;
    let timer: number | null = null;
    const check = () => {
      void apiRequest<{ status: string }>(`/guest-session/${participant.session_id}`, {
        headers: { Authorization: `Bearer ${participant.access_token}` },
      })
        .then((response) => {
          if (active) {
            setStatus(response.status);
            if (response.status !== "pending" && timer !== null) {
              window.clearInterval(timer);
              timer = null;
            }
          }
        })
        .catch((cause: unknown) => {
          if (active) setError(errorMessage(cause));
        });
    };
    check();
    timer = window.setInterval(check, 2500);
    return () => {
      active = false;
      if (timer !== null) window.clearInterval(timer);
    };
  }, [participant]);

  async function requestToJoin(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const response = await apiRequest<Participant>(
        `/guest-links/${encodeURIComponent(invitationToken)}/sessions`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ name }),
        },
      );
      setParticipant(response);
      setStatus(response.status);
      sessionStorage.setItem(storageKey, JSON.stringify(response));
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  function reconnectWithSameLink() {
    setParticipant(null);
    setStatus("");
    setInStudio(false);
    sessionStorage.removeItem(storageKey);
  }

  return (
    <main className="shell guest-shell">
      <header className="topbar">
        <div className="brand-mark" aria-hidden="true">CC</div>
        <div>
          <p className="eyebrow">GUEST CONNECTION</p>
          <h1>Cloud Console</h1>
        </div>
        <span className="service-status online"><span /> Invitation link</span>
      </header>
      <section className="welcome">
        <p className="eyebrow">YOU&apos;RE INVITED</p>
        <h2>Join the live conversation.</h2>
        <p className="intro">Enter your name and request to connect. Your microphone stays off until the host approves you.</p>
      </section>

      {inStudio && participant ? (
        <LiveAudioConnection
          participant={participant}
          onLeave={() => {
            setInStudio(false);
            setStatus("disconnected");
            const next = { ...participant, status: "disconnected" };
            setParticipant(next);
            sessionStorage.setItem(storageKey, JSON.stringify(next));
          }}
        />
      ) : !participant ? (
        <section className="panel" aria-labelledby="guest-connect-title">
          <div className="panel-heading">
            <div>
              <p className="eyebrow">REGISTER YOUR NAME</p>
              <h3 id="guest-connect-title">Request to join</h3>
            </div>
          </div>
          <form onSubmit={requestToJoin}>
            <label htmlFor="guest-display-name">Your name</label>
            <input
              id="guest-display-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              maxLength={100}
              autoComplete="name"
              required
            />
            <button type="submit" disabled={busy}>{busy ? "Sending request..." : "Connect"}</button>
          </form>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      ) : status === "pending" ? (
        <section className="panel">
          <p className="eyebrow">HOST APPROVAL</p>
          <h3>Waiting for the host</h3>
          <p className="muted">Your request is pending. You can connect your microphone after approval.</p>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      ) : status === "rejected" ? (
        <section className="panel">
          <p className="eyebrow">HOST RESPONSE</p>
          <h3>Connection Rejected</h3>
          <p className="muted">You may request access again while this invitation link is valid.</p>
          <button type="button" onClick={reconnectWithSameLink}>Request access again</button>
        </section>
      ) : status === "expired" ? (
        <section className="panel">
          <h3>Invitation expired</h3>
          <p className="muted">Ask the host for a new invitation link.</p>
        </section>
      ) : (
        <section className="panel">
          <div className="panel-heading">
            <div>
              <p className="eyebrow">HOST APPROVAL</p>
              <h3>{status === "disconnected" ? "You may reconnect" : "You are approved"}</h3>
            </div>
            <span className="status-pill active">Approved</span>
          </div>
          <p className="muted">Connect your microphone when you are ready to join the conversation.</p>
          <button type="button" onClick={() => setInStudio(true)}>Join the live audio</button>
          {error && <p className="error" role="alert">{error}</p>}
        </section>
      )}
      <footer><span>Cloud Console guest connection</span></footer>
    </main>
  );
}

async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, init);
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error ?? `Request failed (${response.status})`);
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

function readParticipant(key = PARTICIPANT_KEY): Participant | null {
  const saved = sessionStorage.getItem(key);
  if (!saved) return null;
  try {
    return JSON.parse(saved) as Participant;
  } catch {
    sessionStorage.removeItem(key);
    return null;
  }
}

function roleLabel(role: string): string {
  if (role === "co_host") return "Co-Host";
  return role.charAt(0).toUpperCase() + role.slice(1);
}

function defaultExpiry(): string {
  const date = new Date(Date.now() + 24 * 60 * 60 * 1000);
  date.setMinutes(date.getMinutes() - date.getTimezoneOffset());
  return date.toISOString().slice(0, 16);
}

function formatDate(value: string): string {
  return new Date(value).toLocaleString();
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred";
}
