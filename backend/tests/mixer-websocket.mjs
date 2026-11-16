import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { createConnection } from "node:net";
import { setTimeout as sleep } from "node:timers/promises";

const base = "http://cloudconsole-mixer-api:8000";
const key = "synthetic-test-api-key";
const id = "00000000-0000-0000-0000-000000000002";
const wsUrl = `ws://cloudconsole-mixer-api:8000/api/mixer/ws?api_key=${key}`;

async function connect() {
  const ws = new WebSocket(wsUrl);
  const snapshot = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("WebSocket snapshot timed out")), 5000);
    ws.onerror = () => { clearTimeout(timer); reject(new Error("WebSocket connection failed")); };
    ws.onmessage = (event) => { clearTimeout(timer); resolve(JSON.parse(event.data)); };
  });
  assert.equal(snapshot.type, "snapshot");
  return ws;
}

async function close(ws) {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("WebSocket close timed out")), 5000);
    ws.onerror = () => { clearTimeout(timer); reject(new Error("WebSocket close failed")); };
    ws.onclose = (event) => {
      clearTimeout(timer);
      if (!event.wasClean) reject(new Error(`Unclean WebSocket close: ${event.code}`));
      else resolve();
    };
    ws.close();
  });
}

async function abortConnection() {
  await new Promise((resolve, reject) => {
    const socket = createConnection({ host: "cloudconsole-mixer-api", port: 8000 });
    const timer = setTimeout(() => { socket.destroy(); reject(new Error("Abrupt connection timed out")); }, 5000);
    socket.on("error", (error) => { clearTimeout(timer); reject(error); });
    socket.on("connect", () => {
      const wsKey = randomBytes(16).toString("base64");
      socket.write(`GET /api/mixer/ws?api_key=${key} HTTP/1.1\r\nHost: cloudconsole-mixer-api\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: ${wsKey}\r\n\r\n`);
    });
    socket.once("data", (data) => {
      clearTimeout(timer);
      if (!data.toString().startsWith("HTTP/1.1 101")) {
        socket.destroy();
        reject(new Error("WebSocket upgrade failed"));
        return;
      }
      socket.destroy();
      resolve();
    });
  });
}

let ready = false;
for (let i = 0; i < 30; i++) {
  try {
    if ((await fetch(`${base}/api/health`, { signal: AbortSignal.timeout(1000) })).ok) {
      ready = true;
      break;
    }
  } catch {
    // The test backend may still be starting.
  }
  await sleep(1000);
}
assert.ok(ready, "test backend did not become ready");

const sessionResponse = await fetch(`${base}/api/live-sessions`, { headers: { "X-API-Key": key } });
assert.equal(sessionResponse.status, 200);
const sessions = await sessionResponse.json();
assert.equal(sessions.length, 101);
assert.ok(sessions.some((session) => session.id === "00000000-0000-0000-0000-000000000004" && session.status === "connected"),
  "older connected hosts must be included beyond the recent-session limit");

const ws = await connect();
const update = new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("Mixer update timed out")), 5000);
  ws.onmessage = (event) => { clearTimeout(timer); resolve(JSON.parse(event.data)); };
});
const response = await fetch(`${base}/api/mixer/channels/${id}`, {
  method: "PATCH",
  headers: { "X-API-Key": key, "Content-Type": "application/json" },
  body: JSON.stringify({ fader: 0.6 }),
});
assert.equal(response.status, 200);
const message = await update;
assert.equal(message.type, "update");
assert.equal(message.channel.channel_id, id);
assert.equal(message.channel.fader, 0.6);
await close(ws);

for (let i = 0; i < 100; i++) await close(await connect());
for (let i = 0; i < 30; i++) await abortConnection();
await sleep(1000);
console.log("Mixer snapshots, updates, graceful closes and abrupt disconnects passed");
