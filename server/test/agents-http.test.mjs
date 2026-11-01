// Agent messaging over HTTP (the Rust broker plan, phase 5 item 5.6): the
// agents host routes, the outside endpoints, an `ah_` webhook (signature,
// staleness, replays, size, rate), the outgoing webhook's outbox against a
// local receiver (5xx retried, 4xx dropped), the Claude Code bridge's
// refusals and its long poll, and delivery with MOCK turns: the recipient
// answers with the fixed mock reply. agents.test.mjs and
// agents-external.test.mjs cover the modules (A2A-*, HOOK-*) and BRG-01
// through rox-bridge; this file pins the HTTP side.
//
// Timing: the outbox's first retry waits 30 s (setTimeout, no seam), so the
// retry is shown by its row (attempts, next_at) and by a restart, which
// sends what is still queued at once (startAgents → resume).
//
//   AGH-01 POST /v1/host/agents/endpoints/create|list|rotate|delete: shapes,
//          secrets only when made or rotated, refusals
//   AGH-02 a signed hook message: 202 {ok, id, conversation}; Boss answers
//          (MOCK) in the conversation; the notice in Boss's chat merges
//   AGH-03 hook refusals: wrong secret 404, bad / stale / missing signature
//          401, replay with another body 409 (the same body is the same
//          answer), over 512 KB 413, no JSON, v, id
//   AGH-04 60 calls an hour per hook, then 429
//   AGH-05 POST /v1/host/agents/accepts|archive|resume: a hook's agent that
//          takes no messages; 20 an hour pauses a conversation, resume
//   AGH-06 the outbox: a reply to a hook with a url is POSTed signed; a 5xx
//          is kept queued with a backoff and sent again (after a restart);
//          a 4xx is dropped with a notice in the conversation
//   AGH-07 the bridge: 401 without its token, 404 for an unknown route, the
//          agents list, a refused send
//   AGH-08 the bridge's long poll wakes when its agent's reply comes
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { signature } from "../src/agents-external.mjs";
import { PORT, dbRows, nodeIn, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const MOCK_REPLY = "_(mock reply — no model configured)_";
const RECV_PORT = PORT + 1;
const dataDir = tempDir("agents-http");
const env = () => offlineEnv(join(tempDir("agents-http-stubs"), "stubs.log"));
let server;
let token = "";
let receiver;
const received = []; // {path, headers, body}
const answers = { "/later": [500, 200], "/gone": [404] };

const host = (path, obj) => request("POST", path, { headers: { "X-Buwiz-Host": token, "Content-Type": "application/json" }, body: JSON.stringify(obj) });
const refusal = (code, message, retryable = false) => ({ ok: false, code, message, retryable });
const threads = async () => (await request("GET", "/threads")).json;
const turns = async (id) => (await request("GET", `/turns?thread=${id}`)).json.turns;
const convThread = async (conv) => (await threads()).list.find((t) => t.agents?.conversation === conv);
const bossChat = async () => (await threads()).bots.find((b) => b.id === "boss").threadId;
const now = () => Math.floor(Date.now() / 1000);
const envelope = (fields = {}) => JSON.stringify({ v: 1, subject: "Build failed", text: "main is red", ...fields });
/** POST the raw `body` to hook `ep`, signed with `secret` at `t`. */
const hook = (ep, body, { secret = ep.secret, t = now(), sig, path = ep.path, header = "X-Buwiz-Signature" } = {}) =>
  request("POST", path, { headers: { "Content-Type": "application/json", ...(sig === null ? {} : { [header]: sig ?? signature(secret, body, t) }) }, body });
const bridge = (method, path, tok, obj) =>
  request(method, path, { headers: { ...(tok ? { Authorization: `Bearer ${tok}` } : {}), ...(obj ? { "Content-Type": "application/json" } : {}) }, body: obj ? JSON.stringify(obj) : undefined });

before(async () => {
  nodeIn(dataDir, `
    const { store, nextThreadSeq, uuidv7, writeIndex } = await import("./src/state.mjs");
    for (const name of ["Piccolo", "Boss"]) {
      const tid = "t" + nextThreadSeq();
      store.list.push({ id: tid, uid: uuidv7(), title: name, turns: [] });
      store.bots.push({ id: name.toLowerCase(), name, shape: "blob", color: "#FFFFFF", threadId: tid, memories: [], integrations: [] });
    }
    writeIndex();
  `);
  receiver = createServer((req, res) => {
    let body = "";

    req.setEncoding("utf8");
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      received.push({ path: req.url, headers: req.headers, body });
      res.statusCode = answers[req.url]?.shift() ?? 200;
      res.end("");
    });
  });
  await new Promise((r) => receiver.listen(RECV_PORT, "127.0.0.1", r));
  server = await startServer(dataDir, { env: env() });
  token = readFileSync(join(dataDir, "host-token"), "utf8");
});

