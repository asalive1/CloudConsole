import { useCallback, useEffect, useState } from "react";

export type AudioDevices = {
  inputs: MediaDeviceInfo[];
  outputs: MediaDeviceInfo[];
  canSelectOutput: boolean;
  needsPermission: boolean;
  requestPermission: () => Promise<void>;
  error: string;
};

export function useAudioDevices(): AudioDevices {
  const [inputs, setInputs] = useState<MediaDeviceInfo[]>([]);
  const [outputs, setOutputs] = useState<MediaDeviceInfo[]>([]);
  const [error, setError] = useState("");
  const canSelectOutput = typeof HTMLMediaElement !== "undefined" && "setSinkId" in HTMLMediaElement.prototype;

  const refresh = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) {
      setError("This browser cannot list audio devices.");
      return;
    }
    const devices = await navigator.mediaDevices.enumerateDevices();
    setInputs(devices.filter((device) => device.kind === "audioinput"));
    setOutputs(devices.filter((device) => device.kind === "audiooutput"));
  }, []);

  useEffect(() => {
    void refresh();
    navigator.mediaDevices?.addEventListener?.("devicechange", refresh);
    return () => navigator.mediaDevices?.removeEventListener?.("devicechange", refresh);
  }, [refresh]);

  // Device labels stay blank until the user grants microphone permission once.
  const requestPermission = useCallback(async () => {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      stream.getTracks().forEach((track) => track.stop());
      setError("");
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Microphone permission was denied.");
    }
  }, [refresh]);

  return {
    inputs,
    outputs,
    canSelectOutput,
    needsPermission: inputs.length > 0 && inputs.every((device) => device.label === ""),
    requestPermission,
    error,
  };
}

export function deviceLabel(device: MediaDeviceInfo, index: number): string {
  return device.label || `${device.kind === "audioinput" ? "Microphone" : "Speakers"} ${index + 1}`;
}
