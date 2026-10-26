// routines.mjs — work the agent does on a timer, in the chat it was asked
// in. A routine ticks every `every_s` seconds (60 at least); with a `check`
// a tick first calls one MCP tool directly (no model, no cost) and wakes
// the agent only for items it hasn't seen:
//   routines(id, name, thread, every_s, enabled, check, prompt, seen,
//            last_run, last_check, last_error, errors, created_at, template)
//   check = {server, tool, args, ids, args_from?} | null — `ids` picks the
//   items out of the tool's result (itemsOf); `args_from` {server, tool,
//   args, field, into} fills args[into] from another tool's result each
//   tick; `seen` is the ids already handled (JSON).
//   template = "<server>/<id>" for one made from a server's template.
// The turn runs in the routine's own thread, never the active one, so the
// chat the user is in stays where it is. Timers live in this process and are
// recomputed from the table on start and on every change; three failures
// in a row back off.
import { createHash } from "node:crypto";
import { allocJobId, db, findThread, metaGet, metaSet, modelCtx, store, uuidv7 } from "./state.mjs";
import { callToolJson, ensureClient, mcpName, readManifest, statusOf } from "./mcp.mjs";

db.exec(`
  CREATE TABLE IF NOT EXISTS routines(
    id TEXT PRIMARY KEY, name TEXT NOT NULL, thread TEXT NOT NULL,
    every_s INTEGER NOT NULL, enabled INTEGER DEFAULT 1, "check" TEXT, prompt TEXT,
    seen TEXT, last_run INTEGER, last_check INTEGER, last_error TEXT,
    errors INTEGER DEFAULT 0, created_at INTEGER);
`);
try { db.exec("ALTER TABLE routines ADD COLUMN template TEXT"); } catch {}

const MIN_EVERY = 60;
const MAX_EVERY = 7 * 86400;
const SEEN_MAX = 5000;
const parseJson = (s, d) => {
  try { return JSON.parse(s); } catch { return d; }
};
const rowOf = (id) => db.prepare("SELECT * FROM routines WHERE id=?").get(String(id || ""));
const clampEvery = (s) => Math.max(MIN_EVERY, Math.min(MAX_EVERY, Math.round(Number(s) || 0)));

// The wait before the next tick: the interval, doubled per failure past
// the third (at most 64×, a day at most).
function intervalMs(r) {
  const base = r.every_s * 1000;
  return r.errors >= 3 ? Math.min(base * 2 ** Math.min(r.errors - 2, 6), Math.max(base, 86400_000)) : base;
}
const nextRun = (r) => (r.enabled ? (r.last_run || 0) + intervalMs(r) : null);

/** A routine as the endpoints and tools hand it out. */
export function routineJson(r) {
  const t = store.list.find((x) => x.id === r.thread);
  const bot = store.bots.find((b) => b.threadId === r.thread);
  return {
    id: r.id,
    name: r.name,
    thread: r.thread,
    threadTitle: t?.title || "",
    botName: bot?.name || "",
    every_s: r.every_s,
    enabled: !!r.enabled,
    check: parseJson(r.check, null),
    prompt: r.prompt || "",
    seen_count: parseJson(r.seen, []).length,
    last_run: r.last_run || null,
    last_check: r.last_check || null,
    last_error: r.last_error || null,
    errors: r.errors || 0,
    next_run: nextRun(r),
    created_at: r.created_at,
    template: r.template || null,
  };
}

export const listRoutines = () =>
  db.prepare("SELECT * FROM routines ORDER BY created_at").all().map(routineJson);

