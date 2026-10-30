// The chat routes (Rust broker plan, phase 5 item 5.2), as Node answers
// them today: threads, turns, reactions, retry, /status, /chat's refusals,
// copy and share. MOCK on; the Mac tools are PATH stubs (fixtures/bin) and
// every model URL points at a closed port.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { dbRows, nodeIn, offlineEnv, postJson, request, startServer, stubCalls, tempDir, waitFor } from "./helpers.mjs";

const dir = tempDir("chat-routes");
const log = join(dir, "stubs.log");
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

let server;
// an agents' conversation, seeded through Node's store before the start
let agentsThread;

before(async () => {
  agentsThread = nodeIn(dir, `
    const { store, nextThreadSeq, uuidv7, writeIndex } = await import("./src/state.mjs");
    const id = "t" + nextThreadSeq();
    store.list.push({ id, uid: uuidv7(), title: "Q3 payments", turns: [], agents: { subject: "Q3 payments", participants: ["piccolo", "boss"] } });
    writeIndex();
    console.log(id);
  `);
  server = await startServer(dir, { env: offlineEnv(log) });
});

after(async () => {
  await server?.stop();
});

const threads = async () => (await request("GET", "/threads")).json;
const turnsOf = async (id) => (await request("GET", `/turns${id ? `?thread=${id}` : ""}`)).json.turns;
const done = (id) => waitFor(async () => {
  const j = (await request("GET", `/status?id=${id}`)).json;

  return j.done && j;
}, 30000);
// a /chat that runs to its end; its job's last /status
const chat = async (body) => {
  const r = await postJson("/chat", body);

  assert.equal(r.status, 200, r.text);

  return done(r.json.id);
};
const bad = (r, error) => {
  assert.equal(r.status, 400, r.text);
  assert.deepEqual(r.json, { ok: false, error });
};

test("CHAT-01 /thread/new puts a new chat first and makes it active", async () => {
  const before = await threads();

  assert.equal(before.model, "mock");
  const r = await postJson("/thread/new", {});

  assert.equal(r.status, 200);
  assert.equal(r.headers["content-type"], "application/json");
  assert.deepEqual(Object.keys(r.json), ["ok", "id"]);
  assert.equal(r.json.ok, true);
  const after = await threads();

  assert.equal(after.active, r.json.id);
  assert.match(after.activeUid, UUID);
  assert.deepEqual(after.list[0], { id: r.json.id, uid: after.activeUid, title: "new chat", count: 0, hidden: false, archived: false, lastApp: "", bot: null });
  assert.equal(after.list.length, before.list.length + 1);
  assert.deepEqual(after.sections, []);
  assert.deepEqual(after.bots, []);
});

test("CHAT-02 /thread/update renames (trimmed; blank keeps the title) and flags hidden and archived", async () => {
  const id = (await postJson("/thread/new", {})).json.id;

  assert.deepEqual((await postJson("/thread/update", { id, title: "  Renamed  ", hidden: true })).json, { ok: true });
  let t = (await threads()).list.find((x) => x.id === id);

  assert.equal(t.title, "Renamed");
  assert.equal(t.hidden, true);
  assert.equal(t.archived, false);
  await postJson("/thread/update", { id, title: "   ", hidden: false, archived: 1 });
  t = (await threads()).list.find((x) => x.id === id);
  assert.deepEqual([t.title, t.hidden, t.archived], ["Renamed", false, true]);
  bad(await postJson("/thread/update", { id: "t999", title: "x" }), "unknown thread t999");
});

test("CHAT-03 /thread/select by id or uid answers the chat's turns; an unknown one is refused", async () => {
  const a = (await postJson("/thread/new", {})).json.id;
  const uid = (await threads()).activeUid;
  const b = (await postJson("/thread/new", {})).json.id;

  assert.equal((await threads()).active, b);
  assert.deepEqual((await postJson("/thread/select", { id: a })).json, { ok: true, id: a, uid, turns: [] });
  assert.equal((await threads()).active, a);
  await postJson("/thread/select", { id: b });
  assert.deepEqual((await postJson("/thread/select", { uid })).json, { ok: true, id: a, uid, turns: [] });
  assert.equal((await threads()).active, a);
  bad(await postJson("/thread/select", { id: "nope" }), "unknown thread nope");
  bad(await postJson("/thread/select", { uid: "x" }), "unknown thread x");
  bad(await postJson("/thread/select", {}), "unknown thread undefined");
});

