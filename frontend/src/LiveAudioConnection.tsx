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
  cue_room_id: number;
  room_pin: string;
  ice_servers: RTCIceServer[];
};

type MixerChannel = {
  channel_id: string;
  fader: number;
  is_on: boolean;
  cue_enabled: boolean;
  muted: boolean;
  revision: number;
};

type Props = {
  participant: Participant;
  onLeave: () => void;
};

const API_BASE = "/api";
const PLUGIN = "janus.plugin.audiobridge";

let janusInit: Promise<void> | null = null;

export default function LiveAudioConnection({ participant, onLeave }: Props) {
  const [status, setStatus] = useState("Ready to connect");
  const [error, setError] = useState("");
  const [manualMute, setManualMute] = useState(false);
  const [monitorVolume, setMonitorVolume] = useState(1);
  const [connecting, setConnecting] = useState(false);
  const [connected, setConnected] = useState(false);
  const [channel, setChannel] = useState<MixerChannel>({
    channel_id: participant.session_id,
    fader: 1,
    is_on: true,
    cue_enabled: false,
    muted: false,
    revision: 0,
  });
  const janusRef = useRef<Janus | null>(null);
  const mixerSocket = useRef<WebSocket | null>(null);
  const handles = useRef(new Map<"program" | "cue", JanusPluginHandle>());
  const remoteAudio = useRef(new Map<"program" | "cue", HTMLAudioElement>());
  const reconnectTimer = useRef<number | null>(null);
  const reconnectAttempts = useRef(0);
  const stopped = useRef(false);
  const wasConnected = useRef(false);
  const channelRef = useRef(channel);
  const manualMuteRef = useRef(manualMute);
  const monitorVolumeRef = useRef(monitorVolume);

  useEffect(() => {
    channelRef.current = channel;
    applyMixerState();
  }, [channel]);

  useEffect(() => {
    manualMuteRef.current = manualMute;
    applyMixerState();
  }, [manualMute]);

  useEffect(() => {
    monitorVolumeRef.current = monitorVolume;
    for (const audio of remoteAudio.current.values()) audio.volume = monitorVolume;
  }, [monitorVolume]);

  useEffect(() => {
    stopped.current = false;
    return () => {
      stopped.current = true;
      if (reconnectTimer.current !== null) window.clearTimeout(reconnectTimer.current);
      void updateConnectionState("disconnected");
      mixerSocket.current?.close();
      mixerSocket.current = null;
      janusRef.current?.destroy({});
      janusRef.current = null;
      handles.current.clear();
      clearRemoteAudio();
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

  function connectMixerSocket() {
    mixerSocket.current?.close();
    const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(
      `${scheme}//${window.location.host}/api/mixer/ws?token=${encodeURIComponent(participant.access_token)}`,
    );
    mixerSocket.current = socket;
    socket.onmessage = (event: MessageEvent<string>) => {
      const message = JSON.parse(event.data) as {
        type: "snapshot" | "update";
        channels?: MixerChannel[];
        channel?: MixerChannel;
      };
      const next = message.type === "update"
        ? (message.channel ? [message.channel] : [])
        : message.channels ?? [];
      const own = next.find((item) => item.channel_id === participant.session_id);
      if (own && own.revision >= channelRef.current.revision) {
        channelRef.current = own;
        setChannel(own);
      }
    };
    socket.onerror = () => {
      if (!stopped.current) setError("Mixer synchronization is unavailable.");
    };
    return socket;
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
    handles.current.clear();
    clearRemoteAudio();
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
      connectMixerSocket();
      const websocketUrl = new URL(
        config.websocket_url ||
          `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/janus`,
      );
      websocketUrl.searchParams.set("token", participant.access_token);
      const options: JanusJS.ConstructorOptions = {
        server: websocketUrl.toString(),
        success: () => {
          attachRoom("program", config, config.room_id);
          attachRoom("cue", config, config.cue_room_id);
        },
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

  function attachRoom(
    bus: "program" | "cue",
    config: ConnectionConfig,
    roomId: number,
  ) {
    janusRef.current?.attach({
      plugin: PLUGIN,
      opaqueId: `cloud-console-${bus}-${participant.session_id}`,
      success: (handle) => {
        handles.current.set(bus, handle);
        handle.send({
          message: {
            request: "join",
            room: roomId,
            display: participant.display_name,
            pin: config.room_pin,
            muted: true,
          },
        });
      },
      error: (cause) => {
        setError(`Could not join ${bus} audio: ${errorMessage(cause)}`);
        if (bus === "program") scheduleReconnect();
      },
      onmessage: (message, jsep) => {
        const event = pluginData(message);
        if (jsep) handles.current.get(bus)?.handleRemoteJsep({ jsep });
        if (event.audiobridge === "joined") publishAudio(bus);
        if (typeof event.error === "string") {
          setError(`${bus} audio error: ${event.error}`);
        }
      },
      onremotetrack: (track, _mid, added) => {
        if (track.kind !== "audio") return;
        if (!added) {
          removeRemoteAudio(bus);
          return;
        }
        let audio = remoteAudio.current.get(bus);
        if (!audio) {
          audio = new Audio();
          audio.autoplay = true;
          audio.volume = monitorVolumeRef.current;
          remoteAudio.current.set(bus, audio);
        }
        audio.srcObject = new MediaStream([track]);
        void audio.play().catch((cause: unknown) => {
          setError(`Click Enable monitor audio to hear the studio: ${errorMessage(cause)}`);
        });
      },
      webrtcState: (isActive) => {
        if (bus !== "program" || stopped.current) return;
        if (isActive) {
          wasConnected.current = true;
          reconnectAttempts.current = 0;
          setConnected(true);
          setStatus("Connected");
          void updateConnectionState("connected");
        } else if (wasConnected.current) {
          setConnected(false);
          void updateConnectionState("disconnected");
          scheduleReconnect();
        }
      },
      oncleanup: () => {
        handles.current.delete(bus);
        removeRemoteAudio(bus);
      },
    });
  }

  function publishAudio(bus: "program" | "cue") {
    const handle = handles.current.get(bus);
    if (!handle) return;
    handle.createOffer({
      media: { audio: true, video: false },
      success: (jsep) => {
        handle.send({
          message: { request: "configure", ...roomSettings(bus) },
          jsep,
        });
      },
      error: (cause) => {
        setError(`Microphone access failed: ${errorMessage(cause)}`);
        if (bus === "program") scheduleReconnect();
      },
    });
  }

  function roomSettings(bus: "program" | "cue") {
    const current = channelRef.current;
    const commonMuted = manualMuteRef.current || current.muted;
    const muted = bus === "program"
      ? commonMuted || !current.is_on
      : commonMuted || !current.cue_enabled;
    return { muted, volume: Math.round(current.fader * 100) };
  }

  function applyMixerState() {
    for (const bus of ["program", "cue"] as const) {
      const handle = handles.current.get(bus);
      if (handle) handle.send({ message: { request: "configure", ...roomSettings(bus) } });
    }
  }

  function toggleMute() {
    setManualMute((value) => !value);
  }

  function enableMonitorAudio() {
    for (const audio of remoteAudio.current.values()) {
      void audio.play().catch((cause: unknown) => setError(errorMessage(cause)));
    }
  }

  function clearRemoteAudio() {
    for (const bus of ["program", "cue"] as const) removeRemoteAudio(bus);
  }

  function removeRemoteAudio(bus: "program" | "cue") {
    const audio = remoteAudio.current.get(bus);
    if (audio) {
      audio.pause();
      audio.srcObject = null;
      remoteAudio.current.delete(bus);
    }
  }

  function leave() {
    stopped.current = true;
    if (reconnectTimer.current !== null) window.clearTimeout(reconnectTimer.current);
    void updateConnectionState("disconnected");
    mixerSocket.current?.close();
    mixerSocket.current = null;
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
        {" "}microphone · server-mixed Program and private Cue buses.
      </p>
      {!connected && (
        <button type="button" onClick={() => void startConnection(true)} disabled={connecting}>
          {connecting ? "Connecting..." : reconnectAttempts.current > 0 ? "Retry connection" : "Connect microphone"}
        </button>
      )}
      {connected && (
        <div className="audio-controls">
          <button type="button" className={manualMute ? "secondary" : ""} onClick={toggleMute}>
            {manualMute ? "Unmute microphone" : "Mute microphone"}
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