// ————— picking items out of a check's result —————
// `ids` is a path into the result's JSON: `messages[].id` is each item of
// `messages` and its `id`; a path ending in `[]` (`changes[]`) identifies an
// item by its whole JSON, so an item that changes is new again. A text
// result takes `/regex/` instead: each match's first group is an item's id
// (`/Message ID: (\S+)/` for workspace-mcp's Gmail search).
const hashOf = (v) => createHash("sha1").update(JSON.stringify(v)).digest("hex").slice(0, 16);
function walk(v, path) {
  let vals = [v];
  for (const seg of path.split(".").filter(Boolean)) {
    const name = seg.replace(/\[\]$/, "");
    vals = vals.map((x) => (name ? x?.[name] : x));
    if (seg.endsWith("[]")) vals = vals.flatMap((x) => (Array.isArray(x) ? x : []));
  }
  return vals.filter((x) => x !== undefined && x !== null);
}
export function itemsOf(result, ids) {
  ids = String(ids || "").trim();
  const re = ids.match(/^\/(.+)\/([a-z]*)$/s);
  if (re) {
    const text = typeof result === "string" ? result : JSON.stringify(result);
    const flags = re[2].includes("g") ? re[2] : `${re[2]}g`;
    return [...text.matchAll(new RegExp(re[1], flags))].map((m) => ({ id: String(m[1] ?? m[0]), item: { id: String(m[1] ?? m[0]) } }));
  }
  if (typeof result === "string") {
    const j = parseJson(result, undefined);
    if (j === undefined) throw new Error("the check's result is text, not JSON — give ids as a /regex/");
    result = j;
  }
  const cut = ids.lastIndexOf("[]");
  const [itemPath, idPath] = cut < 0 ? [`${ids}[]`, ""] : [ids.slice(0, cut + 2), ids.slice(cut + 2).replace(/^\./, "")];
  return walk(result, itemPath).map((item) => {
    const id = idPath ? walk(item, idPath)[0] : typeof item === "object" ? hashOf(item) : item;
    return { id: id == null ? hashOf(item) : String(id), item };
  });
}

// "$last_check" anywhere in a check's args → when the last check that was
// fully handled started (ISO; the epoch before the first).
function fillArgs(v, lastCheck) {
  if (v === "$last_check") return new Date(lastCheck || 0).toISOString();
  if (Array.isArray(v)) return v.map((x) => fillArgs(x, lastCheck));
  if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, fillArgs(x, lastCheck)]));
  return v;
}

// A check's args for this tick: its own, with args[into] taken from
// `args_from`'s tool's result (its `field` path) when it has one — so a
// query that holds dates or ids is built fresh every time.
async function checkArgs(check, lastCheck) {
  const args = fillArgs(check.args || {}, lastCheck);
  const f = check.args_from;
  if (!f) return args;
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(String(f.server || ""));
  if (!row) throw new Error(`${f.server} isn't connected`);
  const res = await callToolJson(row, String(f.tool || ""), fillArgs(f.args || {}, lastCheck));
  if (!res.ok) throw new Error(`${f.tool}: ${String(res.error || "failed").slice(0, 300)}`);
  const v = walk(typeof res.result === "string" ? parseJson(res.result, {}) : res.result, String(f.field || ""))[0];
  if (v === undefined || v === "") throw new Error(`${f.tool} gave no ${f.field}`);
  return { ...args, [f.into || "query"]: v };
}

// ————— the scheduler —————
const timers = new Map(); // id → timeout
const running = new Set();

function arm(id) {
  clearTimeout(timers.get(id));
  timers.delete(id);
  const r = rowOf(id);
  if (!r?.enabled) return;
  const at = nextRun(r);
  const wait = Math.max(1000, Math.min(at - Date.now(), 2 ** 31 - 1));
  timers.set(id, setTimeout(() => {
    timers.delete(id);
    const cur = rowOf(id);
    if (!cur?.enabled) return;
    // a timer capped short of a far-off tick just re-arms
    if (nextRun(cur) > Date.now() + 1000) return arm(id);
    tick(id).catch(() => {});
  }, wait));
}

/** On start: a timer per enabled routine; overdue ones tick at once. */
export function startRoutines() {
  for (const r of db.prepare("SELECT id FROM routines WHERE enabled=1").all()) arm(r.id);
}

