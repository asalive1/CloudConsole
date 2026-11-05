import { useEffect, useRef, useState } from "react";
import Janus, { type JanusJS } from "janus-gateway";
import adapter from "webrtc-adapter";
import { API_BASE, type MixerChannel, type Participant } from "./api";

type Bus = "program" | "cue";
type JanusMessage = JanusJS.Message;
type JanusPluginHandle = JanusJS.PluginHandle;

type ConnectionConfig = {
  websocket_url: string;
  room_id: number;
  cue_room_id: number;
  room_pin: string;
  ice_servers: RTCIceServer[];
};

export type LiveMode = "off" | "listen" | "live";

export type LiveAudioOptions = {
  participant: Participant | null;
  // "listen" joins receive-only so the user can hear a monitor feed without sending audio.
  mode: LiveMode;
  inputDeviceId: string;
  outputDeviceId: string;
  monitorBus: Bus;
  monitorVolume: number;
  monitorMuted: boolean;
  micMuted: boolean;
  // Plays the local microphone in the monitor so "Program (full mix)" includes the user.
  sidetone: boolean;
};

export type LiveAudioState = {
  status: string;
  error: string;
  connected: boolean;
  channel: MixerChannel | null;
  // Approximate dBFS per session id, from Janus talking events and the local microphone.
  levels: Record<string, number>;
};

const PLUGIN = "janus.plugin.audiobridge";
let janusInit: Promise<void> | null = null;

const DEFAULT_CHANNEL = (id: string): MixerChannel => ({
  channel_id: id,
  display_name: "",
  role: "",
  fader: 1,
  is_on: true,
  cue_enabled: false,
  muted: false,
  revision: 0,
});

