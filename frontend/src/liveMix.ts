import type { MixerChannel } from "./api";

type Bus = "program" | "cue";
type MixChannel = Pick<MixerChannel, "is_on" | "cue_enabled" | "muted" | "fader">;

export function roomMix(bus: Bus, channel: MixChannel | null, micMuted: boolean, live: boolean) {
  const muted = !channel || micMuted || channel.muted || !live ||
    (bus === "program" ? !channel.is_on : !channel.cue_enabled);
  return { muted, volume: Math.round((channel?.fader ?? 1) * 100) };
}