// One tick: the check (when there is one), then at most one agent turn.
async function tick(id) {
  if (running.has(id)) return;
  running.add(id);
  const r = rowOf(id);
  const started = Date.now();
  try {
    if (!r) return;
    const t = findThread(r.thread);
    if (!t) {
      db.prepare("UPDATE routines SET enabled=0, last_run=?, last_error=? WHERE id=?").run(started, "its chat was deleted", id);
      return;
    }
    const check = parseJson(r.check, null);
    let fresh = null;
    if (check) {
      const row = db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(String(check.server || ""));
      if (!row) throw new Error(`${check.server} isn't connected`);
      const res = await callToolJson(row, String(check.tool || ""), await checkArgs(check, r.last_check));
      if (!res.ok) throw new Error(`${check.tool}: ${String(res.error || "failed").slice(0, 300)}`);
      const seen = new Set(parseJson(r.seen, []));
      const byId = new Map();
      for (const x of itemsOf(res.result, check.ids)) if (!seen.has(x.id) && !byId.has(x.id)) byId.set(x.id, x.item);
      fresh = [...byId];
      // nothing new: no turn, no model
      if (!fresh.length) {
        db.prepare("UPDATE routines SET last_run=?, last_check=?, last_error=NULL, errors=0 WHERE id=?").run(started, started, id);
        return;
      }
    }
    await runTurn(r, t, check, fresh && fresh.map(([, item]) => item));
    const seen = [...parseJson(r.seen, []), ...(fresh || []).map(([k]) => k)].slice(-SEEN_MAX);
    db.prepare("UPDATE routines SET seen=?, last_run=?, last_check=?, last_error=NULL, errors=0 WHERE id=?")
      .run(JSON.stringify(seen), started, check ? started : r.last_check ?? null, id);
  } catch (e) {
    db.prepare("UPDATE routines SET last_run=?, last_error=?, errors=COALESCE(errors,0)+1 WHERE id=?")
      .run(started, String(e?.message || e).slice(0, 500), id);
    console.error(`  routine ${r?.name || id}: ${String(e?.message || e).slice(0, 200)}`);
  } finally {
    running.delete(id);
    arm(id);
  }
}

// The agent turn, posted into the routine's own chat (the active chat is
// never touched). The chat shows a one-line note; the model reads the
// prompt plus the new items.
async function runTurn(r, thread, check, items) {
  const { backgroundTurn } = await import("./agent.mjs");
  const { choiceFor } = await import("./model.mjs");
  const shown = items ? `Routine · ${r.name} · ${items.length} new` : `Routine · ${r.name}`;
  const prompt = [
    `(Routine "${r.name}" (${r.id}) is running on its own, every ${r.every_s}s — the user isn't waiting on this message. Do what it asks and report briefly.)`,
    r.prompt || "",
    items ? `New items from ${check.server} ${check.tool} (${items.length}):\n${JSON.stringify(items, null, 1).slice(0, 40000)}` : "",
  ].filter(Boolean).join("\n\n");
  const job = { id: allocJobId(), thread: thread.id, seq: 0, phase: "queued", detail: "", done: false, result: null, routine: r.id };
  await modelCtx.run(choiceFor(thread.id), () => backgroundTurn(job, thread, shown, prompt));
  console.log(`✓ routine ${r.name} → ${thread.id}${items ? ` (${items.length} new)` : ""}`);
}

