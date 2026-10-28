// v1/routines.mjs — v1 plugin routines (spec 5.9, "How a routine runs").
//
// A v1 routine is declared in a plugin's manifest; the user enables it in
// a chat and, when it has `reads`, approves once what it may read from
// another plugin. The approval is stored as the exact `reads` it covered,
// so an update that changes them needs a new one. Every `every_s` the
// runner calls the routine's own query tool (origin `routine`), takes the
// string `structuredContent[query.field]`, hands it to the first tool of
// the first `reads` entry as {query} (or uses the own result without
// reads), and runs the routine's prompt as one agent turn per item whose
// id it has not seen. The turn's v1 tools are the plugin's own plus the
// listed `reads` tools only. Seen ids (the last 1000) and the approval
// live in `v1_routines` beside the install records.
//
// The turns run in the routine's own thread (`threads`, routine-threads.mjs),
// not in the chat it was enabled in: that chat gets one notice per run that
// ran items, and one when it fails after working (not on every retry). The
// thread exists as soon as the routine is enabled (and, for one enabled
// before, from the server's start or its next tick), so it can be opened
// before anything new came; a run with nothing new adds no turn to it.
//
// The runner sits beside v0's timer (routines.mjs), which is unchanged.
// It never runs what may not run: a routine of a plugin off the approved
// list, a `reads` plugin that is missing, unapproved by the trust store,
// disabled, not running, or a non-read-only or switched-off tool leaves it
// unavailable with a reason, and nothing is called. Calls go through the
// plugin manager, so its running checks and timeouts apply. The clock and
// the agent turn (`runTurn`) are options so the tests run a tick without
// a model.
import { approvedText, v1RoutinePrompt } from "./agent.mjs";
import { refuse } from "./errors.mjs";
import { isObject, isString } from "./jcs.mjs";
import { systemClock } from "./plugins.mjs";

export const SEEN_MAX = 1000;
export const POLL_MS = 15_000;
const ITEM_MAX = 40_000;

