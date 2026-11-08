import { useEffect, useState, type CSSProperties } from "react";
import { apiRequest, errorMessage, type DelayStatus } from "./api";

type Props = {
  apiKey?: string;
  accessToken?: string;
};

const STATE_LABEL: Record<DelayStatus["state"], string> = {
  normal: "Ready",
  live: "Delay off",
  rebuilding: "Building",
  ending_delay: "Ending",
};

export default function DelayControls({ apiKey, accessToken }: Props) {
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
  const buffered = status?.state === "live" ? 0 : (status?.buffered_delay_seconds ?? 0);
  const percent = target > 0 ? Math.min(100, (buffered / target) * 100) : 0;
  const state = status?.state ?? "normal";

  if (status && status.enabled === false) return null;

  return (
    <div className={`strip delay-strip state-${state}`} aria-label="Profanity delay">
      <div className="delay-readout">
        <span className="strip-name delay-value" title="Delay buffer / target">
          {buffered.toFixed(1)}s<span className="muted">/{target.toFixed(0)}s</span>
        </span>
        <span className={`chip chip-${state}`}>{status ? STATE_LABEL[state] : "…"}</span>
      </div>
      <div className="strip-body delay-body">
        <div className="vbar" aria-hidden="true" style={{ "--delay-fill": `${percent}%` } as CSSProperties}>
          <div className="vbar-fill" />
        </div>
        <div className="delay-actions">
          <button
            type="button"
            className="btn btn-small btn-build"
            disabled={busy || !status || state === "rebuilding" || buffered >= target - 0.05}
            onClick={() => void act("build")}
          >
            Build
          </button>
          <button
            type="button"
            className="btn btn-small"
            disabled={busy || !status || state === "ending_delay" || buffered <= 0.05}
            onClick={() => void act("end")}
          >
            End
          </button>
        </div>
      </div>
      <button
        type="button"
        className="btn btn-dump"
        disabled={busy || !status || buffered <= 0.05}
        onClick={() => void act("dump")}
      >
        DUMP
      </button>
      {error && <span className="small error" role="alert" title={error}>!</span>}
    </div>
  );
}