// ————— changes (endpoints and agent tools) —————
const isObj = (v) => !!v && typeof v === "object" && !Array.isArray(v);
async function toolOf(server, tool) {
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(server);
  if (!row) throw new Error(`no connected server ${server}`);
  const rec = await ensureClient(row);
  if (rec.client && !(rec.tools || []).some((t) => t.name === tool)) throw new Error(`${server} has no tool ${tool}`);
}
async function checkOf(c) {
  if (c == null) return null;
  if (typeof c !== "object") throw new Error("check must be {server, tool, args, ids}");
  const server = String(c.server || ""), tool = String(c.tool || ""), ids = String(c.ids || "").trim();
  await toolOf(server, tool);
  if (!ids) throw new Error("check.ids is required — a path like messages[].id, or a /regex/ for a text result");
  const args = isObj(c.args) ? c.args : {};
  if (c.args_from == null) return { server, tool, args, ids };
  const f = c.args_from;
  if (!isObj(f) || !f.server || !f.tool || !f.field) throw new Error("check.args_from must be {server, tool, args, field, into}");
  await toolOf(String(f.server), String(f.tool));
  const args_from = { server: String(f.server), tool: String(f.tool), args: isObj(f.args) ? f.args : {}, field: String(f.field), into: String(f.into || "query") };
  return { server, tool, args, ids, args_from };
}

export async function createRoutine({ name, thread, every_s, prompt, check, enabled = true, template = null }) {
  name = String(name || "").trim().slice(0, 80);
  if (!name) throw new Error("a routine needs a name");
  const t = findThread(String(thread || ""));
  if (!t) throw new Error(`unknown chat ${thread}`);
  if (!String(prompt || "").trim() && !check) throw new Error("a routine needs a prompt or a check");
  const c = await checkOf(check);
  const id = uuidv7();
  db.prepare(`INSERT INTO routines(id,name,thread,every_s,enabled,"check",prompt,seen,errors,created_at,template) VALUES(?,?,?,?,?,?,?,?,0,?,?)`)
    .run(id, name, t.id, clampEvery(every_s), enabled ? 1 : 0, c ? JSON.stringify(c) : null, String(prompt || "").slice(0, 8000), "[]", Date.now(), template);
  arm(id);
  return routineJson(rowOf(id));
}

/** enabled, every_s, prompt, name — omitted ones keep their values.
 *  Re-enabling clears the failure count. */
export function updateRoutine({ id, enabled, every_s, prompt, name }) {
  const r = rowOf(id);
  if (!r) throw new Error(`no routine ${id}`);
  if (enabled !== undefined) db.prepare("UPDATE routines SET enabled=?, errors=CASE WHEN ? THEN 0 ELSE errors END WHERE id=?").run(enabled ? 1 : 0, enabled ? 1 : 0, r.id);
  if (every_s !== undefined) db.prepare("UPDATE routines SET every_s=? WHERE id=?").run(clampEvery(every_s), r.id);
  if (prompt !== undefined) db.prepare("UPDATE routines SET prompt=? WHERE id=?").run(String(prompt ?? "").slice(0, 8000), r.id);
  if (name !== undefined && String(name).trim()) db.prepare("UPDATE routines SET name=? WHERE id=?").run(String(name).trim().slice(0, 80), r.id);
  arm(r.id);
  return routineJson(rowOf(r.id));
}

export function deleteRoutine(id) {
  const r = rowOf(id);
  if (!r) throw new Error(`no routine ${id}`);
  clearTimeout(timers.get(r.id));
  timers.delete(r.id);
  db.prepare("DELETE FROM routines WHERE id=?").run(r.id);
}

/** POST /routine/run {id} — tick now (Run now); resolves when it's done. */
export async function runRoutine(id) {
  const r = rowOf(id);
  if (!r) throw new Error(`no routine ${id}`);
  await tick(r.id);
  return routineJson(rowOf(r.id));
}

// ————— templates servers announce —————
// A connected server's `rox://manifest` may carry `routines`: [{id, name,
// description, every_s, needs, query:{tool, field, args?}, prompt}] (other
// fields ignored). `needs: "gmail"` makes the check a Gmail search through
// whichever connected server has a search tool below; `query` is a cheap
// tool on the announcing server whose result's `field` is that search,
// called every tick (args_from). Enabling one makes an ordinary routine.