test("CHAT-04 a MOCK /chat builds a card; the title is the prompt's first 42 UTF-16 units; /status keeps the event ring", async () => {
  const id = (await postJson("/thread/new", {})).json.id;
  // the cut falls inside the emoji's surrogate pair
  const prompt = `counter ${"x".repeat(33)}😀 and more`;
  const r = await postJson("/chat", { prompt, thread: id });

  assert.equal(r.status, 200);
  assert.deepEqual(Object.keys(r.json), ["ok", "id"]);
  const st = await done(r.json.id);
  const name = /^(R\d+)\.tsx via mock$/.exec(st.result)?.[1];

  assert.ok(name, st.result);
  assert.deepEqual(st, {
    id: r.json.id,
    seq: 4,
    phase: "done",
    detail: `rendered — ${name}.tsx via mock`,
    done: true,
    result: `${name}.tsx via mock`,
    events: [
      { seq: 1, phase: "fallback", detail: "Trying another way" },
      { seq: 2, phase: "checking", detail: "Checking the code" },
      { seq: 3, phase: "publishing", detail: "Putting it in place" },
      { seq: 4, phase: "done", detail: `rendered — ${name}.tsx via mock` },
    ],
    open: null,
  });
  const t = (await threads()).list.find((x) => x.id === id);

  assert.equal(t.title, prompt.slice(0, 42));
  assert.equal(t.title.at(-1), "\ud83d");
  assert.equal(t.count, 2);
  assert.equal(t.lastApp, name);
  const turns = await turnsOf(id);

  assert.deepEqual(turns.map(({ i, role, text, name: n, label, caption, retries, v, widget }) => ({ i, role, text, name: n, label, caption, retries, v, widget })), [
    { i: 0, role: "user", text: prompt, name: "", label: "", caption: "", retries: 0, v: null, widget: false },
    { i: 1, role: "ai", text: "", name, label: "mock", caption: "", retries: 0, v: 1, widget: false },
  ]);
  assert.deepEqual(Object.keys(turns[1]), ["i", "id", "role", "text", "name", "label", "caption", "reaction", "retries", "widget", "reply", "attachments", "files", "links", "v", "suggest"]);
  assert.ok(existsSync(join(dir, "generated", `${name}.tsx`)));
});

test("CHAT-05 /status of a job nobody knows", async () => {
  assert.deepEqual((await request("GET", "/status?id=999")).json, { id: 999, seq: 0, phase: "unknown", detail: "", done: true, events: [], open: null });
  assert.deepEqual((await request("GET", "/status?id=abc")).json, { id: null, seq: 0, phase: "unknown", detail: "", done: true, events: [], open: null });
});

test("CHAT-06 /react: idx: in the active chat, name: on every card of that name, empty clears", async () => {
  const id = (await postJson("/thread/new", {})).json.id;

  await chat({ prompt: "make a counter", thread: id });
  const [, card] = await turnsOf(id);

  assert.deepEqual((await postJson("/react", { key: "idx:0", r: "❤️" })).json, { ok: true });
  assert.equal((await turnsOf(id))[0].reaction, "❤️");
  await postJson("/react", { name: card.name, r: "👍" });
  assert.equal((await turnsOf(id))[1].reaction, "👍");
  await postJson("/react", { name: card.name, r: "" });
  await postJson("/react", { key: "idx:0" });
  assert.deepEqual((await turnsOf(id)).map((t) => t.reaction), ["", ""]);
  // an index past the end changes nothing and is not refused
  assert.deepEqual((await postJson("/react", { key: "idx:99", r: "x" })).json, { ok: true });
});

test("CHAT-07 /turn/delete: idx: drops one message, name: the card and its prompt; misses are refused", async () => {
  const id = (await postJson("/thread/new", {})).json.id;

  await chat({ prompt: "make a counter", thread: id });
  await chat({ prompt: "/upload", thread: id });
  const turns = await turnsOf(id);

  assert.deepEqual(turns.map((t) => t.role), ["user", "ai", "user", "ai"]);
  assert.equal(turns[3].text, "Attach a file and /upload echoes it back as a viewable card — nothing goes to the model.");
  assert.equal(turns[3].label, "upload");
  assert.deepEqual((await postJson("/turn/delete", { key: "idx:3" })).json, { ok: true, dropped: 1 });
  assert.deepEqual((await postJson("/turn/delete", { key: `name:${turns[1].name}` })).json, { ok: true, dropped: 2 });
  assert.deepEqual((await turnsOf(id)).map((t) => t.text), ["/upload"]);
  bad(await postJson("/turn/delete", { key: "idx:99" }), "no such turn");
  bad(await postJson("/turn/delete", { key: "name:R999" }), "no such turn");
  bad(await postJson("/turn/delete", { key: "bogus" }), "no such turn");
  bad(await postJson("/turn/delete", {}), "no such turn");
});

