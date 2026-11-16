import assert from "node:assert/strict";
import test from "node:test";
import { moveInput, orderInputs } from "../src/inputOrder.ts";

test("saved order survives new and removed inputs", () => {
  const inputs = ["a", "b", "c"].map((channel_id) => ({ channel_id }));
  assert.deepEqual(orderInputs(inputs, ["missing", "b", "a"]).map((item) => item.channel_id), ["b", "a", "c"]);
  assert.deepEqual(orderInputs(inputs, []).map((item) => item.channel_id), ["a", "b", "c"]);
});

test("moving inputs respects boundaries", () => {
  assert.deepEqual(moveInput(["a", "b", "c"], "b", -1), ["b", "a", "c"]);
  assert.deepEqual(moveInput(["a", "b", "c"], "b", 1), ["a", "c", "b"]);
  assert.deepEqual(moveInput(["a", "b"], "a", -1), ["a", "b"]);
  assert.deepEqual(moveInput(["a", "b"], "missing", 1), ["a", "b"]);
});
