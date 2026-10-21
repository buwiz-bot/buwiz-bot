// state.mjs — the server's shared state: sqlite db, thread store, job
// registry, window/slot maps. Everything else imports singletons from here
// so route handlers and tools see the same objects.
import { AsyncLocalStorage } from "node:async_hooks";
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { execFile, execFileSync } from "node:child_process";
import { CHUNK_DIR, GEN_DIR, WIDGETS_DIR } from "./config.mjs";

// Model picker: a thread's choice ({ model, effort, fast }) rides an async
// context; every Claude call made for it goes to opencodex's Anthropic
// Messages endpoint with that model instead (fast = its "--fast" variant,
// effort as output_config.effort / reasoning_effort).
export const modelCtx = new AsyncLocalStorage();

// ————— live job status (polled by the app via GET /status?id=N) —————
export const jobs = new Map();
let nextJobId = 1;
export const allocJobId = () => nextJobId++;

export function setJob(job, phase, detail = "") {
  if (!job) return;
  Object.assign(job, { phase, detail, seq: job.seq + 1 });
  // a building card mirrors the job's phase — the placeholder's status line
  if (job.turn?.building) job.turn.status = detail || phase;
  // a job that ends with a card still mid-build drops the placeholder
  if (phase === "done" && job.turn?.building) {
    const turns = store.list.find((t) => t.id === job.thread)?.turns;
    const i = turns?.indexOf(job.turn) ?? -1;
    if (i >= 0) turns.splice(i, 1);
    job.turn = null;
  }
}

export function jobJson(job) {
  return JSON.stringify({
    id: job.id,
    seq: job.seq,
    phase: job.phase,
    detail: job.detail,
    done: job.done,
    result: job.result,
  });
}

// ————— the app's own database (node:sqlite, generated/app.db) —————
//   threads / turns — the chat history
//   widgets         — published copies of generated modules (the shelf)
//   kv              — per-widget persisted state, written by the widgets
//                     themselves through GET/POST /state

export const openWindows = new Map(); // "mode:name" → child process
export const deskSlots = new Map(); // name → desktop column slot index
export const shotCache = new Map(); // name → { t: number, png: Buffer }
export const shotInflight = new Map(); // name → Promise<Buffer | null>
let deskBounds = null;
export const deskBoundsGet = () => deskBounds;
export const deskBoundsSet = (v) => (deskBounds = v);

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
// Remote (streamable-HTTP) MCP servers carry url instead of command.
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN url TEXT"); } catch {}
// Per-tool off switches on a connected server (JSON array of tool names).
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN disabled_tools TEXT"); } catch {}
// OAuth client state for remote servers (JSON: client_info, tokens, verifier).
try { db.exec("ALTER TABLE mcp_servers ADD COLUMN auth TEXT"); } catch {}
// Private skills — instructions the user (or the agent for them) authored.
// Enabled bodies are injected into the agent system prompt.
try {
  db.exec("CREATE TABLE IF NOT EXISTS skills(id TEXT PRIMARY KEY, name TEXT, description TEXT, body TEXT, enabled INTEGER DEFAULT 1, created_at INTEGER)");
} catch {}
// Marketplace plugins the user installed (cloned under plugins/<name>).
try { db.exec("CREATE TABLE IF NOT EXISTS installed_plugins(name TEXT PRIMARY KEY, meta TEXT, created_at INTEGER)"); } catch {}
try { db.exec("ALTER TABLE turns ADD COLUMN v INTEGER"); } catch {}
// Grok's sidebar organization: pinned tiles, named sections, hidden bots.
for (const col of ["pinned INTEGER DEFAULT 0", "section TEXT", "hidden INTEGER DEFAULT 0", "image TEXT", "crop TEXT", "model TEXT", "effort TEXT", "fast INTEGER DEFAULT 0"]) {
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
      title: t.title,
      hidden: !!t.hidden,
      archived: !!t.archived,
      turns: db
        .prepare("SELECT * FROM turns WHERE thread_id=? ORDER BY seq")
        .all(t.id)
        .map((u) =>
          u.role === "user"
            ? { role: "user", text: u.text, ...(u.meta ? JSON.parse(u.meta) : {}) }
            : {
                role: "ai",
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
      "INSERT INTO threads(id,title,ord,hidden,archived) VALUES(?,?,?,?,?)",
    );
    const ib = db.prepare(
      "INSERT INTO bots(id,name,shape,color,thread_id,ord,pinned,section,hidden,image,crop,model,effort,fast) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    );
    const iu = db.prepare(
      `INSERT INTO turns(thread_id,seq,role,text,name,label,caption,reaction,retries,widget,meta,v)
       VALUES(?,?,?,?,?,?,?,?,?,?,?,?)`,
    );
    store.bots.forEach((b, k) =>
      ib.run(b.id, b.name, b.shape, b.color, b.threadId ?? null, k,
        b.pinned ? 1 : 0, b.section || null, b.hidden ? 1 : 0,
        b.image || null, b.crop ? JSON.stringify(b.crop) : null,
        b.model || null, b.effort || null, b.fast ? 1 : 0),
    );
    store.list.forEach((t, i) => {
      it.run(t.id, t.title, i, t.hidden ? 1 : 0, t.archived ? 1 : 0);
      t.turns.forEach((u, j) => {
        // a still-building placeholder is in-flight work, not history
        if (u.building) return;
        iu.run(
          t.id, j, u.role, u.text ?? null, u.name ?? null, u.label ?? null,
          u.caption ?? null, u.reaction ?? null, u.retries ?? 0,
          u.widget ? 1 : 0,
          u.reply || u.attachments?.length || u.files?.length || u.art
            ? JSON.stringify({ reply: u.reply, attachments: u.attachments, files: u.files, art: u.art })
            : null,
          u.v ?? null,
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
  const art = (t) => {
    const a = t.art || {};
    const title = (t.name && appTitles.get(t.name)) || a.title || null;
    if (!title && !a.desc && !a.w && !a.h) return undefined;
    return { title, desc: a.desc || "", w: a.w || 0, h: a.h || 0 };
  };
  return thread.turns.map((t, i) => ({
    i,
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
    files: t.files || [],
    v: t.v ?? null,
    art: art(t),
    building: t.building || undefined,
    status: t.status || undefined,
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