after(async () => {
  await server?.stop();
  await new Promise((r) => receiver?.close(r));
});

let ci; // the hook endpoint most cases use
let bridgeEp;

test("AGH-01 endpoints: create, list, rotate, delete; secrets only when made or rotated; refusals", async () => {
  const made = await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "hook", name: "  CI  " });

  assert.equal(made.status, 200, made.text);
  ci = made.json;
  assert.deepEqual(Object.keys(ci), ["ok", "id", "bot", "kind", "name", "created_at", "secret", "path"]);
  assert.match(ci.id, /^ah_[A-Za-z0-9_-]{12}$/);
  assert.match(ci.secret, /^[A-Za-z0-9_-]{32}$/);
  assert.deepEqual([ci.ok, ci.bot, ci.kind, ci.name, ci.path], [true, "boss", "hook", "CI", `/v1/hooks/${ci.id}/${ci.secret}`]);
  const out = (await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "hook", name: "Deploys", url: `http://127.0.0.1:${RECV_PORT}/later` })).json;

  assert.deepEqual(Object.keys(out), ["ok", "id", "bot", "kind", "name", "url", "created_at", "secret", "out_secret", "path"]);
  bridgeEp = (await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "bridge", name: "Claude Code · test" })).json;
  assert.deepEqual(Object.keys(bridgeEp), ["ok", "id", "bot", "kind", "name", "created_at", "secret"]);
  assert.match(bridgeEp.id, /^ab_/);
  const refused = async (body, message) => {
    const r = await host("/v1/host/agents/endpoints/create", body);

    assert.deepEqual([r.status, r.json], [400, refusal("invalid_args", message)], JSON.stringify(body));
  };

  await refused({ bot: "boss", kind: "mail", name: "x" }, 'kind is "hook" or "bridge"');
  await refused({ bot: "boss", kind: "hook", name: "   " }, "name is required");
  await refused({ bot: "nobody", kind: "hook", name: "x" }, "no such agent");
  await refused({ bot: "boss", kind: "hook", name: "x", url: "ftp://example.com" }, "url is an http(s) URL");
  const listed = await host("/v1/host/agents/endpoints/list", { bot: "boss" });

  assert.equal(
    listed.text,
    JSON.stringify({
      ok: true,
      endpoints: [
        { id: ci.id, bot: "boss", kind: "hook", name: "CI", created_at: ci.created_at },
        { id: out.id, bot: "boss", kind: "hook", name: "Deploys", url: `http://127.0.0.1:${RECV_PORT}/later`, created_at: out.created_at },
        { id: bridgeEp.id, bot: "boss", kind: "bridge", name: "Claude Code · test", created_at: bridgeEp.created_at },
      ],
    }),
  );
  assert.deepEqual((await host("/v1/host/agents/endpoints/list", {})).json.endpoints.length, 3, "every agent's");
  assert.deepEqual((await host("/v1/host/agents/endpoints/list", { bot: "piccolo" })).json, { ok: true, endpoints: [] });
  // rotate: the old secret stops working
  const temp = (await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "hook", name: "Temp" })).json;
  const rot = await host("/v1/host/agents/endpoints/rotate", { id: temp.id });

  assert.deepEqual(Object.keys(rot.json), ["ok", "id", "secret"]);
  assert.notEqual(rot.json.secret, temp.secret);
  assert.equal((await hook(temp, envelope())).status, 404);
  assert.deepEqual((await host("/v1/host/agents/endpoints/rotate", { id: "ah_nope" })).json, refusal("invalid_args", "no such endpoint"));
  assert.equal((await host("/v1/host/agents/endpoints/delete", { id: temp.id })).text, '{"ok":true}');
  assert.equal((await host("/v1/host/agents/endpoints/delete", { id: "ah_nope" })).text, '{"ok":true}', "an unknown id is fine");
  assert.equal((await hook({ ...temp, secret: rot.json.secret, path: `/v1/hooks/${temp.id}/${rot.json.secret}` }, envelope())).status, 404, "gone");
  // the host token guards them
  assert.equal((await postJson("/v1/host/agents/endpoints/list", {})).status, 401);
  // secrets live in the broker's database
  assert.equal(dbRows(dataDir, "SELECT secret FROM agent_endpoints WHERE id=?", ci.id)[0].secret, ci.secret);
});

