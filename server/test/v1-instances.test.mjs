// Routine instances (server/v1/routines.mjs, spec 5.9 "Routine
// instances"): a plugin routine is a template; each instance has its own
// chat, params, trigger, thread and seen ids. Run against the fixture
// plugin and the fake workspace-mcp upstream on an injected clock, the
// agent turn recorded instead of run. INS-* are this suite's own IDs:
//   INS-01 a routine enabled before instances becomes one instance with
//          its chat, seen ids, last run and thread (once)
//   INS-02 two instances of one routine keep separate seen ids, threads
//          and notices
//   INS-03 params go to the query tool (beside query.args), the turn's
//          prompt and its surface; ones that don't fit the schema are
//          refused (the manifest's schema: MAN-14)
//   INS-04 the agent's instance of a routine that changes data, or reads
//          unapproved, waits behind a consent card until the user's
//          Enable; one without needs none; new params ask again
//   INS-05 a webhook instance runs on its secret URL only, with the items
//          it is handed, at most HOOK_RATE times a minute; a new secret
//          retires the old; the schedule leaves it alone
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createRoutineThreads } from "../server/v1/routine-threads.mjs";
import { HOOK_RATE, createRoutines } from "../server/v1/routines.mjs";
import { FX, GWS, bed, broker, codeOf, installFixture, installGws, upstreamCalls, writeMail } from "./v1-helpers.mjs";

let now = 1_800_000_000_000;
const clock = { now: () => now, setTimeout: (f, ms) => setTimeout(f, ms), clearTimeout: (t) => clearTimeout(t) };
const b = bed("instances");
const { plugins, db } = broker(b, { clock });
const turns = [];
const chats = new Set(["chat-1", "chat-2"]);
const store = {
  list: [
    { id: "chat-1", uid: "uid-chat-1", title: "Piccolo", turns: [] },
    { id: "chat-2", uid: "uid-chat-2", title: "Vegeta", turns: [] },
    // the thread of a routine from before instances
    { id: "t50", uid: "uid-t50", title: "BIR mail", turns: [{ id: "old", role: "user", text: "New item" }], routine: { plugin: FX, id: "bir-mail", name: "BIR mail", thread: "chat-1" } },
  ],
};
let ids = 0;
const threads = createRoutineThreads({ store, nextId: () => `t${100 + ++ids}`, uid: () => `uid-${++ids}`, save: () => {} });

async function runTurn(t) {
  turns.push(t);
  const th = store.list.find((x) => x.id === t.thread);
  const at = { seq: th.turns.length, turn: `turn-${++ids}` };

  th.turns.push({ id: at.turn, role: "user", text: t.shown, routine: t.mark }, { id: `turn-${++ids}`, role: "ai", text: "Recorded." });

  return at;
}

const READS = [{ plugin: GWS, tools: ["gws_search_mail", "gws_read_mail"] }];
const PARAMS = { type: "object", properties: { q: { type: "string", maxLength: 200 }, forms: { type: "array", items: { type: "string" }, maxItems: 4 } }, required: ["q"] };
const ROUTINES = [
  { id: "bir-mail", name: "BIR mail", description: "Files BIR confirmations.", every_s: 300, query: { tool: "fx_query", field: "query" }, prompt: "Record each BIR confirmation.", reads: READS, params: { ...PARAMS, required: [] } },
  { id: "payments", name: "Payments", description: "Records payments.", every_s: 300, query: { tool: "fx_query", field: "query" }, prompt: "Record each payment.", reads: READS, writes: ["fx_echo"], params: PARAMS },
  { id: "hooked", name: "Hooked", description: "Runs on a webhook.", every_s: 300, query: { tool: "fx_query", field: "query" }, prompt: "Handle it.", reads: READS },
];
const MAIL1 = { id: "m1", from: "BIR eFPS <efps@bir.gov.ph>", subject: "Tax Return Receipt Confirmation", date: "Mon, 26 Oct 2026 09:12:00 +0800", body: "1701Q received." };
const MAIL2 = { id: "m2", from: "BIR eFPS <efps@bir.gov.ph>", subject: "Payment Confirmation", date: "Tue, 27 Oct 2026 10:00:00 +0800", body: "2550Q paid." };

let routines;
const searches = () => upstreamCalls(b).filter((c) => c.tool === "search_gmail_messages");
const notices = (chat = "chat-1") => store.list.find((t) => t.id === chat).turns.filter((t) => t.role === "notice");
const inst = (id) => db.prepare("SELECT * FROM v1_instances WHERE id=?").get(id);
const homeOf = (id) => store.list.find((t) => t.routine?.instance === id);
const step = (s) => (now += s * 1000);

