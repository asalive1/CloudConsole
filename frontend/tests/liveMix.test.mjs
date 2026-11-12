import assert from "node:assert/strict";
import test from "node:test";
import { roomMix } from "../src/liveMix.ts";

test("microphones remain muted until their mixer state is known", () => {
  assert.equal(roomMix("program", null, false, true).muted, true);
  assert.equal(roomMix("cue", null, false, true).muted, true);
});

test("program OFF mutes the mic without disabling its cue", () => {
  const channel = { is_on: false, cue_enabled: true, muted: false, fader: 0.6 };
  assert.deepEqual(roomMix("program", channel, false, true), { muted: true, volume: 60 });
  assert.deepEqual(roomMix("cue", channel, false, true), { muted: false, volume: 60 });
  assert.equal(roomMix("program", { ...channel, is_on: true }, false, true).muted, false);
  assert.equal(roomMix("program", { ...channel, is_on: true }, true, true).muted, true);
  assert.equal(roomMix("program", { ...channel, is_on: true }, false, false).muted, true);
});