let conv = "";

test("AGH-02 a signed hook message: 202; Boss answers with a MOCK turn; the notice in Boss's chat merges", async () => {
  const body = envelope({ id: "msg_build00000001", expects_reply: true });
  const r = await hook(ci, body);

  assert.equal(r.status, 202, r.text);
  assert.deepEqual(Object.keys(r.json), ["ok", "id", "conversation"]);
  assert.deepEqual([r.json.ok, r.json.id], [true, "msg_build00000001"]);
  conv = r.json.conversation;
  assert.match(conv, /^conv_[0-9a-f-]{36}$/);
  const t = await convThread(conv);

  assert.deepEqual(t.agents.participants, [`ext:${ci.id}`, "boss"]);
  assert.equal(t.title, "Build failed");
  const replied = await waitFor(async () => {
    const ts = await turns(t.id);

    return ts.length === 2 && !ts[0].pending && ts;
  }, 20000);

  assert.ok(replied, "Boss replied");
  assert.deepEqual(replied.map((u) => [u.role, u.text]), [["agent_message", "main is red"], ["agent_message", MOCK_REPLY]]);
  const [ask, reply] = replied.map((u) => u.message);

  assert.deepEqual(ask.from, { kind: "external", id: `ext:${ci.id}`, name: "CI" });
  assert.deepEqual(ask.to, [{ kind: "agent", id: "boss", name: "Boss" }]);
  assert.deepEqual(Object.keys(reply), ["v", "id", "conversation", "in_reply_to", "from", "to", "subject", "text", "expects_reply", "hop", "sent_at"]);
  assert.deepEqual([reply.in_reply_to, reply.from.id, reply.to, reply.hop, reply.expects_reply], ["msg_build00000001", "boss", [{ kind: "external", id: `ext:${ci.id}`, name: "CI" }], 1, false]);
  // one notice in Boss's chat for the conversation, updated in place
  const notices = (await turns(await bossChat())).filter((u) => u.role === "notice");

  assert.equal(notices.length, 1);
  assert.equal(notices[0].text, "Boss → CI · Build failed · 2 messages");
  assert.deepEqual([notices[0].notice.kind, notices[0].notice.count, notices[0].notice.conversation, notices[0].notice.last_id], ["agents", 2, conv, reply.id]);
  // CI has no url: the reply is held, then dropped after a day
  assert.deepEqual(dbRows(dataDir, "SELECT state, attempts, next_at FROM agent_outbox WHERE endpoint=?", ci.id), [{ state: "held", attempts: 0, next_at: null }]);
  // the same id and body again: the same answer, nothing new
  const again = await hook(ci, body);

  assert.equal(again.status, 202);
  assert.equal(again.text, r.text);
  assert.equal((await turns(t.id)).length, 2);
  // a message into the same conversation reaches Boss again
  const more = await hook(ci, envelope({ id: "msg_build00000002", conversation: conv, text: "still red" }));

  assert.equal(more.json.conversation, conv);
});

