// Agents and the world outside Rox (drafts/agent-messaging.md §10, §12):
// HOOK-01..03 on agents-external.mjs with an in-memory database, and
// BRG-01: rox-bridge round-trips a message through a test server, the
// recipient answering (MOCK) and the bridge receiving it.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { createAgents } from "../src/agents.mjs";
import { createDelivery } from "../src/agents-run.mjs";
import { BODY_MAX, HOOK_RATE, createExternals, signature } from "../src/agents-external.mjs";
import { HUB, PORT, postJson, ROOT, startServer, tempDir } from "./helpers.mjs";

// Node-only under the hub (BUWIZ_BROKER_HUB=1): agents-external.mjs in this process; with the hub the Rust ports run them (hub/agents/tests.rs)
const NODE_ONLY = { skip: HUB && "Node-only: agents-external.mjs in this process; with the hub the Rust ports run them (hub/agents/tests.rs)" };

function world({ post = async () => 200 } = {}) {
  let n = 0;
  const clock = { t: Date.parse("2026-10-29T08:00:00Z") };
  const store = { list: [], bots: [] };

  for (const name of ["Piccolo", "Boss"]) {
    store.list.push({ id: `t${++n}`, uid: `u${n}`, title: name, turns: [] });
    store.bots.push({ id: name.toLowerCase(), name, threadId: `t${n}`, integrations: [] });
  }
  const timers = [];
  const posts = [];
  const deliveredOut = [];
  let externals;
  const agents = createAgents({
    store, nextId: () => `t${++n}`, uid: () => `x${++n}`, save: () => {}, now: () => clock.t,
    externals: { get: (id) => externals.get(id) },
    deliver: (to, msg, t) => (to.external ? (externals.deliver(to, msg), agents.delivered(t, msg.id, to.id)) : deliveredOut.push({ to: to.id, msg })),
  });

  externals = createExternals({
    db: new DatabaseSync(":memory:"), agents, now: () => clock.t,
    post: async (url, body, headers) => (posts.push({ url, body, headers }), post(url, body, headers)),
    setTimer: (fn, ms) => timers.push({ fn, ms }),
  });

  return { store, agents, externals, clock, timers, posts, deliveredOut, bot: (id) => store.bots.find((b) => b.id === id) };
}

const env = (fields = {}) => JSON.stringify({ v: 1, id: "msg_hook00000001", subject: "Build failed", text: "main is red", expects_reply: true, ...fields });
const signed = (w, secret, body, skew = 0) => signature(secret, body, Math.floor(w.clock.t / 1000) + skew);

const status = (fn) => {
  try {
    fn();
  } catch (e) {
    return [e.status, e.code];
  }
  assert.fail("expected a refusal");
};

test("HOOK-01 a signed message is accepted; a bad or stale signature, a replay with another body and an oversize body are refused", NODE_ONLY, () => {
  const w = world();
  const hook = w.externals.create({ bot: "boss", kind: "hook", name: "CI" });

  assert.match(hook.id, /^ah_/);
  assert.equal(hook.path, `/v1/hooks/${hook.id}/${hook.secret}`);
  const body = env();
  const ok = w.externals.incoming(hook.id, hook.secret, body, signed(w, hook.secret, body));

  assert.match(ok.id, /^msg_hook/);
  const t = w.agents.conversation(ok.conversation);

  assert.deepEqual(t.agents.participants, [`ext:${hook.id}`, "boss"]);
  assert.deepEqual(t.turns[0].message.from, { kind: "external", id: `ext:${hook.id}`, name: "CI" });
  assert.equal(w.deliveredOut[0].to, "boss");
  assert.ok(w.store.list.find((x) => x.id === "t2").turns.some((u) => u.text === "CI → Boss · Build failed"), "a notice in Boss's chat");

  assert.deepEqual(status(() => w.externals.incoming(hook.id, "wrong", body, signed(w, hook.secret, body))), [404, "not_found"]);
  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, body, signed(w, "not-the-secret", body))), [401, "bad_signature"]);
  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, body, signed(w, hook.secret, body, -301))), [401, "bad_signature"]);
  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, body, undefined)), [401, "bad_signature"]);
  const other = env({ text: "something else" });

  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, other, signed(w, hook.secret, other))), [409, "replayed"]);
  const big = "x".repeat(BODY_MAX + 1);

  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, big, signed(w, hook.secret, big))), [413, "too_large"]);
  const v2 = env({ v: 2, id: "msg_hook00000002" });

  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, v2, signed(w, hook.secret, v2))), [400, "unsupported_version"]);
  const elsewhere = env({ id: "msg_hook00000003", to: ["piccolo"] });

  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, elsewhere, signed(w, hook.secret, elsewhere))), [400, "not_allowed"], "a hook reaches only its own agent");
});

