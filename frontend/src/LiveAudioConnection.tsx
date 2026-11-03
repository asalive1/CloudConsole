import { useEffect, useRef, useState } from "react";
import Janus, { type JanusJS } from "janus-gateway";

type JanusMessage = JanusJS.Message;
type JanusPluginHandle = JanusJS.PluginHandle;

type Participant = {
  session_id: string;
  display_name: string;
  role: string;
  access_token: string;
};

type ConnectionConfig = {
  session_id: string;
  display_name: string;
  role: string;
  websocket_url: string;
  room_id: number;
  room_pin: string;
  ice_servers: RTCIceServer[];
};

type Props = {
  participant: Participant;
  onLeave: () => void;
};

const API_BASE = "/api";
const PLUGIN = "janus.plugin.videoroom";

let janusInit: Promise<void> | null = null;

export default function LiveAudioConnection({ participant, onLeave }: Props) {
  const [status, setStatus] = useState("Ready to connect");
  const [error, setError] = useState("");
  const [muted, setMuted] = useState(false);
  const [monitorVolume, setMonitorVolume] = useState(1);
  const [connecting, setConnecting] = useState(false);
  const [connected, setConnected] = useState(false);
  const janusRef = useRef<Janus | null>(null);
  const publisherRef = useRef<JanusPluginHandle | null>(null);
  const remoteHandles = useRef(new Map<number, JanusPluginHandle>());
  const remoteAudio = useRef(new Map<number, HTMLAudioElement>());
  const activeHandles = useRef(new Set<JanusPluginHandle>());
  const reconnectTimer = useRef<number | null>(null);
  const reconnectAttempts = useRef(0);
  const stopped = useRef(false);
  const wasConnected = useRef(false);
  const volumeRef = useRef(monitorVolume);

  useEffect(() => {
    volumeRef.current = monitorVolume;
    for (const audio of remoteAudio.current.values()) audio.volume = monitorVolume;
  }, [monitorVolume]);

  useEffect(() => {
    stopped.current = false;
    return () => {
      stopped.current = true;
      if (reconnectTimer.current !== null) window.clearTimeout(reconnectTimer.current);
      void updateConnectionState("disconnected");
      janusRef.current?.destroy({});
      janusRef.current = null;
      publisherRef.current = null;
      remoteHandles.current.clear();
      activeHandles.current.clear();
      for (const audio of remoteAudio.current.values()) {
        audio.pause();
        audio.srcObject = null;
      }
      remoteAudio.current.clear();
    };
  }, [participant.session_id, participant.access_token]);

  async function updateConnectionState(state: "connected" | "disconnected") {
    try {
      await request(`/live-sessions/${participant.session_id}/connection`, {
        method: "PATCH",
        headers: authorizationHeaders(),
        body: JSON.stringify({ state }),
      });
    } catch (cause) {
      if (!stopped.current) setError(errorMessage(cause));
    }
  }

  function authorizationHeaders(): HeadersInit {
    return {
      Authorization: `Bearer ${participant.access_token}`,
      "Content-Type": "application/json",
    };
  }

  async function request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${API_BASE}${path}`, init);
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as { error?: string };
      throw new Error(body.error ?? `Request failed (${response.status})`);
    }
    return (await response.json()) as T;
  }

  function scheduleReconnect() {
    if (stopped.current || reconnectTimer.current !== null || reconnectAttempts.current >= 6) {
      if (reconnectAttempts.current >= 6) {
        setStatus("Connection lost");
        setError("Automatic reconnection failed. Retry when your network is available.");
        void updateConnectionState("disconnected");
      }
      return;
    }
    const delay = Math.min(1000 * 2 ** reconnectAttempts.current, 15000);
    reconnectAttempts.current += 1;
    setStatus("Reconnecting");
    reconnectTimer.current = window.setTimeout(() => {
      reconnectTimer.current = null;
      void startConnection();
    }, delay);
  }

  async function startConnection(manualRetry = false) {
    if (connecting || stopped.current) return;
    if (manualRetry) reconnectAttempts.current = 0;
    wasConnected.current = false;
    janusRef.current?.destroy({});
    janusRef.current = null;
    publisherRef.current = null;
    remoteHandles.current.clear();
    activeHandles.current.clear();
    for (const audio of remoteAudio.current.values()) {
      audio.pause();
      audio.srcObject = null;
    }
    remoteAudio.current.clear();
    setConnected(false);
    setConnecting(true);
    setError("");
    setStatus("Connecting");
    try {
      await initializeJanus();
      if (!Janus.isWebrtcSupported()) {
        throw new Error("This browser does not support WebRTC.");
      }
      const config = await request<ConnectionConfig>(
        `/live-sessions/${participant.session_id}/connection`,
        { headers: authorizationHeaders() },
      );
      const websocketUrl = new URL(
        config.websocket_url ||
          `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/janus`,
      );
      websocketUrl.searchParams.set("token", participant.access_token);
      const options: JanusJS.ConstructorOptions = {
        server: websocketUrl.toString(),
        success: () => attachPublisher(config),
        error: (cause) => {
          setError(errorMessage(cause));
          scheduleReconnect();
        },
        destroyed: () => {
          if (!stopped.current && wasConnected.current) scheduleReconnect();
        },
      };
      if (config.ice_servers.length > 0) options.iceServers = config.ice_servers;
      janusRef.current = new Janus(options);
    } catch (cause) {
      setError(errorMessage(cause));
      scheduleReconnect();
    } finally {
      setConnecting(false);
    }
  }

  function attachPublisher(config: ConnectionConfig) {
    janusRef.current?.attach({
      plugin: PLUGIN,
      opaqueId: `cloud-console-${participant.session_id}`,
      success: (handle) => {
        publisherRef.current = handle;
        handle.send({
          message: {
            request: "join",
            room: config.room_id,
            ptype: "publisher",
            display: config.display_name,
            pin: config.room_pin,
          },
        });
      },
      error: (cause) => {
        setError(errorMessage(cause));
        scheduleReconnect();
      },
      onmessage: (message, jsep) => {
        const event = pluginData(message);
        if (jsep) publisherRef.current?.handleRemoteJsep({ jsep });
        if (event.videoroom === "joined") {
          if (Array.isArray(event.publishers)) {
            for (const publisher of event.publishers) {
              if (isPublisher(publisher)) subscribeToPublisher(publisher.id, config);
            }
          }
          publishAudio();
        } else if (Array.isArray(event.publishers)) {
          for (const publisher of event.publishers) {
            if (isPublisher(publisher)) subscribeToPublisher(publisher.id, config);
          }
        }
        if (typeof event.leaving === "number") removePublisher(event.leaving);
        if (typeof event.unpublished === "number") removePublisher(event.unpublished);
      },
      webrtcState: (isActive) => updateHandleState(publisherRef.current, isActive),
      oncleanup: () => {
        publisherRef.current = null;
      },
    });
  }

  function publishAudio() {
    publisherRef.current?.createOffer({
      media: { audio: true, video: false },
      success: (jsep) => {
        publisherRef.current?.send({
          message: { request: "publish", audio: true, video: false },
          jsep,
        });
      },
      error: (cause) => {
        setError(`Microphone access failed: ${errorMessage(cause)}`);
        scheduleReconnect();
      },
    });
  }

  function subscribeToPublisher(id: number, config: ConnectionConfig) {
    if (remoteHandles.current.has(id)) return;
    janusRef.current?.attach({
      plugin: PLUGIN,
      success: (handle) => {
        remoteHandles.current.set(id, handle);
        handle.send({
          message: {
            request: "join",
            room: config.room_id,
            ptype: "subscriber",
            feed: id,
            pin: config.room_pin,
          },
        });
      },
      error: (cause) => setError(`Could not connect to another participant: ${errorMessage(cause)}`),
      onmessage: (message, jsep) => {
        if (jsep) {
          const event = pluginData(message);
          const handle = remoteHandles.current.get(id);
          handle?.createAnswer({
            jsep,
            tracks: [{ type: "audio", capture: false, recv: true }],
            success: (answer) => {
              handle.send({
                message: { request: "start", room: config.room_id },
                jsep: answer,
              });
            },
            error: (cause) => setError(`Could not receive audio: ${errorMessage(cause)}`),
          });
          if (event.videoroom === "event") setStatus("Connected");
        }
      },
      onremotetrack: (track, _mid, added) => {
        if (track.kind !== "audio") return;
        if (added) {
          let audio = remoteAudio.current.get(id);
          if (!audio) {
            audio = new Audio();
            audio.autoplay = true;
            audio.volume = volumeRef.current;
            remoteAudio.current.set(id, audio);
          }
          audio.srcObject = new MediaStream([track]);
          void audio.play().catch((cause: unknown) => {
            setError(`Click Enable monitor audio to hear the studio: ${errorMessage(cause)}`);
          });
        } else {
          const audio = remoteAudio.current.get(id);
          if (audio) {
            audio.pause();
            audio.srcObject = null;
            remoteAudio.current.delete(id);
          }
        }
      },
      webrtcState: (isActive) => updateHandleState(remoteHandles.current.get(id), isActive),
    });
  }

  function updateHandleState(handle: JanusPluginHandle | undefined | null, isActive: boolean) {
    if (!handle || stopped.current) return;
    if (isActive) {
      activeHandles.current.add(handle);
      wasConnected.current = true;
      reconnectAttempts.current = 0;
      setConnected(true);
      setStatus("Connected");
      void updateConnectionState("connected");
    } else {
      activeHandles.current.delete(handle);
      if (wasConnected.current && activeHandles.current.size === 0) {
        setConnected(false);
        void updateConnectionState("disconnected");
        scheduleReconnect();
      }
    }
  }

  function removePublisher(id: number) {
    remoteHandles.current.delete(id);
    const audio = remoteAudio.current.get(id);
    if (audio) {
      audio.pause();
      audio.srcObject = null;
      remoteAudio.current.delete(id);
    }
  }

  function toggleMute() {
    const publisher = publisherRef.current;
    if (!publisher) return;
    if (muted) publisher.unmuteAudio();
    else publisher.muteAudio();
    setMuted(!muted);
  }

  function enableMonitorAudio() {
    for (const audio of remoteAudio.current.values()) {
      void audio.play().catch((cause: unknown) => setError(errorMessage(cause)));
    }
  }

  function leave() {
    stopped.current = true;
    if (reconnectTimer.current !== null) window.clearTimeout(reconnectTimer.current);
    void updateConnectionState("disconnected");
    janusRef.current?.destroy({});
    janusRef.current = null;
    onLeave();
  }

  return (
    <section className="panel live-panel" aria-labelledby="live-title">
      <div className="panel-heading">
        <div>
          <p className="eyebrow">LIVE AUDIO</p>
          <h3 id="live-title">{participant.display_name}</h3>
        </div>
        <span className={`status-pill ${connected ? "active" : ""}`}>{status}</span>
      </div>
      <p className="live-description">
        {participant.role === "guest" ? "Guest" : participant.role === "co_host" ? "Co-Host" : "Host"}
        {" "}microphone connection to the studio.
      </p>
      {!connected && (
        <button type="button" onClick={() => void startConnection(true)} disabled={connecting}>
          {connecting ? "Connecting..." : reconnectAttempts.current > 0 ? "Retry connection" : "Connect microphone"}
        </button>
      )}
      {connected && (
        <div className="audio-controls">
          <button type="button" className={muted ? "secondary" : ""} onClick={toggleMute}>
            {muted ? "Unmute microphone" : "Mute microphone"}
          </button>
          <label className="volume-control" htmlFor="monitor-volume">
            Monitor volume
            <input
              id="monitor-volume"
              type="range"
              min="0"
              max="1"
              step="0.05"
              value={monitorVolume}
              onChange={(event) => setMonitorVolume(Number(event.target.value))}
            />
          </label>
          <button type="button" className="secondary" onClick={enableMonitorAudio}>
            Enable monitor audio
          </button>
        </div>
      )}
      {error && <p className="error" role="alert">{error}</p>}
      <button type="button" className="text-button" onClick={leave}>Leave studio</button>
    </section>
  );
}

function pluginData(message: JanusMessage): JanusMessage {
  return message.plugindata?.data ?? message;
}

function isPublisher(value: unknown): value is { id: number; display?: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    "id" in value &&
    typeof value.id === "number"
  );
}

function initializeJanus(): Promise<void> {
  if (!janusInit) {
    janusInit = new Promise((resolve) => {
      Janus.init({
        debug: false,
        dependencies: Janus.useDefaultDependencies(),
        callback: () => resolve(),
      });
    });
  }
  return janusInit;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred";
}
