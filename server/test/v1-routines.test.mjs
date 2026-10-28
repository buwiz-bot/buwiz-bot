// The v1 routine runner (server/v1/routines.mjs, spec 5.9 "How a routine
// runs"): a fixture routine that reads Gmail through the gws plugin over
// the fake workspace-mcp upstream, run tick by tick on an injected clock
// with the agent turn recorded instead of run (no model). RTN-* are this
// suite's own IDs, an extension of spec 17:
//   RTN-01 an enabled routine the user has not approved does not run
//   RTN-02 a tick calls the own query tool with origin routine, then the
//          reads search with that text; a new item runs the prompt once
//   RTN-03 a seen item does not run again; only new ones do; not before due
//   RTN-04 a reads tool without readOnlyHint makes the routine unavailable
//   RTN-05 an empty search text is nothing to do; a switched-off tool or a
//          stopped reads plugin makes it unavailable
//   RTN-06 the turn's v1 tools are the plugin's own plus the listed reads
//   RTN-07 an update that changes `reads` needs a new approval
//   RTN-08 bad requests are refused; a deleted chat stops the routine;
//          uninstall forgets it
//   RTN-09 (v1-host.test.mjs) the host route approves; the list shows it
//   RTN-10 a run's turns go to the routine's own thread, made on its first
//          run and reused after; the chat it reports to holds no item turn
//   RTN-11 each run with new items leaves one notice in the chat it reports
//          to, pointing at the run; a run with nothing new leaves none
//   RTN-12 a failure leaves one notice per failure streak
//   RTN-13 the routine's thread exists once it is enabled, before any run
//          (one enabled earlier gets it at the server's start or its next
//          tick); a run with nothing new adds no turn to it
//   RTN-14 in a routine's turn a call to its own plugin names the routine
//          (rox/caller.routine), and a tool of it that changes data runs
//          only when the routine lists it in `writes`
//   RTN-15 Run now (POST /v1/host/routines/run) ticks once out of schedule
//          and counts like a run; paused, running or unavailable is refused
//   RTN-16 the agent lists and runs the plugin routines of its chats only;
//          it has no way to enable, approve or change one
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { v1AgentCall, v1ToolPlugin, v1Tools } from "../server/v1/agent.mjs";
import { createRoutineThreads } from "../server/v1/routine-threads.mjs";
import { createRoutines } from "../server/v1/routines.mjs";
import { FX, GWS, bed, broker, codeOf, fixtureSpec, installFixture, installGws, upstreamCalls, writeMail } from "./v1-helpers.mjs";

let now = 1_800_000_000_000;
const clock = { now: () => now, setTimeout: (f, ms) => setTimeout(f, ms), clearTimeout: (t) => clearTimeout(t) };
const b = bed("routines");
const { plugins, db } = broker(b, { clock });
const turns = [];
const chats = new Set(["chat-1"]);
// the chat store's shape, as routine-threads.mjs sees it
const store = { list: [{ id: "chat-1", uid: "uid-chat-1", title: "Piccolo", turns: [] }] };
let ids = 0;
let failTurn = false;
const threads = createRoutineThreads({ store, nextId: () => `t${100 + ++ids}`, uid: () => `uid-${++ids}`, save: () => {} });
// the agent turn, recorded instead of run: the shown turn and a reply land
// in the thread the runner named, as backgroundTurn's would
async function runTurn(t) {
  if (failTurn) throw new Error("the model is unreachable");
  turns.push(t);
  const th = store.list.find((x) => x.id === t.thread);
  const at = { seq: th.turns.length, turn: `turn-${++ids}` };

  th.turns.push({ id: at.turn, role: "user", text: t.shown, routine: t.mark }, { id: `turn-${++ids}`, role: "ai", text: "Recorded." });

  return at;
}
const routines = createRoutines({ db, plugins, clock, runTurn, threads, threadExists: (id) => chats.has(id) });
const chat = () => store.list[0];
const notices = () => chat().turns.filter((t) => t.role === "notice");
const home = () => store.list.find((t) => t.routine?.id === "bir-mail");
// a routine's only instance
const inst = (id) => db.prepare("SELECT id FROM v1_instances WHERE plugin=? AND routine=?").get(FX, id)?.id;