// Gmail searches, by tool name: `query` the search argument, `ids` the
// message ids in its result, `email` the account argument (when the tool
// takes one), `read` the tool the agent reads the messages with.
const GMAIL = [
  // workspace-mcp (google-workspace): text, one "Message ID: <id>" line each
  { tool: "search_gmail_messages", query: "query", args: { page_size: 25 }, ids: "/Message ID: (\\S+)/", email: "user_google_email", read: "get_gmail_messages_content_batch" },
  // Google's Gmail MCP: JSON threads or messages
  { tool: "search_threads", query: "query", args: {}, ids: '/"(?:id|threadId)"\\s*:\\s*"([^"]+)"/', read: "get_thread" },
];
// Any other server's mail search: a search/list tool over mail, messages
// or threads that takes a query.
function gmailAdapter(tools) {
  for (const a of GMAIL) {
    const t = tools.find((x) => x.name === a.tool);
    if (t) return { ...a, schema: t.inputSchema };
  }
  for (const t of tools) {
    const props = t.inputSchema?.properties || {};
    const q = ["query", "q"].find((k) => props[k]);
    if (q && /^(search|list)_?\w*(mail|message|thread)/i.test(t.name))
      return { tool: t.name, query: q, args: {}, ids: '/"(?:id|threadId)"\\s*:\\s*"([^"]+)"/', schema: t.inputSchema };
  }
  return null;
}

// Settled within `ms`, else the fallback — a slow-starting server doesn't
// hold up the page.
const within = (p, ms, fallback) => Promise.race([p, new Promise((r) => setTimeout(() => r(fallback), ms))]);

// The connected server that can search Gmail: {row, adapter} | {reason}.
async function gmailSource() {
  let reason = "No Gmail app is connected — connect Google Workspace or Gmail in Connect Apps";
  for (const row of db.prepare("SELECT * FROM mcp_servers WHERE enabled=1 ORDER BY created_at").all()) {
    const rec = await within(ensureClient(row), 15000, {});
    const adapter = rec.client ? gmailAdapter(rec.tools || []) : null;
    if (!adapter) continue;
    const st = statusOf(row.id);
    if (st.status !== "connected") {
      reason = `${row.name || row.id} can search Gmail but ${st.status === "needs-auth" ? "needs sign-in (Connect Apps › Authenticate)" : st.err || "isn't working"}`;
      continue;
    }
    return { row, adapter };
  }
  return { reason };
}

const isEmail = (v) => typeof v === "string" && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v.trim());
/** The user's Google address for a Gmail server's account argument: the
 *  one the user gave (Enable asks once), the server's USER_GOOGLE_EMAIL,
 *  a Google plugin's GOOGLE_ACCOUNT_EMAIL setup value, the connector's
 *  account label, then the last one a tool call used (tools.mjs notes it)
 *  or a Drive file a chat recorded. "" when none is known. */
export function googleEmail(row) {
  const env = parseJson(row?.env, {}) || {};
  const settings = db.prepare("SELECT vals FROM plugin_settings").all().map((x) => parseJson(x.vals, {}) || {});
  const linked = () => {
    for (const t of store.list)
      for (const u of [...(t.turns || [])].reverse())
        for (const l of u.links || []) if (isEmail(l.account)) return l.account;
  };
  const found = [metaGet("google_email"), env.USER_GOOGLE_EMAIL, ...settings.flatMap((v) => [v.GOOGLE_ACCOUNT_EMAIL, v.USER_GOOGLE_EMAIL]), row?.account, metaGet("google_email_seen")].find(isEmail) || linked();
  return found ? found.trim() : "";
}
/** A tool call that named the user's Google address (`user_google_email`)
 *  — the fallback googleEmail() remembers. */
export function noteGoogleEmail(input) {
  if (isEmail(input?.user_google_email) && input.user_google_email.trim() !== metaGet("google_email_seen")) metaSet("google_email_seen", input.user_google_email.trim());
}

