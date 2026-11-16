import type { MixerChannel } from "./api";

export type MixerChannelUpdate = Pick<MixerChannel, "channel_id" | "revision"> &
  Partial<Pick<MixerChannel, "fader" | "is_on" | "cue_enabled" | "muted">>;

export function mergeMixerUpdate(channel: MixerChannel, update: MixerChannelUpdate): MixerChannel {
  if (channel.channel_id !== update.channel_id || update.revision < channel.revision) return channel;
  return { ...channel, ...update };
}
