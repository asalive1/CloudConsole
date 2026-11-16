import assert from "node:assert/strict";
import test from "node:test";
import { guestMonitor } from "../src/guestMonitor.ts";

test("guest monitor selection uses the correct bus and sidetone", () => {
  assert.deepEqual(guestMonitor("program_mix"), { bus: "program", sidetone: true, available: true });
  assert.deepEqual(guestMonitor("mix_minus"), { bus: "program", sidetone: false, available: true });
  assert.deepEqual(guestMonitor("cue"), { bus: "cue", sidetone: false, available: true });
  assert.equal(guestMonitor("input").available, false);
  assert.equal(guestMonitor(null).available, false);
});
