import assert from "node:assert/strict";
import test from "node:test";
import { mergeMixerUpdate } from "../src/mixerUpdates.ts";

test("partial drop and fader updates preserve channel identity", () => {
  const channel = { channel_id: "host", display_name: "Operator", role: "host", fader: 1, is_on: true, cue_enabled: false, muted: false, revision: 1 };
  const dropped = mergeMixerUpdate(channel, { channel_id: "host", muted: true, revision: 2 });
  assert.deepEqual(dropped, { ...channel, muted: true, revision: 2 });
  assert.equal(mergeMixerUpdate(dropped, { channel_id: "host", muted: false, revision: 1 }), dropped);
  assert.equal(mergeMixerUpdate(dropped, { channel_id: "other", muted: false, revision: 3 }), dropped);
});