test("AGH-03 hook refusals: secret, signature, staleness, replay, size, JSON, version, id", async () => {
  const body = envelope({ id: "msg_refused000001" });
  const is = async (r, status, code, message) => assert.deepEqual([r.status, r.json], [status, refusal(code, message, status === 429)]);

  await is(await hook({ ...ci, path: `/v1/hooks/${ci.id}/wrong` }, body), 404, "not_found", "no such hook");
  await is(await hook({ ...ci, path: `/v1/hooks/ah_nope/${ci.secret}` }, body), 404, "not_found", "no such hook");
  const bad = "X-Buwiz-Signature is missing, wrong or older than 5 minutes";

  await is(await hook(ci, body, { secret: "not-the-secret" }), 401, "bad_signature", bad);
  await is(await hook(ci, body, { sig: null }), 401, "bad_signature", bad);
  // the buwiz names (protocol 1.0): Rox's header name signs nothing
  await is(await hook(ci, body, { header: "X-Rox-Signature" }), 401, "bad_signature", bad);
  await is(await hook(ci, body, { sig: "t=1,v1=zz" }), 401, "bad_signature", bad);
  await is(await hook(ci, body, { t: now() - 301 }), 401, "bad_signature", bad);
  await is(await hook(ci, body, { t: now() + 301 }), 401, "bad_signature", bad);
  // a v1 over another body
  await is(await hook(ci, body, { sig: signature(ci.secret, envelope({ id: "msg_other0000001" })) }), 401, "bad_signature", bad);
  await is(await hook(ci, envelope({ id: "msg_build00000001", text: "another body" })), 409, "replayed", "this id came with another body in the last 24 hours");
  const big = "x".repeat(512 * 1024 + 1);

  await is(await hook(ci, big, { sig: null }), 413, "too_large", "the body is over 512 KB");
  await is(await hook(ci, "not json"), 400, "invalid_args", "the body is not JSON");
  await is(await hook(ci, "[1]"), 400, "invalid_args", "the body is an envelope object");
  await is(await hook(ci, envelope({ v: 2 })), 400, "unsupported_version", "v must be 1");
  await is(await hook(ci, envelope({ id: "nope" })), 400, "invalid_args", "id is msg_ and 8 to 80 letters, digits, _ or -");
  await is(await hook(ci, envelope({ to: ["piccolo"] })), 400, "not_allowed", "Piccolo does not take messages from you");
  await is(await hook(ci, envelope({ subject: "" })), 400, "invalid_args", "`subject` is required for a new conversation");
  // a hook route of no endpoint kind, and a GET
  assert.equal((await request("GET", ci.path)).status, 404);
});

test("AGH-04 a hook takes 60 calls an hour, then 429", async () => {
  const ep = (await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "hook", name: "Noisy" })).json;

  // a signed call counts even when its body is refused
  for (let k = 0; k < 60; k++) assert.equal((await hook(ep, "not json")).status, 400);
  const over = await hook(ep, envelope());

  assert.deepEqual([over.status, over.json], [429, refusal("rate_limited", "at most 60 calls an hour", true)]);
  // an unsigned one is refused before the count
  assert.equal((await hook(ep, envelope(), { sig: null })).status, 401);
});

test("AGH-05 accepts, archive, resume: refusals, a hook's agent that takes no messages, 20 an hour pauses", async () => {
  const is = async (path, body, message) => assert.deepEqual(await host(path, body).then((r) => [r.status, r.json]), [400, refusal("invalid_args", message)], path);

  await is("/v1/host/agents/accepts", { bot: "nobody", accepts: "all" }, "no such agent");
  await is("/v1/host/agents/accepts", { bot: "boss", accepts: "some" }, 'accepts is "all", "none" or a list of agent ids');
  await is("/v1/host/agents/accepts", { bot: "boss", accepts: [1] }, 'accepts is "all", "none" or a list of agent ids');
  await is("/v1/host/agents/archive", { conversation: "conv_nope" }, "no such conversation");
  await is("/v1/host/agents/resume", {}, "no such conversation");
  assert.equal((await host("/v1/host/agents/accepts", { bot: "boss", accepts: "none" })).text, '{"ok":true}');
  assert.equal((await threads()).bots.find((b) => b.id === "boss").accepts, "none");
  const no = await hook(ci, envelope());

  assert.deepEqual([no.status, no.json], [400, refusal("invalid_args", "no agent to send to")]);
  assert.equal((await host("/v1/host/agents/accepts", { bot: "boss", accepts: ["piccolo"] })).status, 200);
  assert.deepEqual((await threads()).bots.find((b) => b.id === "boss").accepts, ["piccolo"]);
  assert.equal((await host("/v1/host/agents/accepts", { bot: "boss", accepts: "all" })).status, 200);
  // archive and back
  assert.equal((await host("/v1/host/agents/archive", { conversation: conv })).text, '{"ok":true}');
  assert.equal((await convThread(conv)).agents.archived, true);
  assert.equal((await host("/v1/host/agents/archive", { conversation: conv, archived: false })).status, 200);
  assert.equal((await convThread(conv)).agents.archived, undefined);
  // 20 messages in an hour pause it (no replies asked: no turns)
  const ep = (await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "hook", name: "Chatty" })).json;
  const first = await hook(ep, envelope({ subject: "Chatter" }));
  const c = first.json.conversation;

  for (let k = 1; k < 20; k++) assert.equal((await hook(ep, envelope({ conversation: c, text: `n${k}` }))).status, 202);
  const paused = await hook(ep, envelope({ conversation: c }));

  assert.deepEqual([paused.status, paused.json], [400, refusal("paused", "these agents exchanged 20 messages in an hour; the user resumes the conversation")]);
  const t = await convThread(c);

  assert.equal(typeof t.agents.paused, "number");
  assert.equal((await turns(t.id)).at(-1).text, "Paused: these agents have exchanged 20 messages in an hour");
  assert.ok((await turns(await bossChat())).some((u) => u.notice?.kind === "agents_paused" && u.notice.conversation === c), "the participants' chats are told");
  const still = await hook(ep, envelope({ conversation: c }));

  assert.deepEqual([still.status, still.json.message], [400, "this conversation is paused until the user resumes it"]);
  assert.equal((await host("/v1/host/agents/resume", { conversation: c })).text, '{"ok":true}');
  const back = await convThread(c);

  assert.equal(back.agents.paused, undefined);
  assert.equal(typeof back.agents.resumed_at, "number");
  assert.equal((await hook(ep, envelope({ conversation: c, text: "after the resume" }))).status, 202);
});