before(async () => {
  writeMail(b, [MAIL1]);
  b.writeManifest((m) => (m.routines = ROUTINES));
  await installGws(plugins, b);
  await installFixture(plugins, b);
  // a routine enabled and run before instances existed
  db.exec(`CREATE TABLE v1_routines(plugin TEXT NOT NULL, routine TEXT NOT NULL, thread TEXT, enabled INTEGER NOT NULL DEFAULT 0,
    approved_reads TEXT, seen TEXT NOT NULL DEFAULT '[]', last_run INTEGER, last_error TEXT, errors INTEGER NOT NULL DEFAULT 0, last_items INTEGER, PRIMARY KEY(plugin, routine))`);
  db.prepare("INSERT INTO v1_routines VALUES(?,?,?,?,?,?,?,?,?,?)").run(FX, "bir-mail", "chat-1", 1, JSON.stringify(READS), JSON.stringify(["m1"]), now - 1000, null, 0, 1);
  routines = createRoutines({ db, plugins, clock, runTurn, threads, threadExists: (id) => chats.has(id) });
});

after(() => plugins.stopAll());

test("INS-01 a routine enabled before instances becomes one instance with its chat, seen ids, last run and thread", () => {
  const rows = db.prepare("SELECT * FROM v1_instances WHERE routine='bir-mail'").all();

  assert.equal(rows.length, 1);
  const [x] = rows;

  assert.match(x.id, /^ri_/);
  assert.deepEqual([x.thread, x.enabled, x.seen, x.last_run, x.last_items, x.trigger, x.params], ["chat-1", 1, '["m1"]', now - 1000, 1, "schedule", "{}"]);
  assert.equal(store.list.find((t) => t.id === "t50").routine.instance, x.id, "its thread is the instance's");
  // once: a second runner on the same db moves nothing again
  createRoutines({ db, plugins, clock, runTurn, threads, threadExists: (id) => chats.has(id) });
  assert.equal(db.prepare("SELECT COUNT(*) AS n FROM v1_instances").get().n, 1);
  plugins.useRoutineInfo(routines.info);
  const listed = plugins.list().find((p) => p.id === FX).routines.find((r) => r.id === "bir-mail");

  assert.equal(listed.instances.length, 1);
  assert.equal(listed.instances[0].home, "t50");
  assert.equal(listed.home, "t50", "the first instance's state stays at the top for older clients");
  assert.equal(listed.seen_count, 1);
});

test("INS-03 params go to the query tool, the prompt and the surface; ones that don't fit are refused", async () => {
  const [old] = db.prepare("SELECT id FROM v1_instances WHERE routine='bir-mail'").all();
  const e = routines.enable({ plugin: FX, routine: "bir-mail", thread: "chat-2", params: { q: "from:taxpayer-a", forms: ["2550Q"] }, label: "Taxpayer A" });

  assert.notEqual(e.instance.instance, old.id, "a new instance in chat-2; chat-1's stays");
  assert.equal(e.instance.summary, "Taxpayer A");
  assert.deepEqual(e.instance.params, { q: "from:taxpayer-a", forms: ["2550Q"] });
  writeMail(b, [MAIL1]);
  const r = await routines.run({ plugin: FX, instance: e.instance.instance, routine: "bir-mail", wait: true });

  assert.deepEqual(r.run, { ran: true, items: 1, prompts: 1 });
  assert.equal(searches().at(-1).args.query, "from:taxpayer-a", "the query tool got params.q");
  const t = turns.at(-1);

  assert.match(t.prompt, /runs with these params; keep to them:\n\{"q":"from:taxpayer-a","forms":\["2550Q"\]\}/);
  assert.deepEqual(t.surface.params, { q: "from:taxpayer-a", forms: ["2550Q"] });
  assert.equal(t.surface.instance, e.instance.instance);
  assert.equal(t.thread, homeOf(e.instance.instance).id);
  assert.equal(homeOf(e.instance.instance).title, "BIR mail · Taxpayer A");
  // refused: unknown keys, wrong types, missing required
  for (const params of [{ nope: 1 }, { q: 5 }, { forms: "2550Q" }, { forms: ["a", "b", "c", "d", "e"] }])
    assert.equal(await codeOf(Promise.resolve().then(() => routines.update({ plugin: FX, routine: "bir-mail", instance: e.instance.instance, params }))), "invalid_args", JSON.stringify(params));
  assert.equal(await codeOf(Promise.resolve().then(() => routines.enable({ plugin: FX, routine: "payments", thread: "chat-1" }))), "invalid_args", "payments needs q");
});