const READS = [{ plugin: GWS, tools: ["gws_search_mail", "gws_read_mail"] }];

const ROUTINES = [
  { id: "bir-mail", name: "BIR mail", description: "Files BIR confirmations.", every_s: 300, query: { tool: "fx_query", field: "query", args: { q: "from:bir.gov.ph" } }, prompt: "Record each BIR confirmation.", reads: READS },
  { id: "bad-reads", name: "Bad reads", description: "Reads a tool that writes.", every_s: 300, query: { tool: "fx_query", field: "query" }, prompt: "Echo.", reads: [{ plugin: FX, tools: ["fx_echo"] }] },
  { id: "empty", name: "Empty", description: "Has nothing to search for.", every_s: 300, query: { tool: "fx_query", field: "query", args: { q: "" } }, prompt: "Nothing.", reads: READS },
];

const MAIL1 = { id: "m1", from: "BIR eFPS <efps@bir.gov.ph>", to: "owner@example.com", subject: "Tax Return Receipt Confirmation", date: "Mon, 26 Oct 2026 09:12:00 +0800", body: "1701Q received. Reference 2026-ABC-123." };
const MAIL2 = { id: "m2", from: "BIR eFPS <efps@bir.gov.ph>", subject: "Tax Return Receipt Confirmation", date: "Tue, 27 Oct 2026 10:00:00 +0800", body: "2550Q received." };

const stats = async () => (await plugins.callTool(FX, "fx_stats", {})).structuredContent;
const searches = () => upstreamCalls(b).filter((c) => c.tool === "search_gmail_messages");
const listed = (id) => plugins.list().find((p) => p.id === FX).routines.find((r) => r.id === id);
const step = (s) => (now += s * 1000);

before(async () => {
  writeMail(b, [MAIL1]);
  b.writeManifest((m) => (m.routines = ROUTINES));
  await installGws(plugins, b);
  await installFixture(plugins, b);
});

after(() => plugins.stopAll());

test("RTN-01 an enabled routine the user has not approved does not run", async () => {
  routines.enable({ plugin: FX, routine: "bir-mail", thread: "chat-1" });
  const r = listed("bir-mail");

  assert.equal(r.enabled, true);
  assert.equal(r.approved, false);
  assert.equal(r.available, false);
  assert.match(r.reason, /not approved/);
  assert.equal(home().title, "BIR mail", "its thread exists before any run");
  assert.deepEqual(home().turns, []);
  assert.equal(r.home, home().id);
  assert.equal(r.home_uid, home().uid);
  assert.deepEqual(r.reads_status.map((s) => s.allowed), [true], "the reads themselves are allowed");
  await routines.runDue();
  assert.equal(turns.length, 0);
  assert.equal((await stats()).calls.fx_query, undefined, "the query tool was not called");
  assert.equal(searches().length, 0, "nothing was searched");
});