export function useLiveAudio(options: LiveAudioOptions): LiveAudioState {
  const { participant, mode, inputDeviceId } = options;
  const [status, setStatus] = useState("Not connected");
  const [error, setError] = useState("");
  const [connected, setConnected] = useState(false);
  const [channel, setChannel] = useState<MixerChannel | null>(null);
  const [levels, setLevels] = useState<Record<string, number>>({});
  const roomNames = useRef(new Map<number, string>());
  const meterStop = useRef<(() => void) | null>(null);

  const setLevel = (id: string, db: number | null) =>
    setLevels((previous) => {
      if (db === null) {
        if (!(id in previous)) return previous;
        const { [id]: _removed, ...rest } = previous;
        return rest;
      }
      return { ...previous, [id]: db };
    });
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const channelRef = useRef<MixerChannel | null>(null);
  const handles = useRef(new Map<Bus, JanusPluginHandle>());
  const remoteAudio = useRef(new Map<Bus, HTMLAudioElement>());
  const sidetoneAudio = useRef<HTMLAudioElement | null>(null);

  function applyMonitor() {
    const current = optionsRef.current;
    for (const [bus, audio] of remoteAudio.current) {
      audio.volume = current.monitorVolume;
      audio.muted = current.monitorMuted || bus !== current.monitorBus;
      if (current.outputDeviceId && "setSinkId" in audio) {
        void (audio as HTMLAudioElement & { setSinkId: (id: string) => Promise<void> })
          .setSinkId(current.outputDeviceId)
          .catch(() => undefined);
      }
    }
    const sidetone = sidetoneAudio.current;
    if (sidetone) {
      sidetone.volume = current.monitorVolume;
      sidetone.muted = current.monitorMuted || !current.sidetone || current.monitorBus !== "program";
      if (current.outputDeviceId && "setSinkId" in sidetone) {
        void (sidetone as HTMLAudioElement & { setSinkId: (id: string) => Promise<void> })
          .setSinkId(current.outputDeviceId)
          .catch(() => undefined);
      }
    }
  }

  function roomSettings(bus: Bus) {
    const current = optionsRef.current;
    const mixer = channelRef.current ?? DEFAULT_CHANNEL(current.participant?.session_id ?? "");
    const commonMuted = current.micMuted || mixer.muted || current.mode !== "live";
    const muted = bus === "program" ? commonMuted || !mixer.is_on : commonMuted || !mixer.cue_enabled;
    return { muted, volume: Math.round(mixer.fader * 100) };
  }

  function applyMixer() {
    if (optionsRef.current.mode !== "live") return;
    for (const bus of ["program", "cue"] as const) {
      handles.current.get(bus)?.send({ message: { request: "configure", ...roomSettings(bus) } });
    }
  }

  useEffect(applyMonitor, [
    options.monitorBus,
    options.monitorVolume,
    options.monitorMuted,
    options.outputDeviceId,
    options.sidetone,
  ]);
  useEffect(applyMixer, [options.micMuted, channel]);

  // Restart the Janus session whenever the participant, mode, or input device changes.
  useEffect(() => {
    if (!participant || mode === "off") {
      setStatus("Not connected");
      setConnected(false);
      return;
    }
    const session = participant;
    let stopped = false;
    let janus: Janus | null = null;
    let socket: WebSocket | null = null;
    let reconnectTimer: number | null = null;
    let attempts = 0;
    let wasConnected = false;
    const authorization = { Authorization: `Bearer ${session.access_token}` };

    const report = async (state: "connected" | "disconnected") => {
      try {
        await fetch(`${API_BASE}/live-sessions/${session.session_id}/connection`, {
          method: "PATCH",
          headers: { ...authorization, "Content-Type": "application/json" },
          body: JSON.stringify({ state }),
        });
      } catch {
        /* the next status update retries */
      }
    };

    const cleanup = () => {
      handles.current.clear();
      for (const audio of remoteAudio.current.values()) {
        audio.pause();
        audio.srcObject = null;
      }
      remoteAudio.current.clear();
      if (sidetoneAudio.current) {
        sidetoneAudio.current.pause();
        sidetoneAudio.current.srcObject = null;
        sidetoneAudio.current = null;
      }
    };

    const scheduleReconnect = () => {
      if (stopped || reconnectTimer !== null) return;
      if (attempts >= 6) {
        setStatus("Connection lost");
        setError("Automatic reconnection failed. Toggle the connection to retry.");
        void report("disconnected");
        return;
      }
      const delay = Math.min(1000 * 2 ** attempts, 15000);
      attempts += 1;
      setStatus("Reconnecting");
      reconnectTimer = window.setTimeout(() => {
        reconnectTimer = null;
        void start();
      }, delay);
    };

    const connectMixerSocket = () => {
      socket?.close();
      const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
      socket = new WebSocket(
        `${scheme}//${window.location.host}/api/mixer/ws?token=${encodeURIComponent(session.access_token)}`,
      );
      socket.onmessage = (event: MessageEvent<string>) => {
        const message = JSON.parse(event.data) as {
          type: "snapshot" | "update";
          channels?: MixerChannel[];
          channel?: MixerChannel;
        };
        const next = message.type === "update" ? (message.channel ? [message.channel] : []) : message.channels ?? [];
        const own = next.find((item) => item.channel_id === session.session_id);
        if (own && own.revision >= (channelRef.current?.revision ?? 0)) {
          channelRef.current = own;
          setChannel(own);
        }
      };
    };

    const publish = (bus: Bus) => {
      const handle = handles.current.get(bus);
      if (!handle) return;
      const live = optionsRef.current.mode === "live";
      const audio: boolean | { deviceId: string } =
        live && inputDeviceId ? { deviceId: inputDeviceId } : true;
      handle.createOffer({
        media: live
          ? { audio, video: false }
          : { audioSend: false, audioRecv: true, video: false },
        success: (jsep: JanusJS.JSEP) => {
          handle.send({ message: { request: "configure", ...roomSettings(bus) }, jsep });
        },
        error: (cause: unknown) => {
          setError(`Audio setup failed: ${errorMessage(cause)}`);
          if (bus === "program") scheduleReconnect();
        },
      });
    };

    const trackRoomLevels = (event: JanusMessage) => {
      const names = roomNames.current;
      const list = event.participants as { id: number; display?: string }[] | undefined;
      if (Array.isArray(list)) for (const item of list) if (item.display) names.set(item.id, item.display);
      const id = event.id as number | undefined;
      if (typeof id !== "number") return;
      if (event.audiobridge === "joined") names.set(id, session.session_id);
      const name = names.get(id);
      if (event.audiobridge === "event" && event.leaving !== undefined) {
        const gone = event.leaving as number;
        const goneName = names.get(gone);
        if (goneName) setLevel(goneName, null);
        names.delete(gone);
      } else if (event.audiobridge === "talking" && name && name !== session.session_id) {
        const avg = Number(event["audio-level-dBov-avg"] ?? event["audio-level-dBov"] ?? 30);
        setLevel(name, -Math.abs(avg) / 2);
      } else if (event.audiobridge === "stopped-talking" && name) {
        if (name !== session.session_id) setLevel(name, null);
      }
    };

    const attachRoom = (bus: Bus, config: ConnectionConfig, roomId: number) => {
      const pluginOptions = {
        plugin: PLUGIN,
        opaqueId: `cloud-console-${bus}-${session.session_id}`,
        success: (handle: JanusPluginHandle) => {
          handles.current.set(bus, handle);
          handle.send({
            message: {
              request: "join",
              room: roomId,
              display: session.session_id,
              pin: config.room_pin,
              muted: true,
            },
          });
        },
        error: (cause: unknown) => {
          setError(`Could not join ${bus} audio: ${errorMessage(cause)}`);
          if (bus === "program") scheduleReconnect();
        },
        onmessage: (message: JanusMessage, jsep?: JanusJS.JSEP) => {
          const event = pluginData(message);
          if (jsep) handles.current.get(bus)?.handleRemoteJsep({ jsep });
          if (event.audiobridge === "joined") publish(bus);
          if (bus === "program") trackRoomLevels(event);
          if (typeof event.error === "string") setError(`${bus} audio error: ${event.error}`);
        },
        onlocaltrack: (track: MediaStreamTrack, added: boolean) => {
          if (bus !== "program" || track.kind !== "audio") return;
          if (!added) {
            sidetoneAudio.current = null;
            meterStop.current?.();
            meterStop.current = null;
            setLevel(session.session_id, null);
            return;
          }
          meterStop.current?.();
          meterStop.current = startLocalMeter(track, (db) => setLevel(session.session_id, db));
          const element = new Audio();
          element.srcObject = new MediaStream([track]);
          sidetoneAudio.current = element;
          applyMonitor();
          void element.play().catch(() => undefined);
        },
        onremotetrack: (track: MediaStreamTrack, _mid: string, added: boolean) => {
          if (track.kind !== "audio") return;
          if (!added) {
            const existing = remoteAudio.current.get(bus);
            if (existing) {
              existing.pause();
              existing.srcObject = null;
              remoteAudio.current.delete(bus);
            }
            return;
          }
          let element = remoteAudio.current.get(bus);
          if (!element) {
            element = new Audio();
            element.autoplay = true;
            remoteAudio.current.set(bus, element);
          }
          const stream = new MediaStream([track]);
          if (element.srcObject instanceof MediaStream && element.srcObject.getTracks()[0]?.id === track.id) {
            applyMonitor();
            return;
          }
          element.srcObject = stream;
          applyMonitor();
          void element.play().catch((cause: unknown) => {
            if (cause instanceof DOMException && cause.name === "AbortError") return;
            setError(`Click the page to enable monitor audio: ${errorMessage(cause)}`);
          });
        },
        webrtcState: (isActive: boolean) => {
          if (bus !== "program" || stopped) return;
          if (isActive) {
            wasConnected = true;
            attempts = 0;
            setConnected(true);
            setStatus(optionsRef.current.mode === "live" ? "Live" : "Listening");
            setError("");
            void report("connected");
          } else if (wasConnected) {
            setConnected(false);
            void report("disconnected");
            scheduleReconnect();
          }
        },
        oncleanup: () => {
          handles.current.delete(bus);
        },
      };
      janus?.attach(pluginOptions as unknown as JanusJS.PluginOptions);
    };

    const start = async () => {
      if (stopped) return;
      wasConnected = false;
      roomNames.current.clear();
      janus?.destroy({});
      janus = null;
      cleanup();
      setConnected(false);
      setStatus("Connecting");
      try {
        await initializeJanus();
        if (!Janus.isWebrtcSupported()) throw new Error("This browser does not support WebRTC.");
        const response = await fetch(`${API_BASE}/live-sessions/${session.session_id}/connection`, {
          headers: authorization,
        });
        if (!response.ok) {
          const body = (await response.json().catch(() => ({}))) as { error?: string };
          throw new Error(body.error ?? `Connection request failed (${response.status})`);
        }
        const config = (await response.json()) as ConnectionConfig;
        connectMixerSocket();
        const url = new URL(
          config.websocket_url ||
            `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/janus`,
        );
        url.searchParams.set("token", session.access_token);
        const janusOptions: JanusJS.ConstructorOptions = {
          server: url.toString(),
          success: () => {
            attachRoom("program", config, config.room_id);
            attachRoom("cue", config, config.cue_room_id);
          },
          error: (cause) => {
            setError(errorMessage(cause));
            scheduleReconnect();
          },
          destroyed: () => {
            if (!stopped && wasConnected) scheduleReconnect();
          },
        };
        if (config.ice_servers.length > 0) janusOptions.iceServers = config.ice_servers;
        janus = new Janus(janusOptions);
      } catch (cause) {
        setError(errorMessage(cause));
        scheduleReconnect();
      }
    };

    void start();
    return () => {
      stopped = true;
      if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
      void report("disconnected");
      socket?.close();
      janus?.destroy({});
      cleanup();
      setConnected(false);
    };
  }, [participant?.session_id, participant?.access_token, mode, inputDeviceId]);

  return { status, error, connected, channel, levels };
}

