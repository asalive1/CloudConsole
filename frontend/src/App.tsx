import { useEffect, useState } from "react";

type Health = {
  status: string;
};

type AuthResponse = {
  authenticated: boolean;
  station_id: string;
  role: string;
};

const API_BASE = "/api";

export default function App() {
  const [apiKey, setApiKey] = useState("");
  const [health, setHealth] = useState<Health | null>(null);
  const [auth, setAuth] = useState<AuthResponse | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    const url = new URL(window.location.href);
    const key = url.searchParams.get("api_key");
    if (key) {
      setApiKey(key);
      url.searchParams.delete("api_key");
      window.history.replaceState({}, "", url);
    }
    void fetch(`${API_BASE}/health`)
      .then(async (response) => {
        if (!response.ok) throw new Error(`Health check failed (${response.status})`);
        setHealth((await response.json()) as Health);
      })
      .catch((cause: unknown) => setError(errorMessage(cause)));
  }, []);

  async function signIn(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError("");
    try {
      const response = await fetch(`${API_BASE}/auth/login`, {
        method: "POST",
        headers: { "X-API-Key": apiKey },
      });
      if (!response.ok) throw new Error(`Authentication failed (${response.status})`);
      setAuth((await response.json()) as AuthResponse);
    } catch (cause) {
      setError(errorMessage(cause));
      setAuth(null);
    }
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
          The platform foundation is ready. Sign in with the station API key to
          verify this station&apos;s connection.
        </p>
      </section>

      <section className="panel" aria-labelledby="connection-title">
        <div className="panel-heading">
          <div>
            <p className="eyebrow">PHASE 1–2 FOUNDATION</p>
            <h3 id="connection-title">Station connection</h3>
          </div>
          <span className="status-pill">{auth ? "Authenticated" : "Not connected"}</span>
        </div>

        {auth ? (
          <div className="connection-details">
            <p><span>Station</span><code>{auth.station_id}</code></p>
            <p><span>Access</span><strong>{auth.role}</strong></p>
          </div>
        ) : (
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
        )}
        {error && <p className="error" role="alert">{error}</p>}
      </section>

      <footer>
        <span>Cloud Console</span>
        <a href="/api/docs">API documentation</a>
      </footer>
    </main>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred";
}