test("RTN-02 a tick: own query with origin routine, the reads search with its text, one prompt per new item", async () => {
  const a = routines.approve({ plugin: FX, routine: "bir-mail" });

  assert.equal(a.routine.approved, true);
  assert.equal(a.routine.available, true);
  step(300);
  await routines.runDue();
  const s = await stats();

  assert.equal(s.calls.fx_query, 1);
  assert.equal(s.origins.fx_query, "routine", "8.4: the timer's own query is origin routine");
  assert.deepEqual(searches().map((c) => c.args.query), ["from:bir.gov.ph"], "the search got the query string");
  assert.equal(turns.length, 1, "exactly one prompt run");
  const [t] = turns;

  assert.equal(t.thread, home().id, "the routine's own thread");
  assert.equal(t.chat, "chat-1");
  assert.equal(t.shown, "New item · Tax Return Receipt Confirmation");
  assert.deepEqual(t.item, { id: "m1", from: MAIL1.from, subject: MAIL1.subject, date: MAIL1.date, snippet: MAIL1.body });
  assert.match(t.prompt, /^\(This routine runs on its own every 300s/);
  assert.match(t.prompt, /## Routine: BIR mail \(approved plugin dev\.goldcoders\.fixture\)\nRecord each BIR confirmation\./, "10.2 label");
  assert.match(t.prompt, /New item from dev\.goldcoders\.google-workspace gws_search_mail:\n\{\n "id": "m1"/);
  assert.deepEqual(t.surface, { plugin: FX, routine: "bir-mail", instance: inst("bir-mail"), params: {}, reads: READS, writes: [] });
  assert.equal(listed("bir-mail").seen_count, 1);
  assert.equal(listed("bir-mail").last_error, null);
  assert.equal(listed("bir-mail").last_items, 1);
});

test("RTN-03 a seen item does not run again; a new one runs once; nothing runs before it is due", async () => {
  step(300);
  await routines.runDue();
  assert.equal(searches().length, 2);
  assert.equal(turns.length, 1, "m1 was seen");
  writeMail(b, [MAIL2, MAIL1]);
  step(299);
  await routines.runDue();
  assert.equal(searches().length, 2, "not due yet");
  step(1);
  await routines.runDue();
  assert.equal(turns.length, 2);
  assert.equal(turns[1].item.id, "m2");
  // the seen ids survive a new runner on the same db
  const again = createRoutines({ db, plugins, clock, runTurn, threads, threadExists: (id) => chats.has(id) });

  assert.deepEqual(await again.tick(FX, "bir-mail"), { ran: true, items: 2, prompts: 0 });
});

test("RTN-04 a reads tool without readOnlyHint makes the routine unavailable", async () => {
  routines.enable({ plugin: FX, routine: "bad-reads", thread: "chat-1" });
  routines.approve({ plugin: FX, routine: "bad-reads" });
  const r = listed("bad-reads");

  assert.equal(r.approved, true);
  assert.equal(r.available, false);
  assert.match(r.reason, /read-only/);
  const before = turns.length;
  const res = await routines.tick(FX, "bad-reads");

  assert.equal(res.ran, false);
  assert.match(res.reason, /fx_echo/);
  assert.equal((await stats()).calls.fx_echo, undefined, "fx_echo was never called");
  assert.equal(turns.length, before);
  routines.disable({ plugin: FX, routine: "bad-reads" });
});

test("RTN-05 an empty search text is nothing to do; a switched-off tool or a stopped reads plugin makes it unavailable", async () => {
  routines.enable({ plugin: FX, routine: "empty", thread: "chat-1" });
  routines.approve({ plugin: FX, routine: "empty" });
  const n = searches().length;

  assert.deepEqual(await routines.tick(FX, "empty"), { ran: true, items: 0, prompts: 0 });
  assert.equal(searches().length, n, "no search for an empty text");
  assert.deepEqual(store.list.find((t) => t.routine?.id === "empty").turns, [], "nothing new adds no turn");
  assert.equal(listed("empty").last_items, 0);
  routines.disable({ plugin: FX, routine: "empty" });
  plugins.setTool(GWS, "gws_search_mail", true);
  assert.match((await routines.tick(FX, "bir-mail")).reason, /gws_search_mail is switched off/);
  plugins.setTool(GWS, "gws_search_mail", false);
  plugins.setTool(FX, "fx_query", true);
  assert.match((await routines.tick(FX, "bir-mail")).reason, /fx_query is switched off/);
  plugins.setTool(FX, "fx_query", false);
  await plugins.disable(GWS);
  assert.match(listed("bir-mail").reason, /google-workspace is disabled/);
  assert.equal((await routines.tick(FX, "bir-mail")).ran, false);
  assert.equal((await plugins.enable(GWS)).state, "running");
  assert.equal((await routines.tick(FX, "bir-mail")).ran, true);
});

test("RTN-13 an enabled routine's thread is made at the server's start or its next tick, empty", async () => {
  const empty = () => store.list.find((t) => t.routine?.id === "empty");
  const notes = notices().length;

  // enabled before threads were made early: no thread yet
  store.list.splice(store.list.indexOf(empty()), 1);
  db.prepare("UPDATE v1_instances SET enabled=1 WHERE plugin=? AND routine=?").run(FX, "empty");
  const again = createRoutines({ db, plugins, clock, runTurn, threads, threadExists: (id) => chats.has(id), pollMs: 3_600_000 });

  again.start();
  again.stop();
  assert.deepEqual(empty().routine, { plugin: FX, id: "empty", instance: inst("empty"), name: "Empty", thread: "chat-1" });
  assert.equal(empty().title, "Empty");
  assert.deepEqual(empty().turns, []);
  store.list.splice(store.list.indexOf(empty()), 1);
  assert.deepEqual(await routines.tick(FX, "empty"), { ran: true, items: 0, prompts: 0 });
  assert.deepEqual(empty().turns, [], "made by the tick, and nothing new adds no turn");
  assert.equal(listed("empty").home, empty().id);
  const id = empty().id;

  await routines.tick(FX, "empty");
  routines.enable({ plugin: FX, routine: "empty", thread: "chat-1" });
  assert.equal(store.list.filter((t) => t.routine?.id === "empty").length, 1, "made once");
  assert.equal(empty().id, id);
  assert.equal(notices().length, notes, "the chat it reports to got no notice");
  routines.disable({ plugin: FX, routine: "empty" });
});

test("RTN-06 the turn's v1 tools are the routine plugin's own plus only the listed reads tools", () => {
  const surface = { plugin: FX, reads: [{ plugin: GWS, tools: ["gws_search_mail"] }] };
  const names = v1Tools(plugins, new Set(), surface).map((t) => t.name);

  assert.ok(names.includes("fx_echo") && names.includes("fx_query"), "the plugin's own tools");
  assert.ok(names.includes("gws_search_mail"), "a listed reads tool");
  assert.ok(!names.includes("gws_read_mail"), "an unlisted tool of the reads plugin");
  assert.equal(v1ToolPlugin(plugins, "gws_read_mail", surface), null, "and it cannot be called");
  assert.equal(v1ToolPlugin(plugins, "gws_read_mail"), GWS, "outside a routine it can");
  const full = v1Tools(plugins, new Set(), turns[0].surface).map((t) => t.name);

  assert.ok(full.includes("gws_search_mail") && full.includes("gws_read_mail"));
});

test("RTN-14 a routine's turn calls its own tools that change data only when its writes list them", async () => {
  const job = { v1Surface: turns[0].surface };
  const refused = await v1AgentCall(plugins, job, "fx_echo", { a: 1 });

  assert.equal(refused.is_error, true);
  assert.match(refused.content, /fx_echo changes data, and routine bir-mail may change data only with the tools its manifest lists in writes/);
  assert.equal((await stats()).calls.fx_echo, undefined, "nothing reached the plugin");
  // a read-only tool of its own runs
  assert.equal((await v1AgentCall(plugins, job, "fx_search", { query: "x" })).is_error, false);
  // a write the routine lists runs, and the plugin sees which routine calls
  const allowed = await v1AgentCall(plugins, { v1Surface: { ...turns[0].surface, writes: ["fx_echo"] } }, "fx_echo", { a: 1 });

  assert.equal(allowed.is_error, false, allowed.content);
  const caller = JSON.parse(allowed.content).caller;

  assert.equal(caller.origin, "agent");
  assert.equal(caller.routine, "bir-mail");
  assert.equal(caller.plugin, FX);
  // outside a routine nothing changes: no routine named, no writes needed
  const chat = JSON.parse((await v1AgentCall(plugins, { id: 1 }, "fx_echo", { a: 1 })).content).caller;

  assert.equal(chat.origin, "agent");
  assert.equal(chat.routine, undefined);
});

test("RTN-07 an update that changes what a routine reads needs a new approval", async () => {
  b.writeManifest((m) => {
    m.version = "1.0.1";
    m.routines = ROUTINES.map((r) => (r.id === "bir-mail" ? { ...r, reads: [{ plugin: GWS, tools: ["gws_search_mail"] }] } : r));
  });
  const spec = fixtureSpec(b);
  const i = await plugins.inspect(spec);

  assert.equal(await codeOf(plugins.install({ ...spec, manifest_sha256: i.manifest_sha256 })), "resolved", "a narrower reads adds no permission");
  const r = listed("bir-mail");

  assert.equal(r.approved, false);
  assert.match(r.reason, /changed in an update/);
  assert.equal((await routines.tick(FX, "bir-mail")).ran, false);
  assert.equal(routines.approve({ plugin: FX, routine: "bir-mail" }).routine.available, true);
});

test("RTN-10 a run's turns go to the routine's own thread, made on its first run and reused after", () => {
  const h = home();

  assert.equal(store.list.filter((t) => t.routine?.id === "bir-mail").length, 1, "one thread for the routine");
  assert.equal(h.title, "BIR mail");
  assert.deepEqual(h.routine, { plugin: FX, id: "bir-mail", instance: inst("bir-mail"), name: "BIR mail", thread: "chat-1" });
  assert.deepEqual(turns.map((t) => t.thread), [h.id, h.id], "the second run reused it");
  const shown = h.turns.filter((t) => t.role === "user");

  assert.deepEqual(shown.map((t) => t.routine.name), ["BIR mail", "BIR mail"], "each item's turn carries the routine's mark");
  assert.notEqual(shown[0].routine.run, shown[1].routine.run, "one run id per run");
  assert.ok(chat().turns.every((t) => t.role === "notice"), "the chat holds no item turn");
});

test("RTN-11 each run with new items leaves one notice pointing at the run; nothing new leaves none", () => {
  const h = home();
  const [a, b] = notices();

  assert.equal(notices().length, 2, "two runs had new items; the empty and unchanged runs left nothing");
  assert.equal(a.text, "BIR mail ran · 1 new item");
  assert.deepEqual(a.notice, { kind: "routine", plugin: FX, routine: "bir-mail", instance: inst("bir-mail"), name: "BIR mail", thread: h.id, thread_uid: h.uid, run: h.turns[0].routine.run, seq: 0, turn: h.turns[0].id, items: 1 });
  assert.equal(b.notice.seq, 2, "the second run starts after the first's two turns");
  assert.equal(b.notice.turn, h.turns[2].id);
  assert.match(b.notice.run, /^run_\d+$/);
});

test("RTN-12 a failure leaves one notice per failure streak", async () => {
  writeMail(b, [{ id: "m3", subject: "Payment confirmation", body: "Paid." }, MAIL2, MAIL1]);
  failTurn = true;
  const n = notices().length;
  const r1 = await routines.tick(FX, "bir-mail");

  assert.match(r1.error, /model is unreachable/);
  assert.equal(notices().length, n + 1);
  const f = notices().at(-1);

  assert.equal(f.text, "BIR mail failed: the model is unreachable");
  assert.equal(f.notice.error, "the model is unreachable");
  assert.equal(f.notice.items, 0);
  assert.equal(f.notice.thread, home().id, "the thread from earlier runs");
  step(3600);
  await routines.tick(FX, "bir-mail");
  await routines.tick(FX, "bir-mail");
  assert.equal(notices().length, n + 1, "no notice for the same streak");
  failTurn = false;
  await routines.tick(FX, "bir-mail");
  assert.equal(notices().at(-1).text, "BIR mail ran · 1 new item", "it recovered");
  failTurn = true;
  writeMail(b, [{ id: "m4", subject: "Another", body: "x" }]);
  await routines.tick(FX, "bir-mail");
  assert.equal(notices().length, n + 3, "a new streak notices again");
  failTurn = false;
});

test("RTN-15 Run now ticks once out of schedule and counts like a run; paused, running or unavailable is refused", async () => {
  const before = listed("bir-mail");
  const n = turns.length;

  // just ran: not due, and a backoff would wait longer still
  db.prepare("UPDATE v1_routines SET errors=5 WHERE plugin=? AND routine=?").run(FX, "bir-mail");
  await routines.runDue();
  assert.equal(turns.length, n, "the schedule doesn't run it");
  step(1);
  writeMail(b, [{ id: "m5", subject: "Run now mail", body: "x" }]);
  const r = await routines.run({ plugin: FX, routine: "bir-mail", wait: true });

  assert.deepEqual(r.run, { ran: true, items: 1, prompts: 1 });
  assert.equal(turns.length, n + 1, "one turn for the one new item");
  assert.equal(r.routine.last_run, now, "it counts as the last run");
  assert.ok(r.routine.last_run > before.last_run);
  assert.equal(r.routine.last_items, 1);
  assert.equal(r.routine.running, false);
  assert.equal(notices().at(-1).text, "BIR mail ran · 1 new item", "the chat gets its notice");
  // nothing new: it ran, nothing posted
  assert.deepEqual((await routines.run({ plugin: FX, routine: "bir-mail", wait: true })).run, { ran: true, items: 1, prompts: 0 });
  // reread forgets what it saw
  assert.deepEqual((await routines.run({ plugin: FX, routine: "bir-mail", wait: true, reread: true })).run, { ran: true, items: 1, prompts: 1 });
  // without wait it answers at once, running; a second one is refused
  const started = routines.run({ plugin: FX, routine: "bir-mail" });
  const first = await started;

  assert.equal(first.routine.running, true);
  assert.equal(await codeOf(routines.run({ plugin: FX, routine: "bir-mail" })), "rate_limited");
  // (this runner's own view: listed() reads the last runner made)
  const running = () => routines.ofChats(["chat-1"]).find((m) => m.id === "bir-mail").running;

  for (let i = 0; i < 400 && running(); i++) await new Promise((r) => setTimeout(r, 25));
  assert.equal(running(), false, "it finished");
  // paused
  assert.equal(await codeOf(routines.run({ plugin: FX, routine: "empty" })), "disabled");
  // unavailable: refused, and the refusal is no run
  plugins.setTool(FX, "fx_query", true);
  const last = listed("bir-mail").last_run;

  step(5);
  assert.equal(await codeOf(routines.run({ plugin: FX, routine: "bir-mail" })), "plugin_unavailable");
  assert.equal(listed("bir-mail").last_run, last);
  plugins.setTool(FX, "fx_query", false);
  assert.equal(await codeOf(routines.run({ plugin: FX, routine: "nope" })), "not_found");
});

test("RTN-16 the agent lists and runs the plugin routines of its chats only, and cannot enable one", async () => {
  const mine = routines.ofChats(["chat-1"]);

  assert.deepEqual(mine.map((x) => [x.id, x.enabled]).sort(), [["bad-reads", false], ["bir-mail", true], ["empty", false]]);
  const x = mine.find((m) => m.id === "bir-mail");

  assert.equal(x.plugin, FX);
  assert.equal(x.name, "BIR mail");
  assert.equal(x.every_s, 300);
  assert.equal(x.home, home().id, "its thread");
  assert.deepEqual(routines.ofChats(["chat-2"]), [], "another chat's bot sees none");
  writeMail(b, [{ id: "m6", subject: "Agent run", body: "x" }]);
  const ran = await routines.agentRun(`${FX}/bir-mail`, ["chat-1"]);

  assert.equal(ran.is_error, undefined, ran.content);
  assert.match(ran.content, /^BIR mail ran: 1 item found, 1 new \(each ran in its thread t\d+\)$/);
  assert.equal((await routines.agentRun(`${FX}/bir-mail`, ["chat-2"])).is_error, true, "not its chat");
  const paused = await routines.agentRun(`${FX}/empty`, ["chat-1"]);

  assert.equal(paused.is_error, true);
  assert.match(paused.content, /Empty is paused; resume it first/);
  assert.equal(listed("empty").enabled, false, "running it did not enable it");
  // the agent's side has run and list, nothing that enables or approves
  for (const k of ["agentEnable", "agentApprove"]) assert.equal(routines[k], undefined);
});

test("RTN-08 bad requests are refused; a deleted chat stops the routine; uninstall forgets it", async () => {
  assert.equal(await codeOf(Promise.resolve().then(() => routines.approve({ plugin: FX, routine: "nope" }))), "not_found");
  assert.equal(await codeOf(Promise.resolve().then(() => routines.enable({ plugin: FX, routine: "bir-mail", thread: "no-such-chat" }))), "invalid_args");
  assert.equal(await codeOf(Promise.resolve().then(() => routines.approve({ plugin: FX }))), "invalid_args");
  chats.delete("chat-1");
  assert.equal((await routines.tick(FX, "bir-mail")).reason, "its chat was deleted");
  assert.equal(listed("bir-mail").enabled, false);
  await plugins.uninstall(FX);
  assert.equal(db.prepare("SELECT COUNT(*) AS n FROM v1_routines WHERE plugin=?").get(FX).n, 0);
});