test("INS-02 two instances of one routine keep separate seen ids, threads and notices", async () => {
  const [a, c] = db.prepare("SELECT id, thread FROM v1_instances WHERE routine='bir-mail' ORDER BY created_at, rowid").all();

  assert.deepEqual([a.thread, c.thread], ["chat-1", "chat-2"]);
  writeMail(b, [MAIL2, MAIL1]);
  const n = turns.length;

  step(600);
  await routines.runDue();
  // chat-1's had seen m1 (migrated), chat-2's had seen m1 (INS-03): each runs m2 once
  assert.equal(turns.length, n + 2);
  assert.deepEqual(turns.slice(-2).map((t) => [t.instance, t.item.id]), [[a.id, "m2"], [c.id, "m2"]]);
  assert.notEqual(homeOf(a.id).id, homeOf(c.id).id, "a thread each");
  assert.deepEqual(JSON.parse(inst(a.id).seen), ["m1", "m2"]);
  assert.deepEqual(JSON.parse(inst(c.id).seen), ["m1", "m2"]);
  assert.equal(notices("chat-1").at(-1).notice.instance, a.id);
  assert.equal(notices("chat-2").at(-1).notice.instance, c.id);
  // pausing one leaves the other running
  routines.disable({ plugin: FX, routine: "bir-mail", instance: a.id });
  writeMail(b, [{ id: "m3", subject: "Third" }]);
  step(600);
  await routines.runDue();
  assert.deepEqual(turns.slice(-1).map((t) => t.instance), [c.id]);
  assert.equal(JSON.parse(inst(a.id).seen).length, 2, "the paused one saw nothing");
  // an ambiguous {plugin, routine} is refused; the instance names it
  assert.equal(await codeOf(routines.run({ plugin: FX, routine: "bir-mail" })), "invalid_args");
});

