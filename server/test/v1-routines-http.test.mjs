// v1 plugin routines and the Rox-facing host surface over HTTP (the Rust
// broker plan, phase 5 items 5.5 and 5.5h): the fixture plugin installed
// through /v1/host/plugins/*, its routines enabled, approved, run (Run now
// with MOCK turns), changed, hooked, disabled and deleted; the plugin list
// with each routine's state merged; the events stream; the plugin actions
// and the session routes' refusals; the host-token file. v1-host.test.mjs
// covers the happy path of inspect/install/open/call/resolve/title/source
// and RTN-09; this file extends it. The consent card needs the agent's
// routine_create: one case runs a broker on the scripted fake opencodex.
//
//   RTH-01 every /v1/host route's refusals: 401, 405, 404, a body that is no
//          JSON object or over 2 MiB; an action on a plugin not installed
//   RTH-02 install; GET /v1/host/plugins byte for byte, the routines' state
//          merged (fresh: no instances, approved only without reads)
//   RTH-03 GET /v1/host/events: the comment line first, then plugin events
//          in order (stopped, starting, running, a tool switch, ...) to every
//          listener
//   RTH-04 disable/enable/tools answers; the list while stopped (tools from
//          the record); a switched-off query tool makes routines unavailable
//   RTH-05 enable a routine in a chat: an instance and its own thread;
//          refusals; Run now refused until approved; approve
//   RTH-06 Run now with MOCK turns: wait gives the run's result; the turn
//          lands in the instance's thread, the notice in the chat; seen ids;
//          reread; without wait the answer comes at once
//   RTH-07 POST /v1/host/routines/update: params, label, every_s, trigger;
//          the thread is renamed; refusals
//   RTH-08 a failing routine: one notice per failure streak, the backoff in
//          next_run, re-enabling clears the count
//   RTH-09 the ri_ webhook: a secret per hook call, items as the new items
//          (seen ids), 100 items at most, 10 calls a minute, a new secret
//          ends the old one; wrong secrets and methods are not_found
//   RTH-10 disable one instance or all; Run now on a paused one is refused
//   RTH-11 delete an instance; its thread stays
//   RTH-12 the 5.5h session routes' refusals: open, close, call, resolve,
//          tickets/title, apps/source
//   RTH-13 a deleted chat disables its instance at the next poll (15 s)
//   RTH-14 a restart writes a new host-token (0600); the old one is refused;
//          enabled instances' threads are there at start
//   RTH-15 the consent card: the agent's routine_create (fake opencodex)
//          leaves a pending instance behind a card; the host's enable with
//          approve switches it on and the card reads enabled; deleting a
//          pending one reads declined
//   RTH-16 uninstall: the routines and instances go with it; the event
import assert from "node:assert/strict";
import { readFileSync, statSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { generate, random128 } from "../src/v1/ed.mjs";
import { permsDigest } from "../src/v1/manifest.mjs";
import { say, startFakeOpencodex, use } from "./fixtures/fake-opencodex.mjs";
import { PORT, dbRows, modelEnv, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";
import { FX, bed, envelope, fixtureSpec } from "./v1-helpers.mjs";

const MOCK_REPLY = "_(mock reply — no model configured)_";
const b = bed("routines-http");
const dataDir = tempDir("v1-routines-http");
const stubLog = join(tempDir("v1-routines-http-stubs"), "stubs.log");

const READS = [{ plugin: FX, tools: ["fx_search"] }];
const WATCH = {
  id: "fx-watch", name: "Fixture watch", description: "Watches fixture items.", every_s: 300,
  query: { tool: "fx_query", field: "query" }, prompt: "Note each new item.", reads: READS,
  params: { type: "object", properties: { q: { type: "string", maxLength: 40 } } },
};
const WRITER = { id: "fx-writer", name: "Fixture writer", description: "Echoes what its webhook hands it.", every_s: 600, query: { tool: "fx_query", field: "query" }, prompt: "Echo each item.", writes: ["fx_echo"] };
let manifest;

const brokerEnv = () => ({ ...offlineEnv(stubLog), ROX_KEYSTORE: `file:${b.keysDir}`, ROX_TRUST_FILE: b.trustFile });
let server;
let token = "";
let installed;

const host = (method, path, obj, extra = {}) =>
  request(method, path, {
    headers: { "X-Rox-Host": token, ...(obj === undefined ? {} : { "Content-Type": "application/json" }), ...extra },
    body: obj === undefined ? undefined : typeof obj === "string" ? obj : JSON.stringify(obj),
  });
const refusal = (code, message, retryable = false) => ({ ok: false, code, message, retryable });
const plugin = async () => (await host("GET", "/v1/host/plugins")).json.plugins.find((p) => p.id === FX);
const routineOf = async (id) => (await plugin()).routines.find((r) => r.id === id);
const turns = async (id) => (await request("GET", `/turns?thread=${id}`)).json.turns;
const newChat = async () => (await postJson("/thread/new", {})).json.id;
const R = (path, body) => host("POST", `/v1/host/routines/${path}`, { plugin: FX, ...body });

/** /v1/host/events as a listener: {raw, events, close}. */
function listen() {
  const out = { raw: "", events: [], status: 0, headers: {} };
  const req = httpRequest({ host: "127.0.0.1", port: PORT, path: "/v1/host/events", headers: { "X-Rox-Host": token } }, (res) => {
    out.status = res.statusCode;
    out.headers = res.headers;
    res.setEncoding("utf8");
    res.on("data", (d) => {
      out.raw += d;
      out.events = [...out.raw.matchAll(/event: plugin\ndata: (.+)\n\n/g)].map((m) => JSON.parse(m[1]));
    });
  });

  req.on("error", () => {});
  req.end();
  out.close = () => req.destroy();

  return out;
}

const FIXTURE_TOOLS = [
  ["fx_echo", "Echo the arguments and the caller.", false],
  ["fx_slow", "Sleep for ms milliseconds, then answer.", false],
  ["fx_fail", "Fail on purpose.", false],
  ["fx_big", 'Answer with bytes characters of text (as: "resource" embeds them as an HTML document).', false],
  ["fx_resolve", "Resolve the fixture's command and scopes.", false],
  ["fx_open", "Answer with a card for the fixture card app.", false],
  ["fx_search", "Search fixture items (read only).", true],
  ["fx_query", "A routine's search text: {query} from args.q.", false],
  ["fx_secret", "A tool no app may call.", false],
  ["fx_app_only", "A tool for apps only (10.1 audience).", false],
  ["fx_stats", "What the fixture was asked.", false],
  ["fx_reload", "Tell the host the resources changed.", false],
];

// a routine's user state before anything was done with it (routines.mjs info)
const fresh = (r) => ({
  enabled: false, thread: null, home: null, home_uid: null,
  approved: !(r.reads || []).length,
  available: !(r.reads || []).length,
  reason: (r.reads || []).length ? "you have not approved what it reads" : null,
  seen_count: 0, last_run: null, last_error: null, last_items: null, next_run: null, running: false, instances: [],
});

before(async () => {
  manifest = b.writeManifest((m) => m.routines.push(WATCH, WRITER));
  server = await startServer(dataDir, { env: brokerEnv() });
  token = readFileSync(join(dataDir, "host-token"), "utf8");
});

after(async () => {
  await server?.stop();
});

test("RTH-01 every /v1/host route's refusals: 401, 405, 404, a body that is no JSON object or over 2 MiB", async () => {
  const no = await request("GET", "/v1/host/plugins");

  assert.equal(no.status, 401);
  assert.equal(no.text, JSON.stringify(refusal("unauthorized", "X-Rox-Host is missing or wrong")));
  assert.match(no.headers["content-type"], /^application\/json/);
  const get = await host("GET", "/v1/host/call");

  assert.equal(get.status, 405);
  assert.equal(get.text, JSON.stringify(refusal("not_supported", "GET /v1/host/call")));
  const put = await host("PUT", "/v1/host/plugins");

  assert.deepEqual([put.status, put.json], [405, refusal("not_supported", "PUT /v1/host/plugins")]);
  const nope = await host("POST", "/v1/host/nope", {});

  assert.deepEqual([nope.status, nope.json], [404, refusal("not_found", "no route /v1/host/nope")]);
  // the query string is not part of the route
  assert.equal((await host("GET", "/v1/host/plugins?x=1")).status, 200);
  const body = refusal("invalid_args", "the body must be a JSON object (at most 2 MiB)");

  for (const raw of ["[]", "1", "null", "{", '"x"'])
    assert.deepEqual(await host("POST", "/v1/host/routines/approve", raw).then((r) => [r.status, r.json]), [400, body], raw);
  const big = await host("POST", "/v1/host/routines/approve", JSON.stringify({ pad: "x".repeat(2 * 1024 * 1024) }));

  assert.deepEqual([big.status, big.json], [400, body]);
  // an empty body is {}
  const empty = await request("POST", "/v1/host/routines/approve", { headers: { "X-Rox-Host": token, "Content-Type": "application/json" } });

  assert.deepEqual([empty.status, empty.json], [400, refusal("invalid_args", "send {plugin, routine}")]);

  for (const action of ["enable", "disable", "uninstall", "tools"]) {
    const r = await host("POST", `/v1/host/plugins/${FX}/${action}`, {});

    assert.deepEqual([r.status, r.json], [404, refusal("not_found", `${FX} is not installed`)], action);
  }
  // an action the table has not
  assert.equal((await host("POST", `/v1/host/plugins/${FX}/restart`, {})).status, 404);
  const install = await host("POST", "/v1/host/plugins/install", { ...fixtureSpec(b), manifest_sha256: "nope" });

  assert.deepEqual([install.status, install.json], [400, refusal("invalid_args", "manifest_sha256 (the reviewed manifest's hash) is required")]);
  const routine = await R("approve", { routine: "fx-digest" });

  assert.deepEqual([routine.status, routine.json], [404, refusal("not_found", `${FX} has no routine fx-digest`)], "no plugin, no routine");
});

test("RTH-02 install, then GET /v1/host/plugins byte for byte with each routine's state merged", async () => {
  const spec = fixtureSpec(b);
  const i = await host("POST", "/v1/host/plugins/inspect", spec);

  assert.equal(i.status, 200, i.text);
  const inst = await host("POST", "/v1/host/plugins/install", { ...spec, manifest_sha256: i.json.manifest_sha256 });

  assert.equal(inst.status, 200, inst.text);
  installed = inst.json;
  assert.deepEqual(Object.keys(installed), ["ok", "id", "state", "generation", "plugin_pub", "updated"]);
  assert.deepEqual([installed.ok, installed.id, installed.state, installed.generation, installed.updated], [true, FX, "running", 1, false]);
  const again = await host("POST", "/v1/host/plugins/install", { ...spec, manifest_sha256: i.json.manifest_sha256 });

  assert.equal(again.text, JSON.stringify({ ok: true, id: FX, unchanged: true }));
  const list = await host("GET", "/v1/host/plugins");

  assert.equal(list.status, 200);
  assert.match(list.headers["content-type"], /^application\/json/);
  const expected = {
    ok: true,
    plugins: [
      {
        id: FX,
        short: "fx",
        title: "Fixture",
        version: "1.0.0",
        protocol: "1.0.0",
        publisher: manifest.publisher,
        state: "running",
        code: null,
        message: "",
        enabled: true,
        generation: 1,
        plugin_pub: installed.plugin_pub,
        first_party: true,
        manifest_sha256: i.json.manifest_sha256,
        apps: manifest.apps.map((a) => ({ ...a, perms_sha256: permsDigest(a.permissions) })),
        commands: manifest.commands,
        scopes: manifest.scopes,
        routines: manifest.routines.map((r) => ({
          ...r,
          reads_status: (r.reads || []).map((x) => ({ plugin: x.plugin, tools: x.tools, allowed: true, reason: null })),
          ...fresh(r),
        })),
        tools: FIXTURE_TOOLS.map(([name, description, readOnly]) => ({ name, description, disabled: false, readOnly })),
      },
    ],
  };

  assert.equal(list.text, JSON.stringify(expected));
});

test("RTH-03 GET /v1/host/events: the comment first, then each plugin event in order, to every listener", async () => {
  const a = listen();
  const c = listen();

  await waitFor(() => a.raw && c.raw, 5000);
  assert.equal(a.status, 200);
  assert.equal(a.headers["content-type"], "text/event-stream");
  assert.equal(a.headers["cache-control"], "no-cache");
  assert.equal(a.raw, ": rox broker events\n\n");
  assert.deepEqual((await host("POST", `/v1/host/plugins/${FX}/disable`, {})).json, { ok: true, state: "stopped" });
  assert.deepEqual((await host("POST", `/v1/host/plugins/${FX}/enable`, {})).json, { ok: true, state: "running", code: null, message: "", generation: 2 });
  assert.deepEqual((await host("POST", `/v1/host/plugins/${FX}/tools`, { tool: "fx_secret", disabled: true })).json, { ok: true, tool: "fx_secret", disabled: true });
  assert.ok(await waitFor(() => c.events.length === 4, 5000), c.raw);
  c.close();
  await new Promise((r) => setTimeout(r, 100));
  assert.deepEqual((await host("POST", `/v1/host/plugins/${FX}/tools`, { tool: "fx_secret", disabled: false })).json, { ok: true, tool: "fx_secret", disabled: false });
  const want = [
    { id: FX, state: "stopped", code: null, generation: 1, enabled: false },
    { id: FX, state: "starting", code: null, generation: 1, enabled: true },
    { id: FX, state: "running", code: null, generation: 2, enabled: true },
    { id: FX, state: "running", code: null, generation: 2, enabled: true },
    { id: FX, state: "running", code: null, generation: 2, enabled: true },
  ];

  assert.ok(await waitFor(() => a.events.length >= want.length, 5000), a.raw);
  assert.equal(a.raw.split("\n\n").filter(Boolean).length, 1 + want.length, "nothing else on the stream");
  assert.deepEqual(a.events, want);
  assert.equal(a.raw.slice(0, 80), `: rox broker events\n\nevent: plugin\ndata: ${JSON.stringify(want[0])}`.slice(0, 80));
  assert.deepEqual(c.events, want.slice(0, 4), "a closed listener hears no more");
  a.close();
});

test("RTH-04 the plugin actions: answers, the list while stopped, a switched-off query tool", async () => {
  const off = await host("POST", `/v1/host/plugins/${FX}/disable`, {});

  assert.equal(off.text, '{"ok":true,"state":"stopped"}');
  const p = await plugin();

  assert.deepEqual([p.state, p.enabled, p.generation, p.code, p.message], ["stopped", false, 2, null, ""]);
  // stopped: the tools come from the record, the same shape
  assert.deepEqual(p.tools, FIXTURE_TOOLS.map(([name, description, readOnly]) => ({ name, description, disabled: false, readOnly })));
  const w = p.routines.find((r) => r.id === "fx-writer");

  assert.deepEqual([w.available, w.reason], [false, `${FX} is disabled`]);
  assert.deepEqual(p.routines.find((r) => r.id === "fx-watch").reads_status, [{ plugin: FX, tools: ["fx_search"], allowed: false, reason: `${FX} is disabled` }]);
  assert.deepEqual((await host("POST", `/v1/host/plugins/${FX}/disable`, {})).json, { ok: true, state: "stopped" }, "twice is fine");
  const on = await host("POST", `/v1/host/plugins/${FX}/enable`, {});

  assert.equal(on.text, '{"ok":true,"state":"running","code":null,"message":"","generation":3}');
  assert.equal((await host("POST", `/v1/host/plugins/${FX}/enable`, {})).text, '{"ok":true,"state":"running","generation":3}', "already running");
  const bad = await host("POST", `/v1/host/plugins/${FX}/tools`, { tool: "fx_echo", disabled: "yes" });

  assert.deepEqual([bad.status, bad.json], [400, refusal("invalid_args", "disabled must be a boolean")]);
  await host("POST", `/v1/host/plugins/${FX}/tools`, { tool: "fx_query", disabled: true });
  const q = await routineOf("fx-writer");

  assert.deepEqual([q.available, q.reason], [false, "fx_query is switched off"]);
  await host("POST", `/v1/host/plugins/${FX}/tools`, { tool: "fx_query", disabled: false });
  assert.equal((await routineOf("fx-writer")).available, true);
  // a plugin id with an escaped character is decoded
  assert.equal((await host("POST", `/v1/host/plugins/${encodeURIComponent(FX).replace(/\./g, "%2E")}/enable`, {})).json.ok, true);
});

let chat = "";
let watch = null; // the fx-watch instance

test("RTH-05 enable a routine in a chat: its instance and own thread; refusals; Run now refused until approved", async () => {
  chat = await newChat();
  const refusedEnable = async (body, code, message, status = 400) => {
    const r = await R("enable", { routine: "fx-watch", ...body });

    assert.deepEqual([r.status, r.json], [status, refusal(code, message)], JSON.stringify(body));
  };

  await refusedEnable({}, "invalid_args", "thread must name an existing chat");
  await refusedEnable({ thread: "t999" }, "invalid_args", "thread must name an existing chat");
  await refusedEnable({ thread: chat, params: { zzz: 1 } }, "invalid_args", "params.zzz is not one of q");
  await refusedEnable({ thread: chat, params: { q: 5 } }, "invalid_args", "params.q must be a string");
  await refusedEnable({ thread: chat, params: [] }, "invalid_args", "params must be an object");
  await refusedEnable({ thread: chat, every_s: 30 }, "invalid_args", "every_s is an integer from 60 to 86400");
  await refusedEnable({ thread: chat, every_s: 86401 }, "invalid_args", "every_s is an integer from 60 to 86400");
  await refusedEnable({ thread: chat, trigger: "cron" }, "invalid_args", 'trigger is "schedule" or "webhook"');
  await refusedEnable({ thread: chat, label: 5 }, "invalid_args", "label must be a string");
  await refusedEnable({ thread: chat, instance: "ri_nope000" }, "not_found", "no instance ri_nope000 of fx-watch", 404);
  assert.deepEqual(dbRows(dataDir, "SELECT * FROM v1_instances"), [], "nothing was made");
  const r = await R("enable", { routine: "fx-watch", thread: chat });

  assert.equal(r.status, 200, r.text);
  assert.deepEqual(Object.keys(r.json), ["ok", "routine", "instance"]);
  watch = r.json.instance;
  assert.match(watch.instance, /^ri_[A-Za-z0-9_-]{12}$/);
  const threads = (await request("GET", "/threads")).json.list;
  const home = threads.find((t) => t.routine?.instance === watch.instance);

  assert.ok(home, "its own thread exists before any run");
  assert.deepEqual(home.routine, { plugin: FX, id: "fx-watch", instance: watch.instance, name: "Fixture watch", thread: chat });
  assert.equal(home.title, "Fixture watch");
  assert.equal(
    JSON.stringify(watch),
    JSON.stringify({
      instance: watch.instance, label: null, params: {}, summary: "", trigger: "schedule", every_s: 300, enabled: true, pending: false,
      thread: chat, home: home.id, home_uid: home.uid, available: false, reason: "you have not approved what it reads",
      seen_count: 0, last_run: null, last_error: null, last_items: null, next_run: 300_000, running: false, hook: false,
    }),
  );
  const top = r.json.routine;

  assert.deepEqual(Object.keys(top), ["id", "enabled", "thread", "home", "home_uid", "approved", "available", "reason", "seen_count", "last_run", "last_error", "last_items", "next_run", "running", "instances"]);
  assert.deepEqual([top.id, top.enabled, top.thread, top.home, top.approved], ["fx-watch", true, chat, home.id, false]);
  const run = await R("run", { routine: "fx-watch", wait: true });

  assert.deepEqual([run.status, run.json], [503, refusal("plugin_unavailable", "Fixture watch can't run: you have not approved what it reads", true)]);
  const ap = await R("approve", { routine: "fx-watch" });

  assert.equal(ap.status, 200);
  assert.deepEqual(Object.keys(ap.json), ["ok", "routine"]);
  assert.deepEqual([ap.json.routine.approved, ap.json.routine.available, ap.json.routine.reason], [true, true, null]);
  assert.deepEqual(dbRows(dataDir, "SELECT plugin, routine, approved_reads FROM v1_routines WHERE routine='fx-watch'"), [{ plugin: FX, routine: "fx-watch", approved_reads: JSON.stringify(READS) }]);
  // enabling again in the same chat is the same instance
  const same = await R("enable", { routine: "fx-watch", thread: chat });

  assert.equal(same.json.instance.instance, watch.instance);
  assert.equal(dbRows(dataDir, "SELECT COUNT(*) AS n FROM v1_instances").at(0).n, 1);
});

test("RTH-06 Run now with MOCK turns: the turn in the instance's thread, the notice in the chat, seen ids, reread", async () => {
  const run = await R("run", { routine: "fx-watch", wait: true });

  assert.equal(run.status, 200, run.text);
  assert.deepEqual(Object.keys(run.json), ["ok", "routine", "instance", "run"]);
  assert.deepEqual(run.json.run, { ran: true, items: 1, prompts: 1 });
  // the answer's instance is read after the run
  const i = run.json.instance;

  assert.deepEqual([i.seen_count, i.last_items, i.last_error, i.running], [1, 1, null, false]);
  assert.equal(i.next_run, i.last_run + 300_000);
  const home = await turns(watch.home);

  assert.deepEqual(home.map((u) => [u.role, u.text]), [["user", "New item"], ["ai", MOCK_REPLY]]);
  assert.deepEqual(home[0].routine, { plugin: FX, id: "fx-watch", instance: watch.instance, name: "Fixture watch", run: `run_${i.last_run}` });
  const notices = (await turns(chat)).filter((u) => u.role === "notice");

  assert.equal(notices.length, 1);
  assert.equal(notices[0].text, "Fixture watch ran · 1 new item");
  assert.deepEqual(notices[0].notice, {
    kind: "routine", plugin: FX, routine: "fx-watch", instance: watch.instance, name: "Fixture watch", thread: watch.home, thread_uid: watch.home_uid,
    run: `run_${i.last_run}`, seq: 0, turn: home[0].id, items: 1,
  });
  assert.deepEqual(JSON.parse(dbRows(dataDir, "SELECT seen FROM v1_instances WHERE id=?", watch.instance)[0].seen), ["from:fixture"]);
  // the same item again: nothing new, no turn, no notice
  const quiet = await R("run", { routine: "fx-watch", wait: true });

  assert.deepEqual(quiet.json.run, { ran: true, items: 1, prompts: 0 });
  assert.equal(quiet.json.instance.last_items, 0);
  assert.equal((await turns(watch.home)).length, 2);
  assert.equal((await turns(chat)).filter((u) => u.role === "notice").length, 1);
  // reread forgets the seen ids
  const again = await R("run", { routine: "fx-watch", wait: true, reread: true });

  assert.deepEqual(again.json.run, { ran: true, items: 1, prompts: 1 });
  assert.equal((await turns(watch.home)).length, 4);
  assert.equal((await turns(chat)).filter((u) => u.role === "notice").at(-1).notice.seq, 2);
  // without wait: the answer at once, no run in it
  const now = await R("run", { routine: "fx-watch" });

  assert.equal(now.status, 200);
  assert.deepEqual(Object.keys(now.json), ["ok", "routine", "instance"]);
  await waitFor(async () => !(await routineOf("fx-watch")).running, 5000);
  // the plugin list shows the instance
  const listed = await routineOf("fx-watch");

  assert.deepEqual(listed.instances.map((x) => x.instance), [watch.instance]);
  assert.deepEqual([listed.enabled, listed.thread, listed.home, listed.seen_count], [true, chat, watch.home, 1]);
});

test("RTH-07 POST /v1/host/routines/update: params, label, every_s, trigger; the thread is renamed; refusals", async () => {
  const up = async (body) => {
    const r = await R("update", { routine: "fx-watch", instance: watch.instance, ...body });

    assert.equal(r.status, 200, r.text);

    return r.json.instance;
  };

  // the answer's instance is read after the change
  const answered = await up({ params: { q: "acme" } });
  let i = (await routineOf("fx-watch")).instances[0];

  assert.deepEqual(answered, i);

  assert.deepEqual([i.params, i.summary, i.enabled], [{ q: "acme" }, "q: acme", true]);
  const home = (await request("GET", "/threads")).json.list.find((t) => t.id === watch.home);

  assert.equal(home.title, "Fixture watch · q: acme");
  assert.equal(home.routine.name, "Fixture watch · q: acme");
  const ran = await R("run", { routine: "fx-watch", wait: true });

  assert.deepEqual(ran.json.run, { ran: true, items: 1, prompts: 1 });
  assert.equal((await turns(watch.home)).at(-2).routine.name, "Fixture watch · q: acme");
  await up({ label: "  ## Acme Corp  " });
  i = (await routineOf("fx-watch")).instances[0];
  assert.deepEqual([i.label, i.summary], ["Acme Corp", "Acme Corp"]);
  await up({ every_s: 120 });
  i = (await routineOf("fx-watch")).instances[0];
  assert.deepEqual([i.every_s, i.next_run], [120, i.last_run + 120_000]);
  const refused = async (body, code, message, status = 400) => {
    const r = await R("update", { routine: "fx-watch", ...body });

    assert.deepEqual([r.status, r.json], [status, refusal(code, message)], JSON.stringify(body));
  };

  await refused({ instance: watch.instance, every_s: 59 }, "invalid_args", "every_s is an integer from 60 to 86400");
  await refused({ instance: watch.instance, params: { q: "x".repeat(41) } }, "invalid_args", "params.q is longer than 40");
  await refused({ instance: "ri_nope000" }, "not_found", "no instance ri_nope000 of fx-watch", 404);
  await refused({ instance: 5 }, "not_found", "no instance 5 of fx-watch", 404);
  const none = await R("update", { routine: "fx-digest" });

  assert.deepEqual([none.status, none.json], [404, refusal("not_found", "fx-digest has no instance")]);
  // the routine's one instance is found without naming it
  assert.equal((await R("update", { routine: "fx-watch", every_s: 300 })).json.instance.instance, watch.instance);
});

test("RTH-08 a failing routine: one notice per failure streak, the backoff in next_run, re-enabling clears the count", async () => {
  // fx-digest as the fixture ships it: fx_search answers {items}, no query
  assert.equal((await R("approve", { routine: "fx-digest" })).status, 200);
  const on = await R("enable", { routine: "fx-digest", thread: chat });

  assert.equal(on.status, 200, on.text);
  const id = on.json.instance.instance;
  const before = (await turns(chat)).length;
  const runs = [];

  for (let k = 0; k < 4; k++) runs.push((await R("run", { routine: "fx-digest", wait: true })).json);
  assert.deepEqual(runs.map((r) => r.run), Array(4).fill({ ran: false, error: "fx_search gave no string query" }));
  const after = (await turns(chat)).slice(before);

  assert.equal(after.length, 1, "one notice for the streak");
  assert.equal(after[0].text, "Fixture digest failed: fx_search gave no string query");
  assert.deepEqual(after[0].notice, {
    kind: "routine", plugin: FX, routine: "fx-digest", instance: id, name: "Fixture digest", thread: on.json.instance.home, thread_uid: on.json.instance.home_uid,
    run: after[0].notice.run, seq: null, turn: null, items: 0, error: "fx_search gave no string query",
  });
  assert.match(after[0].notice.run, /^run_\d{13}$/);
  const i = (await routineOf("fx-digest")).instances[0];

  assert.deepEqual([i.last_error, i.last_items], ["fx_search gave no string query", null]);
  assert.equal(dbRows(dataDir, "SELECT errors FROM v1_instances WHERE id=?", id)[0].errors, 4);
  assert.equal(i.next_run, i.last_run + 300_000 * 4, "x4 at the fourth failure");
  assert.deepEqual(await turns(on.json.instance.home), [], "a failed run posts no turn in its thread");
  const again = await R("enable", { routine: "fx-digest", thread: chat });

  assert.equal(dbRows(dataDir, "SELECT errors FROM v1_instances WHERE id=?", id)[0].errors, 0);
  assert.equal(again.json.instance.next_run, again.json.instance.last_run + 300_000);
  // the next failure starts a new streak: a notice again
  await R("run", { routine: "fx-digest", wait: true });
  assert.equal((await turns(chat)).slice(before).length, 2);
  assert.equal((await R("disable", { routine: "fx-digest" })).status, 200);
});

let hookPath = "";
let writer = null;

test("RTH-09 the ri_ webhook: secrets, items, seen ids, 100 items, 10 calls a minute, a new secret", async () => {
  const on = await R("enable", { routine: "fx-writer", thread: chat });

  writer = on.json.instance;
  assert.equal(on.status, 200, on.text);
  const sched = await R("hook", { routine: "fx-writer" });

  assert.deepEqual([sched.status, sched.json], [400, refusal("invalid_args", "its trigger is a schedule; set trigger to webhook first")]);
  assert.equal((await R("update", { routine: "fx-writer", trigger: "webhook" })).status, 200);
  const h = await R("hook", { routine: "fx-writer" });

  assert.equal(h.status, 200, h.text);
  assert.deepEqual(Object.keys(h.json), ["ok", "routine", "instance", "path"]);
  hookPath = h.json.path;
  assert.match(hookPath, new RegExp(`^/v1/hooks/${writer.instance}/[A-Za-z0-9_-]{32}$`));
  assert.equal((await routineOf("fx-writer")).instances[0].hook, true);
  assert.equal(dbRows(dataDir, "SELECT hook_hash FROM v1_instances WHERE id=?", writer.instance)[0].hook_hash.length, 64, "only the hash is kept");
  const hook = (body, path = hookPath) => request("POST", path, { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  const notFound = refusal("not_found", "no such hook");
  const wrong = await hook({}, `/v1/hooks/${writer.instance}/not-the-secret`);

  assert.deepEqual([wrong.status, wrong.json], [404, notFound]);
  assert.deepEqual(await hook({}, `/v1/hooks/${writer.instance}`).then((r) => [r.status, r.json]), [404, notFound], "no secret");
  assert.deepEqual(await request("GET", hookPath).then((r) => [r.status, r.json]), [404, notFound], "GET");
  assert.deepEqual(await hook({}, `/v1/hooks/ri_x/${hookPath.split("/").pop()}`).then((r) => [r.status, r.json]), [404, notFound], "a short id");
  // 1: two items run as the new items (no query)
  const one = await hook({ items: [{ id: "a1", subject: "  Hello\n  world  " }, { id: "a2", title: "Second" }, { id: "a1" }] });

  assert.equal(one.status, 202);
  assert.equal(one.text, JSON.stringify({ ok: true, instance: writer.instance, items: 3 }));
  assert.ok(await waitFor(async () => (await turns(writer.home)).length === 4, 10000), "two turns");
  assert.deepEqual((await turns(writer.home)).map((u) => u.text), ["New item · Hello world", MOCK_REPLY, "New item · Second", MOCK_REPLY]);
  assert.equal((await turns(chat)).filter((u) => u.notice?.instance === writer.instance).at(-1).text, "Fixture writer ran · 2 new items");
  // 2: a seen id runs nothing
  await waitFor(async () => !(await routineOf("fx-writer")).running, 5000);
  assert.equal((await hook({ items: [{ id: "a1" }] })).status, 202);
  await waitFor(async () => (await routineOf("fx-writer")).instances[0].last_items === 0, 5000);
  assert.equal((await turns(writer.home)).length, 4);
  // 3-4: bad items are refused, and still count
  const many = await hook({ items: Array.from({ length: 101 }, (_, k) => ({ id: `x${k}` })) });

  assert.deepEqual([many.status, many.json], [400, refusal("invalid_args", "items is a list of at most 100 objects with a string id")]);
  assert.equal((await hook({ items: [{ id: "" }] })).status, 400);
  // 5-10: one more each, up to the limit
  for (let k = 5; k <= 10; k++) {
    await waitFor(async () => !(await routineOf("fx-writer")).running, 5000);
    const r = await hook({ items: [{ id: "a2" }] });

    assert.equal(r.status, 202, `call ${k}: ${r.text}`);
  }
  const over = await hook({ items: [{ id: "a3" }] });

  assert.deepEqual([over.status, over.json], [429, refusal("rate_limited", "at most 10 calls a minute", true)]);
  // a new secret: the old path is gone (the rate is per instance)
  const next = await R("hook", { routine: "fx-writer" });

  assert.notEqual(next.json.path, hookPath);
  assert.deepEqual(await hook({ items: [] }).then((r) => [r.status, r.json]), [404, notFound]);
  assert.deepEqual(await hook({ items: [] }, next.json.path).then((r) => [r.status, r.json]), [429, refusal("rate_limited", "at most 10 calls a minute", true)]);
  hookPath = next.json.path;
});

test("RTH-10 disable one instance or all; Run now on a paused one is refused", async () => {
  const off = await R("disable", { routine: "fx-watch", instance: watch.instance });

  assert.equal(off.status, 200);
  assert.deepEqual(Object.keys(off.json), ["ok", "routine", "instance"]);
  assert.equal((await routineOf("fx-watch")).enabled, false);
  const run = await R("run", { routine: "fx-watch" });

  assert.deepEqual([run.status, run.json], [409, refusal("disabled", "Fixture watch · Acme Corp is paused; resume it first")]);
  const all = await R("disable", { routine: "fx-writer" });

  assert.deepEqual(Object.keys(all.json), ["ok", "routine"]);
  assert.equal(all.json.routine.enabled, false);
  const hooked = await request("POST", hookPath, { headers: { "Content-Type": "application/json" }, body: "{}" });

  assert.equal(hooked.status, 429, "the rate limit is checked before the switch");
});

test("RTH-11 delete an instance; its thread stays", async () => {
  const del = await R("delete", { routine: "fx-watch", instance: watch.instance });

  assert.equal(del.status, 200);
  assert.deepEqual(Object.keys(del.json), ["ok", "routine"]);
  assert.deepEqual(del.json.routine.instances, []);
  assert.ok((await turns(watch.home)).length >= 4, "its thread stays");
  const again = await R("delete", { routine: "fx-watch", instance: watch.instance });

  assert.deepEqual([again.status, again.json], [404, refusal("not_found", `no instance ${watch.instance} of fx-watch`)]);
  const none = await R("delete", { routine: "fx-watch" });

  assert.deepEqual([none.status, none.json], [404, refusal("not_found", "fx-watch has no instance")]);
  const run = await R("run", { routine: "fx-watch" });

  assert.deepEqual([run.status, run.json], [409, refusal("disabled", "Fixture watch is not enabled in any chat")]);
});

test("RTH-12 the 5.5h session routes' refusals: open, close, call, resolve, tickets/title, apps/source", async () => {
  const board = (await plugin()).apps.find((a) => a.id === "board");
  const ks = generate();
  const open = { plugin: FX, app: "board", open_id: random128(), ks_pub: ks.pub, perms_sha256: board.perms_sha256, not_after: Date.now() + 600_000 };
  const is = async (path, body, status, code, message) => {
    const r = await host("POST", `/v1/host/${path}`, body);

    assert.deepEqual([r.status, r.json?.code, message === undefined ? undefined : r.json.message], [status, code, message], `${path} ${r.text}`);

    return r.json;
  };

  await is("sessions/open", { ...open, plugin: "dev.goldcoders.nope" }, 404, "not_found", "dev.goldcoders.nope is not installed");
  await is("sessions/open", { ...open, app: "nope" }, 404, "not_found", `${FX} has no app nope`);
  await is("sessions/open", { ...open, perms_sha256: "0".repeat(64) }, 400, "invalid_args", "perms_sha256 is not the digest of the installed app's permissions");
  await is("sessions/open", { ...open, not_after: Date.now() - 1 }, 400, "invalid_args", "not_after must be in the next 8 hours (unix ms)");
  await is("sessions/open", { ...open, open_id: "short" }, 400, "invalid_args", "open_id must be 128 bits of base64url");
  const ok = await host("POST", "/v1/host/sessions/open", open);

  assert.equal(ok.json.ok, true, ok.text);
  await is("sessions/open", open, 400, "invalid_args", "open_id is already open");
  await is("call", {}, 400, "invalid_args", "send {envelope, sig}");
  const env = envelope({ ...open, ks }, "fx_secret", {});

  await is("call", env, 400, "permission_denied", "fx_secret is not in this app's permissions");
  await is("call", envelope({ ...open, ks, open_id: random128() }, "fx_echo", {}), 400, "session_invalid", "no open with a valid certificate has this open_id");
  await is("call", envelope({ ...open, ks }, "fx_echo", {}, { ts: Date.now() - 120_000 }), 400, "stale", "the envelope's time is more than 60 s from the broker's");
  await is("resolve", { plugin: FX, kind: "nope" }, 400, "invalid_args", "kind must be command or scope");
  await is("resolve", { plugin: FX, kind: "scope", name: "nope", value: "x" }, 404, "not_found", `${FX} has no scope nope`);
  await is("tickets/title", { plugin: FX, ticket: "nope" }, 400, "invalid_args", "ticket breaks the ticket grammar");
  const unknownTicket = await host("POST", "/v1/host/tickets/title", { plugin: FX, ticket: `tkt_${"0".repeat(26)}` });

  assert.deepEqual([unknownTicket.status, unknownTicket.json], [200, { ok: true, valid: false }]);
  await is("apps/source", { plugin: FX, app: "nope" }, 404, "not_found", `${FX} has no app nope`);
  await is("apps/source", { plugin: "dev.goldcoders.nope", app: "card" }, 404, "not_found", "dev.goldcoders.nope is not installed");
  assert.deepEqual((await host("POST", "/v1/host/sessions/close", { open_id: open.open_id })).json, { ok: true });
  assert.deepEqual((await host("POST", "/v1/host/sessions/close", { open_id: open.open_id })).json, { ok: true }, "closing twice is fine");
  await is("call", envelope({ ...open, ks }, "fx_echo", {}), 400, "session_invalid", "no open with a valid certificate has this open_id");
});

test("RTH-13 a deleted chat disables its instance at the next poll", { timeout: 40_000 }, async () => {
  const doomed = await newChat();
  const on = await R("enable", { routine: "fx-watch", thread: doomed });
  const id = on.json.instance.instance;

  assert.equal((await postJson("/thread/delete", { id: doomed })).json.ok, true);
  const run = await R("run", { routine: "fx-watch", instance: id });

  assert.deepEqual([run.status, run.json], [404, refusal("not_found", "Fixture watch's chat was deleted")]);
  // the poll (every 15 s) ticks it once: off, with the reason
  const off = await waitFor(async () => {
    const i = (await routineOf("fx-watch")).instances.find((x) => x.instance === id);

    return !i.enabled && i;
  }, 35_000);

  assert.ok(off, "switched off by the poll");
  assert.deepEqual([off.last_error, off.home, off.available], ["its chat was deleted", on.json.instance.home, true]);
  assert.equal(off.next_run, null);
});

let fake;

after(async () => {
  await fake?.close();
});

test("RTH-14 a restart writes a new host-token (0600); the old one is refused; enabled instances' threads are there", async () => {
  const old = token;

  fake = await startFakeOpencodex();
  await server.stop();
  // the next broker runs on the fake (MOCK off) for RTH-15
  server = await startServer(dataDir, { env: { ...modelEnv(fake, stubLog), ROX_KEYSTORE: `file:${b.keysDir}`, ROX_TRUST_FILE: b.trustFile } });
  token = readFileSync(join(dataDir, "host-token"), "utf8");
  assert.notEqual(token, old);
  assert.match(token, /^[A-Za-z0-9_-]{43}$/);
  assert.equal(statSync(join(dataDir, "host-token")).mode & 0o777, 0o600);
  assert.equal((await request("GET", "/v1/host/plugins", { headers: { "X-Rox-Host": old } })).status, 401);
  assert.equal((await host("GET", "/v1/host/plugins")).status, 200);
  assert.ok(await waitFor(async () => (await plugin())?.state === "running", 20000), "the plugin started again");
  const threads = (await request("GET", "/threads")).json.list;

  assert.ok(threads.some((t) => t.id === writer.home && t.routine?.instance === writer.instance));
});

test("RTH-15 the consent card: the agent's routine_create leaves a pending instance; the host's enable switches it on", async () => {
  const room = await newChat();

  fake.script("consent", [use("routine_create", { plugin: FX, routine: "fx-writer", trigger: "webhook", label: "From the agent" }), say("Asked the user.")]);
  fake.reset();
  const job = await postJson("/chat", { prompt: "[[script:consent]] watch my fixture", thread: room });

  assert.equal(job.status, 200, job.text);
  assert.ok(await waitFor(async () => (await request("GET", `/status?id=${job.json.id}`)).json.done, 30000));
  const card = (await turns(room)).find((u) => u.notice?.kind === "routine_consent");

  assert.ok(card, JSON.stringify(await turns(room)));
  const id = card.notice.instance;

  assert.equal(card.text, "Enable Fixture writer (From the agent) here? It may change Fixture data without asking: fx_echo.");
  assert.deepEqual(card.notice, {
    kind: "routine_consent", plugin: FX, routine: "fx-writer", instance: id, name: "Fixture writer", title: "Fixture", summary: "From the agent", writes: ["fx_echo"], reads: [], state: "pending",
  });
  const toolResult = fake.calls()[1].body.messages.at(-1).content.find((c) => c.type === "tool_result");

  assert.equal(
    typeof toolResult.content === "string" ? toolResult.content : toolResult.content.map((c) => c.text).join(""),
    `made ${id} — Fixture writer · From the agent; it waits for the user: a card in this chat asks them to Enable it (it may change Fixture data: fx_echo)`,
  );
  const pending = (await routineOf("fx-writer")).instances.find((x) => x.instance === id);

  assert.deepEqual([pending.enabled, pending.pending, pending.trigger, pending.home], [false, true, "webhook", null]);
  const run = await R("run", { routine: "fx-writer", instance: id });

  assert.deepEqual([run.status, run.json], [409, refusal("disabled", "Fixture writer · From the agent waits for the user's Enable")]);
  const on = await R("enable", { routine: "fx-writer", instance: id, approve: true });

  assert.equal(on.status, 200, on.text);
  assert.deepEqual([on.json.instance.enabled, on.json.instance.pending, on.json.instance.thread], [true, false, room]);
  assert.equal((await turns(room)).find((u) => u.notice?.instance === id).notice.state, "enabled");
  // another one, deleted while pending: its card reads declined
  fake.reset();
  const job2 = await postJson("/chat", { prompt: "[[script:consent]] once more", thread: room });

  await waitFor(async () => (await request("GET", `/status?id=${job2.json.id}`)).json.done, 30000);
  const card2 = (await turns(room)).filter((u) => u.notice?.kind === "routine_consent").at(-1);

  assert.notEqual(card2.notice.instance, id);
  assert.equal((await R("delete", { routine: "fx-writer", instance: card2.notice.instance })).status, 200);
  assert.equal((await turns(room)).find((u) => u.notice?.instance === card2.notice.instance).notice.state, "declined");
});

test("RTH-16 uninstall: the plugin's routines and instances go with it; the event", async () => {
  const ev = listen();

  await waitFor(() => ev.raw, 5000);
  assert.deepEqual((await host("POST", `/v1/host/plugins/${FX}/uninstall`, {})).json, { ok: true });
  assert.ok(await waitFor(() => ev.events.some((e) => e.state === "uninstalled"), 5000));
  assert.deepEqual(ev.events.at(-1), { id: FX, state: "uninstalled", code: null, generation: 1, enabled: false });
  ev.close();
  assert.deepEqual(dbRows(dataDir, "SELECT * FROM v1_routines"), []);
  assert.deepEqual(dbRows(dataDir, "SELECT * FROM v1_instances"), []);
  assert.equal((await host("GET", "/v1/host/plugins")).text, '{"ok":true,"plugins":[]}');
  const hooked = await request("POST", hookPath, { headers: { "Content-Type": "application/json" }, body: "{}" });

  assert.equal(hooked.status, 404);
});