// A manifest's templates, laxly: the ones with an id and a name.
function templatesOf(m) {
  return (Array.isArray(m?.routines) ? m.routines : [])
    .filter((x) => isObj(x) && typeof x.id === "string" && x.id && typeof x.name === "string" && x.name)
    .map((x) => ({
      id: x.id, name: x.name.slice(0, 80), description: typeof x.description === "string" ? x.description : "",
      every_s: clampEvery(x.every_s || 3600), needs: typeof x.needs === "string" ? x.needs : null,
      query: isObj(x.query) && typeof x.query.tool === "string" ? x.query : null, prompt: typeof x.prompt === "string" ? x.prompt : "",
    }));
}
// Why a template can't be enabled now ("" when it can), given the Gmail source.
function unavailable(tpl, gmail) {
  if (tpl.needs && tpl.needs !== "gmail") return `Rox can't provide "${tpl.needs}" yet`;
  if (tpl.needs === "gmail" && !tpl.query) return "The template names no query tool";
  if (tpl.needs === "gmail" && !gmail.row) return gmail.reason;
  if (!tpl.needs && !tpl.prompt.trim()) return "The template has nothing to do";
  return "";
}
// Does the Gmail tool require the account argument, with none known?
const needsEmail = (gmail) =>
  !!(gmail.row && gmail.adapter.email && gmail.adapter.schema?.required?.includes(gmail.adapter.email) && !googleEmail(gmail.row));

/** GET /routines/templates — every connected server's templates:
 *  [{server, id, name, description, every_s, needs, available, reason?,
 *  needs_email?}]; needs_email: Enable has to ask for the Google address. */
export async function listTemplates() {
  const rows = db.prepare("SELECT * FROM mcp_servers WHERE enabled=1 ORDER BY created_at").all();
  const found = await Promise.all(rows.map(async (row) => ({ row, tpls: templatesOf(await within(readManifest(row).catch(() => ({})), 15000, {})) })));
  if (!found.some((f) => f.tpls.length)) return [];
  const gmail = found.some((f) => f.tpls.some((t) => t.needs === "gmail")) ? await gmailSource() : {};
  const askEmail = needsEmail(gmail);
  return found.flatMap(({ row, tpls }) =>
    tpls.map((t) => {
      const reason = unavailable(t, gmail);
      return {
        server: row.id, id: t.id, name: t.name, description: t.description, every_s: t.every_s, needs: t.needs,
        available: !reason, ...(reason ? { reason } : {}), ...(t.needs === "gmail" && askEmail ? { needs_email: true } : {}),
      };
    }));
}

/** POST /routine/from-template {server, id, thread, user_google_email?} →
 *  the routine: the template's check (the Gmail search, its query from the
 *  template's tool each tick) and prompt, in that chat. Nothing is posted.
 *  The same template in the same chat again → the routine it already has.
 *  user_google_email is kept for later ones. */
