import { useEffect, useState } from "react";

type DelayStatus = {
  configured_delay_seconds: number;
  buffered_delay_seconds: number;
  recovery_percent: number;
  state: "normal" | "dumped" | "rebuilding" | "ending_delay" | "live";
};

type Props = {
  apiKey?: string;
  accessToken?: string;
};

export default function DelayControls({ apiKey, accessToken }: Props) {
  const [status, setStatus] = useState<DelayStatus | null>(null);
  const [delaySeconds, setDelaySeconds] = useState(5);
  const [delayEdited, setDelayEdited] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function request<T>(path: string, method = "GET", body?: unknown): Promise<T> {
    const headers: HeadersInit = { "Content-Type": "application/json" };
    if (apiKey) headers["X-API-Key"] = apiKey;
    if (accessToken) headers.Authorization = `Bearer ${accessToken}`;
    const response = await fetch(`/api/delay${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const result = (await response.json().catch(() => ({}))) as T & { error?: string };
    if (!response.ok) throw new Error(result.error ?? `Delay request failed (${response.status})`);
    return result;
  }

  async function refresh() {
    try {
      const current = await request<DelayStatus>("");
      setStatus(current);
      if (!delayEdited) setDelaySeconds(current.configured_delay_seconds);
      setError("");
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 500);
    return () => window.clearInterval(timer);
  }, [apiKey, accessToken, delayEdited]);

  async function runAction(path: string, body?: unknown) {
    setBusy(true);
    setError("");
    try {
      const updated = await request<DelayStatus>(path, path === "" ? "PATCH" : "POST", body);
      if (path === "") {
        setDelaySeconds(updated.configured_delay_seconds);
        setDelayEdited(false);
      }
      await refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const buffered = status?.buffered_delay_seconds ?? 0;
  const configured = status?.configured_delay_seconds ?? delaySeconds;
  const hasDelay = buffered > 0.05;

  return (
    <section className="panel delay-panel" aria-labelledby="delay-title">
      <div className="panel-heading">
        <div>
          <p className="eyebrow">BROADCAST SAFETY</p>
          <h3 id="delay-title">Program delay</h3>
        </div>
        <span className={`status-pill ${hasDelay ? "active" : ""}`}>
          {status?.state.replace("_", " ") ?? "Loading"}
        </span>
      </div>
      <div className="delay-controls">
        <label className="delay-config">
          Target delay
          <span>
            <input
              type="number"
              min="5"
              max="60"
              step="1"
              value={delaySeconds}
              onChange={(event) => {
                setDelayEdited(true);
                setDelaySeconds(Number(event.target.value));
              }}
            />
            seconds
          </span>
        </label>
        <button
          type="button"
          className="secondary"
          disabled={busy || delaySeconds < 5 || delaySeconds > 60 || delaySeconds === configured}
          onClick={() => void runAction("", { delay_seconds: delaySeconds })}
        >
          Set delay
        </button>
        <button type="button" disabled={busy || !hasDelay} onClick={() => void runAction("/dump")}>
          DUMP
        </button>
        <button
          type="button"
          className="secondary"
          disabled={busy || !hasDelay}
          onClick={() => void runAction("/end")}
        >
          End delay
        </button>
      </div>
      <div className="delay-meter" aria-live="polite">
        <div className="delay-meter-heading">
          <span>Delay buffer</span>
          <strong>{buffered.toFixed(1)} / {configured}s</strong>
        </div>
        <progress value={Math.min(buffered, configured)} max={configured || 1} />
      </div>
      {status?.state === "rebuilding" && (
        <div className="delay-meter">
          <div className="delay-meter-heading">
            <span>Recovery</span>
            <strong>{status.recovery_percent.toFixed(0)}%</strong>
          </div>
          <progress value={status.recovery_percent} max="100" />
        </div>
      )}
      <p className="muted delay-note">Applied to configured Program outputs; host and guest monitoring and Cue remain pre-delay.</p>
      {error && <p className="error" role="alert">{error}</p>}
    </section>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred";
}