test("HOOK-02 the same id and body are taken once; the hook allows 60 calls an hour", NODE_ONLY, () => {
  const w = world();
  const hook = w.externals.create({ bot: "boss", kind: "hook", name: "CI" });
  const body = env();
  const first = w.externals.incoming(hook.id, hook.secret, body, signed(w, hook.secret, body));
  const again = w.externals.incoming(hook.id, hook.secret, body, signed(w, hook.secret, body));

  assert.deepEqual(again, first);
  assert.equal(w.agents.conversation(first.conversation).turns.filter((u) => u.role === "agent_message").length, 1);

  for (let i = 2; i < HOOK_RATE; i++) {
    const b = env({ id: `msg_hook${String(i).padStart(8, "0")}`, subject: `Build ${i}` });

    w.externals.incoming(hook.id, hook.secret, b, signed(w, hook.secret, b));
  }
  const over = env({ id: "msg_hookover0001" });

  assert.deepEqual(status(() => w.externals.incoming(hook.id, hook.secret, over, signed(w, hook.secret, over))), [429, "rate_limited"]);
});

test("HOOK-03 an agent's answer goes out signed; 5xx is retried, 4xx dropped with a notice", NODE_ONLY, async () => {
  const answers = [500, 200, 404];
  const w = world({ post: async () => answers.shift() });
  const hook = w.externals.create({ bot: "boss", kind: "hook", name: "CI", url: "https://ci.example.com/rox" });

  assert.ok(hook.out_secret);
  const body = env();
  const got = w.externals.incoming(hook.id, hook.secret, body, signed(w, hook.secret, body));
  // the agent's turn answers (a stub): its reply goes to the hook's url
  const delivery = createDelivery({ agents: w.agents, runRequest: async () => "Fixed in a3f2c1.", runReply: async () => {} });

  delivery.deliver(w.bot("boss"), w.agents.conversation(got.conversation).turns[0].message, w.agents.conversation(got.conversation));
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(w.posts.length, 1);
  const sent = w.posts[0];
  const msg = JSON.parse(sent.body);

  assert.equal(msg.text, "Fixed in a3f2c1.");
  assert.equal(msg.in_reply_to, got.id);
  assert.deepEqual(msg.to, [{ kind: "external", id: `ext:${hook.id}`, name: "CI" }]);
  const [, t, v1] = /^t=(\d+),v1=([0-9a-f]+)$/.exec(sent.headers["x-buwiz-signature"]);

  assert.equal(signature(hook.out_secret, sent.body, t), `t=${t},v1=${v1}`);
  // 500: tried again after a backoff, then 200
  assert.equal(w.timers.length, 1);
  await w.timers.shift().fn();
  assert.equal(w.posts.length, 2);
  // the next answer gets a 404: dropped, and the conversation says so
  w.agents.send(w.bot("boss"), { conversation: got.conversation, to: [`ext:${hook.id}`], text: "One more thing." });
  await new Promise((r) => setTimeout(r, 20));
  assert.ok(w.agents.conversation(got.conversation).turns.some((u) => u.notice?.kind === "agents_undelivered"));
  assert.equal(w.timers.length, 0, "a 4xx is not retried");
  // another agent cannot write to Boss's endpoint
  assert.throws(() => w.agents.send(w.bot("piccolo"), { to: [`ext:${hook.id}`], subject: "x", text: "y" }), /not one of your outside endpoints/);
});