export async function routineFromTemplate({ server, id, thread, user_google_email }) {
  const t = findThread(String(thread || ""));
  if (!t) throw new Error(`unknown chat ${thread ?? ""}`);
  const key = `${server}/${id}`;
  const had = db.prepare("SELECT * FROM routines WHERE template=? AND thread=?").get(key, t.id);
  if (had) return routineJson(had);
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(String(server || ""));
  if (!row) throw new Error(`no connected server ${server}`);
  const tpl = templatesOf(await readManifest(row)).find((x) => x.id === id);
  if (!tpl) throw new Error(`${server} has no routine template ${id}`);
  if (user_google_email !== undefined) {
    if (!isEmail(user_google_email)) throw new Error(`${user_google_email} isn't an email address`);
    metaSet("google_email", String(user_google_email).trim());
  }
  const gmail = tpl.needs === "gmail" ? await gmailSource() : {};
  const reason = unavailable(tpl, gmail);
  if (reason) throw new Error(reason);
  let check = null, prompt = tpl.prompt;
  if (gmail.row) {
    const a = gmail.adapter;
    const email = a.email && a.schema?.properties?.[a.email] ? googleEmail(gmail.row) : "";
    if (needsEmail(gmail)) {
      const e = new Error("Rox needs your Google address for Gmail searches");
      e.need = "user_google_email";
      throw e;
    }
    const q = tpl.query;
    check = {
      server: gmail.row.id, tool: a.tool, args: { ...a.args, ...(email ? { [a.email]: email } : {}) }, ids: a.ids,
      args_from: { server: row.id, tool: q.tool, args: isObj(q.args) ? q.args : {}, field: typeof q.field === "string" && q.field ? q.field : "query", into: a.query },
    };
    const reader = a.read && ((await ensureClient(gmail.row)).tools || []).some((x) => x.name === a.read) ? a.read : "";
    prompt = [
      `Each new item is a Gmail message from ${gmail.row.id}'s ${a.tool}.` +
        ` Read them with ${reader ? mcpName(gmail.row.id, reader) : `${gmail.row.id}'s Gmail tools`}${email ? ` (${a.email}: ${email})` : ""}.`,
      tpl.prompt,
    ].filter(Boolean).join("\n\n");
  }
  return createRoutine({ name: tpl.name, thread: t.id, every_s: tpl.every_s, prompt, check, template: key });
}

// ————— agent tools —————
export const ROUTINE_TOOLS = [
  {
    name: "routine_create",
    description:
      "Create a routine: work you do on a timer, in THIS chat (its turns post here and nowhere else). Every every_s seconds (min 60) it ticks. " +
      "With a check, each tick first calls one connected-server tool directly — no model, no cost — and wakes you only when that tool returns items not seen before; you then get the prompt plus the new items as JSON, once per batch. Without a check, you run every tick, so prefer a check whenever a tool can tell what's new.\n" +
      "check = {server, tool, args, ids}: server is the connected server's id (mcp_<server>_<tool> names it), tool its bare tool name, args its input. ids picks the items: a JSON path such as `messages[].id` (each item of messages, identified by its id), a path ending in `[]` such as `changes[]` (an item identified by its whole JSON, so a changed item counts as new), or `/regex/` for a tool that returns text (each match's first group is an id). The string \"$last_check\" in args becomes the ISO time of the last handled check (the epoch the first time).\n" +
      "Example — \"check my BIR confirmation emails every minute for 000000000 and 123456789\": call mcp_bir_bir_confirmation_search {tins:[\"000000000\",\"123456789\"]}, which returns {query, since}; then routine_create {name:\"BIR confirmation emails\", every_s:60, " +
      "check:{server:\"google-workspace\", tool:\"search_gmail_messages\", args:{query:<that query, as is>, page_size:25, user_google_email:<the user's Google address, when you know it>}, ids:\"/Message ID: (\\\\S+)/\"}, " +
      "prompt:\"Read each new email with mcp_google-workspace_get_gmail_messages_content_batch. For each BIR confirmation, find the TIN, form, period and reference number and record it with mcp_bir_bir_confirm_filing {tin, form, period, reference, received_at, message_id}. Report which returns were confirmed and any email that matched no filing.\"}. (search_gmail_messages returns text with one \"Message ID: <id>\" line per email, hence the regex.)\n" +
      "Example — \"sync my BIR calendar to Google\" (daily): call mcp_bir_bir_calendar_changes {since:\"1970-01-01T00:00:00Z\"} once to see its result's shape, then routine_create {name:\"BIR calendar → Google Calendar\", every_s:86400, " +
      "check:{server:\"bir\", tool:\"bir_calendar_changes\", args:{since:\"$last_check\"}, ids:\"<the result's list of changes>[]\"}, " +
      "prompt:\"For each change: an added event → mcp_google-workspace_manage_event {action:\\\"create\\\", summary:<form · taxpayer>, start_time/end_time:<the due date, all day>, description:<period and status>}, then mcp_bir_bir_calendar_link {key, event_id}; a changed one → manage_event {action:\\\"update\\\", event_id}; a removed one → manage_event {action:\\\"delete\\\", event_id}. Report what changed.\"}.\n" +
      "Confirm to the user what you set up: its name, how often, and what wakes it.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "short name the user sees in Settings › Routines" },
        every_s: { type: "number", description: "seconds between ticks, 60 at least (60 = every minute, 3600 hourly, 86400 daily)" },
        prompt: { type: "string", description: "what to do when it wakes — you read it, plus the new items as JSON" },
        check: {
          type: "object",
          description: "optional: the cheap check that decides whether to wake you",
          properties: {
            server: { type: "string", description: "connected server id, e.g. google-workspace, bir" },
            tool: { type: "string", description: "the server's bare tool name, e.g. search_gmail_messages" },
            args: { type: "object", description: "the tool's input" },
            ids: { type: "string", description: "messages[].id | changes[] | /regex/ (text results)" },
            args_from: {
              type: "object",
              description: "optional {server, tool, args, field, into}: each tick, call that tool and put its result's `field` into args[into] (default \"query\") — for a query that has to be rebuilt every time",
            },
          },
          required: ["server", "tool", "ids"],
        },
      },
      required: ["name", "every_s", "prompt"],
    },
  },
  {
    name: "routine_list",
    description: "List the user's routines: id, name, chat, interval, on/off, check, last run and last error.",
    input_schema: { type: "object", properties: {} },
  },
  {
    name: "routine_update",
    description: "Change a routine (routine_list gives ids): turn it on or off, change how often it runs, its prompt or its name. Pass only what changes.",
    input_schema: {
      type: "object",
      properties: {
        id: { type: "string" },
        enabled: { type: "boolean" },
        every_s: { type: "number", description: "seconds between ticks, 60 at least" },
        prompt: { type: "string" },
        name: { type: "string" },
      },
      required: ["id"],
    },
  },
  {
    name: "routine_delete",
    description: "Delete a routine for good (routine_list gives ids). To pause one, routine_update enabled:false instead.",
    input_schema: { type: "object", properties: { id: { type: "string" } }, required: ["id"] },
  },
];
export const isRoutineTool = (name) => ROUTINE_TOOLS.some((t) => t.name === name);

