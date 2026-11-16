import assert from "node:assert/strict";
import test from "node:test";
import { queueConnectionReport } from "../src/connectionReport.ts";

test("connection transitions are serialized even after a failure", async () => {
  const calls = [];
  let release;
  const first = queueConnectionReport("test-session", async () => {
    calls.push("disconnected started");
    await new Promise((resolve) => { release = resolve; });
    throw new Error("failed");
  });
  const second = queueConnectionReport("test-session", async () => { calls.push("connected"); });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, ["disconnected started"]);
  release();
  await assert.rejects(first, /failed/);
  await second;
  assert.deepEqual(calls, ["disconnected started", "connected"]);
});
