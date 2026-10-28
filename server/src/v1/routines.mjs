// v1/routines.mjs — v1 plugin routines (spec 5.9, "How a routine runs",
// "Routine instances").
//
// A v1 routine is declared in a plugin's manifest; it is a template. What
// runs is an instance of it: (plugin, routine, instance id, the chat it
// reports to, params, trigger), each with its own thread, seen ids, last
// run and on/off. The user enables one in a chat and, when the routine
// has `reads`, approves once what it may read from another plugin. The
// approval is per routine, stored as the exact `reads` it covered, so an
// update that changes them needs a new one (`v1_routines`). The instances
// live in `v1_instances`; one made before instances existed was moved
// there from `v1_routines` with its thread and seen ids.
//
// An instance runs on a schedule (every `every_s`, its own or the
// routine's) or on a webhook (`POST /v1/hooks/<instance>/<secret>`). A run
// calls the routine's own query tool (origin `routine`) with its
// `query.args` and the instance's params, takes the string
// `structuredContent[query.field]`, hands it to the first tool of the
// first `reads` entry as {query} (or uses the own result without reads),
// and runs the routine's prompt as one agent turn per item whose id the
// instance has not seen. A webhook may hand the items in instead. The
// turn's v1 tools are the plugin's own plus the listed `reads` tools only,
// and of its own the turn calls only read-only ones and the routine's
// `writes` (agent.mjs). Seen ids are the last 1000.
//
// The turns run in the instance's own thread (`threads`, routine-threads.mjs),
// not in the chat it reports to: that chat gets one notice per run that
// ran items, and one when it fails after working (not on every retry).
//
// Consent stays with the user. The host routes (Rox, the user's taps)
// enable, approve and edit; the agent's tools (`agent*`) list, run, make,
// change and delete instances of the chats it speaks for, but an instance
// of a routine with `writes` (or with reads not yet approved) is never
// switched on by the agent: it waits (`pending`) behind a consent card in
// the chat, and the user's Enable there switches it on.
//
// The runner never runs what may not run: a routine of a plugin off the
// approved list, a `reads` plugin that is missing, unapproved by the trust
// store, disabled, not running, or a non-read-only or switched-off tool
// leaves it unavailable with a reason, and nothing is called. Calls go
// through the plugin manager, so its running checks and timeouts apply.
// The clock and the agent turn (`runTurn`) are options so the tests run a
// tick without a model.
import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { approvedText, v1RoutinePrompt } from "./agent.mjs";
import { refuse } from "./errors.mjs";
import { isObject, isString } from "./jcs.mjs";
import { paramsProblem, paramsSummary } from "./params.mjs";
import { systemClock } from "./plugins.mjs";

export const SEEN_MAX = 1000;
export const POLL_MS = 15_000;
const ITEM_MAX = 40_000;
// a webhook: at most HOOK_RATE calls a minute per instance, HOOK_ITEMS items
export const HOOK_RATE = 10;
const HOOK_ITEMS = 100;
const EVERY_MIN = 60;
const EVERY_MAX = 86_400;

const SCHEMA = `
  CREATE TABLE IF NOT EXISTS v1_routines(
    plugin TEXT NOT NULL, routine TEXT NOT NULL, thread TEXT, enabled INTEGER NOT NULL DEFAULT 0,
    approved_reads TEXT, seen TEXT NOT NULL DEFAULT '[]', last_run INTEGER, last_error TEXT,
    errors INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(plugin, routine));
  CREATE TABLE IF NOT EXISTS v1_instances(
    id TEXT PRIMARY KEY, plugin TEXT NOT NULL, routine TEXT NOT NULL, thread TEXT NOT NULL,
    label TEXT, params TEXT NOT NULL DEFAULT '{}', trigger TEXT NOT NULL DEFAULT 'schedule', every_s INTEGER,
    enabled INTEGER NOT NULL DEFAULT 0, pending INTEGER NOT NULL DEFAULT 0,
    seen TEXT NOT NULL DEFAULT '[]', last_run INTEGER, last_error TEXT, errors INTEGER NOT NULL DEFAULT 0,
    last_items INTEGER, hook_hash TEXT, created_at INTEGER NOT NULL);
`;
// how many new items the last run that went through ran (0: nothing new)
const LAST_ITEMS = "ALTER TABLE v1_routines ADD COLUMN last_items INTEGER";

const parse = (s, d) => {
  try {
    return JSON.parse(s) ?? d;
  } catch {
    return d;
  }
};

const readsKey = (r) => JSON.stringify(r.reads || []);
const firstText = (r) => (r.content || []).find((c) => c.type === "text")?.text || "";
const sha = (s) => createHash("sha256").update(String(s)).digest();
const newId = () => `ri_${randomBytes(9).toString("base64url")}`;
const isInstanceRef = (s) => /^ri_[A-Za-z0-9_-]{6,40}$/.test(String(s ?? ""));