test("a bridge receives what its agent sends, waiting for it when asked", NODE_ONLY, async () => {
  const w = world();
  const bridge = w.externals.create({ bot: "boss", kind: "bridge", name: "Claude Code · rox" });

  assert.equal(w.externals.bridge(bridge.secret).id, bridge.id);
  assert.throws(() => w.externals.bridge("nope"), /bridge token/);
  const asked = w.agents.sendFrom(w.externals.get(bridge.id), { subject: "Status", text: "How is Q3?", expects_reply: true });
  const r = w.externals.bridge(bridge.secret);

  assert.deepEqual((await w.externals.receive(r)).messages, []);
  const waiting = w.externals.receive(r, { wait_s: 5 });

  w.agents.send(w.bot("boss"), { conversation: asked.conversation, in_reply_to: asked.id, text: "All recorded." });
  const { messages } = await waiting;

  assert.equal(messages.length, 1);
  assert.equal(messages[0].text, "All recorded.");
  assert.deepEqual((await w.externals.receive(r)).messages, [], "taken once");
});

test("BRG-01 rox-bridge round-trips a message through a test server", async () => {
  const dir = tempDir("bridge");

  mkdirSync(join(dir, "widgets"));
  execFileSync(process.execPath, ["--input-type=module", "-e", `
    import { store, nextThreadSeq, uuidv7, writeIndex } from "./src/state.mjs";
    const tid = "t" + nextThreadSeq();
    store.list.push({ id: tid, uid: uuidv7(), title: "Boss", turns: [] });
    store.bots.push({ id: "boss", name: "Boss", shape: "blob", color: "#FFFFFF", threadId: tid, memories: [], integrations: [] });
    writeIndex();
  `], { cwd: ROOT, env: { ...process.env, BUWIZ_BROKER_DATA_DIR: dir, MOCK: "1" } });
  const server = await startServer(dir);
  let client;

  try {
    const token = readFileSync(join(dir, "host-token"), "utf8").trim();
    const made = await postJsonHost("/v1/host/agents/endpoints/create", { bot: "boss", kind: "bridge", name: "Claude Code · test" }, token);

    assert.equal(made.json.ok, true, made.text);
    client = new Client({ name: "test", version: "1.0.0" });
    await client.connect(new StdioClientTransport({
      command: process.execPath,
      args: [join(ROOT, "bridge", "rox-bridge.mjs")],
      env: { ...process.env, BUWIZ_BRIDGE_URL: `http://127.0.0.1:${PORT}`, BUWIZ_BRIDGE_TOKEN: made.json.secret },
    }));
    const tools = (await client.listTools()).tools.map((t) => t.name);

    assert.deepEqual(tools, ["rox_agents_list", "rox_send", "rox_receive", "rox_conversations"]);
    const text = async (name, args) => JSON.parse((await client.callTool({ name, arguments: args })).content[0].text);

    assert.equal((await text("rox_agents_list", {})).agents[0].id, "boss");
    const sent = await text("rox_send", { subject: "Status", text: "How is Q3?", expects_reply: true });

    assert.match(sent.id, /^msg_/);
    // MOCK answers as Boss; the bridge waits for it
    const got = await text("rox_receive", { wait_s: 20 });

    assert.equal(got.messages.length, 1, JSON.stringify(got));
    assert.equal(got.messages[0].in_reply_to, sent.id);
    assert.equal(got.messages[0].from.id, "boss");
    assert.equal((await text("rox_conversations", {})).conversations[0].count, 2);
    const refused = await client.callTool({ name: "rox_send", arguments: { to: ["nobody"], subject: "x", text: "y" } });

    assert.equal(refused.isError, true);
  } finally {
    await client?.close();
    await server.stop();
  }
});

async function postJsonHost(path, obj, token) {
  const { request } = await import("./helpers.mjs");

  return request("POST", path, { headers: { "Content-Type": "application/json", "X-Buwiz-Host": token }, body: JSON.stringify(obj) });
}

void postJson;