test("CHAT-08 /retry (MOCK) lands a new version of the card, no new prompt; an unknown card is refused", async () => {
  const id = (await postJson("/thread/new", {})).json.id;

  await chat({ prompt: "make a counter", thread: id });
  const name = (await turnsOf(id))[1].name;
  const r = await postJson("/retry", { name });

  assert.deepEqual(Object.keys(r.json), ["ok", "id"]);
  const st = await done(r.json.id);

  assert.equal(st.result, `${name}.tsx via mock`);
  const turns = await turnsOf(id);

  assert.deepEqual(turns.map(({ role, name: n, label, retries, v }) => ({ role, name: n, label, retries, v })), [
    { role: "user", name: "", label: "", retries: 0, v: null },
    { role: "ai", name, label: "mock", retries: 0, v: 1 },
    { role: "ai", name, label: "mock", retries: 1, v: 2 },
  ]);
  bad(await postJson("/retry", { name: "R999" }), "no prompt found for R999");
});

test("CHAT-09 /chat refusals: 400 without a prompt or with an unknown thread (no content-type), 409 in an agents' conversation", async () => {
  const none = await postJson("/chat", {});

  bad(none, "missing prompt");
  assert.equal(none.headers["content-type"], undefined);
  bad(await postJson("/chat", { prompt: "", attachments: [] }), "missing prompt");
  bad(await postJson("/chat", { prompt: "x", thread: "t9999" }), "unknown thread t9999");
  bad(await postJson("/chat", { prompt: "x", thread: "y".repeat(60) }), `unknown thread ${"y".repeat(40)}`);
  const junk = await request("POST", "/chat", { headers: { "Content-Type": "application/json" }, body: "{nope" });

  assert.equal(junk.status, 400);
  assert.equal(junk.json.ok, false);
  assert.match(junk.json.error, /JSON/);
  const ro = { ok: false, code: "read_only", error: "an agents' conversation is read-only: message an agent in its own chat" };
  const named = await postJson("/chat", { prompt: "let me in", thread: agentsThread });

  assert.equal(named.status, 409);
  assert.equal(named.headers["content-type"], "application/json");
  assert.deepEqual(named.json, ro);
  // the active chat when none is named
  await postJson("/thread/select", { id: agentsThread });
  const active = await postJson("/chat", { prompt: "let me in" });

  assert.equal(active.status, 409);
  assert.deepEqual(active.json, ro);
  assert.deepEqual(await turnsOf(agentsThread), []);
  await postJson("/thread/new", {});
});

test("CHAT-10 /status remembers the last 25 jobs", async () => {
  const id = (await postJson("/thread/new", {})).json.id;
  const ids = [];

  for (let k = 0; k < 26; k++) ids.push((await postJson("/chat", { prompt: "/upload", thread: id })).json.id);
  await done(ids.at(-1));
  assert.equal((await request("GET", `/status?id=${ids[0]}`)).json.phase, "unknown");
  assert.equal((await request("GET", `/status?id=${ids[1]}`)).json.phase, "done");
  assert.deepEqual((await request("GET", `/status?id=${ids[1]}`)).json.events, [{ seq: 1, phase: "done", detail: "replied" }]);
});

