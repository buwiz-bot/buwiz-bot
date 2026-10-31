// v0 routines over HTTP (routes/routines.mjs, routines.mjs; the Rust
// broker plan, phase 5 item 5.5): the routes Rox's Details pane calls, a
// routine's run with MOCK turns, the checks through a connected MCP server
// (an inline stub, no network), the failure-streak backoff and a deleted
// chat. Written against Node; the contract the Rust port meets.
//
// Timing: Node has no clock seam for v0 routines. These cases use what it
// gives: Run now (POST /routine/run), the 1 s floor of a timer armed for an
// overdue routine ("first tick now"), and a restart for "overdue routines
// run at start". next_run is last_run + the interval, so the backoff is
// read off it without waiting.
//
//   RV0-01 a fresh server: no routines, no templates
//   RV0-02 GET /routines/templates: every connected server's templates,
//          clamped, with available / reason / needs_email
//   RV0-03 POST /routine/from-template refusals (400 {ok:false, error}),
//          the Gmail one asking for the Google address (need)
//   RV0-04 a Gmail template becomes a routine with a check; the address is
//          kept; the same template in the same chat again is the same one
//   RV0-05 a new routine ticks on its own about a second later (first tick
//          now): the check runs, one MOCK turn for the new items
//   RV0-06 Run now: nothing new → no turn; a new item → one turn; seen ids
//   RV0-07 a prompt-only template: its run posts the shown line and a MOCK
//          reply in its chat; the active chat stays where it is
//   RV0-08 POST /routine/update: on/off, clamps, prompt, name; refusals
//   RV0-09 three failures in a row back off (×2, ×4 … ×64, a day at most);
//          no notice is posted; re-enabling clears the count
//   RV0-10 a deleted chat disables its routine on its next tick
//   RV0-11 Run now on a running routine is refused
//   RV0-12 POST /routine/delete; unknown ids
//   RV0-13 an overdue enabled routine runs at start; a disabled one doesn't
import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { after, before, test } from "node:test";
import { dbRows, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const dataDir = tempDir("routines-v0");
const stubDir = tempDir("routines-v0-stub");
const STUB = join(stubDir, "mcp-stub.mjs");
const STATE = join(stubDir, "state.json");
const CALLS = join(stubDir, "calls.log");
const MOCK_REPLY = "_(mock reply — no model configured)_";
const SERVER = "mailfx";
let server;

// A stdio MCP server whose tools, results and buwiz://manifest come from
// STATE (read on every call, so a test can change what the tools answer),
// each tools/call logged to CALLS. tool: {name, inputSchema, text? |
// structured?, sleep?}.
writeFileSync(STUB, `
import { appendFileSync, readFileSync } from "node:fs";
import { createInterface } from "node:readline";
const st = () => JSON.parse(readFileSync(process.env.STUB_STATE, "utf8"));
const send = (m) => process.stdout.write(JSON.stringify(m) + "\\n");
async function answer(method, params) {
  if (method === "initialize") return { protocolVersion: params?.protocolVersion, capabilities: { tools: {}, resources: {} }, serverInfo: { name: "stub", version: "0.0.0" } };
  if (method === "ping") return {};
  if (method === "tools/list") return { tools: st().tools.map(({ name, inputSchema }) => ({ name, description: name, inputSchema })) };
  if (method === "resources/list") return { resources: [{ uri: "buwiz://manifest", name: "manifest", mimeType: "application/json" }] };
  if (method === "resources/read") return { contents: [{ uri: params.uri, mimeType: "application/json", text: JSON.stringify(st().manifest) }] };
  if (method === "tools/call") {
    const t = st().tools.find((x) => x.name === params.name);
    appendFileSync(process.env.STUB_CALLS, JSON.stringify({ tool: params.name, args: params.arguments ?? {} }) + "\\n");
    if (t?.sleep) await new Promise((r) => setTimeout(r, t.sleep));
    if (!t) return { isError: true, content: [{ type: "text", text: "no tool" }] };
    if (t.structured) return { content: [{ type: "text", text: JSON.stringify(t.structured) }], structuredContent: t.structured };
    return { content: [{ type: "text", text: t.text ?? "" }] };
  }
  return null;
}
createInterface({ input: process.stdin }).on("line", async (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id === undefined) return;
  const result = await answer(msg.method, msg.params);
  send(result ? { jsonrpc: "2.0", id: msg.id, result } : { jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "no method" } });
}).on("close", () => process.exit(0));
`);

const GMAIL_SCHEMA = {
  type: "object",
  properties: { query: { type: "string" }, page_size: { type: "integer" }, user_google_email: { type: "string" } },
  required: ["query", "user_google_email"],
};
const mailText = (ids) => `Found ${ids.length} messages:\n${ids.map((id) => `Message ID: ${id}\nSubject: BIR ${id}\n`).join("")}`;
const state = {
  tools: [
    { name: "bir_query", inputSchema: { type: "object" }, structured: { query: "from:bir.gov.ph" } },
    { name: "search_gmail_messages", inputSchema: GMAIL_SCHEMA, text: mailText(["m1", "m2"]) },
    { name: "get_gmail_messages_content_batch", inputSchema: { type: "object" }, text: "bodies" },
    { name: "slow_check", inputSchema: { type: "object" }, sleep: 1500, structured: { items: [] } },
  ],
  manifest: {
    title: "Mail fixture",
    routines: [
      { id: "bir-mail", name: "BIR mail", description: "Files BIR confirmations.", every_s: 120, needs: "gmail", query: { tool: "bir_query", field: "query" }, prompt: "Record each confirmation." },
      { id: "daily", name: "Daily note", every_s: 30, prompt: "Say good morning." },
      { id: "cal", name: "Calendar sync", needs: "calendar", prompt: "Sync." },
      { id: "noquery", name: "No query", needs: "gmail", prompt: "x" },
      { id: "idle", name: "Idle", description: 5 },
      { id: "", name: "dropped: empty id" },
      { name: "dropped: no id" },
      "dropped: not an object",
    ],
  },
};
const writeState = () => writeFileSync(STATE, JSON.stringify(state));
const calls = () => readFileSync(CALLS, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
const setTool = (name, patch) => (Object.assign(state.tools.find((t) => t.name === name), patch), writeState());

writeState();
writeFileSync(CALLS, "");

const db = () => new DatabaseSync(join(dataDir, "generated", "app.db"), { timeout: 5000 });
function sql(q, ...args) {
  const d = db();

  try {
    return d.prepare(q).run(...args);
  } finally {
    d.close();
  }
}

function addServer(id) {
  sql(
    "INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at, url, auth) VALUES(?,?,?,?,?,?,?,?,?)",
    id, id, process.execPath, JSON.stringify([STUB]), JSON.stringify({ STUB_STATE: STATE, STUB_CALLS: CALLS }), 1, Date.now(), null, null,
  );
}

let rid = 0;
/** A routine row as createRoutine writes it (no v0 route creates one). */
function addRoutine({ name, thread, every_s = 60, enabled = 1, check = null, prompt = "", last_run = null, errors = 0 }) {
  const id = `r-test-${++rid}`;

  sql(
    'INSERT INTO routines(id,name,thread,every_s,enabled,"check",prompt,seen,last_run,errors,created_at,template) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)',
    id, name, thread, every_s, enabled, check ? JSON.stringify(check) : null, prompt, "[]", last_run, errors, Date.now() + rid, null,
  );

  return id;
}

const routines = async () => (await request("GET", "/routines")).json.routines;
const routine = async (id) => (await routines()).find((r) => r.id === id);
const turns = async (id) => (await request("GET", `/turns?thread=${id}`)).json.turns;
const newChat = async () => (await postJson("/thread/new", {})).json.id;
const run = (id) => postJson("/routine/run", { id });

before(async () => {
  mkdirSync(join(dataDir, "widgets"), { recursive: true });
  server = await startServer(dataDir, { env: offlineEnv(join(stubDir, "bin.log")) });
});

after(async () => {
  await server?.stop();
});

test("RV0-01 a fresh server: GET /routines is {routines: []}, GET /routines/templates is []", async () => {
  const r = await request("GET", "/routines");

  assert.equal(r.status, 200);
  assert.match(r.headers["content-type"], /^application\/json/);
  assert.equal(r.text, '{"routines":[]}');
  const t = await request("GET", "/routines/templates");

  assert.equal(t.status, 200);
  assert.equal(t.text, "[]");
});

test("RV0-02 GET /routines/templates lists a connected server's templates: clamped, available or why not, needs_email", async () => {
  addServer(SERVER);
  const t = await request("GET", "/routines/templates");

  assert.equal(t.status, 200, t.text);
  assert.equal(
    t.text,
    JSON.stringify([
      { server: SERVER, id: "bir-mail", name: "BIR mail", description: "Files BIR confirmations.", every_s: 120, needs: "gmail", available: true, needs_email: true },
      { server: SERVER, id: "daily", name: "Daily note", description: "", every_s: 60, needs: null, available: true },
      { server: SERVER, id: "cal", name: "Calendar sync", description: "", every_s: 3600, needs: "calendar", available: false, reason: 'Buwiz Bot can\'t provide "calendar" yet' },
      { server: SERVER, id: "noquery", name: "No query", description: "", every_s: 3600, needs: "gmail", available: false, reason: "The template names no query tool", needs_email: true },
      { server: SERVER, id: "idle", name: "Idle", description: "", every_s: 3600, needs: null, available: false, reason: "The template has nothing to do" },
    ]),
  );
});

test("RV0-03 POST /routine/from-template refusals are 400 {ok:false, error}; the Gmail one asks for the address", async () => {
  const chat = await newChat();
  const refused = async (body) => {
    const r = await postJson("/routine/from-template", body);

    assert.equal(r.status, 400, r.text);
    assert.match(r.headers["content-type"], /^application\/json/);

    return r.json;
  };

  assert.deepEqual(await refused({ server: SERVER, id: "daily", thread: "t999" }), { ok: false, error: "unknown chat t999" });
  assert.deepEqual(await refused({ server: SERVER, id: "daily" }), { ok: false, error: "unknown chat " });
  assert.deepEqual(await refused({ server: "nope", id: "daily", thread: chat }), { ok: false, error: "no connected server nope" });
  assert.deepEqual(await refused({ server: SERVER, id: "zzz", thread: chat }), { ok: false, error: `${SERVER} has no routine template zzz` });
  assert.deepEqual(await refused({ server: SERVER, id: "cal", thread: chat }), { ok: false, error: 'Buwiz Bot can\'t provide "calendar" yet' });
  assert.deepEqual(await refused({ server: SERVER, id: "bir-mail", thread: chat, user_google_email: "bob" }), { ok: false, error: "bob isn't an email address" });
  assert.deepEqual(await refused({ server: SERVER, id: "bir-mail", thread: chat }), {
    ok: false,
    error: "Buwiz Bot needs your Google address for Gmail searches",
    need: "user_google_email",
  });
  // a body that is not JSON
  const bad = await request("POST", "/routine/from-template", { headers: { "Content-Type": "application/json" }, body: "{" });

  assert.equal(bad.status, 400);
  assert.equal(bad.json.ok, false);
  assert.deepEqual(await routines(), [], "nothing was made");
  assert.deepEqual(dbRows(dataDir, "SELECT v FROM meta WHERE k='google_email'"), [], "a bad address is not kept");
});

let mailChat = "";
let mail = null;

test("RV0-04 a Gmail template becomes a routine with a check; the address is kept; the same template again is the same routine", async () => {
  mailChat = await newChat();
  const r = await postJson("/routine/from-template", { server: SERVER, id: "bir-mail", thread: mailChat, user_google_email: " owner@example.com " });

  assert.equal(r.status, 200, r.text);
  assert.equal(r.json.ok, true);
  mail = r.json.routine;
  const prompt =
    `Each new item is a Gmail message from ${SERVER}'s search_gmail_messages. Read them with mcp_${SERVER}_get_gmail_messages_content_batch (user_google_email: owner@example.com).` +
    "\n\nRecord each confirmation.";

  assert.match(mail.id, /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-/, "a UUID v7");
  assert.equal(
    JSON.stringify({ ...mail, created_at: 0, next_run: 0, running: false }),
    JSON.stringify({
      id: mail.id,
      name: "BIR mail",
      thread: mailChat,
      threadTitle: "new chat",
      botName: "",
      every_s: 120,
      enabled: true,
      check: {
        server: SERVER,
        tool: "search_gmail_messages",
        args: { page_size: 25, user_google_email: "owner@example.com" },
        ids: "/Message ID: (\\S+)/",
        args_from: { server: SERVER, tool: "bir_query", args: {}, field: "query", into: "query" },
      },
      prompt,
      seen_count: 0,
      last_run: null,
      last_check: null,
      last_error: null,
      errors: 0,
      next_run: 0,
      running: false,
      created_at: 0,
      template: `${SERVER}/bir-mail`,
    }),
  );
  // never run: next_run is the epoch plus the interval (due at once)
  assert.equal(mail.next_run, 120_000);
  assert.deepEqual(dbRows(dataDir, "SELECT v FROM meta WHERE k='google_email'"), [{ v: "owner@example.com" }]);
  // the address is known now: no template asks for it
  const tpl = (await request("GET", "/routines/templates")).json;

  assert.equal(tpl.find((x) => x.id === "bir-mail").needs_email, undefined);
  const again = await postJson("/routine/from-template", { server: SERVER, id: "bir-mail", thread: mailChat });

  assert.equal(again.json.routine.id, mail.id, "the routine it already has");
  assert.equal(dbRows(dataDir, "SELECT COUNT(*) AS n FROM routines WHERE template=?", `${SERVER}/bir-mail`)[0].n, 1);
});

test("RV0-05 a new routine ticks on its own about a second later: the check, then one MOCK turn for the new items", async () => {
  const done = await waitFor(async () => {
    const r = await routine(mail.id);

    return r.last_run && !r.running && r;
  }, 15000);

  assert.ok(done, "it ran once on its own");
  assert.equal(done.errors, 0, done.last_error);
  assert.equal(done.seen_count, 2);
  assert.equal(done.last_check, done.last_run);
  assert.equal(done.next_run, done.last_run + 120_000);
  const c = calls();

  assert.deepEqual(c.map((x) => x.tool), ["bir_query", "search_gmail_messages"]);
  assert.deepEqual(c[1].args, { page_size: 25, user_google_email: "owner@example.com", query: "from:bir.gov.ph" });
  const t = await turns(mailChat);

  assert.deepEqual(t.map((u) => [u.role, u.text]), [["user", "Routine · BIR mail · 2 new"], ["ai", MOCK_REPLY]]);
  assert.deepEqual(JSON.parse(dbRows(dataDir, "SELECT seen FROM routines WHERE id=?", mail.id)[0].seen), ["m1", "m2"]);
});

test("RV0-06 Run now: nothing new is no turn; a new item is one turn; the seen ids grow", async () => {
  writeFileSync(CALLS, "");
  const quiet = await run(mail.id);

  assert.equal(quiet.status, 200, quiet.text);
  assert.equal(quiet.json.ok, true);
  assert.equal(quiet.json.routine.seen_count, 2);
  assert.equal(quiet.json.routine.running, false);
  assert.equal(quiet.json.routine.last_error, null);
  assert.equal((await turns(mailChat)).length, 2, "no new turn");
  assert.equal(calls().length, 2, "the check ran");
  setTool("search_gmail_messages", { text: mailText(["m1", "m2", "m3"]) });
  const fresh = await run(mail.id);

  assert.equal(fresh.json.routine.seen_count, 3);
  assert.deepEqual((await turns(mailChat)).slice(2).map((u) => [u.role, u.text]), [["user", "Routine · BIR mail · 1 new"], ["ai", MOCK_REPLY]]);
  // a text result the regex finds nothing in: nothing new, no error
  setTool("search_gmail_messages", { text: "No messages found." });
  const none = (await run(mail.id)).json.routine;

  assert.deepEqual([none.errors, none.last_error, none.seen_count], [0, null, 3]);
  assert.equal((await turns(mailChat)).length, 4);
  // the query tool failing: the error is the routine's, no turn
  setTool("bir_query", { name: "bir_query_gone" });
  const failed = (await run(mail.id)).json.routine;

  assert.deepEqual([failed.errors, failed.last_error], [1, "bir_query: no tool"]);
  assert.equal((await turns(mailChat)).length, 4);
  setTool("bir_query_gone", { name: "bir_query" });
  setTool("search_gmail_messages", { text: mailText(["m1", "m2", "m3"]) });
  const ok = (await run(mail.id)).json.routine;

  assert.deepEqual([ok.errors, ok.last_error], [0, null], "a good run clears it");
});

test("RV0-07 a prompt-only template: Run now posts the shown line and a MOCK reply in its own chat; the active chat stays", async () => {
  const chat = await newChat();
  const made = await postJson("/routine/from-template", { server: SERVER, id: "daily", thread: chat });

  assert.equal(made.status, 200, made.text);
  const r = made.json.routine;

  assert.deepEqual([r.check, r.prompt, r.every_s, r.template], [null, "Say good morning.", 60, `${SERVER}/daily`]);
  // its first tick comes on its own; wait for it, then select another chat
  assert.ok(await waitFor(async () => (await routine(r.id)).last_run, 15000));
  const other = await newChat();
  const ran = await run(r.id);

  assert.equal(ran.status, 200, ran.text);
  assert.deepEqual((await turns(chat)).map((u) => [u.role, u.text]), [
    ["user", "Routine · Daily note"],
    ["ai", MOCK_REPLY],
    ["user", "Routine · Daily note"],
    ["ai", MOCK_REPLY],
  ]);
  assert.equal((await request("GET", "/turns")).json.active, other, "the active chat is untouched");
  assert.deepEqual(await turns(other), []);
  // GET /routines: oldest first
  assert.deepEqual((await routines()).map((x) => x.name), ["BIR mail", "Daily note"]);
});

test("RV0-08 POST /routine/update: on/off, clamped interval, prompt, name; refusals", async () => {
  const chat = await newChat();
  const id = addRoutine({ name: "Edit me", thread: chat, every_s: 300, prompt: "p", last_run: Date.now() });
  const up = async (body) => {
    const r = await postJson("/routine/update", { id, ...body });

    assert.equal(r.status, 200, r.text);
    assert.equal(r.json.ok, true);

    return r.json.routine;
  };

  const off = await up({ enabled: false });

  assert.deepEqual([off.enabled, off.next_run], [false, null]);
  assert.equal((await up({ every_s: 10 })).every_s, 60);
  assert.equal((await up({ every_s: 1e9 })).every_s, 604800);
  assert.equal((await up({ every_s: "abc" })).every_s, 60, "not a number is 0, clamped to 60");
  assert.equal((await up({ every_s: 90.6 })).every_s, 91);
  assert.equal((await up({ prompt: "x".repeat(9000) })).prompt.length, 8000);
  assert.equal((await up({ prompt: null })).prompt, "");
  assert.equal((await up({ name: "  Renamed  " })).name, "Renamed");
  assert.equal((await up({ name: "   " })).name, "Renamed", "a blank name is ignored");
  assert.equal((await up({ name: "n".repeat(100) })).name.length, 80);
  const missing = await postJson("/routine/update", { id: "nope", enabled: true });

  assert.equal(missing.status, 400);
  assert.deepEqual(missing.json, { ok: false, error: "no routine nope" });
  // re-enabling an overdue routine arms it: it ticks about a second later
  sql("UPDATE routines SET last_run=NULL, errors=5 WHERE id=?", id);
  const on = await up({ enabled: true });

  assert.deepEqual([on.enabled, on.errors], [true, 0], "re-enabling clears the failure count");
  assert.equal(on.next_run, 91_000, "never run: the epoch plus the interval");
  assert.ok(await waitFor(async () => (await routine(id)).last_run, 10000), "it ticked on its own");
  await waitFor(async () => !(await routine(id)).running, 10000);
  assert.deepEqual((await turns(chat)).map((u) => u.text), [`Routine · ${"n".repeat(80)}`, MOCK_REPLY]);
  sql("UPDATE routines SET enabled=0 WHERE id=?", id);
});

test("RV0-09 three failures in a row back off: x2, x4 ... x64, a day at most; no notice; re-enabling clears the count", async () => {
  const chat = await newChat();
  const id = addRoutine({ name: "Broken", thread: chat, every_s: 60, check: { server: "gone", tool: "x", args: {}, ids: "items[].id" } });
  const step = async () => (await run(id)).json.routine;

  for (const n of [1, 2]) {
    const r = await step();

    assert.deepEqual([r.errors, r.last_error, r.enabled], [n, "gone isn't connected", true]);
    assert.equal(r.next_run, r.last_run + 60_000, "no backoff before the third failure");
  }
  let r = await step();

  assert.equal(r.errors, 3);
  assert.equal(r.next_run, r.last_run + 120_000, "x2 at the third");
  r = await step();
  assert.equal(r.next_run, r.last_run + 240_000, "x4 at the fourth");
  sql("UPDATE routines SET errors=8 WHERE id=?", id);
  r = await step();
  assert.equal(r.errors, 9);
  assert.equal(r.next_run, r.last_run + 60_000 * 64, "x64 at most");
  sql("UPDATE routines SET errors=8, every_s=3600 WHERE id=?", id);
  r = await step();
  assert.equal(r.next_run, r.last_run + 86_400_000, "a day at most");
  sql("UPDATE routines SET errors=8, every_s=604800 WHERE id=?", id);
  r = await step();
  assert.equal(r.next_run, r.last_run + 604_800_000, "an interval over a day is never stretched");
  assert.deepEqual(await turns(chat), [], "v0 posts no notice for a failure streak");
  const on = (await postJson("/routine/update", { id, enabled: true })).json.routine;

  assert.equal(on.errors, 0);
  assert.equal(on.next_run, on.last_run + 604_800_000);
  sql("UPDATE routines SET enabled=0 WHERE id=?", id);
});

test("RV0-10 a deleted chat disables its routine on its next tick", async () => {
  const chat = await newChat();
  const id = addRoutine({ name: "Orphan", thread: chat, prompt: "hi", last_run: Date.now() });

  assert.equal((await postJson("/thread/delete", { id: chat })).json.ok, true);
  const r = await run(id);

  assert.equal(r.status, 200, r.text);
  assert.deepEqual([r.json.routine.enabled, r.json.routine.last_error, r.json.routine.errors, r.json.routine.next_run], [false, "its chat was deleted", 0, null]);
  assert.equal(r.json.routine.threadTitle, "");
});

test("RV0-11 Run now on a routine that is running is refused", async () => {
  const chat = await newChat();
  const id = addRoutine({ name: "Slow check", thread: chat, check: { server: SERVER, tool: "slow_check", args: {}, ids: "items[].id" }, last_run: Date.now() });
  const first = run(id);

  assert.ok(await waitFor(async () => (await routine(id)).running, 5000), "it shows as running");
  const second = await run(id);

  assert.equal(second.status, 400);
  assert.deepEqual(second.json, { ok: false, error: "Slow check is already running" });
  const done = await first;

  assert.equal(done.status, 200);
  assert.deepEqual([done.json.routine.running, done.json.routine.errors, done.json.routine.last_check], [false, 0, done.json.routine.last_run]);
  sql("UPDATE routines SET enabled=0 WHERE id=?", id);
});

test("RV0-12 POST /routine/delete removes it; unknown ids are 400", async () => {
  const chat = await newChat();
  const id = addRoutine({ name: "Doomed", thread: chat, prompt: "x", last_run: Date.now() });
  const r = await postJson("/routine/delete", { id });

  assert.equal(r.status, 200);
  assert.equal(r.text, '{"ok":true}');
  assert.equal(await routine(id), undefined);

  for (const path of ["/routine/delete", "/routine/run"]) {
    const gone = await postJson(path, { id });

    assert.equal(gone.status, 400);
    assert.deepEqual(gone.json, { ok: false, error: `no routine ${id}` });
  }
  // GET on a POST route is not a routine route
  assert.equal((await request("GET", "/routine/run")).status, 404);
});

test("RV0-13 at start an overdue enabled routine runs; a disabled one does not", async () => {
  const chat = await newChat();
  const due = addRoutine({ name: "Due at start", thread: chat, prompt: "x" });
  const off = addRoutine({ name: "Off at start", thread: chat, prompt: "x", enabled: 0 });

  await server.stop();
  server = await startServer(dataDir, { env: offlineEnv(join(stubDir, "bin.log")) });
  const ran = await waitFor(async () => {
    const r = await routine(due);

    return r?.last_run && !r.running && r;
  }, 15000);

  assert.ok(ran, "it ran after the start");
  assert.equal((await routine(off)).last_run, null);
  assert.deepEqual((await turns(chat)).map((u) => u.text), ["Routine · Due at start", MOCK_REPLY]);
});