test("INS-04 the agent's instance of a routine that changes data waits behind a consent card", async () => {
  const before = notices().length;
  const made = routines.agentCreate({ plugin: FX, routine: "payments", params: { q: "from:payments" }, label: "Taxpayer B" }, ["chat-1"]);

  assert.equal(made.is_error, undefined, made.content);
  assert.match(made.content, /waits for the user: a card in this chat asks them to Enable it \(it may change Fixture data: fx_echo\)/);
  const id = made.content.match(/ri_[\w-]+/)[0];

  assert.deepEqual([inst(id).enabled, inst(id).pending], [0, 1]);
  const card = notices().at(-1);

  assert.equal(notices().length, before + 1);
  assert.equal(card.notice.kind, "routine_consent");
  assert.deepEqual([card.notice.instance, card.notice.state, card.notice.writes, card.notice.summary], [id, "pending", ["fx_echo"], "Taxpayer B"]);
  assert.match(card.text, /^Enable Payments \(Taxpayer B\) here\? It may change Fixture data without asking: fx_echo\. It reads Google Workspace: gws_search_mail, gws_read_mail\.$/);
  // it doesn't run, on the schedule or asked
  const n = turns.length;

  step(600);
  await routines.runDue();
  assert.equal(turns.filter((t) => t.instance === id).length, 0);
  assert.equal(turns.length - n, 0);
  assert.match((await routines.agentRun(id, ["chat-1"])).content, /waits for the user's Enable/);
  // the agent can't switch it on
  const tried = routines.agentUpdate(id, { enabled: true }, ["chat-1"]);

  assert.match(tried.content, /waits for the user/);
  assert.equal(inst(id).enabled, 0);
  assert.equal(notices().length, before + 2, "it asked again");
  // the user's Enable on the card
  const on = routines.enable({ plugin: FX, routine: "payments", instance: id, approve: true });

  assert.equal(on.instance.enabled, true);
  assert.equal(on.instance.pending, false);
  assert.ok(notices().filter((t) => t.notice.instance === id).every((t) => t.notice.state === "enabled"), "its cards read as enabled");
  // new params ask again; a paused stays paused
  const changed = routines.agentUpdate(id, { params: { q: "from:other" } }, ["chat-1"]);

  assert.match(changed.content, /waits for the user/);
  assert.deepEqual([inst(id).enabled, inst(id).pending], [0, 1]);
  assert.equal(JSON.parse(inst(id).params).q, "from:other");
  // a label alone needs no consent; delete declines the card
  routines.enable({ plugin: FX, routine: "payments", instance: id });
  assert.match(routines.agentUpdate(id, { label: "Taxpayer B2" }, ["chat-1"]).content, /· on$/);
  // a routine without writes, its reads approved: on at once
  const plain = routines.agentCreate({ plugin: FX, routine: "bir-mail", params: { q: "from:c" } }, ["chat-1"]);

  assert.match(plain.content, /^made and enabled ri_[\w-]+ — BIR mail · q: from:c · every 300s$/);
  // not its chat: nothing
  assert.equal(routines.agentDelete(id, ["chat-2"]).is_error, true);
  assert.match(routines.agentDelete(id, ["chat-1"]).content, /^deleted/);
  assert.equal(inst(id), undefined);
});

test("INS-05 a webhook instance runs on its secret URL only, with the items it is handed, rate limited", async () => {
  const e = routines.enable({ plugin: FX, routine: "hooked", thread: "chat-1", trigger: "webhook", approve: true });
  const id = e.instance.instance;

  assert.equal(e.instance.trigger, "webhook");
  assert.equal(e.instance.hook, false);
  assert.equal(await codeOf(Promise.resolve().then(() => routines.hookCall(id, "guess", {}))), "not_found", "no secret yet");
  const { path } = routines.hook({ plugin: FX, routine: "hooked", instance: id });
  const [, , , hid, secret] = path.split("/");

  assert.equal(hid, id);
  assert.match(secret, /^[\w-]{32}$/);
  assert.equal(inst(id).hook_hash.length, 64, "only its hash is kept");
  assert.ok(!inst(id).hook_hash.includes(secret));
  assert.equal(await codeOf(Promise.resolve().then(() => routines.hookCall(id, `${secret}x`, {}))), "not_found", "a wrong secret");
  assert.equal(await codeOf(Promise.resolve().then(() => routines.hookCall("ri_nosuchinstance", secret, {}))), "not_found");
  // the items it is handed run, without the query
  const queries = searches().length;
  const res = await routines.hookCall(id, secret, { items: [{ id: "h1", subject: "Hooked one" }, { id: "h1" }] });

  assert.deepEqual(res, { ok: true, instance: id, items: 2 });
  for (let i = 0; i < 200 && routines.ofChats(["chat-1"]).find((x) => x.instance === id).running; i++) await new Promise((r) => setTimeout(r, 10));
  assert.equal(searches().length, queries, "no search");
  assert.deepEqual(turns.filter((t) => t.instance === id).map((t) => t.item.id), ["h1"]);
  assert.match(turns.at(-1).prompt, /on its webhook/);
  assert.equal(await codeOf(Promise.resolve().then(() => routines.hookCall(id, secret, { items: [{ nope: 1 }] }))), "invalid_args");
  // the schedule leaves it alone
  step(3600);
  const n = turns.length;

  await routines.runDue();
  assert.equal(turns.filter((t) => t.instance === id).length, 1);
  assert.ok(turns.length >= n);
  // at most HOOK_RATE calls a minute (the earlier ones were an hour ago)
  let limited = null;

  for (let i = 0; i < HOOK_RATE + 1 && !limited; i++) {
    for (let k = 0; k < 200 && routines.ofChats(["chat-1"]).find((x) => x.instance === id).running; k++) await new Promise((r) => setTimeout(r, 10));
    const code = await codeOf(Promise.resolve().then(() => routines.hookCall(id, secret, { items: [] })));

    if (code === "rate_limited") limited = i;
  }

  assert.equal(limited, HOOK_RATE, "the call after HOOK_RATE in a minute");
  step(61);
  // a new secret retires the old
  const { path: next } = routines.hook({ plugin: FX, routine: "hooked", instance: id });

  assert.equal(await codeOf(Promise.resolve().then(() => routines.hookCall(id, secret, {}))), "not_found");
  assert.equal(await codeOf(Promise.resolve().then(() => routines.hookCall(id, next.split("/")[4], {}))), "resolved");
  // a schedule instance has no webhook
  const [sched] = db.prepare("SELECT id FROM v1_instances WHERE routine='bir-mail' ORDER BY created_at").all();

  assert.equal(await codeOf(Promise.resolve().then(() => routines.hook({ plugin: FX, routine: "bir-mail", instance: sched.id }))), "invalid_args");
});
