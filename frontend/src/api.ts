import { useEffect, useState } from "react";

export const API_BASE = "/api";
export const STATION_KEY = "cloud-console-station-key";

export type Participant = {
  session_id: string;
  display_name: string;
  role: string;
  status?: string;
  access_token: string;
  expires_at?: string;
};

export type MixerChannel = {
  channel_id: string;
  display_name: string;
  role: string;
  fader: number;
  is_on: boolean;
  cue_enabled: boolean;
  muted: boolean;
  revision: number;
};

export type LiveSession = {
  id: string;
  display_name: string;
  role: string;
  status: string;
  expires_at: string;
};

export type GuestLink = {
  id: string;
  name: string | null;
  expires_at: string;
  invalidated_at: string | null;
  invitation_token: string | null;
  status: "active" | "expired" | "invalidated";
};

export type SourceMetrics = {
  id: string;
  state: string;
  peak_dbfs: number[] | null;
  jitter_ms: number | null;
  packets_lost: number | null;
  configured_latency_ms: number | null;
};

export type AudioMetrics = {
  pipeline_state: string;
  source_count: number;
  output_count: number;
  program_peak_dbfs: number[] | null;
  pipeline_latency_ms: number | null;
  sources: SourceMetrics[];
  last_pipeline_error: string | null;
};

export type DelayStatus = {
  configured_delay_seconds: number;
  buffered_delay_seconds: number;
  recovery_percent: number;
  enabled?: boolean;
  state: "normal" | "rebuilding" | "ending_delay" | "live";
};

export type MonitorSource = "cue" | "program_mix" | "mix_minus" | "input";

export type MonitorSettings = {
  local_source: MonitorSource;
  local_input_id: string | null;
  local_volume: number;
  local_muted: boolean;
  guest_source: MonitorSource;
  guest_input_id: string | null;
};

export async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, init);
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error ?? `Request failed (${response.status})`);
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred";
}

export function roleLabel(role: string): string {
  if (role === "co_host") return "Co-Host";
  return role.charAt(0).toUpperCase() + role.slice(1);
}

export function readStored<T>(storage: Storage, key: string): T | null {
  const saved = storage.getItem(key);
  if (!saved) return null;
  try {
    return JSON.parse(saved) as T;
  } catch {
    storage.removeItem(key);
    return null;
  }
}

// Polls an authenticated operator endpoint; returns the latest value and error.
export function usePolling<T>(
  path: string,
  apiKey: string,
  intervalMs: number,
): { data: T | null; error: string; refresh: () => void } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);

  useEffect(() => {
    let active = true;
    const load = () => {
      void apiRequest<T>(path, { headers: { "X-API-Key": apiKey } })
        .then((next) => {
          if (active) {
            setData(next);
            setError("");
          }
        })
        .catch((cause: unknown) => {
          if (active) setError(errorMessage(cause));
        });
    };
    load();
    const timer = window.setInterval(load, intervalMs);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [path, apiKey, intervalMs, version]);

  return { data, error, refresh: () => setVersion((value) => value + 1) };
}

// Keeps the operator's mixer channel list in sync over the mixer websocket.
export function useMixerChannels(apiKey: string): { channels: MixerChannel[]; error: string } {
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
      socket.onopen = () => setError("");
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
          setChannels((current) =>
            current.map((item) =>
              item.channel_id === updated.channel_id ? { ...item, ...updated } : item,
            ),
          );
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

  // Sessions joining or leaving change the snapshot, so refresh it periodically.
  useEffect(() => {
    let active = true;
    const load = () => {
      void apiRequest<MixerChannel[]>("/mixer/channels", { headers: { "X-API-Key": apiKey } })
        .then((next) => {
          if (active) setChannels(next);
        })
        .catch(() => undefined);
    };
    load();
    const timer = window.setInterval(load, 3000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [apiKey]);

  return { channels, error };
}
