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
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { v1ToolPlugin, v1Tools } from "../server/v1/agent.mjs";
import { createRoutines } from "../server/v1/routines.mjs";
import { FX, GWS, bed, broker, codeOf, fixtureSpec, installFixture, installGws, upstreamCalls, writeMail } from "./v1-helpers.mjs";

let now = 1_800_000_000_000;
const clock = { now: () => now, setTimeout: (f, ms) => setTimeout(f, ms), clearTimeout: (t) => clearTimeout(t) };
const b = bed("routines");
const { plugins, db } = broker(b, { clock });
const turns = [];
const chats = new Set(["chat-1"]);
const routines = createRoutines({ db, plugins, clock, runTurn: async (t) => turns.push(t), threadExists: (id) => chats.has(id) });

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

  assert.equal(t.thread, "chat-1");
  assert.deepEqual(t.item, { id: "m1", from: MAIL1.from, subject: MAIL1.subject, date: MAIL1.date, snippet: MAIL1.body });
  assert.match(t.prompt, /^\(This routine runs on its own every 300s/);
  assert.match(t.prompt, /## Routine: BIR mail \(approved plugin dev\.goldcoders\.fixture\)\nRecord each BIR confirmation\./, "10.2 label");
  assert.match(t.prompt, /New item from dev\.goldcoders\.google-workspace gws_search_mail:\n\{\n "id": "m1"/);
  assert.deepEqual(t.surface, { plugin: FX, reads: READS });
  assert.equal(listed("bir-mail").seen_count, 1);
  assert.equal(listed("bir-mail").last_error, null);
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
  const again = createRoutines({ db, plugins, clock, runTurn: async (t) => turns.push(t), threadExists: (id) => chats.has(id) });

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
