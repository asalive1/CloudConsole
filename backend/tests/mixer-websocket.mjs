import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { createConnection, createServer } from "node:net";
import { setTimeout as sleep } from "node:timers/promises";

const base = "http://cloudconsole-mixer-api:8000";
const key = "synthetic-test-api-key";
const id = "00000000-0000-0000-0000-000000000002";
const wsUrl = `ws://cloudconsole-mixer-api:8000/api/mixer/ws?api_key=${key}`;

async function connect(url = wsUrl) {
  const ws = new WebSocket(url);
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

const operatorHeaders = { "X-API-Key": key, "Content-Type": "application/json" };
const createdLink = await fetch(`${base}/api/guest-links`, {
  method: "POST", headers: operatorHeaders,
  body: JSON.stringify({ expires_at: new Date(Date.now() + 3600000).toISOString() }),
});
assert.equal(createdLink.status, 201);
const invitation = await createdLink.json();
const listedLinks = await (await fetch(`${base}/api/guest-links`, { headers: operatorHeaders })).json();
assert.equal(listedLinks.find((link) => link.id === invitation.id).invitation_token, invitation.invitation_token);
assert.equal((await fetch(`${base}/api/guest-links`)).status, 401);
const joined = await fetch(`${base}/api/guest-links/${invitation.invitation_token}/sessions`, {
  method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "Guest" }),
});
assert.equal(joined.status, 201);
const guest = await joined.json();
const guestHeaders = { Authorization: `Bearer ${guest.access_token}` };
assert.equal((await fetch(`${base}/api/guest-monitor`, { headers: guestHeaders })).status, 401);
assert.equal((await fetch(`${base}/api/live-sessions/${guest.session_id}/approve`, { method: "POST", headers: operatorHeaders })).status, 200);
for (const source of ["cue", "mix_minus", "program_mix"]) {
  const changed = await fetch(`${base}/api/monitor`, { method: "PUT", headers: operatorHeaders, body: JSON.stringify({ guest_source: source }) });
  assert.equal(changed.status, 200);
  const received = await fetch(`${base}/api/guest-monitor`, { headers: guestHeaders });
  assert.equal(received.status, 200);
  assert.deepEqual(await received.json(), { guest_source: source });
}
assert.equal((await fetch(`${base}/api/monitor`, { method: "PUT", headers: operatorHeaders, body: JSON.stringify({ guest_source: "input", guest_input_id: id }) })).status, 400);
assert.equal((await fetch(`${base}/api/guest-links/${invitation.id}`, { method: "DELETE", headers: operatorHeaders })).status, 204);
assert.equal((await fetch(`${base}/api/guest-monitor`, { headers: guestHeaders })).status, 401);
const revokedLinks = await (await fetch(`${base}/api/guest-links`, { headers: operatorHeaders })).json();
assert.equal(revokedLinks.find((link) => link.id === invitation.id).invitation_token, null);

const hostResponse = await fetch(`${base}/api/live-sessions`, {
  method: "POST",
  headers: { "X-API-Key": key, "Content-Type": "application/json" },
  body: JSON.stringify({ display_name: "Test listener", role: "host" }),
});
assert.equal(hostResponse.status, 201);
const host = await hostResponse.json();
async function setConnection(state, mic_active) {
  const result = await fetch(`${base}/api/live-sessions/${host.session_id}/connection`, {
    method: "PATCH",
    headers: { Authorization: `Bearer ${host.access_token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ state, mic_active }),
  });
  assert.equal(result.status, 200);
  return result.json();
}
assert.equal((await setConnection("connected", false)).mic_active, false);
assert.equal((await setConnection("connected", true)).mic_active, true);
assert.equal((await setConnection("disconnected", true)).mic_active, false);
assert.equal((await setConnection("connected", true)).mic_active, true);
const activeSessions = await (await fetch(`${base}/api/live-sessions`, { headers: { "X-API-Key": key } })).json();
assert.equal(activeSessions.find((session) => session.id === host.session_id).mic_active, true);

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
const dropUpdate = new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("Drop mixer update timed out")), 5000);
  ws.onmessage = (event) => { clearTimeout(timer); resolve(JSON.parse(event.data)); };
});
const drop = await fetch(`${base}/api/live-sessions/${host.session_id}/disconnect`, {
  method: "POST", headers: { "X-API-Key": key },
});
assert.equal(drop.status, 200);
assert.equal((await drop.json()).mic_active, false);
const dropped = await dropUpdate;
assert.equal(dropped.channel.channel_id, host.session_id);
assert.equal(dropped.channel.muted, true);
assert.equal(dropped.channel.role, undefined);
await close(ws);

for (let i = 0; i < 100; i++) await close(await connect());
for (let i = 0; i < 30; i++) await abortConnection();

// Model an intermediary that closes a connection after 90 seconds without server traffic.
let upstreamChunks = 0;
const proxy = createServer((client) => {
  const upstream = createConnection({ host: "cloudconsole-mixer-api", port: 8000 });
  let idleTimer;
  const resetIdleTimer = () => {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(() => { client.destroy(); upstream.destroy(); }, 90000);
  };
  resetIdleTimer();
  upstream.on("data", () => { upstreamChunks += 1; resetIdleTimer(); });
  upstream.on("error", () => client.destroy());
  client.on("error", () => upstream.destroy());
  client.on("close", () => { clearTimeout(idleTimer); upstream.destroy(); });
  upstream.on("close", () => client.destroy());
  client.pipe(upstream);
  upstream.pipe(client);
});
await new Promise((resolve) => proxy.listen(0, "127.0.0.1", resolve));
try {
  const socket = await connect(`ws://127.0.0.1:${proxy.address().port}/api/mixer/ws?api_key=${key}`);
  const unexpectedClose = new Promise((_, reject) => {
    socket.onclose = (event) => reject(new Error(`Idle WebSocket closed early: ${event.code}`));
    socket.onerror = () => reject(new Error("Idle WebSocket failed"));
  });
  await Promise.race([sleep(100000), unexpectedClose]);
  assert.equal(socket.readyState, WebSocket.OPEN, "idle WebSocket must survive a 90-second proxy timeout");
  assert.ok(upstreamChunks >= 4, "server should send periodic keepalive frames");
  await close(socket);
} finally {
  await new Promise((resolve, reject) => proxy.close((error) => error ? reject(error) : resolve()));
}
await sleep(1000);
console.log("Mixer snapshots, updates, closes and idle proxy keepalive passed");