function startLocalMeter(track: MediaStreamTrack, report: (db: number) => void): () => void {
  const context = new AudioContext();
  void context.resume().catch(() => undefined);
  const resumeOnClick = () => void context.resume().catch(() => undefined);
  window.addEventListener("pointerdown", resumeOnClick);
  const analyser = context.createAnalyser();
  analyser.fftSize = 1024;
  context.createMediaStreamSource(new MediaStream([track])).connect(analyser);
  const samples = new Float32Array(analyser.fftSize);
  const timer = window.setInterval(() => {
    analyser.getFloatTimeDomainData(samples);
    let sum = 0;
    for (const value of samples) sum += value * value;
    report(20 * Math.log10(Math.sqrt(sum / samples.length) + 1e-6));
  }, 100);
  return () => {
    window.clearInterval(timer);
    window.removeEventListener("pointerdown", resumeOnClick);
    void context.close().catch(() => undefined);
  };
}

function pluginData(message: JanusMessage): JanusMessage {
  return message.plugindata?.data ?? message;
}

function initializeJanus(): Promise<void> {
  if (!janusInit) {
    janusInit = new Promise((resolve) => {
      Janus.init({
        debug: false,
        dependencies: Janus.useDefaultDependencies({ adapter }),
        callback: () => resolve(),
      });
    });
  }
  return janusInit;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : "An unexpected error occurred";
}