test("CHAT-11 /copy and /share/thread go through pbcopy; /share reveals the module with open -R", async () => {
  const id = (await postJson("/thread/new", {})).json.id;

  await chat({ prompt: "make a counter", thread: id });
  await chat({ prompt: "/upload", thread: id });
  const [, card] = await turnsOf(id);
  const n = stubCalls(log).length;

  assert.deepEqual((await postJson("/copy", { text: "hello clipboard" })).json, { ok: true });
  assert.deepEqual((await postJson("/copy", {})).json, { ok: true });
  const file = join(dir, "generated", `${card.name}.tsx`);

  assert.deepEqual((await postJson("/share", { name: card.name })).json, { ok: true });
  bad(await postJson("/share", { name: "R999" }), "no such module R999");
  bad(await postJson("/share", { name: "../x" }), "no such module ../x");
  assert.deepEqual((await postJson("/share/thread", {})).json, { ok: true });
  await waitFor(() => stubCalls(log).slice(n).some((c) => c.bin === "open"));
  const calls = stubCalls(log).slice(n).filter((c) => c.bin === "pbcopy" || c.bin === "open");
  const code = readFileSync(file, "utf8");

  assert.deepEqual(calls.filter((c) => c.bin === "open").map((c) => c.argv), [["-R", file]]);
  assert.deepEqual(calls.filter((c) => c.bin === "pbcopy").map((c) => c.stdin), [
    "hello clipboard",
    "",
    // BUG: a text reply (no module) is shared as "Gen UI (undefined.tsx via
    // <label>):" with an empty body — Node's behaviour today
    `You: make a counter\n\nGen UI (${card.name}.tsx via mock):\n${code}\n\nYou: /upload\n\nGen UI (undefined.tsx via upload):\n`,
  ]);
});

test("CHAT-12 /turns and /thread/apps fall back to the active chat for an unknown thread; /turns takes a uid", async () => {
  const id = (await postJson("/thread/new", {})).json.id;
  const t = await threads();
  const uid = t.list.find((x) => x.id === id).uid;

  await chat({ prompt: "make a counter", thread: id });
  const byUid = (await request("GET", `/turns?thread=${uid}`)).json;

  assert.equal(byUid.thread, id);
  assert.equal(byUid.turns.length, 2);
  const other = (await postJson("/thread/new", {})).json.id;
  const unknown = (await request("GET", "/turns?thread=t9999")).json;

  assert.deepEqual(Object.keys(unknown), ["active", "activeUid", "thread", "turns"]);
  assert.equal(unknown.thread, other);
  assert.deepEqual((await request("GET", "/thread/apps?thread=t9999")).json, { apps: [] });
  assert.deepEqual((await request("GET", `/thread/apps?thread=${id}`)).json.apps.map((a) => [a.id, a.kind]), [[byUid.turns[1].name, "app"]]);
});

test("CHAT-13 /thread/delete sweeps the chat's unpublished modules, their state and app rows; a bot keeps its persona", async () => {
  const id = (await postJson("/thread/new", {})).json.id;

  await chat({ prompt: "make a counter", thread: id });
  const name = (await turnsOf(id))[1].name;

  await postJson(`/state/${name}`, { n: 1 });
  await postJson("/app/meta", { name, title: "Counter" });
  assert.equal(dbRows(dir, "SELECT name FROM kv WHERE name=?", name).length, 1);
  assert.equal(dbRows(dir, "SELECT name FROM apps WHERE name=?", name).length, 1);
  const bot = (await postJson("/bot/new", { name: "Ada" })).json.bot;
  const r = await postJson("/thread/delete", { id });

  assert.deepEqual(r.json, { ok: true, active: bot.threadId });
  assert.ok(!existsSync(join(dir, "generated", `${name}.tsx`)));
  assert.equal(dbRows(dir, "SELECT name FROM kv WHERE name=?", name).length, 0);
  assert.equal(dbRows(dir, "SELECT name FROM apps WHERE name=?", name).length, 0);
  // the version tree stays
  assert.equal(dbRows(dir, "SELECT v FROM versions WHERE name=?", name).length, 1);
  assert.deepEqual((await postJson("/thread/delete", { id: bot.threadId })).json.ok, true);
  const b = (await threads()).bots.find((x) => x.id === bot.id);

  assert.equal(b.threadId, null);
  assert.equal(b.name, "Ada");
  bad(await postJson("/thread/delete", { id: "nope" }), "unknown thread nope");
});

test("CHAT-14 deleting every chat leaves one new chat, never none", async () => {
  for (const t of (await threads()).list.slice(1)) await postJson("/thread/delete", { id: t.id });
  const last = (await threads()).list[0].id;
  const r = await postJson("/thread/delete", { id: last });
  const after = await threads();

  assert.equal(after.list.length, 1);
  assert.deepEqual(r.json, { ok: true, active: after.list[0].id });
  assert.notEqual(after.list[0].id, last);
  assert.deepEqual({ ...after.list[0], uid: "" }, { id: after.list[0].id, uid: "", title: "new chat", count: 0, hidden: false, archived: false, lastApp: "", bot: null });
  assert.equal(after.active, after.list[0].id);
});