const SCHEMA = `
  CREATE TABLE IF NOT EXISTS v1_routines(
    plugin TEXT NOT NULL, routine TEXT NOT NULL, thread TEXT, enabled INTEGER NOT NULL DEFAULT 0,
    approved_reads TEXT, seen TEXT NOT NULL DEFAULT '[]', last_run INTEGER, last_error TEXT,
    errors INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(plugin, routine));
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

export function createRoutines({ db, plugins, runTurn, threads, threadExists = () => true, clock = plugins.clock || systemClock, pollMs = POLL_MS }) {
  db.exec(SCHEMA);
  try {
    db.exec(LAST_ITEMS);
  } catch {}
  const running = new Set();
  let timer = null;

  const rowOf = (plugin, id) => db.prepare("SELECT * FROM v1_routines WHERE plugin=? AND routine=?").get(String(plugin ?? ""), String(id ?? ""));

  function routineOf(plugin, id) {
    const row = plugins.rowOf(plugin);

    return row ? (plugins.manifestOf(row).routines || []).find((r) => r.id === id) || null : null;
  }

  function mustRoutine(body) {
    if (!isObject(body) || !isString(body.plugin) || !isString(body.routine)) refuse("invalid_args", "send {plugin, routine}");
    const r = routineOf(body.plugin, body.routine);

    if (!r) refuse("not_found", `${body.plugin.slice(0, 80)} has no routine ${body.routine.slice(0, 80)}`);

    return r;
  }

  // a routine without reads has nothing to approve
  const approved = (r, row) => !(r.reads || []).length || row?.approved_reads === readsKey(r);

  // "" when the routine may run now, else why not
  function unavailable(pluginId, r, row) {
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

    if (!approved(r, row)) return row?.approved_reads ? "what it reads changed in an update; approve it again" : "you have not approved what it reads";

    return "";
  }

  // the wait after a run: the interval, doubled per failure past the third
  function intervalMs(r, row) {
    const base = r.every_s * 1000;

    return row.errors >= 3 ? Math.min(base * 2 ** Math.min(row.errors - 2, 6), Math.max(base, 86_400_000)) : base;
  }

  /** The user's side of a routine, for GET /v1/host/plugins: `thread` is
   *  the chat it reports to, `home` its own thread. */
  function info(pluginId, r) {
    const row = rowOf(pluginId, r.id);
    const reason = plugins.rowOf(pluginId) ? unavailable(pluginId, r, row) : `${pluginId} is not installed`;
    const home = threads.find(pluginId, r.id);

    return {
      enabled: !!row?.enabled,
      thread: row?.thread || null,
      home: home?.id ?? null,
      home_uid: home?.uid ?? null,
      approved: approved(r, row),
      available: !reason,
      reason: reason || null,
      seen_count: parse(row?.seen, []).length,
      last_run: row?.last_run ?? null,
      last_error: row?.last_error ?? null,
      last_items: row?.last_items ?? null,
      next_run: row?.enabled ? (row.last_run || 0) + intervalMs(r, row) : null,
    };
  }

  plugins.useRoutineInfo(info);

  // the routine's own thread, made when it is enabled in a chat that exists
  function homeOf(pluginId, r, row) {
    if (!row?.enabled || !row.thread || !threadExists(row.thread)) return null;

    return threads.ensure({ plugin: pluginId, routine: r.id, name: approvedText(r.name, 80), chat: row.thread });
  }

  function upsert(pluginId, id) {
    db.prepare("INSERT OR IGNORE INTO v1_routines(plugin, routine) VALUES(?, ?)").run(pluginId, id);
  }

  /** POST /v1/host/routines/approve {plugin, routine}: the user's one-time
   *  approval of what the routine reads (5.9, Q1). */
  function approve(body) {
    const r = mustRoutine(body);

    upsert(body.plugin, r.id);
    db.prepare("UPDATE v1_routines SET approved_reads=? WHERE plugin=? AND routine=?").run(readsKey(r), body.plugin, r.id);

    return { ok: true, routine: { id: r.id, ...info(body.plugin, r) } };
  }

  /** POST /v1/host/routines/enable {plugin, routine, thread}: runs in that
   *  chat from the next poll; re-enabling clears the failure count. */
  function enable(body) {
    const r = mustRoutine(body);
    const thread = isString(body.thread) && body.thread ? body.thread : rowOf(body.plugin, r.id)?.thread;

    if (!thread || !threadExists(thread)) refuse("invalid_args", "thread must name an existing chat");
    upsert(body.plugin, r.id);
    db.prepare("UPDATE v1_routines SET enabled=1, thread=?, errors=0 WHERE plugin=? AND routine=?").run(thread, body.plugin, r.id);
    homeOf(body.plugin, r, rowOf(body.plugin, r.id));

    return { ok: true, routine: { id: r.id, ...info(body.plugin, r) } };
  }

  /** POST /v1/host/routines/disable {plugin, routine}. */
  function disable(body) {
    const r = mustRoutine(body);

    db.prepare("UPDATE v1_routines SET enabled=0 WHERE plugin=? AND routine=?").run(body.plugin, r.id);

    return { ok: true, routine: { id: r.id, ...info(body.plugin, r) } };
  }

  // ── one run ──────────────────────────────────────────────────────────

  async function call(pluginId, tool, args) {
    const meta = { "rox/caller": { origin: "routine", plugin: pluginId, ts: Date.now() } };
    const r = await plugins.callTool(pluginId, tool, args, { meta });

    if (r.isError) throw new Error(`${tool}: ${firstText(r).slice(0, 300) || "failed"}`);

    if (!isObject(r.structuredContent)) throw new Error(`${tool} answered without structuredContent`);

    return r.structuredContent;
  }

  // steps 1-2: the search text from the own tool, then the items
  async function itemsOf(pluginId, r) {
    const own = await call(pluginId, r.query.tool, r.query.args || {});
    const text = own[r.query.field];

    if (!isString(text)) throw new Error(`${r.query.tool} gave no string ${r.query.field}`);

    if (!text) return [];
    const read = r.reads?.[0];
    const res = read ? await call(read.plugin, read.tools[0], { query: text }) : own;

    if (!Array.isArray(res.items)) throw new Error(`${read ? read.tools[0] : r.query.tool} gave no items`);
    const byId = new Map();

    for (const x of res.items) if (isObject(x) && isString(x.id) && x.id && !byId.has(x.id)) byId.set(x.id, x);

    return [...byId.values()];
  }

  function turnPrompt(r, label, item) {
    const json = JSON.stringify(item, null, 1);
    const from = r.reads?.length ? `${r.reads[0].plugin} ${r.reads[0].tools[0]}` : r.query.tool;

    return [
      `(This routine runs on its own every ${r.every_s}s; the user isn't waiting on this message. Do what it asks and report briefly.)`,
      label,
      `New item from ${from}:\n${json.length > ITEM_MAX ? `${json.slice(0, ITEM_MAX)}…[truncated]` : json}`,
    ].join("\n\n");
  }

  // the shown line of an item's turn: its subject or title when it has one
  function shownOf(item) {
    const head = ["subject", "title", "name"].map((k) => item[k]).find((v) => isString(v) && v.trim());

    return head ? `New item · ${approvedText(head.replace(/\s+/g, " "), 120)}` : "New item";
  }

  // the run's notice in the chat it reports to: where its turns start in
  // the routine's thread (`seq`, and `turn` by id) and how many ran
  function notify(pluginId, r, row, run, error) {
    const home = run.home || threads.find(pluginId, r.id);

    threads.notice(row.thread, {
      kind: "routine",
      plugin: pluginId,
      routine: r.id,
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

  /** One run of a routine: → {ran, items?, prompts?, reason?, error?}. */
  async function tick(pluginId, id) {
    const key = `${pluginId}/${id}`;

    if (running.has(key)) return { ran: false, reason: "already running" };
    running.add(key);
    const started = clock.now();
    let row, r, run;

    try {
      row = rowOf(pluginId, id);
      r = routineOf(pluginId, id);

      if (!row?.enabled || !r) return { ran: false, reason: "not enabled" };

      if (!threadExists(row.thread)) {
        db.prepare("UPDATE v1_routines SET enabled=0, last_run=?, last_error=? WHERE plugin=? AND routine=?").run(started, "its chat was deleted", pluginId, id);

        return { ran: false, reason: "its chat was deleted" };
      }

      // made here too for a routine enabled before threads were made early
      const home = homeOf(pluginId, r, row);
      const why = unavailable(pluginId, r, row);

      if (why) {
        db.prepare("UPDATE v1_routines SET last_run=? WHERE plugin=? AND routine=?").run(started, pluginId, id);

        return { ran: false, reason: why };
      }

      run = { id: `run_${started}`, name: approvedText(r.name, 80), items: 0, home };
      const res = await runItems(pluginId, r, row, started, run);

      if (run.items) notify(pluginId, r, row, run);

      return res;
    } catch (e) {
      const msg = String(e?.message || e).slice(0, 500);

      db.prepare("UPDATE v1_routines SET last_run=?, last_error=?, errors=errors+1 WHERE plugin=? AND routine=?").run(started, msg, pluginId, id);
      console.error(`  v1 routine ${key}: ${msg.slice(0, 200)}`);

      // once per failure streak (the first failure after a success), and
      // whenever the failed run had already posted items
      if (run && (run.items || !row.errors)) {
        try {
          notify(pluginId, r, row, run, msg);
        } catch {}
      }

      return { ran: false, error: msg };
    } finally {
      running.delete(key);
    }
  }

  // step 3: one turn per unseen item, in the routine's own thread; an item
  // counts as seen once its turn ran
  async function runItems(pluginId, r, row, started, run) {
    const items = await itemsOf(pluginId, r);
    const seen = parse(row.seen, []);
    const known = new Set(seen);
    const label = v1RoutinePrompt(plugins, pluginId, r.id);
    const surface = { plugin: pluginId, reads: r.reads || [] };

    for (const item of items) {
      if (known.has(item.id)) continue;
      run.home ||= threads.ensure({ plugin: pluginId, routine: r.id, name: run.name, chat: row.thread });
      const mark = { plugin: pluginId, id: r.id, name: run.name, run: run.id };
      const done = await runTurn({ plugin: pluginId, routine: r.id, thread: run.home.id, chat: row.thread, shown: shownOf(item), prompt: turnPrompt(r, label, item), item, surface, mark });

      if (!run.items && done) Object.assign(run, { seq: done.seq, turn: done.turn });
      seen.push(item.id);
      run.items++;
      db.prepare("UPDATE v1_routines SET seen=? WHERE plugin=? AND routine=?").run(JSON.stringify(seen.slice(-SEEN_MAX)), pluginId, r.id);
    }

    db.prepare("UPDATE v1_routines SET last_run=?, last_error=NULL, errors=0, last_items=? WHERE plugin=? AND routine=?").run(started, run.items, pluginId, r.id);

    return { ran: true, items: items.length, prompts: run.items };
  }

  /** Every enabled routine whose interval has passed, one after another. */
  async function runDue() {
    const now = clock.now();

    for (const row of db.prepare("SELECT * FROM v1_routines WHERE enabled=1").all()) {
      const r = routineOf(row.plugin, row.routine);

      if (r && now >= (row.last_run || 0) + intervalMs(r, row)) await tick(row.plugin, row.routine);
    }
  }

  function poll() {
    timer = clock.setTimeout(() => {
      runDue().catch(() => {}).finally(() => timer && poll());
    }, pollMs);
  }

  // 13.6: an uninstalled plugin's routines go with it
  plugins.events.on("plugin", (e) => {
    if (e.state === "uninstalled") db.prepare("DELETE FROM v1_routines WHERE plugin=?").run(e.id);
  });

  return {
    approve, enable, disable, tick, runDue, info,
    /** Polls for due routines every `pollMs` (server start); the enabled
     *  routines' threads are made first. */
    start: () => {
      if (timer) return;

      for (const row of db.prepare("SELECT * FROM v1_routines WHERE enabled=1").all()) {
        const r = routineOf(row.plugin, row.routine);

        try {
          if (r) homeOf(row.plugin, r, row);
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
