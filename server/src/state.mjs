// state.mjs — the server's shared state: sqlite db, thread store, job
// registry, window/slot maps. Everything else imports singletons from here
// so route handlers and tools see the same objects.
import { AsyncLocalStorage } from "node:async_hooks";
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { execFile, execFileSync } from "node:child_process";
import { randomFillSync } from "node:crypto";
import { CHUNK_DIR, GEN_DIR, WIDGETS_DIR } from "./config.mjs";
import { driveType } from "./drive.mjs";

// Model picker: a thread's choice ({ model, effort, fast }) rides an async
// context; every Claude call made for it goes to opencodex's Anthropic
// Messages endpoint with that model instead (fast = its "--fast" variant,
// effort as output_config.effort / reasoning_effort).
export const modelCtx = new AsyncLocalStorage();

// Permanent ids: UUID v7 (RFC 9562) — 48-bit unix-ms timestamp, version 7,
// variant 10, the rest random. rand_a (12 bits) counts up within the same
// millisecond so ids minted back-to-back stay monotonic. Threads carry `uid`
// next to their "t<n>" id; turns carry `id` next to their index `i`.
let v7Ms = -1;
let v7Ctr = 0;
export function uuidv7(ms = Date.now()) {
  ms = Math.max(0, Math.floor(ms));
  const b = randomFillSync(Buffer.alloc(16));
  let ctr = ((b[6] & 0x0f) << 8) | b[7];
  if (ms > v7Ms) {
    v7Ms = ms;
    v7Ctr = ctr &= 0x7ff; // leave headroom to count up
  } else if (ms === v7Ms) {
    if (v7Ctr >= 0xfff) {
      ms = v7Ms = ms + 1;
      v7Ctr = 0;
    } else v7Ctr++;
    ctr = v7Ctr;
  }
  b.writeUIntBE(ms, 0, 6);
  b[6] = 0x70 | (ctr >> 8);
  b[7] = ctr & 0xff;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = b.toString("hex");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
const v7Time = (id) =>
  typeof id === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-7/i.test(id)
    ? parseInt(id.replace(/-/g, "").slice(0, 12), 16)
    : undefined;

// ————— live job status (polled by the app via GET /status?id=N) —————
export const jobs = new Map();
let nextJobId = 1;
export const allocJobId = () => nextJobId++;

export function setJob(job, phase, detail = "") {
  if (!job) return;
  Object.assign(job, { phase, detail, seq: job.seq + 1 });
  // A short history, so a poller sees a status that lasted less than its
  // interval (a fast tool step) instead of only the latest.
  (job.events ||= []).push({ seq: job.seq, phase, detail });
  if (job.events.length > 30) job.events.shift();
  // a building card mirrors the job's phase — the placeholder's status line
  if (job.turn?.building) job.turn.status = detail || phase;
  // a job that ends mid-revise lets the revised card show again
  if (phase === "done") clearRevising(job);
  // a job that ends with a card still mid-build drops the placeholder
  if (phase === "done" && job.turn?.building) {
    const turns = store.list.find((t) => t.id === job.thread)?.turns;
    const i = turns?.indexOf(job.turn) ?? -1;
    if (i >= 0) turns.splice(i, 1);
    job.turn = null;
  }
}

// A revise (revise_app, a reply-to-iterate) flags the card it revises
// `revising` while the new version builds: Rox covers that card's body with
// the generation placeholder. One card per job; never persisted.
export function cardTurn(thread, name, v) {
  const cards = (thread?.turns || []).filter((t) => t.role === "ai" && t.name === name && !t.building);
  return (v != null && cards.findLast((t) => t.v === v)) || cards.at(-1) || null;
}
export function markRevising(job, turn) {
  if (!job || job.revising === turn) return;
  clearRevising(job);
  if (!turn) return;
  turn.revising = true;
  job.revising = turn;
}
export function clearRevising(job) {
  if (!job?.revising) return;
  delete job.revising.revising;
  job.revising = null;
}

export function jobJson(job) {
  return JSON.stringify({
    id: job.id,
    seq: job.seq,
    phase: job.phase,
    detail: job.detail,
    done: job.done,
    result: job.result,
    events: job.events || [],
    // a mini app a server asked to show ({key, title}, tools.mjs)
    open: job.openApp || null,
  });
}

// ————— the app's own database (node:sqlite, generated/app.db) —————
//   threads / turns — the chat history
//   widgets         — published copies of generated modules (the shelf)
//   kv              — per-widget persisted state, written by the widgets
//                     themselves through GET/POST /state

export const openWindows = new Map(); // "name[@vK]" → child process

export const db = new DatabaseSync(join(GEN_DIR, "app.db"));
db.exec(`
  CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
  CREATE TABLE IF NOT EXISTS threads(
    id TEXT PRIMARY KEY, title TEXT NOT NULL, ord INTEGER NOT NULL DEFAULT 0);
  CREATE TABLE IF NOT EXISTS turns(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id TEXT NOT NULL, seq INTEGER NOT NULL, role TEXT NOT NULL,
    text TEXT, name TEXT, label TEXT, caption TEXT, reaction TEXT,
    retries INTEGER DEFAULT 0, widget INTEGER DEFAULT 0);
  CREATE TABLE IF NOT EXISTS widgets(
    name TEXT PRIMARY KEY, title TEXT, prompt TEXT, published_at TEXT);
  CREATE TABLE IF NOT EXISTS bots(
    id TEXT PRIMARY KEY, name TEXT NOT NULL, shape TEXT NOT NULL,
    color TEXT NOT NULL, thread_id TEXT, ord INTEGER NOT NULL DEFAULT 0);
  CREATE TABLE IF NOT EXISTS kv(name TEXT PRIMARY KEY, data TEXT);
  CREATE TABLE IF NOT EXISTS apps(name TEXT PRIMARY KEY, title TEXT, file TEXT);
  -- Artifacts the user deleted: turns keep naming them (the card shows the
  -- "module missing" tombstone) but Library must never list them again.
  CREATE TABLE IF NOT EXISTS tombstones(name TEXT PRIMARY KEY);
  -- One artifact, a version tree: generated/<name>/v<k>.tsx is immutable;
  -- heads is the "current" pointer (revert moves it, never rewrites).
  CREATE TABLE IF NOT EXISTS versions(
    name TEXT NOT NULL, v INTEGER NOT NULL, parent INTEGER,
    prompt TEXT, created_at INTEGER, PRIMARY KEY(name, v));
  CREATE TABLE IF NOT EXISTS heads(name TEXT PRIMARY KEY, v INTEGER NOT NULL);
  -- User-connected MCP servers: bot NULL = every bot sees the tools,
  -- bot=<id> scopes them to that bot's thread.
  CREATE TABLE IF NOT EXISTS mcp_servers(
    id TEXT PRIMARY KEY, name TEXT, command TEXT, args TEXT, env TEXT,
    enabled INTEGER DEFAULT 1, bot TEXT, created_at INTEGER, url TEXT);
  CREATE TABLE IF NOT EXISTS timeclock(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    clock_in TEXT NOT NULL, clock_out TEXT);
`);
try { db.exec("ALTER TABLE turns ADD COLUMN meta TEXT"); } catch {}
// A branch may carry an optional name (e.g. "slow-ball") — versions.label.
try { db.exec("ALTER TABLE versions ADD COLUMN label TEXT"); } catch {}
// Mini app details: description, generated/adopted icon, size, edit time.
for (const col of ["description TEXT", "icon TEXT", "icon_prompt TEXT", "w REAL", "h REAL", "updated_at INTEGER"])
  try { db.exec(`ALTER TABLE apps ADD COLUMN ${col}`); } catch {}
// The user's edits to a Library image or file (apps keep theirs in `apps`):
// id is R<n> or file:<path>; `file` is a display name — never a rename on
// disk. library_hidden takes an artifact out of the Library only (its file
// stays on disk, its card in the chat).
db.exec(`
  CREATE TABLE IF NOT EXISTS artifact_meta(
    id TEXT PRIMARY KEY, title TEXT, file TEXT, description TEXT, icon TEXT, updated_at INTEGER);
  CREATE TABLE IF NOT EXISTS library_hidden(id TEXT PRIMARY KEY, at INTEGER);
`);
// `@` tokens attached to a chat or bot (attachments.mjs) and the display
// names the user gave tokens.
db.exec(`
  CREATE TABLE IF NOT EXISTS attachments(
    id TEXT PRIMARY KEY, server TEXT NOT NULL, token TEXT NOT NULL, scope TEXT,
    title TEXT, open TEXT, key TEXT, details TEXT, thread TEXT NOT NULL, bot TEXT,
    turn TEXT, created_at INTEGER);
  CREATE TABLE IF NOT EXISTS attachment_names(token TEXT PRIMARY KEY, title TEXT, updated_at INTEGER);
`);
// Remote (streamable-HTTP) MCP servers carry url instead of command.
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN url TEXT"); } catch {}
// Per-tool off switches on a connected server (JSON array of tool names).
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN disabled_tools TEXT"); } catch {}
// OAuth client state for remote servers (JSON: client_info, tokens, verifier).
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN auth TEXT"); } catch {}
// A plugin connector's extra accounts are rows sharing its name; `account`
// is the user's label for one (NULL on the first row = "default").
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN account TEXT"); } catch {}
// Catalog plugin connectors: static request headers ({name: value}, from
// the plugin's MCP config after ${KEY} substitution), a pre-registered OAuth
// client ({client_id, client_secret?, scope?} — servers without dynamic
// client registration) and the transport ("sse" for legacy SSE servers).
for (const col of ["headers TEXT", "oauth_client TEXT", "transport TEXT"])
  try { db.exec(`ALTER TABLE mcp_servers ADD COLUMN ${col}`); } catch {}
// 1 on a server the agent installed (mcp_install) that the user has not yet
// switched on: it stays off and unspawned, and the agent can't enable it.
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN agent_pending INTEGER"); } catch {}
// Per-plugin setup values (the catalog's `fields`), keyed by catalog key;
// substituted into ${KEY} references in the plugin's MCP config.
try { db.exec("CREATE TABLE IF NOT EXISTS plugin_settings(plugin TEXT PRIMARY KEY, vals TEXT, updated_at INTEGER)"); } catch {}
// Private skills — instructions the user (or the agent for them) authored.
// Enabled bodies are injected into the agent system prompt.
try {
  db.exec("CREATE TABLE IF NOT EXISTS skills(id TEXT PRIMARY KEY, name TEXT, description TEXT, body TEXT, enabled INTEGER DEFAULT 1, created_at INTEGER)");
} catch {}
// Marketplace plugins the user installed (cloned under plugins/<name>).
try { db.exec("CREATE TABLE IF NOT EXISTS installed_plugins(name TEXT PRIMARY KEY, meta TEXT, created_at INTEGER)"); } catch {}
try { db.exec("ALTER TABLE turns ADD COLUMN v INTEGER"); } catch {}
// Permanent UUID v7 ids (turns.id is a per-persist rowid, not stable).
try { db.exec("ALTER TABLE threads ADD COLUMN uid TEXT"); } catch {}
try { db.exec("ALTER TABLE turns ADD COLUMN uid TEXT"); } catch {}
// Grok's sidebar organization: pinned tiles, named sections, hidden bots.
for (const col of ["pinned INTEGER DEFAULT 0", "section TEXT", "hidden INTEGER DEFAULT 0", "image TEXT", "crop TEXT", "model TEXT", "effort TEXT", "fast INTEGER DEFAULT 0", "label TEXT",
  // A bot's own context, filled on demand: description + instructions
  // (seeded on create, the bot edits them), memories [{id,text,at}] and
  // integrations [{id,kind,at,via}] as JSON arrays.
  "description TEXT", "instructions TEXT", "memories TEXT", "integrations TEXT"]) {
  try { db.exec(`ALTER TABLE bots ADD COLUMN ${col}`); } catch {}
}
// Chat rows carry Grok's sidebar states too: hidden, archived.
for (const col of ["hidden INTEGER DEFAULT 0", "archived INTEGER DEFAULT 0"]) {
  try { db.exec(`ALTER TABLE threads ADD COLUMN ${col}`); } catch {}
}

export const metaGet = (k) => db.prepare("SELECT v FROM meta WHERE k=?").get(k)?.v;
export const metaSet = (k, v) =>
  db.prepare("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)").run(k, v);

// A module's display filename (Library rows, /app/meta renames): kebab-case
// plus ".tsx", or "" when nothing usable is left. Never the real file —
// R<n>.tsx stays the module's name on disk and in imports.
export const slugFile = (s) => {
  const slug = String(s || "")
    .trim()
    .replace(/\.tsx$/i, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/g, "");
  return /^[a-z0-9][a-z0-9-]{0,39}$/.test(slug) ? `${slug}.tsx` : "";
};
// Defaults a new bot starts with — hidden from the user, rewritten by the
// bot itself (bot_profile_update) when asked.
export const botDefaults = (name) => ({
  description: `${name} is a personal assistant bot in Grok Shell.`,
  instructions:
    `Stay in character as ${name}. Be helpful, concise and friendly; ask one short clarifying question when a request is ambiguous.`,
});
const jsonList = (s) => {
  try {
    const v = JSON.parse(s || "[]");
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
};

function loadStore() {
  const s = {
    active: metaGet("active") || "t1",
    seq: Number(metaGet("seq") || 0),
    list: [],
    bots: [],
  };
  for (const t of db.prepare("SELECT * FROM threads ORDER BY ord").all())
    s.list.push({
      id: t.id,
      ...(t.uid ? { uid: t.uid } : {}),
      title: t.title,
      hidden: !!t.hidden,
      archived: !!t.archived,
      turns: db
        .prepare("SELECT * FROM turns WHERE thread_id=? ORDER BY seq")
        .all(t.id)
        .map((u) =>
          u.role === "user"
            ? { role: "user", ...(u.uid ? { id: u.uid } : {}), text: u.text, ...(u.meta ? JSON.parse(u.meta) : {}) }
            : {
                role: "ai",
                ...(u.uid ? { id: u.uid } : {}),
                text: u.text ?? null,
                name: u.name,
                label: u.label || "",
                caption: u.caption || "",
                reaction: u.reaction || "",
                retries: u.retries || 0,
                v: u.v ?? null,
                widget: !!u.widget,
                ...(u.meta ? JSON.parse(u.meta) : {}),
              },
        ),
    });
  for (const b of db.prepare("SELECT * FROM bots ORDER BY ord").all())
    s.bots.push({
      id: b.id,
      name: b.name,
      shape: b.shape,
      color: b.color,
      threadId: b.thread_id,
      pinned: !!b.pinned,
      section: b.section || "",
      hidden: !!b.hidden,
      image: b.image || "",
      crop: b.crop ? JSON.parse(b.crop) : null,
      model: b.model || "",
      effort: b.effort || "",
      fast: !!b.fast,
      label: b.label || "",
      // NULL = never set (a bot made before contexts existed) → defaults;
      // "" = the bot deliberately cleared it.
      description: b.description ?? botDefaults(b.name).description,
      instructions: b.instructions ?? botDefaults(b.name).instructions,
      memories: jsonList(b.memories),
      integrations: jsonList(b.integrations),
    });
  s.sections = JSON.parse(metaGet("sections") || "[]");
  return s;
}

// First boot: import the JSON files the older server wrote, then the DB is
// the source of truth (atomic multi-statement writes, no more torn JSON).
const LEGACY_STATE = join(GEN_DIR, "threads.json");
const LEGACY_TURNS = join(GEN_DIR, "turns.json");
export let store = loadStore();
if (store.list.length === 0 && existsSync(LEGACY_STATE)) {
  store = JSON.parse(readFileSync(LEGACY_STATE, "utf8"));
  console.log(`migrated threads.json → app.db (${store.list.length} threads)`);
} else if (store.list.length === 0) {
  store = {
    active: "t1",
    seq: 0,
    bots: [],
    list: [
      {
        id: "t1",
        uid: uuidv7(),
        title: "first chat",
        turns: existsSync(LEGACY_TURNS)
          ? JSON.parse(readFileSync(LEGACY_TURNS, "utf8"))
          : [],
      },
    ],
  };
}

// Files with HTTP byte ranges: AVPlayer (nsh's Video) refuses media from a
// server without them ("Operation Stopped").
export function sendRanged(req, res, path, type) {
  const buf = readFileSync(path);
  res.setHeader("content-type", type);
  res.setHeader("accept-ranges", "bytes");
  const m = /bytes=(\d*)-(\d*)/.exec(req.headers.range || "");
  if (!m) {
    res.setHeader("content-length", buf.length);
    return res.end(buf);
  }
  const start = m[1] ? +m[1] : Math.max(0, buf.length - +m[2]);
  const end = m[1] && m[2] ? Math.min(+m[2], buf.length - 1) : buf.length - 1;
  if (start >= buf.length || start > end) {
    res.statusCode = 416;
    res.setHeader("content-range", `bytes */${buf.length}`);
    return res.end();
  }
  res.statusCode = 206;
  res.setHeader("content-range", `bytes ${start}-${end}/${buf.length}`);
  res.setHeader("content-length", end - start + 1);
  return res.end(buf.subarray(start, end + 1));
}

export function persist() {
  db.exec("BEGIN");
  try {
    db.exec("DELETE FROM threads");
    db.exec("DELETE FROM turns");
    db.exec("DELETE FROM bots");
    const it = db.prepare(
      "INSERT INTO threads(id,title,ord,hidden,archived,uid) VALUES(?,?,?,?,?,?)",
    );
    const ib = db.prepare(
      "INSERT INTO bots(id,name,shape,color,thread_id,ord,pinned,section,hidden,image,crop,model,effort,fast,label,description,instructions,memories,integrations) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    );
    const iu = db.prepare(
      `INSERT INTO turns(thread_id,seq,role,text,name,label,caption,reaction,retries,widget,meta,v,uid)
       VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)`,
    );
    store.bots.forEach((b, k) =>
      ib.run(b.id, b.name, b.shape, b.color, b.threadId ?? null, k,
        b.pinned ? 1 : 0, b.section || null, b.hidden ? 1 : 0,
        b.image || null, b.crop ? JSON.stringify(b.crop) : null,
        b.model || null, b.effort || null, b.fast ? 1 : 0, b.label || null,
        b.description ?? null, b.instructions ?? null,
        JSON.stringify(b.memories || []), JSON.stringify(b.integrations || [])),
    );
    store.list.forEach((t, i) => {
      it.run(t.id, t.title, i, t.hidden ? 1 : 0, t.archived ? 1 : 0, (t.uid ||= uuidv7()));
      t.turns.forEach((u, j) => {
        // a still-building placeholder is in-flight work, not history
        if (u.building) return;
        iu.run(
          t.id, j, u.role, u.text ?? null, u.name ?? null, u.label ?? null,
          u.caption ?? null, u.reaction ?? null, u.retries ?? 0,
          u.widget ? 1 : 0,
          u.reply || u.attachments?.length || u.files?.length || u.art || u.links?.length || u.suggest?.length || u.v1Attachments?.length
            ? JSON.stringify({
              reply: u.reply, attachments: u.attachments, files: u.files, art: u.art, links: u.links,
              ...(u.suggest?.length ? { suggest: u.suggest } : {}), ...(u.v1Attachments?.length ? { v1Attachments: u.v1Attachments } : {}),
            })
            : null,
          u.v ?? null,
          (u.id ||= uuidv7()),
        );
      });
    });
    metaSet("active", store.active);
    metaSet("sections", JSON.stringify(store.sections || []));
    metaSet("seq", String(seq));
    db.exec("COMMIT");
  } catch (e) {
    db.exec("ROLLBACK");
    throw e;
  }
}
// Module names must never collide with files already on disk — even files a
const maxOnDisk = Math.max(
  0,
  ...readdirSync(GEN_DIR)
    .map((f) => /^R(\d+)\.tsx$/.exec(f))
    .filter(Boolean)
    .map((m) => Number(m[1])),
);
export let seq = Math.max(store.seq || 0, maxOnDisk);
store.seq = seq;
export let threadSeq =
  Math.max(0, ...store.list.map((t) => Number(t.id.slice(1)) || 0)) + 1;
export const activeThread = () =>
  store.list.find((t) => t.id === store.active) || store.list[0];
// A thread by its "t<n>" id or its permanent uid.
export const findThread = (key) =>
  key ? store.list.find((t) => t.id === key || t.uid === key) : undefined;

// Backfill uids/ids for rows written before they existed, then persist so
// they stay stable. Timestamps: an id that already exists keeps its own; an
// artifact turn uses its version's created_at; everything else falls in
// creation order (thread number, then turn order) right after the previous
// known time. Idempotent — only rows missing an id are touched.
function backfillIds() {
  if (!store.list.some((t) => !t.uid || t.turns.some((u) => !u.id))) return false;
  const vt = new Map();
  for (const r of db.prepare("SELECT name,v,created_at FROM versions WHERE created_at IS NOT NULL").all()) {
    const ms = Number(r.created_at);
    if (!Number.isFinite(ms) || ms <= 0) continue;
    vt.set(`${r.name}@${r.v}`, ms);
    if (!(vt.get(r.name) <= ms)) vt.set(r.name, ms);
  }
  const items = [];
  const byAge = [...store.list].sort(
    (a, b) => (Number(a.id.slice(1)) || 0) - (Number(b.id.slice(1)) || 0),
  );
  for (const t of byAge) {
    items.push({ t, known: v7Time(t.uid) });
    for (const u of t.turns)
      items.push({
        u,
        known: v7Time(u.id) ?? (u.name ? vt.get(`${u.name}@${u.v}`) ?? vt.get(u.name) : undefined),
      });
  }
  const first = items.findIndex((x) => x.known != null);
  let prev = first < 0 ? Date.now() - items.length - 1 : items[first].known - first - 1;
  for (const x of items) {
    prev = x.known != null && x.known > prev ? x.known : prev + 1;
    if (x.t && !x.t.uid) x.t.uid = uuidv7(prev);
    if (x.u && !x.u.id) x.u.id = uuidv7(prev);
  }
  return true;
}
if (backfillIds()) persist();

// ————— data sources —————
// Generated widgets fetch these on 127.0.0.1:8787 — the only host the
// sandbox grants — and the server holds any credentials (Linear key via
// `secret`), so secrets never touch generated code.
export const nextSeq = () => ++seq;
export const nextThreadSeq = () => threadSeq++;

let linearKey;
export const getLinearKey = () => {
  if (!linearKey) {
    linearKey = process.env.LINEAR_API_KEY || null;
    if (!linearKey)
      try {
        linearKey = execFileSync("secret", ["get", "LINEAR_API_KEY"], {
          encoding: "utf8",
        }).trim() || null;
      } catch {}
  }
  return linearKey;
};

// Run a JXA script; env vars carry arguments (no quoting bugs). Returns parsed
// JSON or {error}.
export const jxa = (script, env = {}, timeout = 15000) => {
  try {
    const out = execFileSync("osascript", ["-l", "JavaScript", "-e", script], {
      env: { ...process.env, ...env },
      encoding: "utf8",
      timeout,
    }).trim();
    return JSON.parse(out || "null");
  } catch (e) {
    return {
      error: String(e?.message || e).split("\n")[0],
      hint: "grant permission in System Settings → Privacy & Security → Reminders / Automation",
    };
  }
};

export const dirOf = (t) => (t.widget ? WIDGETS_DIR : GEN_DIR);

// The turns the app renders — plain JSON so a thread switch is a data swap,
// not a code rebuild. `i` is the turn's index (DelBtn keys off it).
export function turnsJson(thread = activeThread()) {
  // `art` rides the turn; its title is resolved live from the apps table so
  // a rename shows on every card of that artifact without rewriting turns.
  const appTitles = new Map(
    db.prepare("SELECT name,title FROM apps").all().map((r) => [r.name, r.title]),
  );
  // An image's or file's edited title / display name (Edit Details).
  const edits = new Map(
    db.prepare("SELECT id,title,file FROM artifact_meta").all().map((r) => [r.id, r]),
  );
  const renamed = new Map(
    db.prepare("SELECT token,title FROM attachment_names").all().map((r) => [r.token, r.title]),
  );
  const art = (t) => {
    // An image card made before cards carried their file: read it from the
    // module once (`asset:/generated/assets/<file>`).
    if (t.label === "image" && t.name && !t.art?.kind) {
      try {
        const file = readFileSync(join(GEN_DIR, `${t.name}.tsx`), "utf8").match(/asset:\/generated\/assets\/([\w.-]+)/)?.[1];
        if (file) t.art = { ...(t.art || {}), kind: "image", file };
      } catch {}
    }
    const a = t.art || {};
    // A later version of a card without its own title (an image edit) shows
    // the card's.
    const title = (t.name && (edits.get(t.name)?.title || appTitles.get(t.name))) || a.title
      || (t.name && thread.turns.find((x) => x.name === t.name && x.art?.title)?.art.title) || null;
    if (!title && !a.desc && !a.w && !a.h) return undefined;
    return { title, desc: a.desc || "", w: a.w || 0, h: a.h || 0, ...(a.kind ? { kind: a.kind, file: a.file || "" } : {}) };
  };
  return thread.turns.map((t, i) => ({
    i,
    id: (t.id ||= uuidv7()),
    role: t.role,
    text: t.text || "",
    name: t.name || "",
    label: t.label || "",
    caption: t.caption || "",
    reaction: t.reaction || "",
    retries: t.retries || 0,
    widget: !!t.widget,
    reply: t.reply || null,
    attachments: t.attachments || [],
    // a file card shows its edited display name
    files: (t.files || []).map((f) => {
      const name = f?.path && edits.get(`file:${f.path}`)?.file;
      return name ? { ...f, name } : f;
    }),
    // Drive files the reply made or changed (drive.mjs), as chips; a
    // server's mini app (`mcp-app`: a `rox/open` or an attachment's card)
    // keeps its own type, and an attachment's shows its token's display name
    links: (t.links || []).map(({ account, ...l }) => ({
      ...l,
      ...(l.token && renamed.get(l.token) ? { name: renamed.get(l.token) } : {}),
      ...(l.provider === "mcp-app" ? { kind: "app", type: l.file_type || "Mini app" } : driveType(l.mime, l.name)),
    })),
    v: t.v ?? null,
    // 9.8 next steps under the answer, and the v1 attachments a message
    // went to the model with (10.3)
    suggest: t.suggest || [],
    ...(t.v1Attachments?.length ? { v1Attachments: t.v1Attachments } : {}),
    art: art(t),
    building: t.building || undefined,
    status: t.status || undefined,
    revising: t.revising || undefined,
  }));
}

// generated/index.tsx is a STABLE registry: every module on disk gets one
// import + one map entry, and the file is only rewritten when the set of
// modules changes — never on a thread switch or a new text turn. That keeps
// the conversation a data-driven view: no bundle rebuild, no blank flash.
let lastModuleKey = "";
export function writeIndex() {
  const gen = readdirSync(GEN_DIR)
    .filter((f) => /^R\d+\.tsx$/.test(f) && f !== "index.tsx")
    .map((f) => f.slice(0, -4));
  const pubs = readdirSync(WIDGETS_DIR)
    .filter((f) => /^R\d+\.tsx$/.test(f) && f !== "index.tsx")
    .map((f) => f.slice(0, -4));
  // Every version on every artifact's tree is addressable: a card pins its
  // own "R6@v3" module, while the bare "R6" key is always the current one.
  // Version files are <slug>.v<n>.tsx inside generated/<name>/.
  const vers = gen.flatMap((n) => {
    try {
      return readdirSync(join(GEN_DIR, n)).flatMap((f) => {
        const m = f.match(/\.v(\d+)\.tsx$/);
        return m ? [{ n, v: Number(m[1]), file: f.slice(0, -4) }] : [];
      });
    } catch {
      return [];
    }
  });
  const key =
    [...gen].sort().join(",") +
    "|" +
    [...pubs].sort().join(",") +
    "|" +
    vers.map((x) => `${x.n}@${x.file}`).sort().join(",");
  if (key !== lastModuleKey) {
    lastModuleKey = key;
    const imports = [
      ...gen.map((n) => `import ${n} from "./${n}";`),
      ...vers.map((x) => `import ${x.n}v${x.v} from "./${x.n}/${x.file}";`),
      // Published widgets get a w-suffix key so a module that exists in both
      // dirs can appear in both turn kinds without a duplicate binding.
      ...pubs.map((n) => `import ${n}w from "../widgets/${n}";`),
    ].join("\n");
    const map = [
      ...gen.map((n) => `  ${JSON.stringify(n)}: ${n},`),
      ...vers.map((x) => `  ${JSON.stringify(`${x.n}@v${x.v}`)}: ${x.n}v${x.v},`),
      ...pubs.map((n) => `  ${JSON.stringify(n + "w")}: ${n}w,`),
    ].join("\n");
    writeFileSync(
      join(GEN_DIR, "index.tsx"),
      `${imports}\n\n// Regenerated by server/app-server.mjs — module registry only.\n\nexport const MODULES: Record<string, any> = {\n${map}\n};\n`,
    );
  }
  store.seq = seq;
  persist();
}

// Regenerate widgets/index.tsx — the Mini Apps tab's shelf. The file is
// STATIC: it renders metadata cards from GET /catalog and never mounts a
// live widget, so the tab stays light no matter how many apps exist.
// A mini-app runs where the user chooses: ↗ pops its own window, ＋chat
// writeShelf lives in templates.mjs — its body was a 480-line embedded TSX
// template, now a real file read at call time.
export function writeShelf() {
  writeFileSync(join(WIDGETS_DIR, "index.tsx"), readFileSync(new URL("./templates/shelf.tsx", import.meta.url), "utf8"));
}

export const widgetRow = (name) =>
  db.prepare("SELECT * FROM widgets WHERE name=?").get(name);
