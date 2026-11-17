// The bridge over stdio against a stand-in broker: tools/list, a call that
// reaches a route, a name no tool has (inherited object names included), and
// answers the broker could give that are not a JSON object.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const BRIDGE = fileURLToPath(new URL("../buwiz-bridge.mjs", import.meta.url));
let broker, child, base, reply = { status: 200, body: "{}" };
const seen = [];
const waiting = new Map();
let buf = "";
let nextId = 1;

const rpc = (method, params) =>
  new Promise((resolve) => {
    const id = nextId++;

    waiting.set(id, resolve);
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  });
const call = async (name, args = {}) => (await rpc("tools/call", { name, arguments: args })).result;

before(async () => {
  broker = createServer((req, res) => {
    seen.push(`${req.method} ${req.url} ${req.headers.authorization}`);
    res.statusCode = reply.status;
    res.end(reply.body);
  });
  await new Promise((r) => broker.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${broker.address().port}`;
  child = spawn(process.execPath, [BRIDGE], { env: { PATH: process.env.PATH, BUWIZ_BRIDGE_URL: base, BUWIZ_BRIDGE_TOKEN: "tok" }, stdio: ["pipe", "pipe", "inherit"] });
  child.stdout.on("data", (d) => {
    buf += d;
    for (let i = buf.indexOf("\n"); i >= 0; i = buf.indexOf("\n")) {
      const line = buf.slice(0, i);

      buf = buf.slice(i + 1);
      if (line.trim()) {
        const m = JSON.parse(line);

        waiting.get(m.id)?.(m);
      }
    }
  });
  await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "0" } });
});

after(async () => {
  child?.kill();
  await new Promise((r) => broker?.close(r));
});

test("tools/list names the four tools", async () => {
  const { tools } = (await rpc("tools/list", {})).result;

  assert.deepEqual(tools.map((t) => t.name), ["rox_agents_list", "rox_send", "rox_receive", "rox_conversations"]);
});

test("a call reaches its route with the bearer token, and ok is dropped from the answer", async () => {
  reply = { status: 200, body: JSON.stringify({ ok: true, agents: [] }) };
  const r = await call("rox_agents_list");

  assert.equal(r.isError, undefined);
  assert.equal(r.content[0].text, '{"agents":[]}');
  assert.equal(seen.at(-1), "GET /v1/bridge/agents Bearer tok");
});

test("a name no tool has is refused, an inherited object name too", async () => {
  const before = seen.length;

  for (const name of ["nope", "constructor", "toString", "__proto__", "hasOwnProperty"]) {
    const r = await call(name);

    assert.equal(r.isError, true, name);
    assert.equal(r.content[0].text, `unknown tool ${name}`);
  }
  assert.equal(seen.length, before, "nothing reached the broker");
});

test("a 200 whose body is JSON null is an empty answer, not an unreachable broker", async () => {
  reply = { status: 200, body: "null" };
  const r = await call("rox_conversations");

  assert.equal(r.isError, undefined);
  assert.equal(r.content[0].text, "{}");
});

test("a refusal says its code and message", async () => {
  reply = { status: 401, body: JSON.stringify({ ok: false, code: "unauthorized", message: "the bridge token is missing or wrong" }) };
  const r = await call("rox_agents_list");

  assert.equal(r.isError, true);
  assert.equal(r.content[0].text, "unauthorized: the bridge token is missing or wrong");
});
