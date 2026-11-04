import { useEffect, useState } from "react";
import AdminConsole from "./AdminConsole";
import Console from "./Console";
import GuestApp from "./GuestApp";
import { apiRequest, errorMessage, STATION_KEY } from "./api";

export default function App() {
  const invite = new URLSearchParams(window.location.search).get("invite");
  if (invite) return <GuestApp invitationToken={invite} />;
  return window.location.pathname.startsWith("/admin") ? <AdminConsole /> : <StationApp />;
}

function StationApp() {
  const [apiKey, setApiKey] = useState(() => sessionStorage.getItem(STATION_KEY) ?? "");
  const [signedIn, setSignedIn] = useState(false);
  const [error, setError] = useState("");

  async function signIn(key: string) {
    try {
      await apiRequest("/auth/login", { method: "POST", headers: { "X-API-Key": key } });
      sessionStorage.setItem(STATION_KEY, key);
      setApiKey(key);
      setSignedIn(true);
      setError("");
    } catch (cause) {
      setError(errorMessage(cause));
      setSignedIn(false);
    }
  }

  useEffect(() => {
    const url = new URL(window.location.href);
    const key = url.searchParams.get("api_key");
    if (key) {
      url.searchParams.delete("api_key");
      window.history.replaceState({}, "", url);
      void signIn(key);
    } else if (apiKey) {
      void signIn(apiKey);
    }
  }, []);

  if (signedIn) {
    return (
      <Console
        apiKey={apiKey}
        onSignOut={() => {
          sessionStorage.removeItem(STATION_KEY);
          setSignedIn(false);
          setApiKey("");
        }}
      />
    );
  }

  return (
    <main className="console">
      <header className="bar"><strong className="brand">Cloud Console</strong></header>
      <section className="card">
        <h2>Operator sign-in</h2>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void signIn(apiKey);
          }}
        >
          <label>
            <span className="label">Station API key</span>
            <input type="password" autoComplete="off" value={apiKey} onChange={(event) => setApiKey(event.target.value)} required />
          </label>
          <button type="submit" className="btn btn-build" disabled={!apiKey}>Connect to station</button>
        </form>
        {error && <p className="error" role="alert">{error}</p>}
      </section>
    </main>
  );
}