export function createRoutines({ db, plugins, runTurn, threads, threadExists = () => true, clock = plugins.clock || systemClock, pollMs = POLL_MS }) {
  db.exec(SCHEMA);
  try {
    db.exec(LAST_ITEMS);
  } catch {}
  const running = new Set(); // instance ids
  const hookCalls = new Map(); // instance id → call times (ms)
  let timer = null;

  const approvalOf = (plugin, id) => db.prepare("SELECT * FROM v1_routines WHERE plugin=? AND routine=?").get(String(plugin ?? ""), String(id ?? ""));
  const rowOf = (id) => db.prepare("SELECT * FROM v1_instances WHERE id=?").get(String(id ?? ""));
  const instancesOf = (plugin, id) => db.prepare("SELECT * FROM v1_instances WHERE plugin=? AND routine=? ORDER BY created_at, rowid").all(plugin, id);

  function routineOf(plugin, id) {
    const row = plugins.rowOf(plugin);

    return row ? (plugins.manifestOf(row).routines || []).find((r) => r.id === id) || null : null;
  }

  const titleOf = (plugin) => {
    const row = plugins.rowOf(plugin);

    return row ? plugins.manifestOf(row).title || plugin : plugin;
  };

  // ── moving single routines to instances (once) ───────────────────────
  // A routine enabled before instances existed becomes its one instance:
  // same chat, on/off, seen ids and last run; its thread is stamped with
  // the instance id so the instance keeps it.
  for (const row of db.prepare("SELECT * FROM v1_routines WHERE thread IS NOT NULL").all()) {
    if (db.prepare("SELECT 1 FROM v1_instances WHERE plugin=? AND routine=?").get(row.plugin, row.routine)) continue;
    const id = newId();

    db.prepare(
      "INSERT INTO v1_instances(id, plugin, routine, thread, enabled, seen, last_run, last_error, errors, last_items, created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
    ).run(id, row.plugin, row.routine, row.thread, row.enabled ? 1 : 0, row.seen || "[]", row.last_run ?? null, row.last_error ?? null, row.errors || 0, row.last_items ?? null, clock.now());
    threads.adopt(row.plugin, row.routine, id);
  }

  function mustRoutine(body) {
    if (!isObject(body) || !isString(body.plugin) || !isString(body.routine)) refuse("invalid_args", "send {plugin, routine}");
    const r = routineOf(body.plugin, body.routine);

    if (!r) refuse("not_found", `${body.plugin.slice(0, 80)} has no routine ${body.routine.slice(0, 80)}`);

    return r;
  }

  // a routine without reads has nothing to approve
  const approved = (r, ap) => !(r.reads || []).length || ap?.approved_reads === readsKey(r);
  // the user's consent an instance needs before it runs: a routine that
  // changes data, or reads what the user has not approved
  const needsConsent = (pluginId, r) => !!(r.writes || []).length || !approved(r, approvalOf(pluginId, r.id));

  // "" when the routine may run now, else why not
  function unavailable(pluginId, r) {
    const own = plugins.rowOf(pluginId);

    if (!own?.enabled) return `${pluginId} is disabled`;

    if (plugins.runtime(pluginId).state !== "running") return `${pluginId} is ${plugins.runtime(pluginId).state}`;

    if (!v1RoutinePrompt(plugins, pluginId, r.id)) return `${pluginId} is not an approved plugin`;

    if (plugins.disabledOf(own).has(r.query.tool)) return `${r.query.tool} is switched off`;

    for (const s of plugins.readsStatus(r)) if (!s.allowed) return s.reason;

    for (const x of r.reads || []) {
      const state = plugins.runtime(x.plugin).state;

      if (state !== "running") return `${x.plugin} is ${state}`;
      const off = x.tools.find((t) => plugins.disabledOf(plugins.rowOf(x.plugin)).has(t));

      if (off) return `${off} is switched off`;
    }

    const ap = approvalOf(pluginId, r.id);

    if (!approved(r, ap)) return ap?.approved_reads ? "what it reads changed in an update; approve it again" : "you have not approved what it reads";

    return "";
  }

  const everyOf = (r, row) => row?.every_s || r.every_s;

  // the wait after a run: the interval, doubled per failure past the third
  function intervalMs(r, row) {
    const base = everyOf(r, row) * 1000;

    return row.errors >= 3 ? Math.min(base * 2 ** Math.min(row.errors - 2, 6), Math.max(base, 86_400_000)) : base;
  }

  const paramsOf = (row) => parse(row?.params, {});
  const summaryOf = (r, row) => row.label || paramsSummary(r.params, paramsOf(row));
  const nameOf = (r, row) => {
    const sum = summaryOf(r, row);

    return approvedText(sum ? `${r.name} · ${sum}` : r.name, 120);
  };

  /** One instance as the user and the agent see it. */
  function instanceInfo(r, row) {
    const reason = plugins.rowOf(row.plugin) ? unavailable(row.plugin, r) : `${row.plugin} is not installed`;
    const home = threads.find(row.plugin, r.id, row.id);

    return {
      instance: row.id,
      label: row.label || null,
      params: paramsOf(row),
      summary: summaryOf(r, row),
      trigger: row.trigger,
      every_s: everyOf(r, row),
      enabled: !!row.enabled,
      pending: !!row.pending,
      thread: row.thread,
      home: home?.id ?? null,
      home_uid: home?.uid ?? null,
      available: !reason,
      reason: reason || null,
      seen_count: parse(row.seen, []).length,
      last_run: row.last_run ?? null,
      last_error: row.last_error ?? null,
      last_items: row.last_items ?? null,
      next_run: row.enabled && row.trigger === "schedule" ? (row.last_run || 0) + intervalMs(r, row) : null,
      running: running.has(row.id),
      hook: !!row.hook_hash,
    };
  }

  /** The user's side of a routine, for GET /v1/host/plugins: its instances,
   *  and (for clients before instances) the first one's state at the top,
   *  `enabled` when any is on. */
  function info(pluginId, r) {
    const ap = approvalOf(pluginId, r.id);
    const reason = plugins.rowOf(pluginId) ? unavailable(pluginId, r) : `${pluginId} is not installed`;
    const instances = instancesOf(pluginId, r.id).map((row) => instanceInfo(r, row));
    const first = instances.find((x) => x.enabled) || instances[0];

    return {
      enabled: instances.some((x) => x.enabled),
      thread: first?.thread ?? null,
      home: first?.home ?? null,
      home_uid: first?.home_uid ?? null,
      approved: approved(r, ap),
      available: !reason,
      reason: reason || null,
      seen_count: first?.seen_count ?? 0,
      last_run: first?.last_run ?? null,
      last_error: first?.last_error ?? null,
      last_items: first?.last_items ?? null,
      next_run: first?.next_run ?? null,
      running: instances.some((x) => x.running),
      instances,
    };
  }

  plugins.useRoutineInfo(info);

  // the instance's own thread, made when it is enabled in a chat that exists
  function homeOf(r, row) {
    if (!row?.enabled || !row.thread || !threadExists(row.thread)) return null;

    return threads.ensure({ plugin: row.plugin, routine: r.id, instance: row.id, name: nameOf(r, row), chat: row.thread });
  }

  function upsert(pluginId, id) {
    db.prepare("INSERT OR IGNORE INTO v1_routines(plugin, routine) VALUES(?, ?)").run(pluginId, id);
  }

  function setApproved(pluginId, r) {
    upsert(pluginId, r.id);
    db.prepare("UPDATE v1_routines SET approved_reads=? WHERE plugin=? AND routine=?").run(readsKey(r), pluginId, r.id);
  }

  const answer = (pluginId, r, row) => ({ ok: true, routine: { id: r.id, ...info(pluginId, r) }, ...(row ? { instance: instanceInfo(r, rowOf(row.id)) } : {}) });

  // {plugin, routine, instance?, thread?} → the instance meant: the named
  // one; else the routine's one in `thread`; else its only one. null when
  // there is none; refused when it is ambiguous.
  function resolve(body, r) {
    if (body.instance !== undefined) {
      const row = isString(body.instance) ? rowOf(body.instance) : null;

      if (!row || row.plugin !== body.plugin || row.routine !== r.id) refuse("not_found", `no instance ${String(body.instance).slice(0, 60)} of ${r.id}`);

      return row;
    }

    const all = instancesOf(body.plugin, r.id);

    if (isString(body.thread) && body.thread) return all.find((x) => x.thread === body.thread) || null;

    if (all.length > 1) refuse("invalid_args", `${r.id} has ${all.length} instances; name one (instance)`);

    return all[0] || null;
  }

  // params, trigger, every_s and label as given, checked; → the columns
  function fields(r, b, was = null) {
    const out = {};

    if (b.params !== undefined) {
      const why = paramsProblem(r.params, b.params);

      if (why) refuse("invalid_args", why);
      out.params = JSON.stringify(b.params);
    } else if (!was) {
      const why = paramsProblem(r.params, {});

      if (why) refuse("invalid_args", `${r.id} needs params: ${why}`);
    }

    if (b.trigger !== undefined) {
      if (b.trigger !== "schedule" && b.trigger !== "webhook") refuse("invalid_args", 'trigger is "schedule" or "webhook"');
      out.trigger = b.trigger;
    }

    if (b.every_s !== undefined && b.every_s !== null) {
      if (!Number.isInteger(b.every_s) || b.every_s < EVERY_MIN || b.every_s > EVERY_MAX) refuse("invalid_args", `every_s is an integer from ${EVERY_MIN} to ${EVERY_MAX}`);
      out.every_s = b.every_s;
    }

    if (b.label !== undefined && b.label !== null) {
      if (!isString(b.label)) refuse("invalid_args", "label must be a string");
      out.label = approvedText(b.label, 60) || null;
    }

    return out;
  }

  function create(pluginId, r, thread, f, { enabled, pending }) {
    const id = newId();

    db.prepare(
      "INSERT INTO v1_instances(id, plugin, routine, thread, label, params, trigger, every_s, enabled, pending, created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
    ).run(id, pluginId, r.id, thread, f.label ?? null, f.params ?? "{}", f.trigger ?? "schedule", f.every_s ?? null, enabled ? 1 : 0, pending ? 1 : 0, clock.now());

    return rowOf(id);
  }

  function patch(id, f) {
    for (const [k, v] of Object.entries(f)) db.prepare(`UPDATE v1_instances SET ${k}=? WHERE id=?`).run(v, id);
  }

  // the consent cards of an instance read as decided
  const settle = (row, state) => threads.settle(row.thread, row.id, state);

  /** POST /v1/host/routines/approve {plugin, routine}: the user's one-time
   *  approval of what the routine reads (5.9, Q1). */
  function approve(body) {
    const r = mustRoutine(body);

    setApproved(body.plugin, r);

    return answer(body.plugin, r);
  }

  /** POST /v1/host/routines/enable {plugin, routine, instance?, thread?,
   *  params?, every_s?, trigger?, label?, approve?}: the user switches an
   *  instance on, the named one or the routine's one in `thread` (made
   *  there when it has none), and it runs from the next poll. Re-enabling
   *  clears the failure count; a pending instance's consent card reads as
   *  enabled. `approve` approves what the routine reads too (the card's
   *  Enable). */
  function enable(body) {
    const r = mustRoutine(body);
    let row = resolve({ ...body, thread: body.instance ? undefined : body.thread }, r);
    const thread = isString(body.thread) && body.thread ? body.thread : row?.thread;

    if (!thread || !threadExists(thread)) refuse("invalid_args", "thread must name an existing chat");
    const f = fields(r, body, row);

    if (body.approve === true) setApproved(body.plugin, r);
    upsert(body.plugin, r.id);

    if (!row) row = create(body.plugin, r, thread, f, { enabled: true });
    else {
      patch(row.id, { ...f, thread, enabled: 1, pending: 0, errors: 0 });
      row = rowOf(row.id);
    }

    settle(row, "enabled");
    homeOf(r, row);

    return answer(body.plugin, r, row);
  }

  /** POST /v1/host/routines/disable {plugin, routine, instance?}: the
   *  named instance, or every instance of the routine. */
  function disable(body) {
    const r = mustRoutine(body);
    const rows = body.instance !== undefined ? [resolve(body, r)] : instancesOf(body.plugin, r.id);

    for (const row of rows) db.prepare("UPDATE v1_instances SET enabled=0 WHERE id=?").run(row.id);

    return answer(body.plugin, r, body.instance !== undefined ? rows[0] : null);
  }

  /** POST /v1/host/routines/update {plugin, routine, instance, params?,
   *  every_s?, trigger?, label?}: the user's edit; it stays on or off. */
  function update(body) {
    const r = mustRoutine(body);
    const row = resolve(body, r);

    if (!row) refuse("not_found", `${r.id} has no instance`);
    patch(row.id, fields(r, body, row));

    if (row.enabled) homeOf(r, rowOf(row.id));

    return answer(body.plugin, r, row);
  }

  /** POST /v1/host/routines/delete {plugin, routine, instance}: gone; its
   *  thread stays as it is. A pending one's card reads as declined. */
  function remove(body) {
    const r = mustRoutine(body);
    const row = resolve(body, r);

    if (!row) refuse("not_found", `${r.id} has no instance`);
    db.prepare("DELETE FROM v1_instances WHERE id=?").run(row.id);

    if (row.pending) settle(row, "declined");

    return answer(body.plugin, r);
  }

  /** POST /v1/host/routines/hook {plugin, routine, instance}: a new secret
   *  for the instance's webhook, the old one stops working. The path
   *  `/v1/hooks/<instance>/<secret>` is answered here once; only its hash
   *  is kept. Only an instance whose trigger is webhook has one. */
  function hook(body) {
    const r = mustRoutine(body);
    const row = resolve(body, r);

    if (!row) refuse("not_found", `${r.id} has no instance`);

    if (row.trigger !== "webhook") refuse("invalid_args", "its trigger is a schedule; set trigger to webhook first");
    const secret = randomBytes(24).toString("base64url");

    db.prepare("UPDATE v1_instances SET hook_hash=? WHERE id=?").run(sha(secret).toString("hex"), row.id);

    return { ...answer(body.plugin, r, row), path: `/v1/hooks/${row.id}/${secret}` };
  }

  /** POST /v1/host/routines/run {plugin, routine, instance?, reread?,
   *  wait?}: one run now, out of schedule (interval and backoff aside). It
   *  counts like a scheduled one (last_run, seen, notices). Refused when
   *  the instance is paused (`disabled`), already running
   *  (`rate_limited`) or unavailable (`plugin_unavailable`, with the
   *  reason). `reread` forgets the seen ids first, so every item the
   *  search finds runs again. The answer comes at once with `running:
   *  true`, or with `wait` once the run is done, its result in `run`
   *  ({ran, items, prompts} | {ran:false, reason|error}). */
  async function run(body) {
    const r = mustRoutine(body);
    const row = resolve(body, r);

    if (!row) refuse("disabled", `${approvedText(r.name, 80)} is not enabled in any chat`);
    const res = await runInstance(r, row, { reread: body.reread === true, wait: body.wait === true });

    return { ...answer(body.plugin, r, row), ...res };
  }

  async function runInstance(r, row, { reread = false, wait = false, items = null } = {}) {
    const name = nameOf(r, row);

    if (!row.enabled) refuse("disabled", row.pending ? `${name} waits for the user's Enable` : `${name} is paused; resume it first`);

    if (running.has(row.id)) refuse("rate_limited", `${name} is already running`);

    if (!threadExists(row.thread)) refuse("not_found", `${name}'s chat was deleted`);
    const why = unavailable(row.plugin, r);

    if (why) refuse("plugin_unavailable", `${name} can't run: ${why}`);

    if (reread) db.prepare("UPDATE v1_instances SET seen='[]' WHERE id=?").run(row.id);
    const done = tick(row.id, items);

    if (wait) return { run: await done };

    done.catch(() => {});

    return {};
  }

  // ── webhooks ─────────────────────────────────────────────────────────

  /** POST /v1/hooks/<instance>/<secret>: one run of a webhook instance.
   *  A body with `items` ([{id, …}], at most 100) runs those as the new
   *  items, skipping the query; any other body just wakes it (a normal
   *  run). A wrong or unknown secret is `not_found`, the same as an
   *  unknown instance; more than HOOK_RATE calls a minute, or one while it
   *  runs, `rate_limited`. → {ok, instance, items} at once. */
  function hookCall(id, secret, body) {
    const row = isInstanceRef(id) ? rowOf(id) : null;
    const want = row?.hook_hash ? Buffer.from(row.hook_hash, "hex") : null;
    const got = sha(secret || "");

    if (!row || row.trigger !== "webhook" || !want || want.length !== got.length || !timingSafeEqual(want, got)) refuse("not_found", "no such hook");
    const r = routineOf(row.plugin, row.routine);

    if (!r) refuse("not_found", "no such hook");
    const now = clock.now();
    const calls = (hookCalls.get(id) || []).filter((t) => t > now - 60_000);

    if (calls.length >= HOOK_RATE) refuse("rate_limited", `at most ${HOOK_RATE} calls a minute`);
    calls.push(now);
    hookCalls.set(id, calls);
    let items = null;

    if (isObject(body) && body.items !== undefined) {
      if (!Array.isArray(body.items) || body.items.length > HOOK_ITEMS || body.items.some((x) => !isObject(x) || !isString(x.id) || !x.id || x.id.length > 200))
        refuse("invalid_args", `items is a list of at most ${HOOK_ITEMS} objects with a string id`);
      items = body.items;
    }

    return runInstance(r, row, { items }).then(() => ({ ok: true, instance: row.id, items: items ? items.length : null }));
  }

  // ── the agent's routine tools (routines.mjs) ─────────────────────────
  // The agent sees, runs, makes, changes and deletes the instances of the
  // chats it speaks for (`chats`); it never switches on what needs the
  // user's consent, it asks with a card instead.

  /** The instances of the chats `chats` (the first is the one the agent
   *  makes them in), with their routine's name and plugin. */
  function ofChats(chats) {
    const want = new Set(chats);

    return db
      .prepare("SELECT * FROM v1_instances ORDER BY created_at, rowid")
      .all()
      .filter((row) => want.has(row.thread))
      .flatMap((row) => {
        const r = routineOf(row.plugin, row.routine);

        return r ? [{ plugin: row.plugin, title: titleOf(row.plugin), id: r.id, name: approvedText(r.name, 80), writes: r.writes || [], ...instanceInfo(r, row) }] : [];
      });
  }

  /** An instance id, or `<plugin>/<routine>` when that routine has one
   *  instance among `chats`; null when none. */
  function agentFind(ref, chats) {
    const mine = ofChats(chats);

    if (isInstanceRef(ref)) return mine.find((x) => x.instance === ref) || null;
    const at = String(ref ?? "").indexOf("/");

    if (at < 1) return null;
    const plugin = ref.slice(0, at), id = ref.slice(at + 1);
    const hits = mine.filter((x) => x.plugin === plugin && x.id === id);

    return hits.length === 1 ? hits[0] : hits.length ? { ambiguous: hits } : null;
  }

  const notMine = (ref) => ({ content: `no plugin routine ${String(ref).slice(0, 120)} in this chat (routine_list names them)`, is_error: true });
  const ambiguous = (x) => ({ content: `that routine has ${x.ambiguous.length} instances here; name one: ${x.ambiguous.map((a) => a.instance).join(", ")}`, is_error: true });

  // the card that asks the user to switch an instance on
  function askConsent(r, row) {
    const reads = (r.reads || []).map((x) => `${titleOf(x.plugin)}: ${x.tools.join(", ")}`);
    const summary = summaryOf(r, row);

    threads.notice(row.thread, {
      kind: "routine_consent",
      plugin: row.plugin,
      routine: r.id,
      instance: row.id,
      name: approvedText(r.name, 80),
      title: titleOf(row.plugin),
      summary,
      writes: r.writes || [],
      reads,
      state: "pending",
    });
  }

  const consentLine = (r, row) =>
    `it waits for the user: a card in this chat asks them to Enable it${(r.writes || []).length ? ` (it may change ${titleOf(row.plugin)} data: ${r.writes.join(", ")})` : " (they approve what it reads)"}`;

  /** routine_create for a plugin routine: an instance in `chats[0]`. On at
   *  once unless it needs the user's consent; then pending, behind a card. */
  function agentCreate(input, chats) {
    try {
      const r = mustRoutine(input);

      if (!v1RoutinePrompt(plugins, input.plugin, r.id)) refuse("permission_denied", `${input.plugin} is not an approved plugin`);
      const chat = chats[0];

      if (!chat || !threadExists(chat)) refuse("invalid_args", "no chat to report to");
      const f = fields(r, input);
      const ask = needsConsent(input.plugin, r);
      const row = create(input.plugin, r, chat, f, { enabled: !ask, pending: ask });
      const name = nameOf(r, row);

      if (ask) {
        askConsent(r, row);

        return { content: `made ${row.id} — ${name}; ${consentLine(r, row)}` };
      }

      upsert(input.plugin, r.id);
      homeOf(r, row);

      return { content: `made and enabled ${row.id} — ${name} · ${row.trigger === "webhook" ? "on its webhook (the user gets its URL in the routine's panel)" : `every ${everyOf(r, row)}s`}` };
    } catch (e) {
      return { content: String(e?.message || e), is_error: true };
    }
  }

  /** routine_update for a plugin routine: params, every_s, trigger, label,
   *  enabled. Turning on, or new params for, an instance that needs
   *  consent pauses it behind a new card instead. */
  function agentUpdate(ref, input, chats) {
    const x = agentFind(ref, chats);

    if (!x) return notMine(ref);

    if (x.ambiguous) return ambiguous(x);

    try {
      const r = routineOf(x.plugin, x.id);
      const row = rowOf(x.instance);
      const f = fields(r, input, row);
      const wantsOn = input.enabled === true || (input.enabled === undefined && row.enabled);
      const ask = needsConsent(x.plugin, r) && ((input.enabled === true && !row.enabled) || (f.params !== undefined && f.params !== row.params));

      patch(row.id, f);

      if (input.enabled === false) patch(row.id, { enabled: 0 });
      else if (ask) {
        patch(row.id, { enabled: 0, pending: 1 });
        askConsent(r, rowOf(row.id));

        return { content: `updated ${row.id}; ${consentLine(r, row)}` };
      } else if (wantsOn) patch(row.id, { enabled: 1, pending: 0, errors: input.enabled === true ? 0 : row.errors });
      const now = rowOf(row.id);

      if (now.enabled) homeOf(r, now);

      return { content: `updated ${row.id} — ${nameOf(r, now)} · ${now.enabled ? "on" : now.pending ? "waiting for the user" : "paused"}` };
    } catch (e) {
      return { content: String(e?.message || e), is_error: true };
    }
  }

  /** routine_delete for a plugin routine's instance. */
  function agentDelete(ref, chats) {
    const x = agentFind(ref, chats);

    if (!x) return notMine(ref);

    if (x.ambiguous) return ambiguous(x);
    const row = rowOf(x.instance);

    db.prepare("DELETE FROM v1_instances WHERE id=?").run(row.id);

    if (row.pending) settle(row, "declined");

    return { content: `deleted ${row.id} — ${x.name}; its thread stays` };
  }

  /** routine_run for a plugin routine: waits up to `waitMs` for the result. */
  async function agentRun(ref, chats, waitMs = 60_000) {
    const x = agentFind(ref, chats);

    if (!x) return notMine(ref);

    if (x.ambiguous) return ambiguous(x);
    let started, timer;

    try {
      const r = routineOf(x.plugin, x.id);
      const row = rowOf(x.instance);
      const name = nameOf(r, row);

      started = runInstance(r, row, { wait: true });
      const res = await Promise.race([started, new Promise((r) => (timer = clock.setTimeout(() => r(null), waitMs)))]);

      if (!res) return { content: `${name} is running; what it finds lands in its thread (${x.home || "its own"}) and this chat gets a notice` };
      const out = res.run;

      if (out.error) return { content: `${name} ran and failed: ${out.error}`, is_error: true };

      if (!out.ran) return { content: `${name} didn't run: ${out.reason}`, is_error: true };
      const home = threads.find(x.plugin, x.id, x.instance);

      return { content: `${name} ran: ${out.items} item${out.items === 1 ? "" : "s"} found, ${out.prompts} new${out.prompts ? ` (each ran in its thread ${home?.id})` : ""}` };
    } catch (e) {
      return { content: String(e?.message || e), is_error: true };
    } finally {
      clock.clearTimeout(timer);
      started?.catch(() => {});
    }
  }

  // ── one run ──────────────────────────────────────────────────────────

  // `routine`: the routine's id and instance, on a call to its own plugin (8.4)
  async function call(pluginId, tool, args, mark) {
    const meta = { "rox/caller": { origin: "routine", plugin: pluginId, ...(mark || {}), ts: Date.now() } };
    const r = await plugins.callTool(pluginId, tool, args, { meta });

    if (r.isError) throw new Error(`${tool}: ${firstText(r).slice(0, 300) || "failed"}`);

    if (!isObject(r.structuredContent)) throw new Error(`${tool} answered without structuredContent`);

    return r.structuredContent;
  }

  // steps 1-2: the search text from the own tool (its args plus the
  // instance's params), then the items
  async function itemsOf(r, row) {
    const params = paramsOf(row);
    const own = await call(row.plugin, r.query.tool, { ...(r.query.args || {}), ...params }, { routine: r.id, instance: row.id, params });
    const text = own[r.query.field];

    if (!isString(text)) throw new Error(`${r.query.tool} gave no string ${r.query.field}`);

    if (!text) return [];
    const read = r.reads?.[0];
    const res = read ? await call(read.plugin, read.tools[0], { query: text }) : own;

    if (!Array.isArray(res.items)) throw new Error(`${read ? read.tools[0] : r.query.tool} gave no items`);

    return dedupe(res.items);
  }

  function dedupe(list) {
    const byId = new Map();

    for (const x of list) if (isObject(x) && isString(x.id) && x.id && !byId.has(x.id)) byId.set(x.id, x);

    return [...byId.values()];
  }

  function turnPrompt(r, row, label, item, hooked) {
    const json = JSON.stringify(item, null, 1);
    const from = hooked ? "its webhook" : r.reads?.length ? `${r.reads[0].plugin} ${r.reads[0].tools[0]}` : r.query.tool;
    const params = paramsOf(row);
    const how = row.trigger === "webhook" ? "on its webhook" : `every ${everyOf(r, row)}s`;

    return [
      `(This routine runs on its own ${how}; the user isn't waiting on this message. Do what it asks and report briefly.)`,
      label,
      ...(Object.keys(params).length ? [`This instance (${row.id}${row.label ? `, "${row.label}"` : ""}) runs with these params; keep to them:\n${JSON.stringify(params)}`] : []),
      `New item from ${from}:\n${json.length > ITEM_MAX ? `${json.slice(0, ITEM_MAX)}…[truncated]` : json}`,
    ].join("\n\n");
  }

  // the shown line of an item's turn: its subject or title when it has one
  function shownOf(item) {
    const head = ["subject", "title", "name"].map((k) => item[k]).find((v) => isString(v) && v.trim());

    return head ? `New item · ${approvedText(head.replace(/\s+/g, " "), 120)}` : "New item";
  }

  // the run's notice in the chat it reports to: where its turns start in
  // the instance's thread (`seq`, and `turn` by id) and how many ran
  function notify(r, row, run, error) {
    const home = run.home || threads.find(row.plugin, r.id, row.id);

    threads.notice(row.thread, {
      kind: "routine",
      plugin: row.plugin,
      routine: r.id,
      instance: row.id,
      name: run.name,
      thread: home?.id ?? null,
      thread_uid: home?.uid ?? null,
      run: run.id,
      seq: run.seq ?? null,
      turn: run.turn ?? null,
      items: run.items,
      ...(error ? { error: error.slice(0, 300) } : {}),
    });
  }

  // `tick(instance, items?)`, or `tick(plugin, routine)` for a routine's
  // only instance
  function instanceOfTick(a, b) {
    if (isInstanceRef(a) && (b === undefined || b === null || Array.isArray(b))) return { id: a, items: b ?? null };
    const all = instancesOf(String(a ?? ""), String(b ?? ""));

    return { id: all.length === 1 ? all[0].id : null, items: null };
  }

  /** One run of an instance: → {ran, items?, prompts?, reason?, error?}.
   *  `items` (a webhook's) stand in for the query. */
  async function tick(a, b) {
    const { id, items: given } = instanceOfTick(a, b);

    if (!id) return { ran: false, reason: "not enabled" };

    if (running.has(id)) return { ran: false, reason: "already running" };
    running.add(id);
    const started = clock.now();
    let row, r, run;

    try {
      row = rowOf(id);
      r = row && routineOf(row.plugin, row.routine);

      if (!row?.enabled || !r) return { ran: false, reason: "not enabled" };

      if (!threadExists(row.thread)) {
        db.prepare("UPDATE v1_instances SET enabled=0, last_run=?, last_error=? WHERE id=?").run(started, "its chat was deleted", id);

        return { ran: false, reason: "its chat was deleted" };
      }

      // made here too for an instance enabled before threads were made early
      const home = homeOf(r, row);
      const why = unavailable(row.plugin, r);

      if (why) {
        db.prepare("UPDATE v1_instances SET last_run=? WHERE id=?").run(started, id);

        return { ran: false, reason: why };
      }

      run = { id: `run_${started}`, name: nameOf(r, row), items: 0, home };
      const res = await runItems(r, row, started, run, given);

      if (run.items) notify(r, row, run);

      return res;
    } catch (e) {
      const msg = String(e?.message || e).slice(0, 500);

      db.prepare("UPDATE v1_instances SET last_run=?, last_error=?, errors=errors+1 WHERE id=?").run(started, msg, id);
      console.error(`  v1 routine ${row?.plugin}/${row?.routine} ${id}: ${msg.slice(0, 200)}`);

      // once per failure streak (the first failure after a success), and
      // whenever the failed run had already posted items
      if (run && (run.items || !row.errors)) {
        try {
          notify(r, row, run, msg);
        } catch {}
      }

      return { ran: false, error: msg };
    } finally {
      running.delete(id);
    }
  }

  // step 3: one turn per unseen item, in the instance's own thread; an
  // item counts as seen once its turn ran
  async function runItems(r, row, started, run, given) {
    const items = given ? dedupe(given) : await itemsOf(r, row);
    const seen = parse(row.seen, []);
    const known = new Set(seen);
    const label = v1RoutinePrompt(plugins, row.plugin, r.id);
    const params = paramsOf(row);
    const surface = { plugin: row.plugin, routine: r.id, instance: row.id, params, reads: r.reads || [], writes: r.writes || [] };

    for (const item of items) {
      if (known.has(item.id)) continue;
      run.home ||= threads.ensure({ plugin: row.plugin, routine: r.id, instance: row.id, name: run.name, chat: row.thread });
      const mark = { plugin: row.plugin, id: r.id, instance: row.id, name: run.name, run: run.id };
      const done = await runTurn({
        plugin: row.plugin, routine: r.id, instance: row.id, thread: run.home.id, chat: row.thread, shown: shownOf(item), prompt: turnPrompt(r, row, label, item, !!given), item, surface, mark,
      });

      if (!run.items && done) Object.assign(run, { seq: done.seq, turn: done.turn });
      seen.push(item.id);
      run.items++;
      db.prepare("UPDATE v1_instances SET seen=? WHERE id=?").run(JSON.stringify(seen.slice(-SEEN_MAX)), row.id);
    }

    db.prepare("UPDATE v1_instances SET last_run=?, last_error=NULL, errors=0, last_items=? WHERE id=?").run(started, run.items, row.id);

    return { ran: true, items: items.length, prompts: run.items };
  }

  /** Every enabled scheduled instance whose interval has passed, one after another. */
  async function runDue() {
    const now = clock.now();

    for (const row of db.prepare("SELECT * FROM v1_instances WHERE enabled=1 AND trigger='schedule' ORDER BY created_at, rowid").all()) {
      const r = routineOf(row.plugin, row.routine);

      if (r && now >= (row.last_run || 0) + intervalMs(r, row)) await tick(row.id);
    }
  }

  function poll() {
    timer = clock.setTimeout(() => {
      runDue().catch(() => {}).finally(() => timer && poll());
    }, pollMs);
  }

  // 13.6: an uninstalled plugin's routines go with it
  plugins.events.on("plugin", (e) => {
    if (e.state === "uninstalled") {
      db.prepare("DELETE FROM v1_routines WHERE plugin=?").run(e.id);
      db.prepare("DELETE FROM v1_instances WHERE plugin=?").run(e.id);
    }
  });

  return {
    approve, enable, disable, update, remove, hook, hookCall, run, tick, runDue, info,
    ofChats, agentFind, agentCreate, agentUpdate, agentDelete, agentRun,
    /** Polls for due instances every `pollMs` (server start); the enabled
     *  instances' threads are made first. */
    start: () => {
      if (timer) return;

      for (const row of db.prepare("SELECT * FROM v1_instances WHERE enabled=1").all()) {
        const r = routineOf(row.plugin, row.routine);

        try {
          if (r) homeOf(r, row);
        } catch {}
      }
      poll();
    },
    stop: () => {
      if (timer) clock.clearTimeout(timer);
      timer = null;
    },
  };
}