test("AGH-06 the outbox: a reply POSTed signed; 5xx kept queued and sent again; 4xx dropped with a notice", async () => {
  const later = (await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "hook", name: "Later", url: `http://127.0.0.1:${RECV_PORT}/later` })).json;
  const gone = (await host("/v1/host/agents/endpoints/create", { bot: "boss", kind: "hook", name: "Gone", url: `http://127.0.0.1:${RECV_PORT}/gone` })).json;
  const asked = await hook(later, envelope({ id: "msg_later00000001", expects_reply: true }));

  assert.equal(asked.status, 202, asked.text);
  assert.ok(await waitFor(() => received.some((x) => x.path === "/later"), 20000), "the reply went out");
  const first = received.find((x) => x.path === "/later");
  const msg = JSON.parse(first.body);

  assert.equal(first.headers["content-type"], "application/json");
  const [, t, v1] = /^t=(\d+),v1=([0-9a-f]{64})$/.exec(first.headers["x-buwiz-signature"]);

  assert.equal(first.headers["x-rox-signature"], undefined, "only the buwiz header goes out");

  assert.equal(signature(later.out_secret, first.body, t), `t=${t},v1=${v1}`);
  assert.deepEqual([msg.in_reply_to, msg.text, msg.from.id, msg.to], ["msg_later00000001", MOCK_REPLY, "boss", [{ kind: "external", id: `ext:${later.id}`, name: "Later" }]]);
  // a 500: kept, tried again 30 s later
  const row = await waitFor(() => dbRows(dataDir, "SELECT * FROM agent_outbox WHERE endpoint=?", later.id).find((o) => o.attempts === 1), 5000);

  assert.ok(row);
  assert.deepEqual([row.state, row.next_at - Date.now() > 20_000, row.next_at - Date.now() <= 30_000], ["queued", true, true]);
  assert.equal(row.msg, first.body, "the row holds the bytes it sends");
  // a 404: dropped at once, and the conversation says so
  const asked2 = await hook(gone, envelope({ id: "msg_gone000000001", expects_reply: true }));
  const dropped = await waitFor(() => dbRows(dataDir, "SELECT * FROM agent_outbox WHERE endpoint=?", gone.id).find((o) => o.state === "dropped"), 20000);

  assert.ok(dropped);
  assert.equal(dropped.attempts, 1);
  const goneThread = await convThread(asked2.json.conversation);
  const note = await waitFor(async () => (await turns(goneThread.id)).find((u) => u.role === "notice"), 5000);

  assert.equal(note.text, "Not delivered to Gone (HTTP 404)");
  assert.deepEqual([note.id, note.notice.kind, note.notice.endpoint, note.notice.message], [`drop-${JSON.parse(dropped.msg).id}`, "agents_undelivered", gone.id, JSON.parse(dropped.msg).id]);
  assert.equal(received.filter((x) => x.path === "/gone").length, 1, "a 4xx is not retried");
  // a restart sends what is still queued at once; the 200 delivers it
  await server.stop();
  server = await startServer(dataDir, { env: env() });
  token = readFileSync(join(dataDir, "host-token"), "utf8");
  const delivered = await waitFor(() => dbRows(dataDir, "SELECT * FROM agent_outbox WHERE seq=?", row.seq).find((o) => o.state === "delivered"), 10000);

  assert.ok(delivered, "sent again and delivered");
  assert.equal(delivered.attempts, 2);
  const second = received.filter((x) => x.path === "/later");

  assert.equal(second.length, 2);
  assert.equal(second[1].body, first.body, "the same message");
});