const every = (s) => (s % 86400 === 0 ? `${s / 86400}d` : s % 3600 === 0 ? `${s / 3600}h` : s % 60 === 0 ? `${s / 60}m` : `${s}s`);
const line = (r, thread) =>
  `${r.id} — ${r.name} · every ${every(r.every_s)} · ${r.enabled ? "on" : "off"} · chat ${r.thread === thread?.id ? "this one" : `"${r.threadTitle}"`}` +
  `${r.check ? ` · check ${r.check.server}/${r.check.tool} (${r.seen_count} seen)` : " · no check"}` +
  `${r.last_run ? ` · last run ${new Date(r.last_run).toISOString()}` : ""}${r.last_error ? ` · error: ${r.last_error}` : ""}`;

export async function toolRoutine(thread, name, input = {}) {
  if (name === "routine_create") {
    const r = await createRoutine({ ...input, thread: thread?.id });
    return { content: `created ${line(r, thread)}; first tick ${r.next_run && r.next_run <= Date.now() ? "now" : `at ${new Date(r.next_run).toISOString()}`}` };
  }
  if (name === "routine_list") {
    const rs = listRoutines();
    return { content: rs.length ? rs.map((r) => line(r, thread)).join("\n") : "no routines yet" };
  }
  if (name === "routine_update") {
    const { id, ...patch } = input;
    return { content: `updated ${line(updateRoutine({ id, ...patch }), thread)}` };
  }
  if (name === "routine_delete") {
    deleteRoutine(input.id);
    return { content: `deleted ${input.id}` };
  }
  return { content: `unknown routine tool ${name}`, is_error: true };
}
