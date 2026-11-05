import { useEffect, useState } from "react";
import { apiRequest, errorMessage, type DelayStatus } from "./api";

type Props = {
  apiKey?: string;
  accessToken?: string;
  adminHref?: string;
};

const STATE_LABEL: Record<DelayStatus["state"], string> = {
  normal: "Delay ready",
  live: "Delay live",
  dumped: "Dumped",
  rebuilding: "Building",
  ending_delay: "Ending",
};

export default function DelayControls({ apiKey, accessToken, adminHref }: Props) {
  const [status, setStatus] = useState<DelayStatus | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  function headers(): HeadersInit {
    const result: Record<string, string> = {};
    if (apiKey) result["X-API-Key"] = apiKey;
    if (accessToken) result.Authorization = `Bearer ${accessToken}`;
    return result;
  }

  useEffect(() => {
    let active = true;
    const load = () => {
      void apiRequest<DelayStatus>("/delay", { headers: headers() })
        .then((next) => {
          if (active) {
            setStatus(next);
            setError("");
          }
        })
        .catch((cause: unknown) => {
          if (active) setError(errorMessage(cause));
        });
    };
    load();
    const timer = window.setInterval(load, 500);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [apiKey, accessToken]);

  async function act(action: "build" | "end" | "dump") {
    setBusy(true);
    try {
      setStatus(await apiRequest<DelayStatus>(`/delay/${action}`, { method: "POST", headers: headers() }));
      setError("");
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const target = status?.configured_delay_seconds ?? 0;
  const buffered = status?.buffered_delay_seconds ?? 0;
  const percent = target > 0 ? Math.min(100, (buffered / target) * 100) : 0;
  const state = status?.state ?? "normal";

  return (
    <section className={`card delay-card state-${state}`} aria-label="Profanity delay">
      <div className="delay-readout">
        <div>
          <span className="label">Delay buffer</span>
          <strong className="delay-value">{buffered.toFixed(1)}s</strong>
          <span className="muted"> / {target.toFixed(0)}s</span>
        </div>
        <span className={`chip chip-${state}`}>{status ? STATE_LABEL[state] : "…"}</span>
        <div className="delay-bar" aria-hidden="true">
          <div className="bar-fill" style={{ width: `${percent}%` }} />
        </div>
      </div>
      <div className="delay-actions">
        <button type="button" className="btn btn-dump" disabled={busy || !status || buffered <= 0.05} onClick={() => void act("dump")}>
          DUMP
        </button>
        <button
          type="button"
          className="btn btn-build"
          disabled={busy || !status || state === "rebuilding" || buffered >= target - 0.05}
          onClick={() => void act("build")}
        >
          Build delay
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy || !status || state === "ending_delay" || buffered <= 0.05}
          onClick={() => void act("end")}
        >
          End delay
        </button>
        {adminHref && <a className="link-small" href={adminHref}>Set time</a>}
      </div>
      {error && <p className="error" role="alert">{error}</p>}
    </section>
  );
}