test("AGH-07 the bridge: 401 without its token, 404 for an unknown route, the agents list, a refused send", async () => {
  const unauthorized = refusal("unauthorized", "the bridge token is missing or wrong");

  for (const tok of [null, "nope", ci.secret]) {
    const r = await bridge("GET", "/v1/bridge/agents", tok);

    assert.deepEqual([r.status, r.json], [401, unauthorized], String(tok));
  }
  assert.deepEqual(await bridge("POST", "/v1/bridge/send", null, {}).then((r) => [r.status, r.json]), [401, unauthorized], "the token comes before the route");
  const nope = await bridge("GET", "/v1/bridge/nope", bridgeEp.secret);

  assert.deepEqual([nope.status, nope.json], [404, refusal("not_found", "no route /v1/bridge/nope")]);
  const getSend = await bridge("GET", "/v1/bridge/send", bridgeEp.secret);

  assert.deepEqual([getSend.status, getSend.json], [404, refusal("not_found", "no route /v1/bridge/send")]);
  assert.equal((await bridge("GET", "/v1/bridge/agents", `  ${bridgeEp.secret}`)).status, 200, "Bearer and spaces");
  const agents = await bridge("GET", "/v1/bridge/agents", bridgeEp.secret);

  // the bot's description: the store's default for a bot seeded without one (botDefaults)
  assert.equal(agents.text, '{"ok":true,"agents":[{"id":"boss","name":"Boss","description":"Boss is a personal assistant bot in Buwiz Bot."}]}');
  const empty = await bridge("POST", "/v1/bridge/send", bridgeEp.secret, { subject: "x" });

  assert.deepEqual([empty.status, empty.json], [400, refusal("invalid_args", "`text` is required")]);
  const wrong = await bridge("POST", "/v1/bridge/send", bridgeEp.secret, { to: ["nobody"], subject: "x", text: "y" });

  assert.deepEqual([wrong.status, wrong.json], [400, refusal("invalid_args", "no agent nobody")]);
  assert.deepEqual((await bridge("GET", "/v1/bridge/conversations", bridgeEp.secret)).json, { ok: true, conversations: [] });
});

test("AGH-08 the bridge's long poll wakes when its agent's reply comes", async () => {
  assert.deepEqual((await bridge("POST", "/v1/bridge/receive", bridgeEp.secret, {})).json, { ok: true, messages: [] }, "no wait: at once");
  const started = Date.now();
  const waiting = bridge("POST", "/v1/bridge/receive", bridgeEp.secret, { wait_s: 25 });

  await new Promise((r) => setTimeout(r, 200));
  const sent = await bridge("POST", "/v1/bridge/send", bridgeEp.secret, { subject: "Status", text: "How is Q3?", expects_reply: true });

  assert.equal(sent.status, 202, sent.text);
  assert.deepEqual(Object.keys(sent.json), ["ok", "id", "conversation", "delivered_to", "queued_for"]);
  assert.deepEqual([sent.json.delivered_to, sent.json.queued_for], [[], ["boss"]]);
  const got = await waiting;

  assert.ok(Date.now() - started < 20_000, "woken, not timed out");
  assert.equal(got.status, 200);
  assert.equal(got.json.messages.length, 1);
  assert.deepEqual([got.json.messages[0].in_reply_to, got.json.messages[0].text, got.json.messages[0].from.id], [sent.json.id, MOCK_REPLY, "boss"]);
  assert.deepEqual((await bridge("POST", "/v1/bridge/receive", bridgeEp.secret, {})).json, { ok: true, messages: [] }, "taken once");
  const convs = (await bridge("GET", "/v1/bridge/conversations", bridgeEp.secret)).json.conversations;

  assert.deepEqual(convs.map((c) => [c.conversation, c.subject, c.count]), [[sent.json.conversation, "Status", 2]]);
  assert.deepEqual(convs[0].participants, [{ id: `ext:${bridgeEp.id}`, name: "Claude Code · test" }, { id: "boss", name: "Boss" }]);
});
