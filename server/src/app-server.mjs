// app-server.mjs — the "AI" behind the generative-UI chat.
//
//   node server/app-server.mjs            # uses OPENROUTER_API_KEY if set
//   MOCK=1 node server/app-server.mjs     # canned modules, no network needed
//
// POST http://127.0.0.1:8787/chat {"prompt": "…"}
//
// Flow per message: record the user turn → the model runs a tool loop
// (generate_app, generate_image, web_search, fetch_url, query_source) →
// each widget is a generated/r<N>.tsx file typechecked in server/.stage →
// generated/index.tsx regenerates → nsh's dev server hot-patches the app, so
// cards appear as real native UI. Plain questions answer in text — UI is one
// tool among several, not the only reply format.

import { createServer } from "node:http";
import { AsyncLocalStorage } from "node:async_hooks";
import { fileMeta, highlight, mockDocx, mockFiles, mockPptx, mockXlsx, renderFile } from "./files.mjs";
import { callOfficeTool, ensureOfficeMcp, officeCaps, officeCreateBytes, officeMarkdown, officeMcpTools, officePkgRoot } from "./office.mjs";
import { anthropicTools as mcpAnthropicTools, callTool as mcpCallTool, ensureClient as mcpEnsure, invalidate as mcpInvalidate, parseMcpName, statusOf as mcpStatus } from "./mcp.mjs";
import { execFile, execFileSync, spawn, spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  rmSync,
  readFileSync,
  statSync,
  readdirSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import Anthropic from "@anthropic-ai/sdk";

const APP_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
// Packaged mode: GROK_DATA_DIR points at a writable dir (~/Library/Application
// Support/Grok Shell) — generated/, widgets/, windows/, .stage all live there
// instead of inside the read-only .app bundle. widgets/ is seeded from the
// shipped copy on first launch (the launcher does the copy).
const DATA_DIR = process.env.GROK_DATA_DIR || APP_ROOT;
const GEN_DIR = join(DATA_DIR, "generated");
// Candidates are compiled+typechecked here first — OUTSIDE the app's import
// graph — so a bad module never reaches the dev server (no red overlay flash).
const STAGE_DIR = join(DATA_DIR, ".stage");
const PORT = Number(process.env.PORT || 8787);
// Providers: Claude goes through opencodex's Anthropic Messages endpoint by
// default (OPENCODEX_URL — it owns the real key; the client key is a dummy).
// GEN_PROVIDER=openai + GEN_BASE_URL/GEN_MODEL selects an OpenAI-compatible
// backend instead. Direct-to-Anthropic escape hatch: set ANTHROPIC_BASE_URL
// (+ ANTHROPIC_API_KEY) explicitly — never inferred, so a stray env key can
// no longer silently bypass the gateway.
const PROVIDER = process.env.GEN_PROVIDER || "anthropic";
const BASE_URL =
  process.env.GEN_BASE_URL || "https://openrouter.ai/api/v1";
const MODEL =
  process.env.GEN_MODEL ||
  (PROVIDER === "anthropic" ? "claude-haiku-5-5" : "openai/gpt-4o-mini");
// Output budget covers interleaved thinking (~5k) plus a real module — a
// polished game runs 400+ lines; 6k truncated mid-file and produced mocks.
const MAX_TOKENS = Number(process.env.GEN_MAX_TOKENS || 20000);
const MOCK = !!process.env.MOCK;
// nsh binary for stage typechecks and pop-outs: $NSH, else `nsh` on PATH.
// Needs the patched fork (Window.setFrame/setLevel, onContextMenu —
// upstream longbridge/nsh lacks them); put your build on PATH or point NSH
// (and NSH_HOST for the dev-host binary) at it.
const NSH = process.env.NSH || "nsh";
const NSH_HOST =
  process.env.NSH_HOST ||
  (NSH.includes("/") ? join(dirname(NSH), "nsh-macos-host") : "nsh-macos-host");
// Bundle id the packaged app and its pop-out windows share (nsh.json's "id").
const BUNDLE_ID = process.env.GROK_BUNDLE_ID || "dev.goldcoders.grokshell";
// Model picker: a thread's choice ({ model, effort, fast }) rides an async
// context; every Claude call made for it goes to opencodex's Anthropic
// Messages endpoint with that model instead (fast = its "--fast" variant,
// effort as output_config.effort / reasoning_effort).
const OPENCODEX = process.env.OPENCODEX_URL || "http://localhost:8080";
const anthropicDirect =
  PROVIDER === "anthropic"
    ? new Anthropic({
        baseURL: process.env.ANTHROPIC_BASE_URL || OPENCODEX,
        apiKey: process.env.ANTHROPIC_API_KEY || "opencodex",
      })
    : null;
const modelCtx = new AsyncLocalStorage();
let opencodex = null;
const routed = (params) => {
  const c = modelCtx.getStore();
  if (!c?.model) return [anthropicDirect, params];
  opencodex ??= new Anthropic({ baseURL: OPENCODEX, apiKey: "opencodex" });
  const p = { ...params, model: c.model + (c.fast ? "--fast" : "") };
  if (c.effort) {
    p.output_config = { effort: c.effort };
    p.reasoning_effort = c.effort;
  }
  return [opencodex, p];
};
const anthropic = anthropicDirect && {
  messages: {
    create: (p, o) => { const [c, q] = routed(p); return c.messages.create(q, o); },
    stream: (p, o) => { const [c, q] = routed(p); return c.messages.stream(q, o); },
  },
};
const choiceFor = (threadId) => {
  const b = store.bots.find((x) => x.threadId === threadId);
  const m = metaGet("model");
  const base = b?.model
    ? { model: b.model, effort: b.effort || "", fast: !!b.fast }
    : m
      ? { model: m, effort: metaGet("effort") || "", fast: metaGet("fast") === "1" }
      : null;
  // Status labels credit the bot, not the engine: "Grok is thinking" in a
  // bot's thread, "claude-haiku-5-5 is thinking" in a plain session.
  return b?.name ? { ...(base || {}), name: b.name } : base;
};
const labelOf = (c) =>
  c?.name || (c?.model ? c.model + (c.fast ? "--fast" : "") : MODEL);

mkdirSync(GEN_DIR, { recursive: true });
mkdirSync(STAGE_DIR, { recursive: true });
// Stale stage files (crashed checks, old candidates) sit inside the stage
// tsconfig's include — one broken sibling fails every later check. Clear
// check entries, candidates and outputs left over from earlier runs.
for (const f of readdirSync(STAGE_DIR))
  if (f.endsWith(".tsx") || f.startsWith("out-"))
    rmSync(join(STAGE_DIR, f), { recursive: true, force: true });
// `nsh check` needs a tsconfig.json in the candidate's directory. It extends
// .nsh/tsconfig.json — the build step (which always runs first) regenerates
// .nsh/ and its types there — and scopes its include to the stage files
// themselves (a broken leftover would otherwise fail every later check).
writeFileSync(
  join(STAGE_DIR, "tsconfig.json"),
  JSON.stringify({
    extends: "./.nsh/tsconfig.json",
    include: ["*.ts", "*.tsx"],
  }) + "\n",
);

// ————— live job status (polled by the app via GET /status?id=N) —————
const jobs = new Map();
let nextJobId = 1;

function setJob(job, phase, detail = "") {
  if (job) Object.assign(job, { phase, detail, seq: job.seq + 1 });
}

function jobJson(job) {
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
const WIDGETS_DIR = join(DATA_DIR, "widgets");
// Pop-out size, applied through Window.setFrame (nsh patch: setFrame/setLevel
// on nsh/desktop — see NSH-FEATURE-REQUEST.md / longbridge/nsh#60).
const POP_W = process.env.POP_W || "360";
const POP_H = process.env.POP_H || "480";
// Pop-out windows get a tiny frozen mount wrapper each (windows/pop-<n>.tsx).
const WINDOWS_DIR = join(DATA_DIR, "windows");
mkdirSync(WINDOWS_DIR, { recursive: true }); // runtime dir — absent on a fresh clone
// Modules generated after the app's bundle was built load through
// __nsh_import("<name>.js"), which the host serves from NSH_CHUNK_DIR (set
// by the packaged launcher; dev hosts rebuild the whole bundle instead).
const CHUNK_DIR = join(DATA_DIR, "chunks");
mkdirSync(CHUNK_DIR, { recursive: true });

const openWindows = new Map(); // "mode:name" → child process
const deskSlots = new Map(); // name → desktop column slot index
const shotCache = new Map(); // name → { t: number, png: Buffer }
const shotInflight = new Map(); // name → Promise<Buffer | null>
let deskBounds = null;
const desktopBounds = () => {
  if (deskBounds) return deskBounds;
  try {
    const s = execFileSync(
      "osascript",
      ["-e", 'tell application "Finder" to get bounds of window of desktop'],
      { encoding: "utf8" },
    ).trim();
    const [, , w, h] = s.split(",").map(Number);
    deskBounds = { w: w || 1440, h: h || 900 };
  } catch {
    deskBounds = { w: 1440, h: 900 };
  }
  return deskBounds;
};
const allocDeskSlot = (name) => {
  if (deskSlots.has(name)) return deskSlots.get(name);
  for (let i = 0; ; i++)
    if (![...deskSlots.values()].includes(i)) {
      deskSlots.set(name, i);
      return i;
    }
};
// Raise an already-open pop-out: find its window by title and pull it onto
// the focused workspace. AeroSpace-only (no-op without it).
const raiseWidget = (title) => {
  try {
    const out = execFileSync(
      "aerospace",
      ["list-windows", "--all", "--format", "%{window-id} %{window-title}"],
      { encoding: "utf8" },
    );
    const line = out.split("\n").find((l) => l.includes(title));
    if (!line) return;
    const wid = line.trim().split(/\s+/)[0];
    const ws = execFileSync(
      "aerospace", ["list-workspaces", "--focused"], { encoding: "utf8" },
    ).trim();
    execFileSync("aerospace", [
      "move-node-to-workspace", "--window-id", wid, "--focus-follows-window", ws,
    ]);
    execFileSync("aerospace", ["focus", "--window-id", wid]);
  } catch {}
};

// Render a mini-app to a PNG for the WidgetKit extension. A snapshot
// wrapper parks the host window offscreen at the desktop level — even if
// the tiling WM manages it, it stays behind every window, so nothing
// flashes on screen — then --script waits for the mount, shots the
// window's content in-process (cacheDisplayInRect: needs no Screen
// Recording permission) and quits. Serialized per name, cached ~45 s.
const renderShot = (name, w, h) => {
  const cached = shotCache.get(name);
  if (cached && Date.now() - cached.t < 45_000) return Promise.resolve(cached.png);
  const running = shotInflight.get(name);
  if (running) return running;
  const p = new Promise((resolve) => {
    const widgetFile = join(WIDGETS_DIR, `${name}.tsx`);
    const genFile = join(GEN_DIR, `${name}.tsx`);
    const file = existsSync(widgetFile)
      ? widgetFile
      : existsSync(genFile)
        ? genFile
        : null;
    if (!file) return resolve(null);
    const rel =
      file === widgetFile ? `../widgets/${name}` : `../generated/${name}`;
    const snap = join(WINDOWS_DIR, `snap-${name}.tsx`);
    const png = join(WINDOWS_DIR, `.shot-${name}.png`);
    writeFileSync(
      snap,
      `import { mount, View } from "nsh";
import { isAvailable, Window } from "nsh/desktop";
import { colorScheme } from "nsh/device";
import R from "${rel}";
// Snapshot surface: parked offscreen at the desktop-icon level so the
// tiling WM can never flash it on screen; sized exactly to the widget
// slot so the module lays out the way it will be shown.
if (isAvailable) {
  Window.setTitle("snap · " + ${JSON.stringify(name)});
  Window.setLevel("desktopIcon");
  Window.setFrame({ x: -6000, y: -6000, width: ${w}, height: ${h} });
}
mount(() => (
  <View
    style={{
      flex: 1,
      backgroundColor: colorScheme() === "dark" ? "#161616" : "#ffffff",
      overflow: "hidden",
    }}
  >
    <R />
  </View>
));
`,
    );
    try {
      unlinkSync(png);
    } catch {}
    // Bundle once, then launch the host straight on it — ~2 s end to end,
    // fast enough for WidgetKit's timeline budget (nsh run's dev-server
    // session took ~19 s and the extension's load got killed).
    const outDir = join(WINDOWS_DIR, `.snap-${name}`);
    const host = NSH_HOST;
    execFile(
      NSH,
      ["build", snap, "-o", outDir],
      { timeout: 30_000 },
      (buildErr) => {
        if (buildErr) return resolve(null);
        execFile(
          host,
          [
            "--bundle",
            join(outDir, "bundle.nshc"),
            "--name",
            `snap · ${name}`,
            "--window-size",
            `${w}x${h}`,
            // Mount → settle → capture → exit. The host runs the steps
            // once the window shows; quit ends the process.
            "--script",
            `wait 1500; shot ${png}; quit`,
          ],
          { timeout: 30_000 },
          () => {
            try {
              const buf = readFileSync(png);
              shotCache.set(name, { t: Date.now(), png: buf });
              console.log(`  /widget-shot ${name} → ${buf.length} B`);
              resolve(buf);
            } catch {
              resolve(null);
            }
          },
        );
      },
    );
  }).finally(() => shotInflight.delete(name));
  shotInflight.set(name, p);
  return p;
};
mkdirSync(WIDGETS_DIR, { recursive: true });

const db = new DatabaseSync(join(GEN_DIR, "app.db"));
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
    enabled INTEGER DEFAULT 1, bot TEXT, created_at INTEGER);
  CREATE TABLE IF NOT EXISTS timeclock(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    clock_in TEXT NOT NULL, clock_out TEXT);
`);

// A user turn's reply quote and attachments (JSON).
try { db.exec("ALTER TABLE turns ADD COLUMN meta TEXT"); } catch {}
// A branch may carry an optional name (e.g. "slow-ball") — versions.label.
try { db.exec("ALTER TABLE versions ADD COLUMN label TEXT"); } catch {}
// A card pins the artifact version it rendered (turns.v → versions.v).
try { db.exec("ALTER TABLE turns ADD COLUMN v INTEGER"); } catch {}
// Grok's sidebar organization: pinned tiles, named sections, hidden bots.
for (const col of ["pinned INTEGER DEFAULT 0", "section TEXT", "hidden INTEGER DEFAULT 0", "image TEXT", "crop TEXT", "model TEXT", "effort TEXT", "fast INTEGER DEFAULT 0"]) {
  try { db.exec(`ALTER TABLE bots ADD COLUMN ${col}`); } catch {}
}

const metaGet = (k) => db.prepare("SELECT v FROM meta WHERE k=?").get(k)?.v;
const metaSet = (k, v) =>
  db.prepare("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)").run(k, v);

// A module's display filename (Library rows, /app/meta renames): kebab-case
// plus ".tsx", or "" when nothing usable is left. Never the real file —
// R<n>.tsx stays the module's name on disk and in imports.
const slugFile = (s) => {
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
                ...(u.meta ? { files: JSON.parse(u.meta).files } : {}),
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
let store = loadStore();
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
function sendRanged(req, res, path, type) {
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

const AVATAR_DIR = join(GEN_DIR, "avatars");
mkdirSync(AVATAR_DIR, { recursive: true });
const ATTACH_DIR = join(GEN_DIR, "attachments");
mkdirSync(ATTACH_DIR, { recursive: true });
// files a reply produced (Grok's file cards), one folder per turn
const OUT_DIR = join(GEN_DIR, "files");
mkdirSync(OUT_DIR, { recursive: true });
const outFile = (rel) => {
  const p = join(OUT_DIR, rel);
  return p.startsWith(OUT_DIR + "/") ? p : null;
};
const osa = (script) =>
  new Promise((ok, fail) =>
    execFile("osascript", ["-e", script], (err, out) => (err ? fail(err) : ok(out.trim()))),
  );
const keyOf = (name) => {
  if (process.env[name]) return process.env[name];
  try { return (process.env[name] = execFileSync("secret", ["get", name], { encoding: "utf8" }).trim()); } catch { return ""; }
};
// Renders the avatar with Gemini's image model (falls back to OpenAI's
// gpt-image-1); keys come from the environment or `secret`.
async function generateAvatarImage(description, out) {
  const prompt = `A square profile avatar image, centered subject filling the frame, clean simple background: ${description}`;
  const gem = keyOf("GEMINI_API_KEY");
  if (gem) {
    const r = await fetch(
      `https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent?key=${gem}`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          contents: [{ parts: [{ text: prompt }] }],
          generationConfig: { responseModalities: ["IMAGE"], imageConfig: { aspectRatio: "1:1" } },
        }),
      },
    );
    const j = await r.json();
    const data = j?.candidates?.[0]?.content?.parts?.find((p) => p.inlineData)?.inlineData?.data;
    if (data) {
      writeFileSync(out + ".raw", Buffer.from(data, "base64"));
      execFileSync("sips", ["-s", "format", "png", out + ".raw", "--out", out], { stdio: "ignore" });
      unlinkSync(out + ".raw");
      return;
    }
  }
  const oai = keyOf("OPENAI_API_KEY");
  if (oai) {
    const r = await fetch("https://api.openai.com/v1/images/generations", {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${oai}` },
      body: JSON.stringify({ model: "gpt-image-1", size: "1024x1024", quality: "medium", prompt }),
    });
    const j = await r.json();
    if (r.ok && j?.data?.[0]?.b64_json) {
      writeFileSync(out, Buffer.from(j.data[0].b64_json, "base64"));
      return;
    }
  }
  // No image model with quota: Claude illustrates it as SVG, ImageMagick
  // rasterizes it.
  if (!anthropic) throw new Error("no image model available (add GEMINI or OPENAI quota)");
  const m = await anthropic.messages.create({
    model: MODEL,
    max_tokens: 8000,
    messages: [{
      role: "user",
      content: `Draw a polished, detailed flat-illustration profile avatar as a single SVG (viewBox 0 0 512 512, square, full-bleed colored background, the subject centered and filling most of the frame, rich shading with gradients). Subject: ${description}. Reply with only the <svg>…</svg> markup.`,
    }],
  });
  const text = m.content.map((c) => c.text || "").join("");
  const svg = /<svg[\s\S]*<\/svg>/.exec(text)?.[0];
  if (!svg) throw new Error("the illustrator returned no SVG");
  // QuickLook renders SVG through WebKit (gradients, filters intact)
  const svgPath = out.replace(/\.png$/, ".svg");
  writeFileSync(svgPath, svg);
  execFileSync("qlmanage", ["-t", "-s", "1024", "-o", AVATAR_DIR, svgPath], { stdio: "ignore" });
  execFileSync("mv", [svgPath + ".png", out]);
  unlinkSync(svgPath);
}

function persist() {
  db.exec("BEGIN");
  try {
    db.exec("DELETE FROM threads");
    db.exec("DELETE FROM turns");
    db.exec("DELETE FROM bots");
    const it = db.prepare(
      "INSERT INTO threads(id,title,ord) VALUES(?,?,?)",
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
      it.run(t.id, t.title, i);
      t.turns.forEach((u, j) =>
        iu.run(
          t.id, j, u.role, u.text ?? null, u.name ?? null, u.label ?? null,
          u.caption ?? null, u.reaction ?? null, u.retries ?? 0,
          u.widget ? 1 : 0,
          u.reply || u.attachments?.length || u.files?.length
            ? JSON.stringify({ reply: u.reply, attachments: u.attachments, files: u.files })
            : null,
          u.v ?? null,
        ),
      );
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
// deleted/crashed thread left behind.
const maxOnDisk = Math.max(
  0,
  ...readdirSync(GEN_DIR)
    .map((f) => /^R(\d+)\.tsx$/.exec(f))
    .filter(Boolean)
    .map((m) => Number(m[1])),
);
let seq = Math.max(store.seq || 0, maxOnDisk);
store.seq = seq;
let threadSeq =
  Math.max(0, ...store.list.map((t) => Number(t.id.slice(1)) || 0)) + 1;
const activeThread = () =>
  store.list.find((t) => t.id === store.active) || store.list[0];

// ————— data sources —————
// Generated widgets fetch these on 127.0.0.1:8787 — the only host the
// sandbox grants — and the server holds any credentials (Linear key via
// `secret`), so secrets never touch generated code.
let linearKey;
const getLinearKey = () => {
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
const jxa = (script, env = {}, timeout = 15000) => {
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

// The catalog the model sees — sources that exist only appear when configured.
function sourcesBlock() {
  const srcs = [
    `  timeclock — local clock-in/out tracker stored in the app's sqlite db
     GET  /data/timeclock → {status:"in"|"out", since:iso|null, todayMin:number, sessions:[{in,out,min}]}
     POST /data/timeclock/in  — clock in    POST /data/timeclock/out — clock out`,
    `  reminders — the user's macOS Reminders
     GET  /data/reminders → [{name,list,due,flagged}] (or {error:"…"})
     POST /data/reminders/add {name,list?,due?}    POST /data/reminders/complete {name}`,
  ];
  if (getLinearKey())
    srcs.push(`  linear — the user's Linear issues (assigned, else recent workspace issues). Every issue has uid (internal id for mutations), id (FER-12 label), stateId.
     GET  /data/linear/issues → {scope:"assigned"|"workspace", issues:[{uid,id,title,state,stateId,priority,assignee,team,url}]}
     GET  /data/linear/states → {teams:[{team,states:[{id,name,type}]}]} (transition targets per team)
     POST /data/linear/update {uid, stateId?, priority?}  — change state/priority
     POST /data/linear/assign {uid}                       — assign to me
     POST /data/linear/comment {uid, body}                — comment on it
     Interactive Linear widgets SHOULD: list issues, let a tap select one, then offer per-team state transitions (GET states once), assign-to-me, and a small comment input; refetch issues after any mutation.`);
  return srcs.join("\n");
}

const systemFor = (name) => `You write Native Shell (nsh) modules: ONE TSX file, rendered as native UI (UIKit/AppKit). You are writing module ${name}.
Reply format: optionally ONE short markdown caption line (may use **bold**, \`code\`, $math$ — rendered natively above your widget), then the module inside a single \`\`\`tsx fenced block. No other prose, no second code block.

${contractFor(name)}`;

// The module contract — shared by the one-shot generator and the tool-call
// agent. ${name} is the literal the module uses as its /state key: the real
// name for the legacy path, "__NAME__" for the agent (substituted on write).
const contractFor = (name) => `Contract:
- Exactly one default export: \`export default function R()\` returning JSX. Never call mount().
- IMPORT EVERY NAME YOU USE: signal, Text, View, Scroll, Input come from "nsh" (add every one you reference); List from "nsh/list"; Canvas from "nsh/canvas". Never import react/react-native/npm packages.
- <If when={cond}> and <Each of={array}>{(item, i) => …}</Each> are globals, no import. 'of' takes an ARRAY — pass the value, e.g. of={items()} or of={board()}, never the function itself. Each's row parameters are ACCESSORS — call them: item().field, i(). Never use them as plain values: wrong is 'item * 2' or 'item + "px"', right is 'item() * 2'.
- GAME/ANIMATION LOOPS: setInterval/setTimeout/clearInterval are globals returning number. Wrap timers in effect+cleanup (both imported from "nsh"): effect(() => { const t = setInterval(tick, 300); cleanup(() => clearInterval(t)); }); — tick mutates signals, UI follows.
- GRIDS/CANVAS GAMES: prefer <Canvas draw={(ctx, size) => {...}} style={{ width: W, height: H }} /> from "nsh/canvas" over hundreds of Views — it redraws automatically whenever a signal read inside draw() changes. ctx is CanvasRenderingContext2D (fillRect, fillStyle, etc.).
- Components run ONCE. State: const x = signal(0); read x() in JSX; write x.set(v)/x.update(fn) in handlers. Buttons: <View onPress={fn} activeOpacity={0.7}>.
- Every visible string must be inside <Text> — bare strings in <View> render nothing.
- Style with Tailwind: class="…" using only common utilities: flex, flex-row, flex-1, gap-N, p-N, px-N, py-N, m-N, mt-N, mb-N, rounded*, border, border-*, bg-*, text-* (size AND color), font-*, items-*, justify-*, self-*, w-N, h-N, max-w-*, min-h-*, overflow-hidden, opacity-*. Or style={{ camelCase }} — flexbox, column default, numbers in points.
- PERSISTED STATE: the app has a database — every widget that produces data worth remembering MUST persist it. Scores, streaks, session counts, check-ins, notes, settings, timers' completions: if the widget counts or collects anything, save it. fetch ONLY http://127.0.0.1:8787/state/${name} (all other hosts are sandbox-blocked). On mount restore once: const S = "${name}"; fetch(\`http://127.0.0.1:8787/state/\${S}\`).then(r=>r.json()).then(d=>{ if (d && d.state) /* set your signals from d.state fields */ }). Save via POST (replaces the stored object): fetch(\`http://127.0.0.1:8787/state/\${S}\`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ score: score(), done: done() }) }). Put saves in a save() called from the same handlers that mutate state; never save during render. A pomodoro persists { sessions, minutesFocused }; a habit tracker { streak, lastCheckIn }; a game { best, plays }.
- MULTI-SURFACE SYNC: the same widget may run in the chat AND its own window at once — they share the database, not memory. After the mount restore, poll so other surfaces' writes appear: let snap = ""; effect(() => { const t = setInterval(async () => { try { const d = await fetch(\`http://127.0.0.1:8787/state/\${S}\`).then(r=>r.json()); const s = JSON.stringify(d?.state||{}); if (s !== "{}" && s !== snap) { snap = s; /* re-apply d.state fields to your signals */ } } catch {} }, 2000); cleanup(() => clearInterval(t)); }); — set snap = the same JSON inside your save() too, so your own writes don't echo back. Clocks/elapsed timers must ALSO tick on wall-second boundaries (first tick via setTimeout(1000 - Date.now() % 1000)) so two surfaces show the same second.
- DATA SOURCES: real external data is proxied at http://127.0.0.1:8787/data/… (GET returns JSON; POST takes a JSON body; all other hosts are sandbox-blocked — never fetch elsewhere). When the request needs real data, fetch the right source on mount, keep an {error}-aware fallback (small "couldn't load" text + a retry button), and offer refresh (a button and/or a setInterval poll). Available:
${sourcesBlock()}
- KEYBOARD: import { keymap } from "nsh/keymap"; inside the component call keymap.bind({ left: () => move(-1), right: () => move(1), down: () => step(), up: () => rotate(), space: () => drop() }, { repeat: true }). Keystrokes: letters/numbers/space/enter/escape/tab/arrows/f1-f24, optional mod+/cmd+/ctrl+/alt+/shift+ prefix. Bindings auto-unbind with the component and don't fire while a text input eats the key. Reserved keys (cmd+q etc.) throw — skip them. A binding fires while ITS OWN window has the focus: inside the chat card the chat's input still owns typing keys (arrows/space included), so a keyboard-driven app MUST ALSO render tap controls for the chat surface — the user can send it to its own window (the ↗ action), where every bound key goes to it. Games SHOULD use it: arrows + space beat tap buttons.
- CLIPBOARD: import { Clipboard } from "nsh/clipboard"; Clipboard.setString(text) copies natively — use it for copy-to-clipboard affordances (a small ⧉ button on values, ids, results).
- POP-OUT SIZE (optional): the same module may be opened in its own floating window — resizable, normal z-order. Its WIDTH there is what you declare: export const popoutWidth = 520; (plain number, 240–720). Canvas apps/games may fix BOTH axes: export const popoutSize = { w: 320, h: 560 }; — the window is exactly that, no measuring. Without either: 360pt wide, height hugs content. Pop-out rules: (a) the top ~26pt of the window is an invisible drag strip — keep essential controls out of it (traffic lights sit in its left ~80pt); (b) lists that can grow tall MUST wrap in a bounded Scroll (max-h-96 or a fixed height) — an unbounded list measures its full content height and the window swallows the screen; (c) keep a min-h on the root so a transient empty/loading render never collapses the window.
- THEME (Sand design system — match it exactly): pop-out windows follow the system light/dark appearance — put dark: variants on every bg/text/border color; a hardcoded light card glows on a dark desktop. Surfaces: bg-white dark:bg-[#161616] (pane) · bg-[#f5f5f5] dark:bg-[#1f1f1f] (elevated card) · bg-black/5 dark:bg-white/5 (inset/track). Text: text-[#1c1c1c] dark:text-[#f5f5f5] (primary) · /60 alpha for secondary, /40 for tertiary (e.g. text-[#1c1c1c99] dark:text-[#f5f5f599]). Hairline borders: border-black/10 dark:border-white/10. Accent: #1680f3 light / #1884fc dark. Radius: 6 small, 10 medium, 14 card, 18 bubble/pill-ish. Spacing on a 4pt grid (p-2=8, p-3=12). Type: 12–14px body, 600 weight for emphasis — SF look, no exotic fonts. Canvas code may read colorScheme() from "nsh/device" inside draw() — reading it there redraws on theme change.
- DESKTOP WIDGET: the same module can live in a macOS Notification Center widget, rendered natively from YOUR persisted state — not a screenshot. To control how it looks there, include a "widget" object inside the state you POST to /state (computed live, alongside your real fields). Shape: { kind, ...fields }. Kinds: "clock" {running:boolean, startedAt:ISO|null, todayMin:number, label?} (the widget shows a LIVE ticking timer) · "stat" {value:string, unit?, label?, sub?} (big number) · "progress" {fraction:0-1, value?, sub?} (ring) · "weather" {temp:number, unit?, condition?, high?, low?, days?:[{label,icon?,high?,low?}]} · "list" {rows:[{title,detail?,url?}]} · "note" {headline, body?}. e.g. a pomodoro POSTs {sessions, minutesFocused, widget:{kind:"progress",fraction:elapsed()/1500,value:"25:00",sub:"focus"}}. If you don't emit one, a template is inferred from your state's shape (status/running→clock, numbers→stat). You may also pin the template with export const widgetKind = "stat";.
- No props, no imports beyond nsh modules, no fetch other than /state and /data: render standalone inside a ~340pt-wide chat card (it may also run in its own window).
- Make it a small, polished, interactive widget for the user's request: cards, charts as View bars, forms, lists, counters, steppers, games with real loops. Compact code, but take the lines a working game needs (~150 is fine).`;

const dirOf = (t) => (t.widget ? WIDGETS_DIR : GEN_DIR);

// The turns the app renders — plain JSON so a thread switch is a data swap,
// not a code rebuild. `i` is the turn's index (DelBtn keys off it).
function turnsJson(thread = activeThread()) {
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
  }));
}

// generated/index.tsx is a STABLE registry: every module on disk gets one
// import + one map entry, and the file is only rewritten when the set of
// modules changes — never on a thread switch or a new text turn. That keeps
// the conversation a data-driven view: no bundle rebuild, no blank flash.
let lastModuleKey = "";
function writeIndex() {
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
// re-renders it inside the active conversation.
function writeShelf() {
  const src = `import { cleanup, effect, Input, signal, Text, View } from "nsh";
import { isAvailable, Menu } from "nsh/desktop";
import { keymap } from "nsh/keymap";

const GEN = "http://127.0.0.1:8787";
const get = (p: string) =>
  fetch(\`\${GEN}\${p}\`).then((r) => (r.ok ? r.json() : null)).catch(() => null);
const post = (p: string, b: object) =>
  fetch(\`\${GEN}\${p}\`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(b),
  }).catch(() => null);

type Item = {
  name: string;
  title: string;
  publishedAt: string;
  kinds: string[];
  pinned: boolean;
  archived: boolean;
  tags: string[];
};

// kinds: db = persisted via /state (surfaces share it) · api = fetches a
// /data source · keys = keyboard-driven (↗ window for real key control) ·
// mem = in-memory only, every surface gets fresh state.
const KIND_COLOR: Record<string, string> = {
  db: "#10a37f",
  api: "#5e6ad2",
  keys: "#f2994a",
  mem: "#f5f5f585",
};

function TitleBox(props: {
  item: Item;
  reload: () => void;
  editing: boolean;
  begin: () => void;
  done: () => void;
}) {
  let input!: any;
  // The field is uncontrolled (defaultValue) — its live text rides in
  // 'latest' via a listener that never writes back (that's what kept
  // snapping the old title over an edit). Enter commits it, Escape drops
  // it, blur/tab-away commits it — the usual text-field contract.
  let latest = props.item.title;
  let open = false;
  let cancelled = false;
  let finished = false;
  keymap.bind({
    escape: () => {
      if (!open) return;
      cancelled = true;
      try {
        input?.blur?.();
      } catch {}
      props.done();
    },
  });
  const commit = (t: string) => {
    if (finished) return;
    finished = true;
    props.done();
    t = t.trim();
    if (t && t !== props.item.title)
      post("/widget-meta", { name: props.item.name, title: t }).then(() =>
        props.reload(),
      );
  };
  // Double-click to rename — a single stray click on the title never
  // opens the editor (that was the annoying part).
  let lastTap = 0;
  return (
    <If
      when={props.editing}
      else={
        <View
          class="flex-row items-center gap-1"
          onPress={() => {
            const t = Date.now();
            if (t - lastTap < 380) props.begin();
            lastTap = t;
          }}
          activeOpacity={0.7}
          accessibilityLabel="double-click to rename"
        >
          <Text
            style={{ color: "#f5f5f5", fontSize: 12.5, fontWeight: "600" }}
            numberOfLines={1}
          >
            {props.item.title}
          </Text>
          <Text style={{ color: "#f5f5f566", fontSize: 10 }}>✎</Text>
        </View>
      }
    >
      <Input
        ref={input}
        defaultValue={props.item.title}
        returnKeyType="done"
        autoFocus
        onFocus={() => (open = true)}
        onChangeText={(t) => (latest = t)}
        onBlur={() => {
          open = false;
          if (!cancelled) commit(latest);
          cancelled = false;
        }}
        onSubmitEditing={(e) => commit(e.text)}
        style={{
          fontSize: 12.5,
          fontWeight: "600",
          color: "#f5f5f5",
          paddingVertical: 1,
          paddingHorizontal: 5,
          backgroundColor: "#f5f5f50f",
          borderRadius: 5,
        }}
      />
    </If>
  );
}

function TagBox(props: {
  item: Item;
  reload: () => void;
  open: boolean;
  begin: () => void;
  done: () => void;
}) {
  let input!: any;
  let latest = "";
  let open = false;
  let cancelled = false;
  let finished = false;
  keymap.bind({
    escape: () => {
      if (!open) return;
      cancelled = true;
      try {
        input?.blur?.();
      } catch {}
      props.done();
    },
  });
  const commit = (raw: string) => {
    if (finished) return;
    finished = true;
    props.done();
    const t = raw.trim().toLowerCase();
    if (!t) return;
    post("/widget-meta", {
      name: props.item.name,
      tags: [...props.item.tags, t],
    }).then(() => props.reload());
  };
  return (
    <If
      when={props.open}
      else={
        <View
          onPress={props.begin}
          activeOpacity={0.7}
          style={{
            borderRadius: 999,
            paddingHorizontal: 7,
            paddingVertical: 2,
            backgroundColor: "#f5f5f50f",
          }}
        >
          <Text style={{ color: "#f5f5f566", fontSize: 9 }}>+tag</Text>
        </View>
      }
    >
      <Input
        ref={input}
        defaultValue=""
        placeholder="+tag"
        placeholderTextColor="#f5f5f566"
        returnKeyType="done"
        autoFocus
        onChangeText={(t) => (latest = t)}
        onFocus={() => (open = true)}
        onBlur={() => {
          open = false;
          if (!cancelled) commit(latest);
          cancelled = false;
        }}
        onSubmitEditing={(e) => commit(e.text)}
        style={{
          width: 56,
          fontSize: 10,
          color: "#f5f5f5",
          paddingVertical: 2,
          paddingHorizontal: 6,
          backgroundColor: "#f5f5f50f",
          borderRadius: 6,
        }}
      />
    </If>
  );
}

export function WidgetShelf() {
  const list = signal<Item[]>([]);
  const q = signal("");
  const kind = signal("all");
  const tag = signal("");
  const sent = signal("");
  // Which card is renaming / tagging right now (its name, "" = none).
  const renaming = signal("");
  const tagging = signal("");

  const load = async () => {
    const d = await get("/catalog");
    if (d?.list) list.set(d.list);
  };
  load();
  effect(() => {
    const t = setInterval(load, 3000);
    cleanup(() => clearInterval(t));
  });

  // The card's context menu — a real NSMenu at the pointer (right-click on
  // the card, or its ⋯ button, calls this).
  const openMenu = (i: Item) => {
    if (!isAvailable) return;
    Menu.popup([
      {
        label: "Open window",
        onSelect: () => post("/popout", { name: i.name }),
      },
      {
        label: "Pin to desktop",
        onSelect: () => post("/popout", { name: i.name, mode: "desktop" }),
      },
      { type: "separator" },
      { label: "Rename…", onSelect: () => renaming.set(i.name) },
      { label: "Add tag…", onSelect: () => tagging.set(i.name) },
      {
        // Re-prompt the model against the same request and patch the
        // published file — the fix path for a broken or badly-sized app.
        label: "Regenerate",
        onSelect: () => {
          post("/retry", { name: i.name });
        },
      },
      {
        label: i.pinned ? "Unpin" : "Nominate (★)",
        onSelect: () =>
          post("/pin", { name: i.name, pinned: !i.pinned }).then(load),
      },
      {
        label: i.archived ? "Unarchive" : "Archive",
        onSelect: () =>
          post("/widget-meta", { name: i.name, archived: !i.archived }).then(
            load,
          ),
      },
      { type: "separator" },
      {
        label: "Delete from Mini Apps",
        onSelect: () => post("/unpublish", { name: i.name }).then(load),
      },
    ]);
  };

  const allTags = () => [...new Set(list().flatMap((i) => i.tags))];
  const shown = () =>
    list().filter((i) => {
      const hay = \`\${i.title} \${i.name} \${i.tags.join(" ")}\`.toLowerCase();
      const matchTxt =
        (!tag() || i.tags.includes(tag())) &&
        (!q() || hay.includes(q().toLowerCase()));
      // Archived apps hide unless the archived filter is on.
      if (kind() === "arch") return i.archived && matchTxt;
      return (
        !i.archived &&
        (kind() === "all" || i.kinds.includes(kind())) &&
        matchTxt
      );
    });

  const act = (g: string, onTap: () => void, on: boolean) => (
    <View
      onPress={onTap}
      activeOpacity={0.7}
      style={{
        borderRadius: 7,
        paddingHorizontal: 8,
        paddingVertical: 4,
        backgroundColor: on ? "#f5f5f514" : "transparent",
      }}
    >
      <Text style={{ color: on ? "#f5f5f5" : "#f5f5f585", fontSize: 12 }}>{g}</Text>
    </View>
  );

  const chip = (label: string, on: boolean, onTap: () => void, color = "#5e6ad2") => (
    <View
      onPress={onTap}
      activeOpacity={0.7}
      style={{
        borderRadius: 999,
        paddingHorizontal: 10,
        paddingVertical: 3,
        backgroundColor: on ? color : "#f5f5f50f",
      }}
    >
      <Text style={{ color: on ? "#f5f5f5" : "#f5f5f5a8", fontSize: 10, fontWeight: "600" }}>
        {label}
      </Text>
    </View>
  );

  const card = (w: () => Item) => (
    <View
      onContextMenu={() => openMenu(w())}
      style={{
        width: "48%",
        borderRadius: 14,
        borderWidth: 1,
        borderColor: "#f5f5f51f",
        backgroundColor: "#1f1f1f",
        padding: 12,
        gap: 7,
      }}
    >
      <View class="flex-row items-center gap-2">
        <View
          style={{
            width: 30,
            height: 30,
            borderRadius: 9,
            backgroundColor: KIND_COLOR[w().kinds[0]] || "#f5f5f585",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <Text style={{ color: "#fff", fontSize: 13, fontWeight: "700" }}>
            {w().title.slice(0, 1).toUpperCase()}
          </Text>
        </View>
        <Text style={{ color: "#f5f5f566", fontSize: 9.5, flex: 1 }}>
          {w().name}.tsx · {w().publishedAt?.slice(0, 10)}
        </Text>
        {act("↗", () => post("/popout", { name: w().name }), false)}
        {act("⋯", () => openMenu(w()), false)}
      </View>
      <TitleBox
        item={w()}
        reload={load}
        editing={renaming() === w().name}
        begin={() => renaming.set(w().name)}
        done={() => renaming.set("")}
      />
      <View class="flex-row items-center gap-1" style={{ flexWrap: "wrap" }}>
        <Each of={w().kinds} key={(k) => k}>
          {(k) => (
            <View
              style={{
                borderRadius: 999,
                paddingHorizontal: 7,
                paddingVertical: 2,
                backgroundColor: (KIND_COLOR[k()] || "#f5f5f585") + "26",
              }}
            >
              <Text
                style={{
                  color: KIND_COLOR[k()] || "#f5f5f585",
                  fontSize: 9,
                  fontWeight: "700",
                }}
              >
                {k()}
              </Text>
            </View>
          )}
        </Each>
        <Each of={w().tags} key={(t) => t}>
          {(t) => (
            <View
              onPress={() => {
                post("/widget-meta", {
                  name: w().name,
                  tags: w().tags.filter((x) => x !== t()),
                }).then(load);
              }}
              activeOpacity={0.7}
              style={{
                borderRadius: 999,
                paddingHorizontal: 7,
                paddingVertical: 2,
                backgroundColor: "#f5f5f50f",
              }}
            >
              <Text style={{ color: "#f5f5f5a8", fontSize: 9 }}>#{t()}</Text>
            </View>
          )}
        </Each>
        <TagBox
          item={w()}
          reload={load}
          open={tagging() === w().name}
          begin={() => tagging.set(w().name)}
          done={() => tagging.set("")}
        />
        <View style={{ flex: 1 }} />
        <View
          onPress={() => {
            post("/reuse", { name: w().name }).then(() => {
              sent.set(w().name);
              setTimeout(() => sent.set(""), 1500);
            });
          }}
          activeOpacity={0.7}
          style={{
            borderRadius: 8,
            paddingHorizontal: 9,
            paddingVertical: 4,
            backgroundColor: "#f5f5f514",
          }}
        >
          <Text style={{ color: "#f5f5f5", fontSize: 10.5 }}>
            {sent() === w().name ? "＋ chat ✓" : "＋ chat"}
          </Text>
        </View>
      </View>
    </View>
  );

  return (
    <View class="gap-3 px-4 py-4">
      <View class="flex-row items-center justify-between">
        <Text style={{ color: "#f5f5f566", fontSize: 12, fontWeight: "600" }}>
          {"MINI APPS — " + shown().length}
        </Text>
        <Input
          value={q()}
          onChangeText={(t) => q.set(t)}
          placeholder="filter…"
          placeholderTextColor="#f5f5f566"
          style={{
            width: 140,
            fontSize: 11,
            color: "#f5f5f5",
            paddingVertical: 4,
            paddingHorizontal: 10,
            backgroundColor: "#f5f5f50f",
            borderRadius: 8,
          }}
        />
      </View>
      <View class="flex-row gap-1.5" style={{ flexWrap: "wrap" }}>
        {chip("all", kind() === "all" && !tag(), () => {
          kind.set("all");
          tag.set("");
        }, "#f5f5f53d")}
        {chip("db", kind() === "db", () => kind.set(kind() === "db" ? "all" : "db"), KIND_COLOR.db)}
        {chip("api", kind() === "api", () => kind.set(kind() === "api" ? "all" : "api"), KIND_COLOR.api)}
        {chip("keys", kind() === "keys", () => kind.set(kind() === "keys" ? "all" : "keys"), KIND_COLOR.keys)}
        {chip("mem", kind() === "mem", () => kind.set(kind() === "mem" ? "all" : "mem"), KIND_COLOR.mem)}
        {chip("archived", kind() === "arch", () => kind.set(kind() === "arch" ? "all" : "arch"), "#c95901")}
        <Each of={allTags()} key={(t) => t}>
          {(t) =>
            chip("#" + t(), tag() === t(), () => tag.set(tag() === t() ? "" : t()), "#7c5e10")
          }
        </Each>
      </View>
      <View class="flex-row" style={{ flexWrap: "wrap", gap: 12 }}>
        <Each of={shown()} key={(i) => i.name}>{(w) => card(w)}</Each>
      </View>
      <If when={!shown().length}>
        <Text style={{ color: "#f5f5f566", fontSize: 13 }}>
          Nothing here — publish a generated card with ⬆, or loosen the filter.
        </Text>
      </If>
    </View>
  );
}
`;
  writeFileSync(join(WIDGETS_DIR, "index.tsx"), src);
}

const widgetRow = (name) =>
  db.prepare("SELECT * FROM widgets WHERE name=?").get(name);

function extractCode(raw) {
  const text = String(raw || "").trim();
  let code = text;
  let caption = "";
  const m = text.match(/```(?:tsx?|jsx?)?\s*\n([\s\S]*?)```/);
  if (m) {
    code = m[1].trim();
    // everything outside the fence is the model's markdown caption
    caption = (text.slice(0, m.index) + text.slice((m.index || 0) + m[0].length))
      .replace(/```[\s\S]*?```/g, "")
      .trim();
  }
  if (!code.includes("export default")) {
    const def = code.match(/function\s+R\s*\(/);
    if (def) code += "\nexport default R;\n";
  }
  if (!code.includes("export default") || !code.includes("<"))
    throw new Error("model did not return a module");
  return { caption, code };
}

async function askModel(job, prompt, prevError, prevCode, name) {
  if (MOCK) return { caption: "", code: mockModule(prompt) };
  const messages = [{ role: "user", content: prompt }];
  if (prevError) {
    if (typeof prevCode === "string")
      messages.push({ role: "assistant", content: prevCode });
    messages.push({
      role: "user",
      content: prevError.includes("truncated at the token limit")
        ? "Your reply was cut off at the token limit. Return the SAME module but shorter: fewer comments, tighter code, no extras."
        : prevError.includes("did not return a module")
        ? "Your reply had no module. Reply with ONE fenced ```tsx block containing the complete module (it must include export default) and nothing else."
        : `The nsh compiler rejected it:\n${prevError}\nReturn a corrected module only.`,
    });
  }
  if (PROVIDER === "anthropic") {
    try {
      const stream = anthropic.messages.stream({
        model: MODEL,
        max_tokens: MAX_TOKENS,
        // haiku-5.5 deprecates temperature — omit it
        system: systemFor(name),
        messages,
      });
      stream.on("text", (_delta, snapshot) =>
        setJob(job, "writing", `streaming module · ${snapshot.length} chars`),
      );
      const msg = await stream.finalMessage();
      const text = msg.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("");
      if (msg.stop_reason === "max_tokens")
        throw new Error("module truncated at the token limit");
      return extractCode(text);
    } catch (e) {
      // A malformed reply (no tsx block) or a truncated one is retriable —
      // only transport/API failures get the llm-api prefix that stops retries.
      if (
        e?.message === "model did not return a module" ||
        e?.message === "module truncated at the token limit"
      )
        throw e;
      throw new Error(`llm-api ${e.status ?? ""}: ${e.message}`);
    }
  }
  const r = await fetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      model: MODEL,
      messages: [{ role: "system", content: systemFor(name) }, ...messages],
      temperature: 0.4,
      max_tokens: MAX_TOKENS,
    }),
  });
  if (!r.ok) throw new Error(`llm-api ${r.status}: ${await r.text()}`);
  const data = await r.json();
  if (data.choices?.[0]?.finish_reason === "length")
    throw new Error("module truncated at the token limit");
  return extractCode(data.choices?.[0]?.message?.content);
}

// Not every message wants a widget — classify before burning a generation.
// "ui" only when the user wants an interface built; everything else is chat.
async function classifyIntent(prompt) {
  if (MOCK) return "ui";
  const sys =
    "Classify the user message for a generative-UI chat app. Reply with exactly one word: UI or CHAT.\n" +
    "UI = the user wants an interface, widget, app, card, or game built/rendered — including terse requests that name a thing to build (\"pomodoro timer\", \"make a calculator\").\n" +
    "CHAT = greetings, questions, discussion, feedback, or anything that only wants a text answer.";
  try {
    let answer;
    if (PROVIDER === "anthropic") {
      const m = await anthropic.messages.create({
        model: MODEL,
        max_tokens: 8,
        system: sys,
        messages: [{ role: "user", content: prompt }],
      });
      answer = m.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("");
    } else {
      const r = await fetch(`${BASE_URL}/chat/completions`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          model: MODEL,
          messages: [
            { role: "system", content: sys },
            { role: "user", content: prompt },
          ],
          max_tokens: 8,
        }),
      });
      answer = (await r.json()).choices?.[0]?.message?.content || "";
    }
    return answer.trim().toUpperCase().startsWith("UI") ? "ui" : "chat";
  } catch (e) {
    console.error(`  intent classify failed: ${e.message} — generating anyway`);
    return "ui"; // classifier down → fall back to the old always-generate behavior
  }
}

// Plain conversational reply (no module) — with the recent turns as context.
async function chatReply(prompt, thread, job) {
  const sys =
    "You are the assistant inside a generative-UI chat app: when the user asks for an interface, a tool builds and renders it natively. This message is conversation — answer in GitHub-flavored markdown, concise. If a UI would obviously help, offer to build it in one line.";
  const messages = [
    ...thread.turns.slice(-10).map((t) =>
      t.role === "user"
        ? { role: "user", content: t.text }
        : {
            role: "assistant",
            content: t.name ? `[built widget ${t.name}]` : t.text || "",
          },
    ),
    { role: "user", content: await userContent(prompt, job?.extra, job?.ctx) },
  ];
  if (MOCK) return "_(mock reply — no model configured)_";
  // userContent speaks Anthropic's block shapes; chat/completions wants
  // image_url parts instead of base64 image sources
  const oaiContent = (c) =>
    typeof c === "string"
      ? c
      : c.map((b) => (b.type === "image" ? { type: "image_url", image_url: { url: `data:${b.source.media_type};base64,${b.source.data}` } } : b));
  try {
    if (PROVIDER === "anthropic") {
      const m = await anthropic.messages.create({
        model: MODEL,
        max_tokens: 1024,
        system: sys,
        messages,
      });
      return m.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("")
        .trim();
    }
    const r = await fetch(`${BASE_URL}/chat/completions`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ model: MODEL, messages: messages.map((m) => ({ ...m, content: oaiContent(m.content) })), max_tokens: 1024 }),
    });
    return (
      (await r.json()).choices?.[0]?.message?.content || "(empty reply)"
    ).trim();
  } catch (e) {
    return `_(model error — ${String(e.message || e).slice(0, 200)})_`;
  }
}

function runNsh(args) {
  return new Promise((resolve) => {
    execFile(NSH, args, { timeout: 90000 }, (err, stdout, stderr) =>
      resolve(err ? String(stdout) + String(stderr) : null),
    );
  });
}

// nsh names the model is allowed to use without thinking about imports —
// inject the ones it forgets instead of failing the module.
const CORE_NAMES = [
  "batch", "cleanup", "computed", "context", "effect", "Input", "mount",
  "Scroll", "signal", "split", "Text", "untracked", "View",
];
const MODULE_NAMES = {
  Canvas: "nsh/canvas",
  Image: "nsh/image",
  List: "nsh/list",
  Video: "nsh/video",
  WebView: "nsh/webview",
  Button: "nsh/ui/mobile",
  Card: "nsh/ui/mobile",
  Keyboard: "nsh/ui/mobile",
  KeyboardAvoidingView: "nsh/ui/mobile",
  SafeAreaView: "nsh/ui/mobile",
  StatusBar: "nsh/ui/mobile",
  TextField: "nsh/ui/mobile",
};

function fixImports(code) {
  // Drop imports nsh can't resolve (react, react-native, …).
  code = code.replace(
    /^[ \t]*import[^\n]*from\s*["'](react|react-native|solid-js|preact|vue|svelte)[^\n]*\n/gm,
    "",
  );
  const imported = new Set();
  for (const m of code.matchAll(/import\s*\{([^}]*)\}\s*from\s*["'][^"']+["']/g))
    for (const s of m[1].split(","))
      imported.add(s.trim().split(/\s+as\s+/)[0].trim());
  const missing = (n) => new RegExp(`\\b${n}\\b`).test(code) && !imported.has(n);
  const core = CORE_NAMES.filter(missing);
  const byMod = new Map();
  for (const [n, mod] of Object.entries(MODULE_NAMES))
    if (missing(n)) (byMod.get(mod) ?? byMod.set(mod, []).get(mod)).push(n);

  if (
    core.length &&
    /import\s*\{[^}]*\}\s*from\s*["']nsh["']/.test(code)
  )
    code = code.replace(
      /import\s*\{([^}]*)\}\s*from\s*["']nsh["']/,
      (m, names) =>
        `import { ${[...names.split(",").map((s) => s.trim()), ...core].join(
          ", ",
        )} } from "nsh"`,
    );
  else if (core.length)
    code = `import { ${core.join(", ")} } from "nsh";\n` + code;

  const extra = [...byMod].map(
    ([mod, names]) => `import { ${names.join(", ")} } from "${mod}";`,
  );
  if (extra.length) code = extra.join("\n") + "\n" + code;
  return code;
}

// Compiles one module file into a runtime-loadable chunk
// (CHUNK_DIR/<name>.js). `nsh build` on the module alone produces a bundle
// whose namespace's `default` is the component — the chunk shape
// __nsh_import resolves. Best-effort: a failure leaves the card on its
// "module missing" path, same as before chunks existed.
async function emitChunk(name, srcFile) {
  const out = join(STAGE_DIR, `chunk-${name}`);
  rmSync(out, { recursive: true, force: true });
  const err = await runNsh(["build", srcFile, "--out", out]);
  if (err)
    return console.log(
      `chunk ${name}: ${String(err).trim().split("\n").pop()}`,
    );
  try {
    copyFileSync(join(out, "bundle.js"), join(CHUNK_DIR, `${name}.js`));
  } catch {}
}

// workspace files (OUT_DIR recursively) as {name, rel} for the @ picker.
function walkFiles(dir, base = dir, out = []) {
  try {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name);
      if (e.isDirectory()) walkFiles(p, base, out);
      else out.push({ name: e.name, rel: p.slice(base.length + 1) });
    }
  } catch {}
  return out;
}

const headOf = (name) =>
  db.prepare("SELECT v FROM heads WHERE name=?").get(name)?.v ?? null;
// Version files are named <slug>.v<n>.tsx inside generated/<name>/ — slug
// is the artifact's file label (or title slug, else the R-name). Renaming
// the artifact's file name or title batch-renames every version's file.
const verSlug = (name) => {
  const meta =
    db.prepare("SELECT file,title FROM apps WHERE name=?").get(name) || {};
  // apps.file/title slugs carry their ".tsx" — the version file appends
  // ".v<n>" before it.
  return (meta.file || slugFile(meta.title) || `${name}.tsx`).replace(
    /\.tsx$/i,
    "",
  );
};
const verFile = (name, v) => {
  const dir = join(GEN_DIR, name);
  try {
    const f = readdirSync(dir).find((x) =>
      new RegExp(`\\.v${v}\\.tsx$`).test(x),
    );
    return f ? join(dir, f) : null;
  } catch {
    return null;
  }
};
const versionsOf = (name) =>
  db
    .prepare(
      "SELECT v,parent,created_at,label FROM versions WHERE name=? ORDER BY v",
    )
    .all(name);
// Chunk ids must be flat <stem>.js (nsh-js validates) — "R6@v3" ships as
// "R6v3.js". R-names are digits-only so the mapping can't collide.
const chunkKey = (key) => key.replace(/^R(\d+)@v(\d+)$/, "R$1v$2");

// A generated artifact's immutable version files live under
// generated/<name>/v<k>.tsx; generated/<name>.tsx mirrors the head so every
// consumer that names the bare artifact (baked index, /module, /popout)
// sees current. A revise off a non-head version parents the new version
// there — a branch, never a rewrite.
function writeVersion(name, code, parent, prompt, label) {
  const dir = join(GEN_DIR, name);
  mkdirSync(dir, { recursive: true });
  const v =
    (db.prepare("SELECT MAX(v) AS m FROM versions WHERE name=?").get(name)
      ?.m || 0) + 1;
  const vFile = join(dir, `${verSlug(name)}.v${v}.tsx`);
  writeFileSync(vFile, code);
  db.prepare(
    "INSERT INTO versions(name,v,parent,prompt,created_at,label) VALUES(?,?,?,?,?,?)",
  ).run(
    name, v, parent ?? null,
    String(prompt || "").slice(0, 500) || null, Date.now(),
    String(label || "").trim().slice(0, 40) || null,
  );
  db.prepare("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)").run(name, v);
  writeFileSync(join(GEN_DIR, `${name}.tsx`), code);
  // A published Mini App is the same artifact — its file mirrors head too.
  if (widgetRow(name)) writeFileSync(join(WIDGETS_DIR, `${name}.tsx`), code);
  emitChunk(chunkKey(`${name}@v${v}`), vFile);
  emitChunk(name, join(GEN_DIR, `${name}.tsx`));
  return v;
}

// The current pointer moves; nothing is rewritten — later versions stay on
// the tree as siblings of whatever branches off the reverted one.
function setHead(name, v) {
  const file = verFile(name, v);
  if (!file || !existsSync(file)) return false;
  db.prepare("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)").run(name, v);
  const code = readFileSync(file, "utf8");
  writeFileSync(join(GEN_DIR, `${name}.tsx`), code);
  if (widgetRow(name)) writeFileSync(join(WIDGETS_DIR, `${name}.tsx`), code);
  emitChunk(name, join(GEN_DIR, `${name}.tsx`));
  writeIndex();
  writeShelf();
  return true;
}

// Compile + typecheck the staged candidate; the app on the device is untouched
// until this passes. Entry/output are per-module so concurrent chats don't
// clobber each other's validation.
async function checkCandidate(name) {
  const entry = join(STAGE_DIR, `check-${name}.tsx`);
  const mod = join(STAGE_DIR, `${name}.tsx`);
  writeFileSync(
    entry,
    `import { mount } from "nsh";\nimport R from "./${name}";\nmount(R);\n`,
  );
  const buildErr = await runNsh([
    "build",
    entry,
    "--out",
    join(STAGE_DIR, `out-${name}`),
  ]);
  const err = buildErr ?? (await runNsh(["check", entry]));
  // The stage dir's tsconfig includes every sibling — a broken leftover
  // rejects the NEXT module's check too. Stage files are intermediates only
  // (the real one lands in generated/), so remove the pair as soon as the
  // verdict is in.
  try {
    rmSync(entry);
    rmSync(mod);
  } catch {}
  return err;
}

function errorCard(note) {
  return `import { Text, View } from "nsh";

export default function R() {
  return (
    <View class="gap-2 rounded-xl border border-red-200 bg-red-50 p-3">
      <Text class="text-sm font-semibold text-red-700">generation failed</Text>
      <Text class="text-xs text-red-600">{${JSON.stringify(
        note.slice(0, 600),
      )}}</Text>
    </View>
  );
}
`;
}

// ————— tool-call agent —————
// One /chat turn = a model loop with real tools. Widgets are one tool among
// several: the model answers plainly when no UI is needed, and can search,
// read pages, query local data, or generate images — then optionally build a
// widget from what it found. Module names are server-assigned; modules write
// the literal "__NAME__" where their /state key goes and it's substituted on
// write (the contract below tells the model that).

const ASSETS_DIR = join(GEN_DIR, "assets");
mkdirSync(ASSETS_DIR, { recursive: true });

const CLIENT_SEARCH = {
  name: "web_search",
  description:
    "Search the web for current facts, news, docs, prices. Returns ranked results (title, url, snippet).",
  input_schema: {
    type: "object",
    properties: { query: { type: "string", description: "the search query" } },
    required: ["query"],
  },
};

const TOOLS = [
  {
    name: "generate_app",
    description:
      "Build an interactive native-UI mini-app from a complete Native Shell TSX module and mount it as a card in the conversation. Use it whenever the user wants an interface — apps, games, trackers, dashboards, forms, data views. The server typechecks it and returns the compiler error if it fails; fix the code and call again.",
    input_schema: {
      type: "object",
      properties: {
        code: {
          type: "string",
          description:
            "The complete TSX module: one default export function returning JSX, no mount(), all names imported from nsh modules. Where the module needs its own name (the /state endpoint) write the literal string \"__NAME__\" — the server substitutes it on write.",
        },
        caption: {
          type: "string",
          description: "optional one-line markdown caption rendered above the card",
        },
        title: {
          type: "string",
          description:
            'optional short product name for the app (e.g. "Tiny Trajectory") — the Library lists it under this',
        },
        slug: {
          type: "string",
          description:
            'optional kebab-case display filename, no extension (e.g. "tiny-trajectory" → tiny-trajectory.tsx)',
        },
      },
      required: ["code"],
    },
  },
  {
    name: "generate_image",
    description:
      "Generate a real image from a text prompt and mount it as a card. Use for draw/image/picture/wallpaper/icon requests — describe subject, style, lighting richly. Never use it for UI mockups; interfaces are generate_app.",
    input_schema: {
      type: "object",
      properties: {
        prompt: { type: "string", description: "rich description of the image to generate" },
        caption: { type: "string", description: "optional one-line markdown caption" },
      },
      required: ["prompt"],
    },
  },
  {
    name: "fetch_url",
    description:
      "Fetch a web page and return its readable text (HTML stripped, ~6000 chars). Use it to read a specific URL the user gives or a search result worth opening.",
    input_schema: {
      type: "object",
      properties: { url: { type: "string" } },
      required: ["url"],
    },
  },
  {
    name: "query_source",
    description:
      "Read one of the app's live local data sources and return its JSON. Sources: 'timeclock' (the user's clock-in/out tracker), 'reminders' (macOS Reminders), 'linear' (Linear issues). Use it when the user asks about their own data — answer in text, or feed it into a widget you build.",
    input_schema: {
      type: "object",
      properties: {
        source: { type: "string", enum: ["timeclock", "reminders", "linear"] },
      },
      required: ["source"],
    },
  },
  {
    name: "office_create",
    description:
      "Create a real Office file — .docx, .xlsx, or .pptx — and mount it as a file card. Use it whenever the user asks for a document, spreadsheet, or slide deck (report, budget, memo, pitch). Payload by extension: docx.paragraphs [{text, style: title|heading1|heading2|normal, bold, italic}]; xlsx.rows string[][] of raw inputs (\"42\" is a number, \"=SUM(A1:A3)\" a formula, anything else text); pptx.slides [{title, bullets: string[], notes}]. To revise the file later use the office_* tools (office_files → office_open → office_grep/office_cells → office_propose/office_propose_cells → office_export).",
    input_schema: {
      type: "object",
      properties: {
        path: { type: "string", description: "file name ending .docx, .xlsx, or .pptx" },
        docx: {
          type: "object",
          properties: {
            paragraphs: {
              type: "array",
              items: {
                type: "object",
                properties: { text: { type: "string" }, style: { type: "string", enum: ["title", "heading1", "heading2", "normal"] }, bold: { type: "boolean" }, italic: { type: "boolean" } },
                required: ["text"],
              },
            },
          },
        },
        xlsx: { type: "object", properties: { rows: { type: "array", items: { type: "array", items: { type: "string" } } } } },
        pptx: {
          type: "object",
          properties: {
            slides: {
              type: "array",
              items: {
                type: "object",
                properties: { title: { type: "string" }, bullets: { type: "array", items: { type: "string" } }, notes: { type: "string" } },
              },
            },
          },
        },
      },
      required: ["path"],
    },
  },
  {
    name: "revise_app",
    description:
      "Revise an existing mini-app artifact in place: writes the code as a NEW version on top of base_v (default: the current version) and makes it what Library launches. ALWAYS use this — never generate_app — when the user asks to change, fix, or iterate an app that already exists (its current source is in the conversation wherever the artifact was replied to or @-mentioned). base_v names the version to branch from.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "the artifact id, e.g. R6" },
        base_v: { type: "number", description: "version to revise from (default: current); revising an older version starts a branch" },
        branch_name: { type: "string", description: "optional short name for the branch when base_v isn't the current version (e.g. \"slow-ball\")" },
        code: { type: "string", description: "the complete revised TSX module (same contract as generate_app)" },
        caption: { type: "string", description: "optional one-line markdown caption" },
        title: { type: "string", description: "optional new product name" },
        slug: { type: "string", description: "optional kebab-case display filename" },
      },
      required: ["name", "code"],
    },
  },
  {
    name: "launch_app",
    description: "Open a mini-app artifact in its own pop-out window — the current version unless v is given.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" }, v: { type: "number" } },
      required: ["name"],
    },
  },
  {
    name: "close_app",
    description: "Close a mini-app's open pop-out windows.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    },
  },
  {
    name: "promote_app",
    description: "Promote a mini-app artifact into the user's Mini Apps library (publish). demote_app removes it from Mini Apps without deleting it.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    },
  },
  {
    name: "demote_app",
    description: "Remove a mini-app artifact from Mini Apps (unpublish) — the artifact and its versions stay in the conversation's Library.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    },
  },
  {
    name: "set_version",
    description: "Move an artifact's current pointer to version v — a revert. Later versions stay on the tree; the next revise off a reverted version starts a branch.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" }, v: { type: "number" } },
      required: ["name", "v"],
    },
  },
  {
    name: "view_code",
    description: "Read a mini-app artifact's TSX source — version v, or the current version when v is omitted.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" }, v: { type: "number" } },
      required: ["name"],
    },
  },
  {
    name: "mcp_install",
    description: "Install a stdio MCP server and connect it — its tools become callable from your next step. Use when the user wants an external capability you lack (filesystem, GitHub, Slack, a database, …). command is the executable (npx, uvx, node, python, …), args its arguments, env the KEY=VALUE pairs it needs (ask the user for any secrets; never invent them). scope \"bot\" binds it to this bot only; default shares it with every bot.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "short label, e.g. \"github\"" },
        command: { type: "string" },
        args: { type: "array", items: { type: "string" }, description: "e.g. [\"-y\",\"@modelcontextprotocol/server-github\"]" },
        env: { type: "object", additionalProperties: { type: "string" } },
        scope: { type: "string", enum: ["all", "bot"] },
      },
      required: ["name", "command"],
    },
  },
  {
    name: "mcp_uninstall",
    description: "Remove an MCP server connection entirely — its tools disappear. id is the slug mcp_install returned (mcp_list shows them); or pass the server's name.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "mcp_enable",
    description: "Re-enable a disabled MCP server (id or name) — its tools return next step.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "mcp_disable",
    description: "Disable an MCP server (id or name) without removing its configuration — its tools are withheld until mcp_enable.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "mcp_list",
    description: "List configured MCP servers: id, name, status, tool count, scope, env key names.",
    input_schema: { type: "object", properties: {} },
  },
];

const agentSystem = () => `You are the assistant inside a generative-UI chat app: a chat where you answer in text OR native UI, whichever fits the request.

Reply style: GitHub-flavored markdown, concise. Conversation, questions, and explanations get a text reply — do NOT build a card for those. When the user asks about their own data use query_source first, then answer (or build a widget from what it returns).

When a real interface is what they want — apps, games, trackers, dashboards, forms, anything interactive or persistent — call generate_app once with the complete module. The server typechecks it; if it returns a compiler error, fix the code and call again (a few tries is normal). You may build more than one card in a turn when the request implies it. Always set generate_app's title (a short product name, e.g. "Tiny Trajectory") and slug (kebab-case, e.g. tiny-trajectory) — they name the app in the user's Library.

Artifacts are versioned: every app you mount keeps its versions under one name (R<n>), and each card in the conversation pins the version it rendered. When the user asks to change, fix, restyle, or extend an app that already exists — especially when they replied to its card or @-mentioned it, so its source is attached — call revise_app with that name; never rebuild it as a new artifact. To iterate an older version on purpose, pass base_v — that starts a branch the user can see in the Library's version history. launch_app pops an app into its own window, close_app closes it, promote_app/demote_app publish/unpublish to Mini Apps, set_version reverts the current pointer, view_code reads any version's source.

Compose tools: search the web or query a source first, then build the widget around real results — that composition is the point of this app.

Connected apps: tools named mcp_<server>_<tool> come from MCP servers the user connected — use them like built-in tools; their [server] tag says which app they belong to. When the user wants a capability you don't have, install it yourself with mcp_install (a stdio MCP server command — npx/uvx/node/…); it connects immediately and its mcp_* tools are callable from your next step. mcp_list, mcp_uninstall, mcp_enable and mcp_disable manage what is connected. Never invent env secrets — ask the user for keys a server needs.
Office files: when the user wants a document, spreadsheet, or deck, call office_create with the right payload — never paste file-like text into the reply. The office_* tools revise existing files: office_files lists what's under the workspace (including files from earlier replies), office_open starts a session, grep/cells find exact spans, propose/propose_cells stage changes, office_export writes a new file (it lands as a card). Accept edits only when the user wants the same file updated in place.

When you DO write a widget module, this contract applies:
${contractFor("__NAME__")}`;

// ————— tool executors —————

function imageModule(file, prompt) {
  return `import { signal, Text, View } from "nsh";
import { Image } from "nsh/image";

export default function R() {
  const failed = signal(false);
  return (
    <View class="gap-2">
      <If when={!failed()}>
        <Image src="asset:/generated/assets/${file}" resizeMode="cover" onError={() => failed.set(true)} class="h-64 w-full rounded-xl" />
      </If>
      <If when={failed()}>
        <Text class="text-sm text-zinc-500">image failed to load</Text>
      </If>
      <Text class="text-xs text-zinc-500" numberOfLines={2}>{${JSON.stringify(
        prompt.slice(0, 140),
      )}}</Text>
    </View>
  );
}
`;
}

async function toolGenerateApp(job, thread, input) {
  let code = String(input.code || "").trim();
  if (!code.includes("export default") && /function\s+R\s*\(/.test(code))
    code += "\nexport default R;\n";
  if (!code.includes("export default") || !code.includes("<"))
    return {
      content:
        "not a module: it needs `export default` on a component returning JSX.",
      is_error: true,
    };
  // input.name (set by revise_app) revises an existing artifact: the code is
  // a new version parented to base_v (or current), never an overwrite.
  const revName = /^R\d+$/.test(String(input.name || ""))
    ? String(input.name)
    : null;
  if (revName && !existsSync(join(GEN_DIR, `${revName}.tsx`)))
    return { content: `no such artifact ${revName}`, is_error: true };
  const name = revName || `R${++seq}`;
  setJob(job, "checking", `nsh build + typecheck ${name}.tsx`);
  code = code.split("__NAME__").join(name);
  // Models that self-name their state key (const S = "tipcalc") save under a
  // key the /state/<module> lookups (widget feed, pop-outs) can never find —
  // pin the state const to the real name.
  if (code.includes("/state/"))
    code = code.replace(
      /const\s+(S|NAME|WIDGET|WIDGET_NAME|KEY|MOD|MODULE|SELF)\s*=\s*(["'`])([a-z][\w-]{1,40})\2/,
      (m) => m.replace(/["'`][a-z][\w-]{1,40}["'`]/, JSON.stringify(name)),
    );
  code = fixImports(code);
  writeFileSync(join(STAGE_DIR, `${name}.tsx`), code);
  const err = await checkCandidate(name);
  if (err)
    return {
      content: `nsh check rejected ${name}:\n${err.slice(0, 3000)}\nFix the module and call generate_app again with the corrected code.`,
      is_error: true,
    };
  // Revising off a specific version (base_v) branches; default parent is the
  // current pointer. Nothing earlier is touched — cards keep their version.
  const headBefore = revName ? headOf(name) : null;
  const parent = revName
    ? db.prepare("SELECT v FROM versions WHERE name=? AND v=?").get(name, Number(input.base_v) || 0)
        ? Number(input.base_v)
        : headBefore
    : null;
  const branched = revName && parent != null && parent !== headBefore;
  // The apps row lands FIRST so writeVersion names the file <slug>.v<n>.tsx.
  const prior = db
    .prepare("SELECT title,file FROM apps WHERE name=?")
    .get(name) || {};
  const title = String(input.title || "").trim().slice(0, 60) || prior.title || null;
  const file = slugFile(input.slug) || prior.file || null;
  db.prepare(
    "INSERT OR REPLACE INTO apps(name,title,file) VALUES(?,?,?)",
  ).run(name, title, file);
  const v = writeVersion(
    name, code, parent, job?.prompt || input.caption || "",
    input.branch_name,
  );
  thread.turns.push({
    role: "ai",
    name,
    v,
    label: labelOf(modelCtx.getStore()),
    caption: String(input.caption || ""),
  });
  job.cards = (job.cards || 0) + 1;
  writeIndex();
  console.log(`✓ ${name}@v${v}.tsx via ${labelOf(modelCtx.getStore())} (tool)`);
  const branchNote = branched
    ? ` — this is a new branch off v${parent}${input.branch_name ? ` named "${String(input.branch_name).trim().slice(0, 40)}"` : ""}; mention the fork to the user (they can rename it in Library history)`
    : "";
  return {
    content: `mounted as ${name} v${v}${branchNote} — the card is live in the conversation. Library now launches v${v}; earlier versions stay on the tree.`,
  };
}

async function toolGenerateImage(job, thread, input) {
  const prompt = String(input.prompt || "").trim();
  if (!prompt) return { content: "prompt is required", is_error: true };
  setJob(job, "generating", `image: ${prompt.slice(0, 60)}`);
  const url = `https://image.pollinations.ai/prompt/${encodeURIComponent(
    prompt,
  )}?width=768&height=768&nologo=true&seed=${Math.floor(Math.random() * 1e6)}`;
  const r = await fetch(url, { signal: AbortSignal.timeout(90000) });
  if (!r.ok)
    return { content: `image service returned ${r.status}`, is_error: true };
  const buf = Buffer.from(await r.arrayBuffer());
  if (buf.length < 2000)
    return { content: "image service returned no image", is_error: true };
  const file = `img-${Date.now()}.png`;
  writeFileSync(join(ASSETS_DIR, file), buf);
  const name = `R${++seq}`;
  const v = writeVersion(name, imageModule(file, prompt), null, prompt);
  thread.turns.push({
    role: "ai",
    name,
    v,
    label: "image",
    caption: String(input.caption || ""),
  });
  job.cards = (job.cards || 0) + 1;
  console.log(`✓ ${name}.tsx image (${prompt.slice(0, 50)})`);
  return {
    content: `image generated and mounted as ${name} (asset:/generated/assets/${file}) — to embed it inside a widget, use <Image src="asset:/generated/assets/${file}" …>.`,
  };
}

async function toolFetchUrl(input) {
  const url = String(input.url || "");
  const r = await fetch(url, {
    signal: AbortSignal.timeout(20000),
    redirect: "follow",
    headers: { "user-agent": "Mozilla/5.0 (grok-shell)" },
  });
  if (!r.ok) return { content: `fetch failed: ${r.status}`, is_error: true };
  const html = await r.text();
  const text = html
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/<style[\s\S]*?<\/style>/gi, "")
    .replace(/<[^>]+>/g, " ")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 6000);
  return { content: text || "(empty page)" };
}

async function toolQuerySource(input) {
  const src = String(input.source || "");
  const url =
    src === "linear"
      ? "http://127.0.0.1:8787/data/linear/issues"
      : `http://127.0.0.1:8787/data/${src}`;
  try {
    const r = await fetch(url, { signal: AbortSignal.timeout(25000) });
    const j = await r.json();
    return { content: JSON.stringify(j).slice(0, 12000) };
  } catch (e) {
    return { content: `source error: ${String(e?.message || e)}`, is_error: true };
  }
}

async function toolWebSearch(input) {
  // Client-side fallback (DuckDuckGo html) — only used if the model rejects
  // the Anthropic server-side web_search tool.
  const q = encodeURIComponent(String(input.query || ""));
  const r = await fetch(`https://html.duckduckgo.com/html/?q=${q}`, {
    signal: AbortSignal.timeout(20000),
    headers: { "user-agent": "Mozilla/5.0 (grok-shell)" },
  });
  if (!r.ok) return { content: `search failed: ${r.status}`, is_error: true };
  const html = await r.text();
  const results = [
    ...html.matchAll(
      /<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/g,
    ),
  ]
    .slice(0, 8)
    .map((m) => {
      const href = m[1].replace(/.*uddg=/, "").split("&")[0];
      const title = m[2].replace(/<[^>]+>/g, "").trim();
      return `${title}\n${decodeURIComponent(href)}`;
    });
  return { content: results.join("\n\n") || "(no results)" };
}

// A file an office tool produced becomes a card on its own turn — like a
// widget card, it lands the moment it exists instead of waiting for the reply.
function cardOfficeFile(thread, rel) {
  const p = outFile(rel);
  if (!p || !existsSync(p)) return;
  const m = fileMeta(p);
  thread.turns.push({ role: "ai", label: "office", files: [{ path: rel, ...m }] });
}

async function toolOffice(job, thread, block) {
  const input = block.input || {};
  if (block.name === "office_create") {
    const name = String(input.path || "").split("/").pop() || "";
    const format = /\.xlsx$/i.test(name) ? "xlsx" : /\.pptx$/i.test(name) ? "pptx" : /\.docx$/i.test(name) ? "docx" : null;
    if (!format) return { content: "path must end in .docx, .xlsx, or .pptx", is_error: true };
    const payload = input[format];
    if (!payload) return { content: `a .${format} path requires a ${format} payload`, is_error: true };
    const rel = `o${Date.now().toString(36)}`;
    mkdirSync(join(OUT_DIR, rel), { recursive: true });
    try {
      writeFileSync(join(OUT_DIR, rel, name), await officeCreateBytes(format, { [format]: payload }));
    } catch (e) {
      return { content: `office_create failed: ${String(e?.message || e).slice(0, 400)}`, is_error: true };
    }
    const key = `${rel}/${name}`;
    cardOfficeFile(thread, key);
    console.log(`✓ office ${format} → ${key}`);
    return { content: `created ${key} — the file card is in the reply. To revise it, open this path with office_open and export the result.` };
  }
  const r = await callOfficeTool(OUT_DIR, block.name, input);
  if (r.is_error) return r;
  // office_export's JSON result carries the workspace-relative path of the
  // file it wrote — card it so the new file shows up in the reply.
  if (block.name === "office_export" || block.name === "office_create") {
    try {
      const rel = JSON.parse(r.content).path;
      if (typeof rel === "string" && rel) cardOfficeFile(thread, rel);
    } catch {}
  }
  return r;
}

// Artifact action tools ride the same endpoints the UI uses (self-POST over
// the loopback server — always listening by the time a tool runs), except
// view_code/set_version which are pure reads/writes of the version store.
async function toolArtifact(block, input) {
  const name = String(input.name || "");
  if (!/^R\d+$/.test(name))
    return { content: `bad artifact name ${name}`, is_error: true };
  const self = async (path, b) => {
    const r = await fetch(`http://127.0.0.1:8787${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(b),
    });
    return r.json().catch(() => ({}));
  };
  if (block.name === "launch_app") {
    const r = await self("/popout", { name, ...(input.v ? { v: Number(input.v) } : {}) });
    return r?.ok
      ? { content: `${name}${input.v ? ` v${input.v}` : ""} opened in its own window.` }
      : { content: `launch failed: ${r?.error || "no such module"}`, is_error: true };
  }
  if (block.name === "close_app") {
    let n = 0;
    for (const [k, child] of openWindows)
      if (k.split(":")[1]?.startsWith(`${name}@`) || k.split(":")[1] === name)
        try { child.kill(); n++; } catch {}
    return { content: n ? `closed ${n} window(s).` : `no open window for ${name}.` };
  }
  if (block.name === "promote_app") {
    const r = await self("/publish", { name });
    return r?.ok
      ? { content: `${name} is now in Mini Apps.` }
      : { content: `promote failed: ${r?.error || "?"}`, is_error: true };
  }
  if (block.name === "demote_app") {
    const r = await self("/unpublish", { name });
    return r?.ok
      ? { content: `${name} removed from Mini Apps.` }
      : { content: `demote failed: ${r?.error || "?"}`, is_error: true };
  }
  if (block.name === "set_version") {
    const v = Number(input.v);
    return setHead(name, v)
      ? { content: `${name} now points at v${v} — Library launches it; later versions stay on the tree.` }
      : { content: `no version ${v} on ${name}`, is_error: true };
  }
  if (block.name === "view_code") {
    const v = input.v ? Number(input.v) : headOf(name);
    const file = v ? verFile(name, v) : join(GEN_DIR, `${name}.tsx`);
    if (!existsSync(file))
      return { content: `no source for ${name}${v ? `@v${v}` : ""}`, is_error: true };
    return {
      content: `<code name="${name}@v${v ?? ""}">\n${readFileSync(file, "utf8").slice(0, 60000)}\n</code>`,
    };
  }
  return { content: `unknown artifact tool ${block.name}`, is_error: true };
}

async function execTool(job, thread, block) {
  try {
    if (block.name === "generate_app")
      return await toolGenerateApp(job, thread, block.input || {});
    if (block.name === "revise_app") {
      const input = { ...(block.input || {}) };
      if (!/^R\d+$/.test(String(input.name || "")))
        return { content: "revise_app needs the artifact name (R<n>)", is_error: true };
      return await toolGenerateApp(job, thread, input);
    }
    if (block.name === "generate_image")
      return await toolGenerateImage(job, thread, block.input || {});
    if (block.name === "fetch_url") return await toolFetchUrl(block.input || {});
    if (block.name === "query_source")
      return await toolQuerySource(block.input || {});
    if (block.name === "web_search")
      return await toolWebSearch(block.input || {});
    if (/^(launch_app|close_app|promote_app|demote_app|set_version|view_code)$/.test(block.name))
      return await toolArtifact(block, block.input || {});
    if (block.name.startsWith("office_"))
      return await toolOffice(job, thread, block);
    if (/^mcp_(install|uninstall|enable|disable|list)$/.test(block.name))
      return await toolMcpAdmin(thread, block.name, block.input || {});
    const mm = parseMcpName(block.name);
    if (mm) {
      const row = db
        .prepare("SELECT * FROM mcp_servers WHERE id=?")
        .get(mm[0]);
      if (!row) return { content: `mcp server ${mm[0]} is not configured`, is_error: true };
      setJob(job, "tool", `${row.name || mm[0]}: ${mm[1]}`);
      return await mcpCallTool(row, mm[1], block.input || {});
    }
    return { content: `unknown tool ${block.name}`, is_error: true };
  } catch (e) {
    return {
      content: `tool error: ${String(e?.message || e).slice(0, 500)}`,
      is_error: true,
    };
  }
}

// The Anthropic server-side web_search tool gets one shot — if this build of
// the API/model rejects it we drop it and use the client-side version.
let serverSearch = true;
let agentStream = true;
const TOOL_ACTIVITY = {
  web_search: ["searching", "Searching the web"],
  fetch_url: ["browsing", "Reading the web"],
  generate_app: ["writing", "Drafting the file"],
  generate_image: ["generating", "Generating a photo"],
  query_source: ["connecting", "Connecting to a third party app"],
  mcp_install: ["connecting", "Installing an app connection"],
  mcp_uninstall: ["working", "Removing an app connection"],
  mcp_enable: ["connecting", "Enabling an app connection"],
  mcp_disable: ["working", "Disabling an app connection"],
  mcp_list: ["working", "Listing app connections"],
};

function toolActivity(name, input = {}) {
  if (name === "query_source" && typeof input?.source === "string" && input.source.trim()) {
    const source = input.source
      .trim()
      .replace(/[-_]+/g, " ")
      .replace(/\b\w/g, (letter) => letter.toUpperCase());
    return ["connecting", `Connecting to ${source}`];
  }
  return TOOL_ACTIVITY[name] || ["working", "Working"];
}

// A thread's bot scopes which connected MCP servers the agent sees:
// global servers (bot IS NULL) plus servers bound to this bot.
async function mcpToolDefs(thread) {
  const bot = store.bots.find((b) => b.threadId === thread?.id);
  const rows = db
    .prepare("SELECT * FROM mcp_servers WHERE enabled=1 AND (bot IS NULL OR bot=?)")
    .all(bot?.id || "");
  const defs = [];
  for (const row of rows) {
    const rec = await mcpEnsure(row);
    defs.push(...mcpAnthropicTools(row.id, rec.tools));
  }
  return defs;
}

async function toolList(thread) {
  // The bundled BetterOffice MCP server contributes its office_* surface once
  // connected (office_create stays ours — its version mounts the file card).
  const office = officeMcpTools();
  const mcp = await mcpToolDefs(thread);
  return serverSearch
    ? [{ type: "web_search_20250305", name: "web_search", max_uses: 3 }, ...TOOLS, ...office, ...mcp]
    : [...TOOLS, ...office, ...mcp, CLIENT_SEARCH];
}

// Shared by POST /mcp/servers and the mcp_install agent tool. Omitted fields
// keep their stored values so a partial call edits an existing row.
async function mcpUpsertServer({ id, name, command, args, env, bot } = {}) {
  const cur = id
    ? db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(id))
    : null;
  const finalName = String(name ?? cur?.name ?? "").trim();
  const finalCommand = String(command ?? cur?.command ?? "").trim();
  if (!finalName) throw new Error("server needs a name");
  if (!finalCommand) throw new Error("server needs a command");
  const sid =
    (id && String(id)) ||
    (() => {
      const base = finalName.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24) || "srv";
      let s = base, i = 2;
      while (db.prepare("SELECT id FROM mcp_servers WHERE id=?").get(s)) s = `${base}-${i++}`;
      return s;
    })();
  const argList =
    args === undefined
      ? JSON.parse(cur?.args || "[]")
      : Array.isArray(args)
        ? args.map(String)
        : String(args || "").trim().split(/\s+/).filter(Boolean);
  const envObj =
    env === undefined
      ? JSON.parse(cur?.env || "{}")
      : env && typeof env === "object"
        ? env
        : Object.fromEntries(
            String(env || "")
              .split("\n")
              .map((l) => l.match(/^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/))
              .filter(Boolean)
              .map((m) => [m[1], m[2]]),
          );
  const botId =
    bot === undefined ? cur?.bot ?? null
    : bot && store.bots.some((b) => b.id === bot) ? bot
    : null;
  db.prepare(
    "INSERT OR REPLACE INTO mcp_servers(id,name,command,args,env,enabled,bot,created_at) VALUES(?,?,?,?,?,COALESCE((SELECT enabled FROM mcp_servers WHERE id=?),1),?,COALESCE((SELECT created_at FROM mcp_servers WHERE id=?),?))",
  ).run(sid, finalName.slice(0, 60), finalCommand, JSON.stringify(argList), JSON.stringify(envObj), sid, botId, sid, Date.now());
  mcpInvalidate(sid); // config changed → drop any cached client
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(sid);
  const rec = await mcpEnsure(row); // probe now so the reply carries real status
  const st = mcpStatus(sid);
  return {
    id: sid,
    status: rec.client ? "connected" : "error",
    tools: st.tools,
    toolNames: rec.tools.map((t) => mcpAnthropicTools(sid, [t])[0]?.name ?? ""),
    err: st.err || null,
  };
}

// Resolve a server row by id, or by name (exact, then slugged).
function mcpServerRow({ id, name } = {}) {
  if (id)
    return db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(id));
  const n = String(name || "").trim();
  if (!n) return null;
  const slug = n.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24);
  return (
    db.prepare("SELECT * FROM mcp_servers WHERE name=?").get(n) ||
    db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(slug)
  );
}

// mcp_install / mcp_uninstall / mcp_enable / mcp_disable / mcp_list — the
// model manages the same mcp_servers registry the Connect tab edits.
async function toolMcpAdmin(thread, tool, input) {
  if (tool === "mcp_list") {
    const rows = db.prepare("SELECT * FROM mcp_servers ORDER BY created_at").all();
    if (!rows.length) return { content: "no MCP servers configured" };
    const line = (r) => {
      const s = mcpStatus(r.id);
      const envKeys = Object.keys(JSON.parse(r.env || "{}"));
      return `${r.id} — ${r.name} · ${r.enabled ? s.status + ` (${s.tools} tools)` : "disabled"} · scope: ${r.bot ? `bot ${r.bot}` : "all bots"}${envKeys.length ? ` · env: ${envKeys.join(",")}` : ""}${s.err ? ` · err: ${s.err}` : ""}`;
    };
    return { content: rows.map(line).join("\n") };
  }
  if (tool === "mcp_install") {
    const bot = store.bots.find((b) => b.threadId === thread?.id);
    const r = await mcpUpsertServer({
      name: input.name,
      command: input.command,
      args: input.args,
      env: input.env,
      bot: input.scope === "bot" && bot ? bot.id : null,
    });
    if (r.status !== "connected")
      return { content: `mcp install of "${input.name}" failed: ${r.err || "could not connect"} — check the command and args`, is_error: true };
    return { content: `installed "${input.name}" as server ${r.id} — connected, ${r.tools} tool(s) now callable: ${r.toolNames.join(", ") || "none discovered"}` };
  }
  const row = mcpServerRow(input);
  if (!row)
    return { content: `no MCP server matches ${input.id || input.name || "?"} — mcp_list shows the ids`, is_error: true };
  if (tool === "mcp_uninstall") {
    mcpInvalidate(row.id);
    db.prepare("DELETE FROM mcp_servers WHERE id=?").run(row.id);
    return { content: `removed MCP server ${row.id} (${row.name}) — its tools are gone` };
  }
  const enabled = tool === "mcp_enable";
  db.prepare("UPDATE mcp_servers SET enabled=? WHERE id=?").run(enabled ? 1 : 0, row.id);
  mcpInvalidate(row.id);
  return { content: `${row.id} (${row.name}) ${enabled ? "enabled — its tools return next step" : "disabled — its tools are withheld"}` };
}

async function agentTurn(job, prompt, thread) {
  const messages = [
    ...thread.turns.slice(-10).map((t) =>
      t.role === "user"
        ? { role: "user", content: t.text }
        : {
            role: "assistant",
            content: t.name ? `[mounted card ${t.name}]` : t.text || "",
          },
    ),
    { role: "user", content: await userContent(prompt, job.extra, job.ctx) },
  ];
  let reply = "";
  for (let i = 0; i < 8; i++) {
    const thinkingDetail = i === 0 ? `${labelOf(modelCtx.getStore())} is thinking` : "continuing…";
    setJob(job, "thinking", thinkingDetail);
    let msg;
    const t0 = Date.now();
    let events = 0;
    try {
      const params = {
        model: MODEL,
        max_tokens: MAX_TOKENS,
        system: agentSystem(),
        tools: await toolList(thread),
        messages,
      };
      if (agentStream) {
        const stream = anthropic.messages.stream(params);
        stream.on("streamEvent", (event) => {
          events++;
          if (event.type !== "content_block_start") return;
          const block = event.content_block;
          if (block.type === "tool_use") setJob(job, ...toolActivity(block.name, block.input));
          else if (block.type === "server_tool_use") setJob(job, "searching", "Searching the web");
          else if (block.type === "text") setJob(job, "streaming", "Replying");
          else if (block.type === "thinking" || block.type === "redacted_thinking")
            setJob(job, "thinking", thinkingDetail);
        });
        msg = await stream.finalMessage();
      } else {
        msg = await anthropic.messages.create(params);
      }
    } catch (e) {
      const m = String(e?.message || "");
      if (serverSearch && /web_search|server_tool|tool_use/i.test(m)) {
        console.error(`  server web_search rejected (${m.slice(0, 120)}) — using client search`);
        serverSearch = false;
        continue;
      }
      if (agentStream && events === 0) {
        console.error(`  Messages streaming unavailable (${m.slice(0, 160)}) — using create`);
        agentStream = false;
        continue;
      }
      throw new Error(`llm-api ${e.status ?? ""}: ${m}`);
    }
    messages.push({ role: "assistant", content: msg.content });
    console.log(
      `  · call ${i}: ${Date.now() - t0}ms stop=${msg.stop_reason} blocks=${msg.content.map((b) => b.type).join(",")}`,
    );
    // pause_turn = a server-side tool (web_search) is still mid-run — keep
    // polling with the same messages; tool_use = a client tool needs us.
    if (msg.stop_reason === "pause_turn") continue;
    const uses = msg.content.filter((b) => b.type === "tool_use");
    if (msg.stop_reason !== "tool_use" || !uses.length) {
      reply = msg.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("")
        .trim();
      break;
    }
    const results = [];
    for (const u of uses) {
      setJob(job, ...toolActivity(u.name, u.input));
      results.push({
        type: "tool_result",
        tool_use_id: u.id,
        ...(await execTool(job, thread, u)),
      });
    }
    messages.push({ role: "user", content: results });
    if (msg.stop_reason === "max_tokens") break;
  }
  return reply;
}

// A reply on an artifact: the file itself becomes context — images as image
// blocks, text/code/json/html/csv verbatim, xlsx as tab-separated sheets,
// docx as its text; anything else (audio, video, pdf…) is described.
const IMG_TYPE = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp" };
async function fileBody(p, m) {
  if (["markdown", "text", "json", "html"].includes(m.kind) || m.ext === "svg") return readFileSync(p, "utf8");
  if (m.kind === "table") return ((await renderFile(p)).sheets || []).map((s) => `# sheet: ${s.name}\n` + s.rows.map((r) => r.join("\t")).join("\n")).join("\n\n");
  if (m.kind === "docx") return (await renderFile(p)).text || "";
  if (m.kind === "pptx") return await officeMarkdown(p);
  return null;
}
async function artifactBlocks(rels) {
  const blocks = [];
  for (const rel of (rels || []).slice(0, 8)) {
    const p = outFile(String(rel));
    if (!p || !existsSync(p)) continue;
    const m = fileMeta(p);
    try {
      if (m.kind === "image" && IMG_TYPE[m.ext]) {
        blocks.push({ type: "image", source: { type: "base64", media_type: IMG_TYPE[m.ext], data: readFileSync(p).toString("base64") } });
        blocks.push({ type: "text", text: `(the image above is ${m.name}, ${m.width}×${m.height})` });
        continue;
      }
      const body = await fileBody(p, m);
      if (body != null) blocks.push({ type: "text", text: `<file name="${m.name}">\n${body.slice(0, 60000)}\n</file>` });
      else blocks.push({ type: "text", text: `<file name="${m.name}" kind="${m.kind}" size="${m.size}">(binary ${m.kind} file; its contents can't be shown as text)</file>` });
    } catch {}
  }
  return blocks;
}

// The model's view of a user turn: the quoted message it replies to, then
// the text, with attached images as image blocks and documents inlined.
async function userContent(prompt, extra, ctx = []) {
  const quote = extra?.reply?.text ? `> ${extra.reply.text.replace(/\n/g, "\n> ")}\n\n` : "";
  const text = quote + (prompt || "(see the attachments)");
  const files = (extra?.attachments || []).filter(
    (a) => a.uri?.startsWith("file://") || a.uri?.startsWith("module://"),
  );
  if (!files.length && !ctx.length) return text;
  const blocks = [...ctx];
  for (const a of files) {
    // module://R<n>[@v<k>] resolves to the artifact's TSX (that exact
    // version when pinned) — reply-attaching a mini app hands the model its
    // full source so it can iterate on it.
    const mm = a.uri.match(/^module:\/\/(R\d+)(?:@v(\d+))?$/);
    const path = mm
      ? mm[2]
        ? join(GEN_DIR, mm[1], `v${mm[2]}.tsx`)
        : [GEN_DIR, WIDGETS_DIR]
            .map((d) => join(d, `${mm[1]}.tsx`))
            .find(existsSync)
      : a.uri.startsWith("module://")
        ? null
        : decodeURIComponent(a.uri.slice(7));
    if (!path) continue;
    try {
      if (a.kind === "image") {
        blocks.push({ type: "image", source: { type: "base64", media_type: "image/png", data: readFileSync(path).toString("base64") } });
      } else {
        const m = fileMeta(path, a.name);
        const body = await fileBody(path, m);
        if (body != null) blocks.push({ type: "text", text: `<file name="${a.name}">\n${body.slice(0, 60000)}\n</file>` });
        else blocks.push({ type: "text", text: `<file name="${a.name}" kind="${m.kind}" size="${m.size}">(binary ${m.kind} file; its contents can't be shown as text)</file>` });
      }
    } catch {}
  }
  return [...blocks, { type: "text", text }];
}

async function generate(job, prompt) {
  // The job stays on the thread it was sent from — you can switch threads
  // while a generation is still running and it lands where it belongs.
  const thread =
    store.list.find((t) => t.id === job.thread) || activeThread();
  let name, aiTurn;
  // "/mock files": a canned reply carrying one file of every kind — tests the
  // viewers without a model call
  if (!job.retry && /^\/mock\b/i.test(prompt.trim())) {
    const rel = `m${Date.now().toString(36)}`;
    const dir = join(OUT_DIR, rel);
    const paths = mockFiles(dir);
    // the office samples are the real BetterOffice demo docs (server/office/
    // seeds/ — betteroffice-demo.docx, showcase.xlsx, presentation.pptx), so
    // the cards exercise the editor on real-world content; when a seed is
    // absent we still emit the generated fallback file
    const SEEDS = join(dirname(fileURLToPath(import.meta.url)), "office", "seeds");
    // The pptx fallback depends on the optional BetterOffice engine — absent,
    // /mock still works and the card simply isn't produced.
    const fallback = {
      docx: () => mockDocx(dir),
      xlsx: () => mockXlsx(dir),
      pptx: () => mockPptx(dir).catch((e) => (console.error(`  mock pptx: ${String(e?.message || e).slice(0, 120)}`), null)),
    };
    for (const [ext, file] of [["docx", "betteroffice-demo.docx"], ["xlsx", "showcase.xlsx"], ["pptx", "presentation.pptx"]]) {
      const s = join(SEEDS, file);
      if (existsSync(s)) {
        const p = join(dir, file);
        copyFileSync(s, p);
        paths.push(p);
      } else {
        const p = await fallback[ext]();
        if (p) paths.push(p);
      }
    }
    // a screen recording as the video sample, twice (gallery of two)
    const movs = readdirSync(ATTACH_DIR).filter((f) => f.endsWith(".mov"));
    if (movs.length) {
      copyFileSync(join(ATTACH_DIR, movs[0]), join(dir, "demo.mov"));
      copyFileSync(join(ATTACH_DIR, movs[0]), join(dir, "replay.mov"));
      paths.push(join(dir, "demo.mov"), join(dir, "replay.mov"));
    }
    const files = paths.map((p) => {
      const m = fileMeta(p);
      return { path: `${rel}/${p.split("/").pop()}`, ...m, ...(m.poster ? { poster: `${rel}/${m.poster}` } : {}) };
    });
    // an image whose file is gone: the gallery shows "Image unavailable"
    const firstImg = files.findIndex((f) => f.kind === "image");
    files.splice(firstImg + 1, 0, { path: `${rel}/missing.png`, name: "missing.png", ext: "png", size: 0, kind: "image", variant: "media" });
    // "/mock media": just the gallery (images · videos) and the diagram
    if (/media/i.test(prompt)) files.splice(0, files.length, ...files.filter((f) => f.kind === "image" || f.kind === "video"));
    if (thread.turns.length === 0) thread.title = "Mock files";
    thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    thread.turns.push({
      role: "ai",
      text: `Here are ${files.length} sample files — one of each kind the viewer handles.\n\n\`\`\`mermaid\nflowchart LR\n  A[You ask] --> B[Kiffy makes the file] --> C[You click to view]\n\`\`\``,
      label: "mock",
      files,
    });
    writeIndex();
    setJob(job, "done", "replied");
    job.done = true;
    job.result = "reply";
    return;
  }
  // A module://R<n>[@v<k>] attachment = "iterate on this artifact": the run
  // produces a new version of it (reply-to-revise), not a new artifact.
  const attachRev = (job.extra?.attachments || [])
    .map((a) => (a.uri || "").match(/^module:\/\/(R\d+)(?:@v(\d+))?$/) || null)
    .find(Boolean);
  if (attachRev) job.revise = { name: attachRev[1], base_v: Number(attachRev[2]) || null };
  const useTools = !MOCK && PROVIDER === "anthropic";
  if (!job.retry && useTools) {
    // Tool-call agent — the model decides: text answer, tool calls, widget
    // cards, or a composition (search → build). Cards land as they pass the
    // compiler; the final text answer lands last.
    if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
    thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    const text = await agentTurn(job, prompt, thread);
    if (text) thread.turns.push({ role: "ai", text, label: labelOf(modelCtx.getStore()) });
    writeIndex();
    console.log(`✓ turn via ${labelOf(modelCtx.getStore())} (${prompt.slice(0, 60)}) — ${job.cards || 0} card(s)`);
    setJob(job, "done", job.cards ? `${job.cards} card(s)` : "replied");
    job.done = true;
    job.result = job.cards ? "cards" : "reply";
    return;
  }
  if (!job.retry) {
    // Conversation ≠ generation — "hi" gets a text reply, not a greeting card.
    // a turn carrying files is a question about them: the widget generator
    // never sees file contents, so it always answers in chat
    // a bare module attach still wants generation — it's a revise request.
    const nonModule = (job.extra?.attachments || []).some(
      (a) => !a.uri?.startsWith("module://"),
    );
    const hasFiles = !!job.ctx?.length || nonModule;
    const intent = job.revise
      ? "ui"
      : !hasFiles
        ? await classifyIntent(prompt)
        : "chat";
    if (intent === "chat") {
      setJob(job, "thinking", `${labelOf(modelCtx.getStore())} is replying`);
      if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
      const text = await chatReply(prompt, thread, job);
      thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
      thread.turns.push({ role: "ai", text, label: labelOf(modelCtx.getStore()) });
      writeIndex();
      console.log(`✓ reply (${prompt.slice(0, 60)})`);
      setJob(job, "done", "replied");
      job.done = true;
      job.result = "reply";
      return;
    }
  }
  // Revises (Try-again, or a module:// attachment = reply-to-iterate) mint a
  // NEW version of the same artifact: the old card keeps its version and a
  // fresh card lands. baseV records the branch point — the card's own
  // version for a retry, the explicit @v / current pointer for an attach.
  let baseV = null;
  if (job.retry || job.revise) {
    name = job.retry || job.revise.name;
    const prior =
      thread.turns.find((t) => t.role === "ai" && t.name === name && t.widget) ||
      thread.turns.find((t) => t.role === "ai" && t.name === name);
    baseV = job.retry
      ? prior?.v ?? headOf(name)
      : (job.revise.base_v ?? headOf(name)) ?? prior?.v ?? null;
    if (!job.retry) {
      // A reply-to-iterate carries a real user prompt; Try-again reuses the
      // card's original one — no duplicate user turn.
      if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
      thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    }
    aiTurn = {
      role: "ai",
      name,
      label: labelOf(modelCtx.getStore()),
      retries: (prior?.retries || 0) + 1,
    };
    thread.turns.push(aiTurn);
  } else {
    seq += 1;
    name = `R${seq}`;
    if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
    thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    aiTurn = { role: "ai", name, label: labelOf(modelCtx.getStore()) };
    thread.turns.push(aiTurn);
  }

  // llm → llm with the compiler error → canned mock, whichever compiles first.
  const useLlm =
    !MOCK &&
    (PROVIDER === "anthropic" || !!process.env.OPENROUTER_API_KEY);
  let lastCode;
  let lastCaption = "";
  let err = null;
  let ok = false;
  let used = useLlm ? MODEL : "mock";

  const tryCode = async (code) => {
    lastCode = fixImports(code);
    // check in staging — nothing lands in generated/ until it compiles
    writeFileSync(join(STAGE_DIR, `${name}.tsx`), lastCode);
    setJob(job, "checking", `nsh build + typecheck ${name}.tsx`);
    err = await checkCandidate(name);
    return err === null;
  };

  if (useLlm)
    for (let attempt = 0; attempt < 3 && !ok; attempt++) {
      if (attempt > 0)
        setJob(
          job,
          "retrying",
          `compile error — feeding it back to the model (attempt ${attempt + 1})`,
        );
      else setJob(job, "thinking", `${labelOf(modelCtx.getStore())} is working on it`);
      try {
        const r = await askModel(job, prompt, err, lastCode, name);
        lastCaption = r.caption || "";
        ok = await tryCode(r.code);
        if (!ok)
          console.error(
            `  attempt ${attempt + 1} rejected by nsh check: ${err?.split("\n")[0]}`,
          );
      } catch (e) {
        err = String(e?.message || e);
        console.error(`  llm call failed: ${err.split("\n")[0]}`);
        if (err.startsWith("llm-api")) break; // API dead — don't retry it
      }
    }

  if (!ok) {
    used = useLlm ? "mock-fallback" : "mock";
    setJob(job, "fallback", "model path failed — building a canned module");
    ok = await tryCode(mockModule(prompt)).catch(() => false);
  }
  aiTurn.label = used;
  aiTurn.caption = ok && used === MODEL ? lastCaption : "";
  if (!ok) console.error(`✗ ${name}: generation failed\n${err}`);
  setJob(job, "publishing", `hot-patching ${name}.tsx into the app`);
  // Write the module FIRST, then pause so the dev server's watcher registers
  // it before index.tsx starts importing it — otherwise the rebuild races the
  // new file and the device shows [UNRESOLVED_IMPORT]. A published module is
  // the live one — the shelf and pop-out wrappers import widgets/<name> — so
  // when that file exists the retry must patch it, not a stale generated copy.
  // Versioned write: generated/<name>/v<k>.tsx is immutable, the bare
  // <name>.tsx (and a published widget copy) mirror the new head.
  const v = writeVersion(name, ok ? lastCode : errorCard(err || "unknown"), baseV, prompt);
  aiTurn.v = v;
  await new Promise((r) => setTimeout(r, 500));
  writeIndex(); // one live write → one clean hot patch on the device
  const line = `${name}.tsx via ${used}`;
  console.log(`${ok ? "✓" : "✗"} ${line} (${prompt.slice(0, 60)})`);
  setJob(job, "done", `${ok ? "rendered" : "failed"} — ${line}`);
  job.done = true;
  job.result = line;
}

// ————— canned modules for MOCK mode / offline —————

function mockModule(prompt) {
  const p = prompt.toLowerCase();
  if (/counter|count|stepper/.test(p))
    return `import { signal, Text, View } from "nsh";

export default function R() {
  const n = signal(0);
  return (
    <View class="items-center gap-3 py-2">
      <Text class="text-5xl font-bold text-zinc-900">{n()}</Text>
      <View class="flex-row gap-3">
        <View onPress={() => n.update((v) => v - 1)} activeOpacity={0.6} class="rounded-lg bg-zinc-200 px-5 py-2"><Text class="text-lg">−</Text></View>
        <View onPress={() => n.update((v) => v + 1)} activeOpacity={0.6} class="rounded-lg bg-blue-600 px-5 py-2"><Text class="text-lg font-semibold text-white">+</Text></View>
      </View>
    </View>
  );
}
`;
  if (/chart|graph|bar|week/.test(p))
    return `import { Text, View } from "nsh";

const days = ["M", "T", "W", "T", "F", "S", "S"];
const vals = [42, 68, 35, 90, 74, 20, 55];

export default function R() {
  return (
    <View class="gap-2 py-2">
      <Text class="text-sm font-semibold text-zinc-800">Sessions this week</Text>
      <View class="h-32 flex-row items-end gap-2">
        <Each of={vals}>{(v, i) => (
          <View class="flex-1 items-center gap-1">
            <View class="w-full rounded-t-md bg-blue-500" style={{ height: v() }} />
            <Text class="text-[10px] text-zinc-500">{days[i()]}</Text>
          </View>
        )}
        </Each>
      </View>
    </View>
  );
}
`;
  if (/form|book|sign|input/.test(p))
    return `import { Input, signal, Text, View } from "nsh";

export default function R() {
  const name = signal("");
  const when = signal("");
  const sent = signal(false);
  return (
    <View class="gap-2 py-1">
      <Text class="text-sm font-semibold text-zinc-800">Book a table</Text>
      <Input value={name()} onChangeText={(t) => name.set(t)} placeholder="Name" class="rounded-lg border border-zinc-300 px-3 py-2 text-sm" />
      <Input value={when()} onChangeText={(t) => when.set(t)} placeholder="Friday, 8 pm" class="rounded-lg border border-zinc-300 px-3 py-2 text-sm" />
      <View onPress={() => sent.set(true)} activeOpacity={0.7} class="items-center rounded-lg bg-blue-600 py-2">
        <Text class="text-sm font-semibold text-white">Reserve</Text>
      </View>
      <If when={sent()}><Text class="text-xs text-green-600">Reserved for {name() || "—"}, {when() || "—"} ✓</Text></If>
    </View>
  );
}
`;
  if (/todo|task|check|list/.test(p))
    return `import { signal, Text, View } from "nsh";

const items0 = ["Ship nsh demo", "Write docs", "Test on iOS"];
const done = signal([false, false, false]);

export default function R() {
  return (
    <View class="gap-1 py-1">
      <Each of={items0}>{(t, i) => (
        <View onPress={() => done.update((d) => d.map((v, j) => (j === i() ? !v : v)))} activeOpacity={0.6} class="flex-row items-center gap-2 py-1">
          <View classList={{ "bg-blue-600 border-blue-600": done()[i()] }} class="h-4 w-4 rounded border border-zinc-400" />
          <Text classList={{ "line-through text-zinc-400": done()[i()] }} class="text-sm text-zinc-800">{t()}</Text>
        </View>
      )}
      </Each>
    </View>
  );
}
`;
  if (/stock|nvda|aapl|ticker|price/.test(p))
    return `import { Text, View } from "nsh";

export default function R() {
  return (
    <View class="gap-1 py-1">
      <View class="flex-row items-baseline justify-between">
        <Text class="text-base font-bold text-zinc-900">NVDA</Text>
        <Text class="text-base font-semibold text-green-600">▲ 8.6%</Text>
      </View>
      <Text class="text-3xl font-bold text-zinc-900">$186.42</Text>
      <Text class="text-xs text-zinc-500">NVIDIA Corp · close Fri · mock data</Text>
    </View>
  );
}
`;
  return `import { signal, Text, View } from "nsh";

export default function R() {
  const liked = signal(false);
  return (
    <View class="gap-2 py-1">
      <Text class="text-sm text-zinc-700">{${JSON.stringify(
        `You asked for: ${prompt}`,
      )}}</Text>
      <View onPress={() => liked.update((v) => !v)} activeOpacity={0.6} class="self-start rounded-lg bg-zinc-100 px-3 py-1.5">
        <Text class="text-sm">{liked() ? "♥ liked" : "♡ like"}</Text>
      </View>
    </View>
  );
}
`;
}

const server = createServer((req, res) => {
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Headers", "content-type");
  if (req.method === "OPTIONS") return res.end();
  if (req.method === "GET" && (req.url === "/" || req.url === "/health")) {
    res.setHeader("content-type", "text/plain");
    return res.end(
      `app-server ok — ${store.list.length} threads, model=${MOCK ? "mock" : MODEL}\n`,
    );
  }
  if (req.method === "POST" && req.url === "/chat") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      try {
        const { prompt: rawPrompt, reply, attachments } = JSON.parse(body || "{}");
        const prompt = rawPrompt || (attachments?.length ? "" : null);
        if (prompt == null) throw new Error("missing prompt");
        const job = {
          id: nextJobId++,
          thread: store.active,
          seq: 0,
          phase: "queued",
          detail: "",
          done: false,
          result: null,
          extra: {
            ...(reply?.text ? { reply: { i: reply.i, text: String(reply.text).slice(0, 400), ...(reply.files?.length ? { files: reply.files.slice(0, 8).map(String) } : {}) } } : {}),
            ...(attachments?.length ? { attachments } : {}),
          },
        };
        jobs.set(job.id, job);
        if (jobs.size > 25) jobs.delete(jobs.keys().next().value);
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, id: job.id }));
        // a reply on an artifact: read the file(s) into the model's context first
        artifactBlocks(job.extra.reply?.files)
          .then((ctx) => {
            job.ctx = ctx;
            return modelCtx.run(choiceFor(job.thread), () => generate(job, prompt)); // files land after the response; nsh dev hot-patches
          })
          .catch((e) => {
            console.error(`✗ job ${job.id} crashed:`, e);
            job.done = true;
            setJob(job, "done", `generator error — ${e.message}`);
          });
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "GET" && req.url === "/threads") {
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify({
        active: activeThread().id,
        model: MOCK ? "mock" : MODEL,
        list: store.list.map((t) => {
          const bot = store.bots.find((b) => b.threadId === t.id);
          return {
            id: t.id,
            title: t.title,
            count: t.turns.length,
            lastApp: [...t.turns].reverse().find((x) => x.name)?.name || "",
            bot: bot
              ? { id: bot.id, name: bot.name, shape: bot.shape, color: bot.color }
              : null,
          };
        }),
        sections: store.sections || [],
        bots: store.bots.map((b) => {
          const t = store.list.find((x) => x.id === b.threadId);
          // the subtitle / hover popover: the latest message, flattened
          const last = t && [...t.turns].reverse().find((x) => x.text || x.caption);
          return {
            id: b.id,
            name: b.name,
            shape: b.shape,
            color: b.color,
            threadId: b.threadId,
            pinned: !!b.pinned,
            section: b.section || "",
            hidden: !!b.hidden,
            image: b.image || "",
            crop: b.crop || null,
            model: b.model || "",
            effort: b.effort || "",
            fast: !!b.fast,
            preview: String(last?.text || last?.caption || "").replace(/[#*`>_]/g, "").replace(/\s+/g, " ").trim().slice(0, 160),
          };
        }),
      }),
    );
  }
  // A bot is a named agent persona (blob avatar: shape + color) that owns
  // one thread — Grok Bot's model.
  if (req.method === "POST" && req.url === "/bot/new") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name, shape, color } = JSON.parse(body || "{}");
        if (!name?.trim()) throw new Error("bot needs a name");
        const tid = `t${threadSeq++}`;
        const id = `b${Date.now().toString(36)}${Math.floor(Math.random() * 46656).toString(36)}`;
        store.list.unshift({ id: tid, title: name.trim(), turns: [] });
        store.bots.push({
          id,
          name: name.trim(),
          shape: shape || "blob",
          color: color || "#1084FE",
          threadId: tid,
        });
        store.active = tid;
        writeIndex();
        persist();
        res.setHeader("content-type", "application/json");
        res.end(
          JSON.stringify({
            ok: true,
            bot: {
              id,
              name: name.trim(),
              shape: shape || "blob",
              color: color || "#1084FE",
              threadId: tid,
            },
            turns: turnsJson(),
          }),
        );
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/bot/update") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id, name, shape, color, pinned, section, hidden, image, crop, model, effort, fast } = JSON.parse(body || "{}");
        const bot = store.bots.find((b) => b.id === id);
        if (!bot) throw new Error(`unknown bot ${id}`);
        if (name?.trim()) bot.name = name.trim();
        if (shape) bot.shape = shape;
        if (color) bot.color = color;
        if (pinned !== undefined) bot.pinned = !!pinned;
        if (hidden !== undefined) bot.hidden = !!hidden;
        if (image !== undefined) bot.image = String(image);
        if (crop !== undefined) bot.crop = crop;
        if (model !== undefined) bot.model = String(model);
        if (effort !== undefined) bot.effort = String(effort);
        if (fast !== undefined) bot.fast = !!fast;
        if (section !== undefined) {
          bot.section = String(section).trim();
          store.sections ??= [];
          if (bot.section && !store.sections.includes(bot.section)) store.sections.push(bot.section);
        }
        const thread = store.list.find((t) => t.id === bot.threadId);
        if (thread) thread.title = bot.name;
        persist();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // Deleting a bot removes its thread AND the artifacts its turns point at:
  // the generated module, its /state rows, its apps meta. Published Mini
  // Apps survive — their live copy is widgets/<name>.tsx, untouched here.
  if (req.method === "POST" && req.url === "/bot/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id } = JSON.parse(body || "{}");
        const bot = store.bots.find((b) => b.id === id);
        if (!bot) throw new Error(`unknown bot ${id}`);
        const thread = store.list.find((t) => t.id === bot.threadId);
        const published = new Set(
          db.prepare("SELECT name FROM widgets").all().map((w) => w.name),
        );
        for (const u of thread?.turns || []) {
          const name = u.role === "ai" ? u.name : "";
          if (!/^R\d+$/.test(name || "") || published.has(name)) continue;
          try {
            unlinkSync(join(GEN_DIR, `${name}.tsx`));
          } catch {}
          db.prepare("DELETE FROM kv WHERE name=?").run(name);
          db.prepare("DELETE FROM apps WHERE name=?").run(name);
        }
        store.bots = store.bots.filter((b) => b.id !== id);
        store.list = store.list.filter((t) => t.id !== bot.threadId);
        // never leave the app threadless — /threads dereferences one
        if (!store.list.length)
          store.list.push({ id: `t${threadSeq++}`, title: "new chat", turns: [] });
        if (store.active === bot.threadId) store.active = store.list[0].id;
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // Image avatars (Grok's Generate / Upload tabs): files land in
  // generated/avatars and the app shows them by file:// URI.
  // Mermaid, rendered by mermaid.js in a web view (served locally: the web
  // view's grant is 127.0.0.1). The page reports its size; "full" mode zooms.
  if (req.method === "GET" && req.url === "/mermaid.js") {
    res.setHeader("content-type", "text/javascript");
    return res.end(readFileSync(new URL("../node_modules/mermaid/dist/mermaid.min.js", import.meta.url)));
  }
  if (req.method === "GET" && req.url.startsWith("/mermaid?")) {
    const q = new URL(req.url, "http://x").searchParams;
    const code = Buffer.from(q.get("d") || "", "base64url").toString("utf8");
    const dark = q.get("theme") === "dark";
    const full = q.get("mode") === "full";
    const esc = (t) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;");
    res.setHeader("content-type", "text/html; charset=utf-8");
    return res.end(`<!doctype html><html><head><meta charset="utf-8"><style>
html,body{margin:0;background:transparent;overflow:hidden;font-family:-apple-system,system-ui}
#wrap{display:inline-block;transform-origin:${full ? "center" : "0 0"};${full ? "padding:28px;background:" + (dark ? "#262626" : "#fff") + ";border-radius:12px" : ""}}
${full ? "body{display:flex;align-items:center;justify-content:center;height:100vh}" : ""}
</style></head><body><div id="wrap"><pre class="mermaid">${esc(code)}</pre></div>
<script src="/mermaid.js"></script><script>
mermaid.initialize({startOnLoad:false,markdownAutoWrap:false,flowchart:{wrappingWidth:600,useMaxWidth:false},theme:${JSON.stringify(dark ? "dark" : "neutral")},securityLevel:"strict",fontFamily:"-apple-system,system-ui"});
let z=1;const w=document.getElementById("wrap");
const post=(m)=>window.ReactNativeWebView&&window.ReactNativeWebView.postMessage(JSON.stringify(m));
mermaid.run().then(()=>{const r=w.getBoundingClientRect();post({w:Math.ceil(r.width),h:Math.ceil(r.height)});}).catch(e=>post({error:String(e)}));
window.addEventListener("message",(e)=>{const c=e.data;z=c==="in"?Math.min(4,z*1.25):c==="out"?Math.max(0.25,z/1.25):1;w.style.transform="scale("+z+")";const r=w.getBoundingClientRect();post({w:Math.ceil(r.width),h:Math.ceil(r.height)});});
</script></body></html>`);
  }
  // Shiki tokens for a markdown code fence (Grok's cursor palettes)
  if (req.method === "POST" && req.url === "/highlight") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const { code, lang } = JSON.parse(body || "{}");
      const ALIAS = { rust: "rs", typescript: "ts", javascript: "js", python: "py", shell: "sh", bash: "sh", zsh: "sh", golang: "go", yml: "yaml", "c++": "cpp", "c#": "cs", csharp: "cs", kotlin: "kt", ruby: "rb", markdown: "md" };
      const ext = ALIAS[String(lang).toLowerCase()] || String(lang).toLowerCase();
      res.setHeader("content-type", "application/json");
      highlight(String(code || "").slice(0, 200000), ext)
        .then((lines) => res.end(JSON.stringify({ lines })))
        .catch(() => res.end(JSON.stringify({ lines: null })));
    });
    return;
  }
  // Reply files (Grok's file cards → fullscreen viewer)
  if (req.method === "GET" && req.url.startsWith("/file/render?")) {
    const q = new URL(req.url, "http://x").searchParams;
    const p = outFile(q.get("path") || "");
    res.setHeader("content-type", "application/json");
    if (!p) return res.end(JSON.stringify({ error: "bad path" }));
    renderFile(p, q.get("path") || "")
      .then((r) => res.end(JSON.stringify(r)))
      .catch((e) => res.end(JSON.stringify({ error: String(e?.message || e) })));
    return;
  }
  // BetterOffice viewer: a WebView page that paints the engines' display
  // lists. /office/vendor/<pkg>/… statically serves the installed packages
  // (dist bundles, wasm, font assets) so the page's imports resolve.
  if (req.method === "GET" && (req.url === "/office/" || req.url.startsWith("/office/?") || req.url.startsWith("/office/editor/"))) {
    const editing = req.url.startsWith("/office/editor/");
    const page = join(dirname(fileURLToPath(import.meta.url)), "office", editing ? "editor" : "", editing ? "editor.html" : "viewer.html");
    res.setHeader("content-type", "text/html; charset=utf-8");
    res.setHeader("cache-control", "no-cache");
    let html = readFileSync(page, "utf8");
    if (editing) {
      // WebViews cache /office/dist/editor.js hard; a ?v=mtime makes every
      // bundle rebuild a fresh URL so stale chrome can never come back.
      const dist = join(dirname(fileURLToPath(import.meta.url)), "office", "editor", "dist", "editor.js");
      const v = existsSync(dist) ? Math.floor(statSync(dist).mtimeMs) : 0;
      html = html.replaceAll("/office/dist/editor.js", `/office/dist/editor.js?v=${v}`);
    }
    return res.end(html);
  }
  // The bundled BetterOffice editor app + its esbuild chunks and wasm —
  // resolved relative to the bundle URL, so dist/generated/ holds the wasms.
  if (req.method === "GET" && req.url.startsWith("/office/dist/")) {
    const rest = decodeURIComponent(req.url.slice("/office/dist/".length).split("?")[0]);
    const dir = join(dirname(fileURLToPath(import.meta.url)), "office", "editor", "dist");
    const p = join(dir, rest);
    if (!p.startsWith(dir + "/") || !existsSync(p) || !statSync(p).isFile()) {
      res.statusCode = 404;
      return res.end();
    }
    const ext = (p.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
    const type = { js: "text/javascript", mjs: "text/javascript", wasm: "application/wasm", css: "text/css", ttf: "font/ttf", otf: "font/otf", map: "application/json" }[ext] || "application/octet-stream";
    res.setHeader("content-type", type);
    // no-cache: a stale editor.js against a new editor.html leaves half-dead
    // chrome behind; the bundle is local so revalidation is cheap
    res.setHeader("cache-control", "no-cache");
    return res.end(readFileSync(p));
  }
  if (req.method === "GET" && req.url === "/office/caps") {
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify(officeCaps()));
  }
  if (req.method === "GET" && req.url.startsWith("/office/vendor/")) {
    const rest = decodeURIComponent(req.url.slice("/office/vendor/".length).split("?")[0]);
    const [pkg, ...segs] = rest.split("/");
    const root = officePkgRoot(pkg);
    const p = root ? join(root, ...segs) : null;
    if (!p || !p.startsWith(root + "/") || !existsSync(p) || !statSync(p).isFile()) {
      res.statusCode = 404;
      return res.end();
    }
    const ext = (p.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
    // css must go out as text/css: browsers silently refuse stylesheets
    // served as octet-stream, which leaves the editor chrome unstyled.
    const type = { js: "text/javascript", mjs: "text/javascript", css: "text/css", wasm: "application/wasm", ttf: "font/ttf", otf: "font/otf", woff: "font/woff", woff2: "font/woff2", json: "application/json", map: "application/json" }[ext] || "application/octet-stream";
    res.setHeader("content-type", type);
    res.setHeader("cache-control", "no-cache");
    return res.end(readFileSync(p));
  }
  if (req.method === "GET" && req.url.startsWith("/file/raw/")) {
    const p = outFile(decodeURIComponent(req.url.slice("/file/raw/".length).split("?")[0]));
    if (!p || !existsSync(p)) {
      res.statusCode = 404;
      return res.end();
    }
    const ext = (p.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
    const type = { pdf: "application/pdf", html: "text/html; charset=utf-8", htm: "text/html; charset=utf-8", png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", mp4: "video/mp4", mov: "video/quicktime", m4v: "video/mp4", m4a: "audio/mp4", mp3: "audio/mpeg", wav: "audio/wav", aac: "audio/aac" }[ext] || "application/octet-stream";
    return sendRanged(req, res, p, type);
  }
  // Download: the native save panel, then a copy
  // The BetterOffice editor saves edits in place — same outFile containment
  // as every other file route; base64 body, no picker.
  if (req.method === "POST" && req.url === "/file/write") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { path, data } = JSON.parse(body || "{}");
        const p = outFile(path || "");
        if (!p || !existsSync(dirname(p))) return res.end(JSON.stringify({ ok: false, error: "bad path" }));
        writeFileSync(p, Buffer.from(String(data || ""), "base64"));
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e).slice(0, 200) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/file/save") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const { path } = JSON.parse(body || "{}");
      const p = outFile(path || "");
      res.setHeader("content-type", "application/json");
      if (!p || !existsSync(p)) return res.end(JSON.stringify({ ok: false }));
      const name = p.split("/").pop().replace(/"/g, "");
      osa(`POSIX path of (choose file name with prompt "Download" default name "${name}")`)
        .then((dest) => {
          copyFileSync(p, dest);
          res.end(JSON.stringify({ ok: true, dest }));
        })
        .catch(() => res.end(JSON.stringify({ ok: false, cancelled: true })));
    });
    return;
  }
  // Avatar and attachment files over http: the app's media grant covers
  // 127.0.0.1, not file:// paths. Only these two folders, by bare file name.
  if (req.method === "GET" && /^\/files\/(attachments|avatars)\//.test(req.url)) {
    const [, dir, raw] = req.url.match(/^\/files\/(attachments|avatars)\/([^?]+)/);
    const name = decodeURIComponent(raw);
    const path = join(dir === "attachments" ? ATTACH_DIR : AVATAR_DIR, name.split("/").pop());
    if (!existsSync(path)) {
      res.statusCode = 404;
      return res.end();
    }
    const ext = (path.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
    const type = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", mov: "video/quicktime", mp4: "video/mp4" }[ext] || "application/octet-stream";
    res.setHeader("cache-control", "max-age=86400");
    return sendRanged(req, res, path, type);
  }
  // Composer attachments (Grok's + menu): the native picker, several files;
  // images are normalized to PNG for the model, documents kept as they are.
  if (req.method === "POST" && req.url === "/attach/pick") {
    res.setHeader("content-type", "application/json");
    osa('set fs to choose file with prompt "Attach photos & files" with multiple selections allowed\nset out to ""\nrepeat with f in fs\nset out to out & POSIX path of f & linefeed\nend repeat\nreturn out')
      .then((list) => {
        const files = list.split("\n").filter(Boolean).map((src) => {
          const name = src.split("/").pop();
          const ext = (name.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
          const image = /^(png|jpe?g|gif|heic|webp|tiff?|bmp)$/.test(ext);
          const video = /^(mov|mp4|m4v|webm|avi|mkv)$/.test(ext);
          const id = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
          const out = join(ATTACH_DIR, image ? `${id}.png` : `${id}-${name}`);
          if (image) execFileSync("sips", ["-s", "format", "png", src, "--out", out], { stdio: "ignore" });
          else copyFileSync(src, out);
          // a 256px thumbnail: sips for images, Quick Look's frame for video
          let thumb = "";
          try {
            if (image) {
              thumb = join(ATTACH_DIR, `${id}.thumb.png`);
              execFileSync("sips", ["-Z", "256", out, "--out", thumb], { stdio: "ignore" });
            } else if (video) {
              execFileSync("qlmanage", ["-t", "-s", "256", "-o", ATTACH_DIR, out], { stdio: "ignore", timeout: 15000 });
              const ql = `${out}.png`;
              if (existsSync(ql)) thumb = ql;
            }
          } catch {}
          return {
            uri: `file://${out}`,
            thumb: thumb ? `file://${thumb}` : "",
            name,
            ext,
            kind: image ? "image" : video ? "video" : "file",
          };
        });
        res.end(JSON.stringify({ ok: true, files }));
      })
      .catch(() => res.end(JSON.stringify({ ok: false, cancelled: true, files: [] })));
    return;
  }
  if (req.method === "POST" && req.url.startsWith("/avatar/image/")) {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const kind = req.url.slice("/avatar/image/".length);
        const out = join(AVATAR_DIR, `${Date.now().toString(36)}.png`);
        let name = "";
        if (kind === "gen") {
          const { description } = JSON.parse(body || "{}");
          if (!description?.trim()) throw new Error("describe the avatar");
          await generateAvatarImage(description.trim(), out);
          name = description.trim();
        } else if (kind === "pick") {
          const picked = await osa(
            'POSIX path of (choose file with prompt "Choose an avatar image" of type {"public.image"})',
          ).catch(() => "");
          if (!picked) return res.end(JSON.stringify({ ok: false, cancelled: true }));
          name = picked.split("/").pop();
          execFileSync("sips", ["-s", "format", "png", picked, "--out", out], { stdio: "ignore" });
        } else if (kind === "paste") {
          await osa(
            `set f to open for access POSIX file "${out}" with write permission
             write (the clipboard as «class PNGf») to f
             close access f`,
          );
          name = "Pasted image";
        } else throw new Error(`unknown avatar source ${kind}`);
        const dims = execFileSync("sips", ["-g", "pixelWidth", "-g", "pixelHeight", out], { encoding: "utf8" });
        const w = Number(/pixelWidth: (\d+)/.exec(dims)?.[1] || 0);
        const h = Number(/pixelHeight: (\d+)/.exec(dims)?.[1] || 0);
        res.end(JSON.stringify({ ok: true, uri: `file://${out}`, name, w, h }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/avatar/gen") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      const SHAPES = [
        "blob", "cloud", "square", "sparkle", "clover", "heart", "flower",
        "teardrop", "tablet", "wedge", "house", "star", "hex",
      ];
      const COLORS = [
        "#FFFFFF", "#936439", "#FF263C", "#FF6700", "#FF9800", "#00C972",
        "#00BCA6", "#1084FE", "#9159FE", "#FF309B", "#777777",
      ];
      try {
        const { description } = JSON.parse(body || "{}");
        if (!description?.trim()) throw new Error("describe the avatar");
        let shape = "blob", color = "#1084FE";
        if (PROVIDER === "anthropic" && anthropic) {
          const m = await anthropic.messages.create({
            model: MODEL,
            max_tokens: 120,
            system:
              "Pick an avatar shape and color for a blob character from a description. " +
              `Reply ONLY with JSON {"shape":<one of ${SHAPES.join("|")}>,"color":<one of ${COLORS.join("|")}>}.`,
            messages: [{ role: "user", content: description.trim() }],
          });
          const txt = (m.content || [])
            .filter((b) => b.type === "text")
            .map((b) => b.text)
            .join("");
          const j = JSON.parse((txt.match(/\{[\s\S]*\}/) || ["{}"])[0]);
          if (SHAPES.includes(j.shape)) shape = j.shape;
          if (COLORS.includes(j.color)) color = j.color;
        }
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, shape, color }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/thread/new") {
    const id = `t${threadSeq++}`;
    store.list.unshift({ id, title: "new chat", turns: [] });
    store.active = id;
    writeIndex();
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true, id }));
  }
  if (req.method === "GET" && req.url === "/turns") {
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify({ active: activeThread().id, turns: turnsJson() }),
    );
  }
  if (req.method === "POST" && req.url === "/thread/select") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id } = JSON.parse(body || "{}");
        const thread = store.list.find((t) => t.id === id);
        if (!thread) throw new Error(`unknown thread ${id}`);
        store.active = id;
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, turns: turnsJson(thread) }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "GET" && req.url.startsWith("/status")) {
    const id = Number(new URL(req.url, "http://x").searchParams.get("id"));
    const job = jobs.get(id);
    res.setHeader("content-type", "application/json");
    return res.end(
      jobJson(job || { id, seq: 0, phase: "unknown", detail: "", done: true }),
    );
  }
  if (req.method === "GET" && req.url.startsWith("/module")) {
    // name is "R<n>" (current) or "R<n>@v<k>" (a pinned version).
    const raw = new URL(req.url, "http://x").searchParams.get("name") || "";
    const mm = raw.match(/^(R\d+)(?:@v(\d+))?$/);
    const name = mm?.[1] || "";
    const ver = mm?.[2] ? Number(mm[2]) : null;
    const file = ver
      ? verFile(name, ver)
      : [GEN_DIR, WIDGETS_DIR]
          .map((d) => join(d, `${name}.tsx`))
          .find(existsSync);
    res.setHeader("content-type", "application/json");
    return res.end(
      name && file && existsSync(file)
        ? JSON.stringify({
            source: readFileSync(file, "utf8"),
            published: !!widgetRow(name),
            v: ver ?? headOf(name),
          })
        : JSON.stringify({ source: "", published: !!widgetRow(name) }),
    );
  }
  if (req.method === "POST" && req.url === "/copy") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { text } = JSON.parse(body || "{}");
        execFileSync("pbcopy", [], { input: String(text || "") });
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/react") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name, key, r } = JSON.parse(body || "{}");
        if (typeof key === "string" && key.startsWith("idx:")) {
          const turn = activeThread().turns[Number(key.slice(4))];
          if (turn) turn.reaction = r || undefined;
        } else {
          for (const t of store.list)
            for (const turn of t.turns)
              if (turn.role === "ai" && turn.name === name)
                turn.reaction = r || undefined;
        }
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // POST /turn/delete {key} — removes a conversation exchange: the AI turn
  // and the user prompt that produced it. key is "name:<R#>" (module cards)
  // or "idx:<n>" (text replies — index into the active thread). Only the
  // conversation changes: the module file stays on disk and a published
  // mini-app stays in Mini Apps until removed there.
  if (req.method === "POST" && req.url === "/turn/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { key } = JSON.parse(body || "{}");
        const thread = activeThread();
        let i = -1;
        if (typeof key === "string" && key.startsWith("name:")) {
          const name = key.slice(5);
          i = thread.turns.findIndex(
            (t) => t.role === "ai" && t.name === name,
          );
        } else if (typeof key === "string" && key.startsWith("idx:")) {
          const n = Number(key.slice(4));
          if (thread.turns[n]) i = n;
        }
        if (i < 0) throw new Error("no such turn");
        const drop = [i];
        // name: keys delete the exchange (card + the prompt that made it);
        // idx: keys delete just that one message, either role.
        if (typeof key === "string" && key.startsWith("name:")) {
          const prev = thread.turns[i - 1];
          if (prev?.role === "user") drop.unshift(i - 1);
        }
        thread.turns = thread.turns.filter((_, j) => !drop.includes(j));
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, dropped: drop.length }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/retry") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name } = JSON.parse(body || "{}");
        let thread, prompt;
        for (const t of store.list) {
          // Prefer the published turn — reusing a widget mounts it in the
          // thread too, and retrying must patch widgets/<name>, not a stale
          // generated copy.
          const i =
            t.turns.findIndex(
              (x) => x.role === "ai" && x.name === name && x.widget,
            ) ??
            -1;
          const idx =
            i >= 0
              ? i
              : t.turns.findIndex((x) => x.role === "ai" && x.name === name);
          if (idx < 0) continue;
          thread = t;
          for (let m = idx - 1; m >= 0; m--)
            if (t.turns[m].role === "user") {
              prompt = t.turns[m].text;
              break;
            }
        }
        if (!thread || !prompt) throw new Error(`no prompt found for ${name}`);
        const job = {
          id: nextJobId++,
          thread: thread.id,
          retry: name,
          seq: 0,
          phase: "queued",
          detail: "",
          done: false,
          result: null,
        };
        jobs.set(job.id, job);
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, id: job.id }));
        generate(job, prompt).catch((e) => {
          console.error(`✗ job ${job.id} crashed:`, e);
          job.done = true;
          setJob(job, "done", `generator error — ${e.message}`);
        });
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/share") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name } = JSON.parse(body || "{}");
        const file = [GEN_DIR, WIDGETS_DIR]
          .map((d) => join(d, `${name}.tsx`))
          .find(existsSync);
        if (!/^R\d+$/.test(name) || !file)
          throw new Error(`no such module ${name}`);
        execFile("open", ["-R", file], () => {});
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/share/thread") {
    const thread = activeThread();
    const text = thread.turns
      .map((t) =>
        t.role === "user"
          ? `You: ${t.text}`
          : `Gen UI (${t.name}.tsx via ${t.label}):\n${
              existsSync(join(GEN_DIR, `${t.name}.tsx`))
                ? readFileSync(join(GEN_DIR, `${t.name}.tsx`), "utf8")
                : ""
            }`,
      )
      .join("\n\n");
    execFileSync("pbcopy", [], { input: text });
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true }));
  }
  // ————— publish / widgets shelf / persisted state —————
  // The right pane's Details / Library: the viewed thread's generated apps
  // (newest first) with their caption, Mini Apps state and file age — the
  // app names its thread; the server's own active thread is wherever the
  // last generation ran, which leaked one bot's apps into another's pane.
  if (req.method === "GET" && req.url.split("?")[0] === "/thread/apps") {
    const id = new URL(req.url, "http://localhost").searchParams.get("thread");
    const t = (id && store.list.find((x) => x.id === id)) || activeThread();
    const published = new Set(db.prepare("SELECT name FROM widgets").all().map((w) => w.name));
    const metas = new Map(
      db.prepare("SELECT name,title,file FROM apps").all().map((r) => [r.name, r]),
    );
    const tomb = new Set(
      db.prepare("SELECT name FROM tombstones").all().map((r) => r.name),
    );
    const seen = new Set();
    const apps = [];
    for (let i = t.turns.length - 1; i >= 0; i--) {
      const u = t.turns[i];
      if (u.role !== "ai" || !u.name || seen.has(u.name) || tomb.has(u.name))
        continue;
      seen.add(u.name);
      // A name with no file anywhere was deleted before tombstones
      // existed — same zombie, same filter.
      const gone =
        !existsSync(join(GEN_DIR, `${u.name}.tsx`)) &&
        !existsSync(join(WIDGETS_DIR, `${u.name}.tsx`));
      if (gone) continue;
      let prompt = "";
      for (let j = i - 1; j >= 0; j--) if (t.turns[j].role === "user") { prompt = t.turns[j].text || ""; break; }
      const file = join(GEN_DIR, `${u.name}.tsx`);
      const caption = u.caption && !/error|implicit|rejected/i.test(u.caption) ? u.caption : "";
      const meta = metas.get(u.name) || {};
      apps.push({
        name: u.name,
        title: meta.title || u.label || caption || prompt.slice(0, 60) || u.name,
        file: meta.file || `${u.name}.tsx`,
        subtitle: prompt.replace(/\s+/g, " ").slice(0, 120),
        published: published.has(u.name),
        mtime: existsSync(file) ? statSync(file).mtimeMs : 0,
        v: headOf(u.name),
        versions: versionsOf(u.name),
      });
    }
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ apps }));
  }
  // POST /app/meta {name, title?, file?} — the Library's renames: a display
  // title and/or a kebab-case display filename. The real module file
  // (R<n>.tsx) never moves — `file` is a label, not a rename on disk.
  if (req.method === "POST" && req.url === "/app/meta") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name, title, file } = JSON.parse(body || "{}");
        if (!/^R\d+$/.test(String(name || "")))
          throw new Error(`no such module ${name}`);
        const cur =
          db.prepare("SELECT title,file FROM apps WHERE name=?").get(name) ||
          {};
        const t =
          title === undefined
            ? (cur.title ?? null)
            : String(title).trim().slice(0, 60) || null;
        const f =
          file === undefined ? (cur.file ?? null) : slugFile(file) || null;
        db.prepare(
          "INSERT OR REPLACE INTO apps(name,title,file) VALUES(?,?,?)",
        ).run(name, t, f);
        // A rename moves the whole batch: every <old>.v<n>.tsx in the
        // artifact's dir becomes <new-slug>.v<n>.tsx, and the index re-points
        // at the new paths. (Chunk ids stay flat R<n>v<k>.js — name-stable.)
        const oldSlug = (cur.file || slugFile(cur.title) || `${name}.tsx`).replace(/\.tsx$/i, "");
        const newSlug = (f || slugFile(t) || `${name}.tsx`).replace(/\.tsx$/i, "");
        if (newSlug !== oldSlug) {
          const dir = join(GEN_DIR, name);
          try {
            for (const x of readdirSync(dir)) {
              const m = x.match(/\.v(\d+)\.tsx$/);
              if (m)
                renameSync(join(dir, x), join(dir, `${newSlug}.v${m[1]}.tsx`));
            }
          } catch {}
          writeIndex();
        }
        res.setHeader("content-type", "application/json");
        res.end(
          JSON.stringify({ ok: true, title: t || "", file: f || `${name}.tsx` }),
        );
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // POST /app/version {name, v} — the current pointer moves to v. Revert
  // keeps later versions on the tree; the next revise off it branches.
  if (req.method === "POST" && req.url === "/app/version") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { name, v } = JSON.parse(body || "{}");
        if (!setHead(String(name || ""), Number(v)))
          throw new Error(`no such version`);
        res.end(JSON.stringify({ ok: true, v: Number(v) }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // POST /app/version/label {name, v, label} — name or rename a branch.
  // Empty label clears it.
  if (req.method === "POST" && req.url === "/app/version/label") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { name, v, label } = JSON.parse(body || "{}");
        const r = db
          .prepare("UPDATE versions SET label=? WHERE name=? AND v=?")
          .run(String(label || "").trim().slice(0, 40) || null, String(name || ""), Number(v));
        if (!r.changes) throw new Error("no such version");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // ————— Connected MCP servers —————
  // GET /mcp/servers → live status per server (never echoes env values).
  if (req.method === "GET" && req.url === "/mcp/servers") {
    const rows = db.prepare("SELECT * FROM mcp_servers ORDER BY created_at").all();
    res.setHeader("content-type", "application/json");
    res.end(
      JSON.stringify({
        servers: rows.map((r) => {
          const st = mcpStatus(r.id);
          return {
            id: r.id,
            name: r.name,
            command: r.command,
            args: JSON.parse(r.args || "[]"),
            envKeys: Object.keys(JSON.parse(r.env || "{}")),
            enabled: !!r.enabled,
            bot: r.bot,
            status: r.enabled ? st.status : "off",
            tools: st.tools,
            err: st.err || null,
          };
        }),
      }),
    );
    return;
  }
  // POST /mcp/servers {id?, name, command, args?, env?, bot?} — add or edit;
  // the server is probed immediately so the reply carries its real status.
  if (req.method === "POST" && req.url === "/mcp/servers") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const r = await mcpUpsertServer(JSON.parse(body || "{}"));
        res.end(JSON.stringify({ ok: true, ...r }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // POST /mcp/server/state {id, enabled} — toggle without deleting config.
  if (req.method === "POST" && req.url === "/mcp/server/state") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { id, enabled } = JSON.parse(body || "{}");
        const r = db.prepare("UPDATE mcp_servers SET enabled=? WHERE id=?").run(enabled ? 1 : 0, String(id || ""));
        if (!r.changes) throw new Error("no such server");
        if (!enabled) mcpInvalidate(String(id));
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // POST /mcp/server/delete {id} — kill the client and drop the row.
  if (req.method === "POST" && req.url === "/mcp/server/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { id } = JSON.parse(body || "{}");
        mcpInvalidate(String(id));
        db.prepare("DELETE FROM mcp_servers WHERE id=?").run(String(id || ""));
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // GET /artifacts?q=&thread=<id> — the composer's @ picker: mini-apps the
  // current thread knows first, then every artifact/file globally.
  if (req.method === "GET" && req.url.split("?")[0] === "/artifacts") {
    const u = new URL(req.url, "http://x");
    const q = (u.searchParams.get("q") || "").toLowerCase();
    const tid = u.searchParams.get("thread") || "";
    const t = tid && store.list.find((x) => x.id === tid);
    const scoped = new Set(
      (t?.turns || [])
        .filter((x) => x.role === "ai" && /^R\d+$/.test(x.name || ""))
        .map((x) => x.name),
    );
    const tomb = new Set(
      db.prepare("SELECT name FROM tombstones").all().map((r) => r.name),
    );
    const pubs = new Set(
      db.prepare("SELECT name FROM widgets").all().map((r) => r.name),
    );
    const metas = new Map(
      db.prepare("SELECT name,title,file FROM apps").all().map((r) => [r.name, r]),
    );
    const items = [];
    for (const f of readdirSync(GEN_DIR)) {
      const m = f.match(/^R\d+/);
      if (!m || !/^R\d+\.tsx$/.test(f) || tomb.has(m[0])) continue;
      const meta = metas.get(m[0]) || {};
      const title = meta.title || m[0];
      if (q && !m[0].toLowerCase().includes(q) && !title.toLowerCase().includes(q))
        continue;
      items.push({
        kind: "app",
        name: m[0],
        title,
        file: meta.file || f,
        v: headOf(m[0]),
        published: pubs.has(m[0]),
        uri: `module://${m[0]}`,
        scope: scoped.has(m[0]) ? "thread" : "global",
      });
    }
    // @-mentionable files: everything the workspace has produced.
    for (const f of walkFiles(OUT_DIR)) {
      if (q && !f.name.toLowerCase().includes(q)) continue;
      items.push({
        kind: "file",
        name: f.name,
        title: f.name,
        file: f.rel,
        uri: `file://${join(OUT_DIR, f.rel)}`,
        scope: "global",
      });
    }
    items.sort(
      (a, b) =>
        (a.scope === "thread" ? 0 : 1) - (b.scope === "thread" ? 0 : 1) ||
        String(a.title).localeCompare(String(b.title)),
    );
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ items: items.slice(0, 40) }));
  }
  // The model picker's catalog: opencodex's models (its "--fast" twins fold
  // into a `fast` flag), each with its effort ladder and default.
  if (req.method === "GET" && req.url === "/models") {
    res.setHeader("content-type", "application/json");
    fetch(`${OPENCODEX}/v1/models`)
      .then((r) => r.json())
      .then((j) => {
        const ids = new Set(j.data.map((m) => m.id));
        const models = j.data
          .filter((m) => !m.id.endsWith("--fast"))
          .map((m) => ({
            id: m.id,
            owner: m.owned_by || "",
            fast: ids.has(m.id + "--fast"),
            efforts: (m.reasoning_efforts || []).map((e) => ({ value: e.value, label: String(e.label || e.value).replace(/ Effort$/i, "") })),
            defaultEffort: m.reasoning_effort || "",
          }));
        res.end(JSON.stringify({ ok: true, default: MODEL, models }));
      })
      .catch(() => res.end(JSON.stringify({ ok: false, default: MODEL, models: [], error: `opencodex at ${OPENCODEX} unreachable` })));
    return;
  }
  // App preferences (theme: system | dark | light), kept in the meta table.
  if (req.url === "/prefs") {
    if (req.method === "GET") {
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify({
        theme: metaGet("theme") || "dark",
        model: metaGet("model") || "",
        effort: metaGet("effort") || "",
        fast: metaGet("fast") === "1",
      }));
    }
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const { theme, model, effort, fast } = JSON.parse(body || "{}");
      if (["system", "dark", "light"].includes(theme)) metaSet("theme", theme);
      if (model !== undefined) metaSet("model", String(model));
      if (effort !== undefined) metaSet("effort", String(effort));
      if (fast !== undefined) metaSet("fast", fast ? "1" : "0");
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ ok: true }));
    });
    return;
  }
  if (req.method === "GET" && req.url === "/widgets") {
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify(
        db
          .prepare("SELECT name,title,prompt,published_at FROM widgets ORDER BY published_at")
          .all(),
      ),
    );
  }
  if (req.method === "POST" && req.url === "/publish") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name } = JSON.parse(body || "{}");
        const src = join(GEN_DIR, `${name}.tsx`);
        if (!/^R\d+$/.test(name) || !existsSync(src))
          throw new Error(`no such generated module ${name}`);
        // Title/prompt: the card's caption, else the user prompt before it.
        let title = "", prompt = "";
        for (const t of store.list) {
          const i = t.turns.findIndex((x) => x.role === "ai" && x.name === name);
          if (i < 0) continue;
          title = t.turns[i].caption || "";
          for (let j = i - 1; j >= 0; j--)
            if (t.turns[j].role === "user") {
              prompt = t.turns[j].text;
              break;
            }
          break;
        }
        if (!title || /error|implicit|rejected/i.test(title))
          title = prompt.slice(0, 48) || name;
        copyFileSync(src, join(WIDGETS_DIR, `${name}.tsx`));
        db.prepare(
          "INSERT OR REPLACE INTO widgets(name,title,prompt,published_at) VALUES(?,?,?,?)",
        ).run(name, title, prompt, new Date().toISOString());
        // packaged Mini Apps cards mount through the chunk dir, not the
        // baked bundle — emit the w-key chunk at publish time.
        emitChunk(`${name}w`, join(WIDGETS_DIR, `${name}.tsx`));
        writeShelf();
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, title }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/unpublish") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name } = JSON.parse(body || "{}");
        db.prepare("DELETE FROM widgets WHERE name=?").run(name);
        // Rewrite the shelf AND the conversation index BEFORE the file goes —
        // a live `import … from "../widgets/<name>"` outliving the file is a
        // parse error that kills the whole feed on the next hot reload.
        writeShelf();
        writeIndex();
        setTimeout(() => {
          try {
            unlinkSync(join(WIDGETS_DIR, `${name}.tsx`));
          } catch {}
        }, 700);
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // POST /app/delete {name, everywhere?} — remove an artifact: generated
  // source, versions, kv state, meta row, transcript turns. The published
  // Mini App copy survives unless {everywhere:true} — delete is chat-side
  // cleanup by default, like /bot/delete.
  if (req.method === "POST" && req.url === "/app/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name, everywhere } = JSON.parse(body || "{}");
        if (!/^R\d+$/.test(String(name || "")))
          throw new Error(`no such module ${name}`);
        if (everywhere)
          db.prepare("DELETE FROM widgets WHERE name=?").run(name);
        db.prepare("DELETE FROM kv WHERE name=?").run(name);
        db.prepare("DELETE FROM apps WHERE name=?").run(name);
        db.prepare("INSERT OR IGNORE INTO tombstones(name) VALUES(?)").run(name);
        // Scrub the transcript too: every ai turn naming the artifact goes,
        // and the user prompt directly above it (the "name:" delete rule),
        // across all threads — a delete must never leave a "module missing"
        // card behind.
        for (const t of store.list) {
          const drop = new Set();
          t.turns.forEach((u, i) => {
            if (u.role === "ai" && u.name === name) {
              drop.add(i);
              if (t.turns[i - 1]?.role === "user") drop.add(i - 1);
            }
          });
          if (drop.size) t.turns = t.turns.filter((_, i) => !drop.has(i));
        }
        // Rewrite the shelf + index before the files go — a live import of a
        // missing module is a parse error that kills the whole feed.
        writeShelf();
        writeIndex();
        setTimeout(() => {
          try {
            unlinkSync(join(GEN_DIR, `${name}.tsx`));
          } catch {}
          try {
            rmSync(join(GEN_DIR, name), { recursive: true, force: true });
          } catch {}
          if (everywhere) {
            try {
              unlinkSync(join(WIDGETS_DIR, `${name}.tsx`));
            } catch {}
          }
          try {
            unlinkSync(join(CHUNK_DIR, `${name}.js`));
            if (everywhere) unlinkSync(join(CHUNK_DIR, `${name}w.js`));
            for (const f of readdirSync(CHUNK_DIR))
              if (f.startsWith(`${name}v`) && f.endsWith(".js"))
                unlinkSync(join(CHUNK_DIR, f));
          } catch {}
        }, 700);
        db.prepare("DELETE FROM versions WHERE name=?").run(name);
        db.prepare("DELETE FROM heads WHERE name=?").run(name);
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  if (req.method === "POST" && req.url === "/reuse") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name } = JSON.parse(body || "{}");
        if (!widgetRow(name) || !existsSync(join(WIDGETS_DIR, `${name}.tsx`)))
          throw new Error(`no published widget ${name}`);
        activeThread().turns.push({
          role: "ai",
          name,
          label: "widget",
          caption: "",
          reaction: "",
          retries: 0,
          widget: true,
        });
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }
  // Per-widget key-value state — one JSON blob per module name. Widgets load
  // it on mount and POST a fresh object whenever their state changes. Accepts
  // /state/<name>, /state?name=<name>, and bodies of {name,state} or the raw
  // state object.
  const stateName = () => {
    const u = new URL(req.url, "http://x");
    const p = u.pathname.replace(/^\/state\/?/, "");
    return u.searchParams.get("name") || decodeURIComponent(p);
  };
  if (req.method === "GET" && req.url.startsWith("/state")) {
    const name = stateName();
    res.setHeader("content-type", "application/json");
    if (!/^[\w.-]+$/.test(name)) return res.end(JSON.stringify({ state: null }));
    const row = db.prepare("SELECT data FROM kv WHERE name=?").get(name);
    let state = null;
    try {
      state = row ? JSON.parse(row.data) : null;
    } catch {}
    console.log(`  /state GET ${name} → ${state ? "hit" : "empty"}`);
    return res.end(JSON.stringify({ state }));
  }
  if (req.method === "POST" && req.url.startsWith("/state")) {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const parsed = JSON.parse(body || "{}");
        const name = stateName() || parsed.name;
        const state = parsed.state !== undefined ? parsed.state : parsed;
        if (!/^[\w.-]+$/.test(String(name || "")))
          throw new Error("bad module name");
        db.prepare("INSERT OR REPLACE INTO kv(name,data) VALUES(?,?)").run(
          name,
          JSON.stringify(state ?? {}),
        );
        console.log(`  /state POST ${name} → ${JSON.stringify(state)}`);
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return;
  }

  // ————— data sources for generated widgets —————
  // Everything under /data is safe for a generated module to call: localhost
  // is the one host nsh grants, and any upstream credentials live only here.

  // GET /sources — the catalog the app can render (and the prompt injects).
  if (req.method === "GET" && req.url === "/sources") {
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify({
        sources: ["timeclock", "reminders"]
          .concat(getLinearKey() ? ["linear"] : [])
          .map((name) => ({ name })),
      }),
    );
  }

  // timeclock — clock-in/out rows straight into app.db.
  const minBetween = (a, b) =>
    Math.max(0, Math.round((new Date(b || Date.now()) - new Date(a)) / 60000));
  const tcJson = () => {
    const rows = db
      .prepare(
        "SELECT clock_in AS cin, clock_out AS cout FROM timeclock ORDER BY id DESC LIMIT 25",
      )
      .all();
    const open = rows.find((r) => !r.cout);
    const dayStart = new Date();
    dayStart.setHours(0, 0, 0, 0);
    const sessions = rows
      .map((r) => ({ in: r.cin, out: r.cout, min: minBetween(r.cin, r.cout) }))
      .slice(0, 10);
    return {
      status: open ? "in" : "out",
      since: open?.cin ?? null,
      todayMin: rows
        .filter((r) => new Date(r.cin) >= dayStart)
        .reduce((s, r) => s + minBetween(r.cin, r.cout), 0),
      sessions,
    };
  };
  if (req.method === "GET" && req.url === "/data/timeclock") {
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify(tcJson()));
  }
  if (req.method === "POST" && req.url === "/data/timeclock/in") {
    const open = db
      .prepare("SELECT id FROM timeclock WHERE clock_out IS NULL")
      .get();
    if (!open)
      db.prepare("INSERT INTO timeclock(clock_in) VALUES(?)").run(
        new Date().toISOString(),
      );
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true, already: !!open, ...tcJson() }));
  }
  if (req.method === "POST" && req.url === "/data/timeclock/out") {
    db.prepare("UPDATE timeclock SET clock_out=? WHERE clock_out IS NULL").run(
      new Date().toISOString(),
    );
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true, ...tcJson() }));
  }

  // reminders — JXA (osascript -l JavaScript); args travel via env vars.
  if (req.method === "GET" && req.url === "/data/reminders") {
    const out = jxa(`
      const app = Application("Reminders");
      const out = [];
      outer: for (const l of app.lists()) {
        const ln = l.name();
        for (const r of l.reminders.whose({completed: false})()) {
          let due = null, flagged = false;
          try { const d = r.dueDate(); if (d) due = d.toISOString(); } catch (e) {}
          try { flagged = !!r.flagged(); } catch (e) {}
          out.push({ name: r.name(), list: ln, due, flagged });
          if (out.length >= 50) break outer;
        }
      }
      JSON.stringify(out);`);
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify(out?.error ? out : { reminders: out || [] }),
    );
  }
  if (
    req.method === "POST" &&
    (req.url === "/data/reminders/add" || req.url === "/data/reminders/complete")
  ) {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      let b = {};
      try {
        b = JSON.parse(body || "{}");
      } catch {}
      res.setHeader("content-type", "application/json");
      const env = {
        REM_NAME: String(b.name || ""),
        REM_LIST: String(b.list || ""),
        REM_DUE: String(b.due || ""),
      };
      if (!env.REM_NAME) return res.end(JSON.stringify({ error: "name?" }));
      const script =
        req.url === "/data/reminders/complete"
          ? `
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const name = ObjC.unwrap(env.objectForKey("REM_NAME"));
            let done = false;
            outer: for (const l of app.lists()) {
              const hits = l.reminders.whose({completed: false, name})();
              if (hits.length) { hits[0].completed = true; done = true; break outer; }
            }
            JSON.stringify({ ok: done });`
          : `
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const get = (k) => { const v = env.objectForKey(k); return v ? ObjC.unwrap(v) : null; };
            const name = get("REM_NAME"), listName = get("REM_LIST"), due = get("REM_DUE");
            let list = null;
            if (listName) { const hits = app.lists.whose({name: listName})(); if (hits.length) list = hits[0]; }
            if (!list) list = app.defaultList();
            const props = { name };
            if (due) props.dueDate = new Date(due);
            list.reminders.push(app.Reminder(props));
            JSON.stringify({ ok: true, name, list: list.name() });`;
      return res.end(JSON.stringify(jxa(script, env)));
    });
    return;
  }

  // linear — GraphQL, key resolved lazily (env or `secret get`).
  const lgql = (query, variables) => {
    const key = getLinearKey();
    if (!key) return Promise.resolve({ _nokey: true });
    return fetch("https://api.linear.app/graphql", {
      method: "POST",
      headers: { Authorization: key, "Content-Type": "application/json" },
      body: JSON.stringify(variables ? { query, variables } : { query }),
    }).then((r) => r.json());
  };
  const L_FIELDS = `nodes { id identifier title priority url updatedAt state { id name } assignee { displayName } team { key } }`;
  if (req.method === "GET" && req.url === "/data/linear/issues") {
    res.setHeader("content-type", "application/json");
    const shape = (nodes, scope) => ({
      scope,
      issues: nodes.map((n) => ({
        uid: n.id,
        id: n.identifier,
        title: n.title,
        state: n.state?.name || "",
        stateId: n.state?.id || "",
        priority: n.priority,
        assignee: n.assignee?.displayName || null,
        team: n.team?.key || null,
        url: n.url,
      })),
    });
    // Assigned-to-me first; when that's empty fall back to the workspace's
    // most recently updated issues so the widget still shows real data.
    lgql(
      `{ viewer { assignedIssues(first: 25, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } } }`,
    )
      .then((d) => {
        if (d?._nokey)
          return res.end(
            JSON.stringify({
              error: "linear not configured — run `secret set LINEAR_API_KEY`",
            }),
          );
        if (d?.errors) throw new Error(String(d.errors[0]?.message));
        const assigned = d?.data?.viewer?.assignedIssues?.nodes || [];
        if (assigned.length) return res.end(JSON.stringify(shape(assigned, "assigned")));
        return lgql(
          `{ issues(first: 25, orderBy: updatedAt, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } }`,
        ).then((d2) => {
          if (d2?.errors) throw new Error(String(d2.errors[0]?.message));
          res.end(
            JSON.stringify(shape(d2?.data?.issues?.nodes || [], "workspace")),
          );
        });
      })
      .catch((e) => {
        res.statusCode = 502;
        res.end(JSON.stringify({ error: String(e?.message || e) }));
      });
    return;
  }

  // GET /data/linear/states — every team's workflow states (for transitions).
  if (req.method === "GET" && req.url === "/data/linear/states") {
    res.setHeader("content-type", "application/json");
    lgql(
      `{ teams(first: 10) { nodes { key states { nodes { id name type position } } } } }`,
    )
      .then((d) => {
        if (d?._nokey)
          return res.end(JSON.stringify({ error: "linear not configured" }));
        if (d?.errors) throw new Error(String(d.errors[0]?.message));
        const teams = (d?.data?.teams?.nodes || []).map((t) => ({
          team: t.key,
          states: t.states.nodes
            .slice()
            .sort((a, b) => a.position - b.position)
            .map((s) => ({ id: s.id, name: s.name, type: s.type })),
        }));
        res.end(JSON.stringify({ teams }));
      })
      .catch((e) => {
        res.statusCode = 502;
        res.end(JSON.stringify({ error: String(e?.message || e) }));
      });
    return;
  }

  // POST /data/linear/update {uid, stateId?, priority?} — mutate an issue;
  // POST /data/linear/assign {uid} — assign it to the token's owner;
  // POST /data/linear/comment {uid, body} — leave a comment.
  const linearMutate = (build) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      let b = {};
      try {
        b = JSON.parse(body || "{}");
      } catch {}
      const call = build(b);
      if (call?.error) return res.end(JSON.stringify(call));
      lgql(call.query, call.variables)
        .then((d) => {
          if (d?._nokey)
            return res.end(JSON.stringify({ error: "linear not configured" }));
          if (d?.errors) throw new Error(String(d.errors[0]?.message));
          res.end(JSON.stringify({ ok: true }));
        })
        .catch((e) => {
          res.statusCode = 502;
          res.end(JSON.stringify({ error: String(e?.message || e) }));
        });
    });
  };
  if (req.method === "POST" && req.url === "/data/linear/update")
    return linearMutate((b) => {
      if (!b.uid) return { error: "uid?" };
      const input = {};
      if (b.stateId) input.stateId = b.stateId;
      if (b.priority !== undefined) input.priority = b.priority;
      return {
        query: `mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success } }`,
        variables: { id: b.uid, input },
      };
    });
  if (req.method === "POST" && req.url === "/data/linear/comment")
    return linearMutate((b) => {
      if (!b.uid || !b.body) return { error: "uid+body?" };
      return {
        query: `mutation($input: CommentCreateInput!) { commentCreate(input: $input) { success } }`,
        variables: { input: { issueId: b.uid, body: b.body } },
      };
    });
  // assign resolves the viewer id first, then runs the update mutation.
  if (req.method === "POST" && req.url === "/data/linear/assign") {
    let body = "";
    req.on("data", (c) => (body += c));
    return req.on("end", () => {
      res.setHeader("content-type", "application/json");
      let b = {};
      try {
        b = JSON.parse(body || "{}");
      } catch {}
      if (!b.uid) return res.end(JSON.stringify({ error: "uid?" }));
      lgql(`{ viewer { id } }`)
        .then((d) => {
          const vid = d?.data?.viewer?.id;
          if (!vid) throw new Error("no viewer");
          return lgql(
            `mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success } }`,
            { id: b.uid, input: { assigneeId: vid } },
          );
        })
        .then((d2) => {
          if (d2?.errors) throw new Error(String(d2.errors[0]?.message));
          res.end(JSON.stringify({ ok: true }));
        })
        .catch((e) =>
          res.end(JSON.stringify({ error: String(e?.message || e) })),
        );
    });
  }

  // GET /widget-feed — the generic channel the native WidgetKit extension
  // renders: every persisted widget's state flattened to summary lines, plus
  // first-party sources. items: [{id,title,module?,lines:[{text,url?}],view?}]
  //
  // `view` is the template payload the extension renders natively: a kind
  // + typed fields. Resolution: the module's persisted `widget` object
  // wins (it can compute live values), then its `widgetKind` export +
  // state-shape inference, then a plain list of the summary lines.
  const viewFor = (name, st, lines) => {
    if (st && typeof st.widget === "object" && st.widget?.kind)
      return st.widget;
    let kind = null;
    try {
      const file = [join(WIDGETS_DIR, `${name}.tsx`), join(GEN_DIR, `${name}.tsx`)]
        .find((f) => existsSync(f));
      const src = file && readFileSync(file, "utf8");
      kind = src?.match(/export\s+const\s+widgetKind\s*=\s*["'](\w+)["']/)?.[1] || null;
    } catch {}
    if (!kind) {
      // infer the template from the state's own shape
      if (st.status === "in" || st.status === "out" || typeof st.running === "boolean" ||
          st.since || st.startedAt) kind = "clock";
      else if (st.temp != null || st.temperature != null || st.condition) kind = "weather";
      else if (st.fraction != null || st.progress != null || st.percent != null) kind = "progress";
      else if (Object.values(st).some((v) => typeof v === "number")) kind = "stat";
      else kind = "list";
    }
    const num = (k) => (typeof st[k] === "number" ? st[k] : null);
    if (kind === "clock")
      return {
        kind,
        running: st.running ?? st.status === "in",
        startedAt: st.startedAt ?? st.since ?? null,
        todayMin: num("todayMin") ?? num("minutes") ?? num("elapsedMin") ?? 0,
        label: st.label ?? null,
      };
    if (kind === "progress") {
      const f = num("fraction") ?? num("progress") ??
        (num("percent") != null ? num("percent") / 100 : null) ??
        (num("value") != null && num("total") ? num("value") / num("total") : null);
      return {
        kind,
        fraction: Math.max(0, Math.min(1, f ?? 0)),
        value: st.valueText ?? st.label ?? null,
        sub: st.sub ?? null,
      };
    }
    if (kind === "weather")
      return {
        kind,
        temp: num("temp") ?? num("temperature"),
        unit: st.unit ?? "°",
        condition: st.condition ?? null,
        high: num("high") ?? num("hi"),
        low: num("low") ?? num("lo"),
        days: Array.isArray(st.days) ? st.days.slice(0, 4) : null,
      };
    if (kind === "note")
      return {
        kind,
        headline: st.headline ?? st.title ?? null,
        body: st.body ?? st.text ?? lines[0]?.text ?? null,
      };
    if (kind === "stat") {
      // headline number = the most meaningful numeric key the module stores
      const pick = ["count", "best", "total", "score", "value", "minutesFocused",
        "minutes", "n", "plays", "rolls", "sessions"]
        .find((k) => typeof st[k] === "number") ??
        Object.keys(st).find((k) => typeof st[k] === "number" && k !== "widget");
      return {
        kind,
        value: pick != null ? String(st[pick]) : "—",
        unit: st.unit ?? null,
        label: st.label ?? pick ?? null,
        sub: st.sub ?? lines[1]?.text ?? null,
      };
    }
    // A stateless app has nothing to template — leave `view` unset so the
    // extension falls back to the rendered PNG of the real UI instead of
    // showing a bare "tap to open" row.
    if (lines.length === 1 && lines[0].text === "tap to open") return null;
    return { kind: "list", rows: lines.map((l) => ({ title: l.text, detail: null, url: l.url ?? null })) };
  };

  if (req.method === "GET" && req.url === "/widget-feed") {
    res.setHeader("content-type", "application/json");
    const items = [];
    const pinned = new Set(
      db.prepare("SELECT k FROM meta WHERE k LIKE 'pin:%'").all()
        .map((r) => r.k.slice(4)),
    );
    const finish = () => {
      // nominated (pinned) widgets lead the feed, then linear, then the
      // rest — an unconfigured widget slot shows your pick, not linear.
      items.sort(
        (a, b) =>
          (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0) ||
          (a.id === "linear" ? -1 : b.id === "linear" ? 1 : 0),
      );
      res.end(
        JSON.stringify({ updated: new Date().toISOString(), items }),
      );
    };
    // every published widget AND every generated module that persisted
    // state is a feed row — state lines when present, a launch hint
    // otherwise. Nominated (pinned) ones lead.
    const titles = Object.fromEntries(
      db.prepare("SELECT name,title FROM widgets").all().map((w) => [w.name, w.title]),
    );
    const names = new Set([
      ...Object.keys(titles),
      ...db.prepare("SELECT name FROM kv").all().map((r) => r.name),
    ]);
    for (const name of [...names].sort()) {
      const w = { name, title: titles[name] };
      let st = {};
      try {
        st = JSON.parse(
          db.prepare("SELECT data FROM kv WHERE name=?").get(w.name)?.data || "{}",
        );
      } catch {}
      const lines = Object.entries(st)
        .filter(([, v]) => v == null || ["string", "number", "boolean"].includes(typeof v))
        .slice(0, 3)
        .map(([k, v]) => ({ text: `${k}: ${v}` }));
      if (!lines.length) lines.push({ text: "tap to open" });
      const clean = (w.title || w.name)
        .replace(/\*\*/g, "")
        .split(/[:.\n—]/)[0]
        .trim()
        .slice(0, 28);
      items.push({
        id: w.name,
        title: clean || w.name,
        module: w.name,
        pinned: pinned.has(w.name),
        lines,
        view: viewFor(w.name, st, lines),
      });
    }
    // first-party: clock-in/out status (no module — informational row)
    const tc = tcJson();
    items.push({
      id: "timeclock",
      title: "Time Clock",
      lines: [
        { text: tc.status === "in" ? `clocked in · ${minBetween(tc.since, null)} min` : "clocked out" },
        { text: `${tc.todayMin} min today` },
      ],
      view: {
        kind: "clock",
        running: tc.status === "in",
        startedAt: tc.since,
        todayMin: tc.todayMin,
      },
    });
    // linear — same assigned-first/fallback query as /data/linear/issues
    const pushLinear = (nodes, scope) =>
      items.unshift({
        id: "linear",
        title: `Linear · ${nodes.length} open`,
        lines: nodes.slice(0, 4).map((n) => ({
          text: `${n.identifier} · ${n.title}`,
          url: n.url || null,
        })),
        view: {
          kind: "list",
          rows: nodes.slice(0, 4).map((n) => ({
            title: n.title,
            detail: `${n.identifier}${n.state?.name ? " · " + n.state.name : ""}`,
            url: n.url || null,
          })),
        },
      });
    if (!getLinearKey()) {
      items.unshift({ id: "linear", title: "Linear", lines: [{ text: "not configured" }] });
      return finish();
    }
    lgql(
      `{ viewer { assignedIssues(first: 25, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } } }`,
    )
      .then((d) => {
        const assigned = d?.data?.viewer?.assignedIssues?.nodes || [];
        if (assigned.length) return pushLinear(assigned, "assigned"), finish();
        return lgql(
          `{ issues(first: 25, orderBy: updatedAt, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } }`,
        ).then((d2) => {
          pushLinear(d2?.data?.issues?.nodes || [], "workspace");
          finish();
        });
      })
      .catch(() => {
        items.unshift({ id: "linear", title: "Linear", lines: [{ text: "unreachable" }] });
        finish();
      });
    return;
  }

  // GET /pins — which modules the user nominated to lead the widget feed.
  if (req.method === "GET" && req.url === "/pins") {
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify({
        pins: db.prepare("SELECT k FROM meta WHERE k LIKE 'pin:%'").all()
          .map((r) => r.k.slice(4)),
      }),
    );
  }

  // POST /pin {name, pinned} — nominate a widget for the desktop extension.
  if (req.method === "POST" && req.url === "/pin") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { name, pinned: p } = JSON.parse(body || "{}");
        if (!name) return res.end(JSON.stringify({ ok: false }));
        if (p)
          db.prepare("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)").run(
            `pin:${name}`, "1",
          );
        else db.prepare("DELETE FROM meta WHERE k=?").run(`pin:${name}`);
        return res.end(JSON.stringify({ ok: true, pinned: !!p }));
      } catch (e) {
        return res.end(JSON.stringify({ ok: false, error: String(e) }));
      }
    });
    return;
  }

  // GET /catalog — the Mini Apps tab's data: every published widget as a
  // metadata card. kinds: "db" (persists via /state), "api" (talks to a
  // /data source), "mem" (in-memory only — fresh state per surface).
  // tags are user-set via /widget-meta; pinned via /pin.
  if (req.method === "GET" && req.url === "/catalog") {
    res.setHeader("content-type", "application/json");
    const pins = new Set(
      db.prepare("SELECT k FROM meta WHERE k LIKE 'pin:%'").all()
        .map((r) => r.k.slice(4)),
    );
    const tagRows = db.prepare("SELECT k,v FROM meta WHERE k LIKE 'tags:%'").all();
    const tags = Object.fromEntries(
      tagRows.map((r) => [r.k.slice(5), r.v.split(",").map((t) => t.trim()).filter(Boolean)]),
    );
    const titleRows = db.prepare("SELECT k,v FROM meta WHERE k LIKE 'title:%'").all();
    const titles = Object.fromEntries(
      titleRows.map((r) => [r.k.slice(6), r.v]),
    );
    const arch = new Set(
      db.prepare("SELECT k FROM meta WHERE k LIKE 'arch:%'").all()
        .map((r) => r.k.slice(5)),
    );
    const list = db
      .prepare("SELECT name,title,published_at FROM widgets ORDER BY published_at DESC")
      .all()
      .map((w) => {
        const hasState = !!db.prepare("SELECT 1 FROM kv WHERE name=?").get(w.name);
        const src = readFileSync(join(WIDGETS_DIR, `${w.name}.tsx`), "utf8");
        const kinds = [
          hasState && "db",
          src.includes("/data/") && "api",
          src.includes("nsh/keymap") && "keys",
        ].filter(Boolean);
        if (!kinds.length) kinds.push("mem");
        // A user rename wins verbatim; the model's caption gets cleaned up.
        const title = titles[w.name] || (w.title || w.name)
          .replace(/\*\*/g, "")
          .split(/[:.\n—]/)[0]
          .trim()
          .slice(0, 40);
        return {
          name: w.name,
          title: title || w.name,
          publishedAt: w.published_at,
          kinds,
          pinned: pins.has(w.name),
          archived: arch.has(w.name),
          tags: tags[w.name] || [],
        };
      });
    return res.end(JSON.stringify({ list }));
  }

  // POST /widget-meta {name, tags?:[…], title?:"…", archived?:bool} — user
  // metadata for the Mini Apps tab. tags replaces the tag list; title is a
  // verbatim rename (the model's caption is ugly, the user's word wins);
  // archived hides it from the shelf until the "archived" filter is on.
  if (req.method === "POST" && req.url === "/widget-meta") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const j = JSON.parse(body || "{}");
        const { name, tags: t, title: rt, archived: ar } = j;
        if (typeof ar === "boolean")
          ar
            ? db.prepare("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)").run(`arch:${name}`, "1")
            : db.prepare("DELETE FROM meta WHERE k=?").run(`arch:${name}`);
        if (!name || (t !== undefined && !Array.isArray(t)))
          return res.end(JSON.stringify({ ok: false }));
        if (typeof rt === "string") {
          const clean = rt.trim().slice(0, 60);
          if (clean)
            db.prepare("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)").run(
              `title:${name}`, clean,
            );
          else db.prepare("DELETE FROM meta WHERE k=?").run(`title:${name}`);
        }
        if (t === undefined)
          return res.end(JSON.stringify({ ok: true }));
        const clean = t.map((x) => String(x).trim().toLowerCase()).filter(Boolean).slice(0, 8);
        if (clean.length)
          db.prepare("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)").run(
            `tags:${name}`, clean.join(","),
          );
        else db.prepare("DELETE FROM meta WHERE k=?").run(`tags:${name}`);
        return res.end(JSON.stringify({ ok: true, tags: clean }));
      } catch (e) {
        return res.end(JSON.stringify({ ok: false, error: String(e) }));
      }
    });
    return;
  }

  // POST /popout {name, mode?} — freeze a mount wrapper and spawn
  // `nsh run` on it: the widget gets its own real macOS window.
  // mode "desktop" → pinned at the desktop-icon level (the real mini-app,
  // live + interactive, sitting on the desktop where system widgets do).
  if (req.method === "POST" && req.url === "/popout") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      let name = "", mode = "float", popV = null;
      try {
        const j = JSON.parse(body || "{}");
        name = j.name || "";
        mode = j.mode === "desktop" ? "desktop" : "float";
        popV = j.v ? Number(j.v) : null;
      } catch {}
      res.setHeader("content-type", "application/json");
      if (!/^R\d+$/.test(name)) return res.end(JSON.stringify({ ok: false }));
      const key = `${mode}:${name}${popV ? `@v${popV}` : ""}`;
      // Already open: raise the live window instead of rewriting the
      // wrapper under it — a rewrite hot-reloads the dev session and the
      // module remounts, wiping its in-memory state.
      const prev = openWindows.get(key);
      if (prev && prev.exitCode === null && !prev.killed) {
        raiseWidget(`widget · ${name}`);
        return res.end(JSON.stringify({ ok: true, already: true }));
      }
      // v pins the pop-out to that version's file (a card pops out what it
      // shows); no v = the current head mirror.
      const verFilePath = popV ? verFile(name, popV) : null;
      const widgetFile = join(WIDGETS_DIR, `${name}.tsx`);
      const genFile = join(GEN_DIR, `${name}.tsx`);
      const file = verFilePath && existsSync(verFilePath)
        ? verFilePath
        : popV
          ? null
          : existsSync(widgetFile)
            ? widgetFile
            : existsSync(genFile)
              ? genFile
              : null;
      if (!file)
        return res.end(JSON.stringify({ ok: false, error: "no such module" }));
      const rel = verFilePath && file === verFilePath
        ? `../generated/${name}/${basename(file).slice(0, -4)}`
        : file === widgetFile
          ? `../widgets/${name}`
          : `../generated/${name}`;
      const bounds = desktopBounds();
      // The window hugs content height up to three quarters of the screen.
      const capH = Math.max(560, Math.floor(bounds.h * 0.75));
      const desk = mode === "desktop" ? bounds : null;
      const slot = desk ? allocDeskSlot(`${name}`) : 0;
      const deskBlock = desk
        ? `const DESK = { w: ${desk.w}, h: ${desk.h}, slot: ${slot} };`
        : `const DESK = null as null | { w: number; h: number; slot: number };`;
      // The module's declared size, read here so the window opens at it
      // (--size) instead of flashing the host's default frame first.
      const src = readFileSync(file, "utf8");
      const sizeM = src.match(
        /export\s+const\s+popoutSize\s*=\s*\{[^}]*?w\s*:\s*(\d+(?:\.\d+)?)[^}]*?h\s*:\s*(\d+(?:\.\d+)?)/,
      );
      const widthM = src.match(
        /export\s+const\s+popoutWidth\s*=\s*(\d+(?:\.\d+)?)/,
      );
      const wantW = Math.min(
        720,
        Math.max(240, Math.round(Number(sizeM?.[1] ?? widthM?.[1] ?? POP_W))),
      );
      const wantH = sizeM
        ? Math.min(capH, Math.max(120, Math.round(Number(sizeM[2]))))
        : Number(POP_H);
      const popName = popV ? `${name}@${popV}` : name;
      mkdirSync(WINDOWS_DIR, { recursive: true });
      writeFileSync(
        join(WINDOWS_DIR, `pop-${name}${popV ? `-${popV}` : ""}.tsx`),
        `import { mount, View } from "nsh";
import { isAvailable, Window } from "nsh/desktop";
import { colorScheme } from "nsh/device";
import R, * as mod from "${rel}";
${deskBlock}
// Sizing contract: \`export const popoutSize = { w, h }\` fixes both axes
// (canvas apps, games); \`export const popoutWidth\` sets the width and
// the height hugs the content; neither → ${POP_W}pt wide, hugged height.
const exact = (mod as any).popoutSize as { w?: number; h?: number } | undefined;
const wantW = Math.min(720, Math.max(240, Math.round(exact?.w ?? (mod as any).popoutWidth ?? ${POP_W})));
const capH = ${capH};
// The hidden title bar still paints the traffic lights over the top ~26pt —
// the strip is real chrome: pad the module below it, add it to the frame.
const STRIP = 26;
if (isAvailable) {
  Window.setTitle("widget · " + ${JSON.stringify(popName)});
  Window.setTitleBar("hidden", 32); // natural height: the lights land at AppKit's own (9,9) spot
  if (DESK) {
    // a desktop widget: behind every window, on the desktop surface —
    // slotted down the right edge like the system's own widgets.
    Window.setLevel("desktopIcon");
    Window.setFrame({
      x: DESK.w - 24 - wantW,
      y: DESK.h - 60 - ${wantH} - STRIP - DESK.slot * 500,
      width: wantW,
      height: ${wantH} + STRIP,
    });
  } else {
    // A regular window level: aerospace keeps it out of the tile grid (a
    // free-standing window), but it stacks normally — it can sit behind
    // other windows and comes forward when focused, not pinned on top.
    Window.setLevel("normal");
    Window.setFrame({ width: wantW, height: ${wantH} + STRIP });
    // A spawned host process has no activation context — without this the
    // window orders in behind the front app, never key (grayed lights).
    Window.orderFront();
  }
}
// The window hugs the widget's real height — with manners. A resize is
// applied only once the measured height has been stable for ~150ms, so a
// transient "loading" or empty render can never collapse it; a change
// under 12pt is wobble, not layout. anchorTop keeps the top edge fixed —
// the window grows and shrinks at its bottom edge. popoutSize-h modules
// opt out of hugging entirely: their frame is exactly what they asked.
let lastH = 0, pendingH = 0, pendingT = 0, timer: any = null;
const hug = (raw: number) => {
  if (!isAvailable || exact?.h) return;
  const h = Math.max(120, Math.min(capH, Math.ceil(raw)));
  if (!isFinite(h)) return;
  if (Math.abs(h - lastH) <= 12) return;
  if (h !== pendingH) {
    pendingH = h;
    pendingT = Date.now();
  }
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    if (Date.now() - pendingT < 140) {
      hug(pendingH); // still settling — re-arm
      return;
    }
    lastH = pendingH;
    Window.setFrame({ height: pendingH + STRIP, anchorTop: true, animated: true });
  }, 150);
};
mount(() => (
  <View
    style={{
      flex: 1,
      // Follows the system appearance — a resize or a resize gap shows a
      // themed surface, never a black void behind a light card.
      backgroundColor: colorScheme() === "dark" ? "#161616" : "#ffffff",
      overflow: "hidden",
    }}
  >
    <View style={{ paddingTop: STRIP }}>
      <View onLayout={(e) => hug(e.height)}>
        <R />
      </View>
    </View>
    {/* The drag holdout: invisible, and above the strip — presses on the
        top 26pt move the window instead of reaching the app, so canvas
        and pressable apps stay movable. */}
    <View
      style={{ position: "absolute", top: 0, left: 0, right: 0, height: STRIP }}
      desktop={{ windowDrag: true }}
    />
  </View>
));
`,
      );
      // Production path: compile the wrapper once (cached by content) and
      // launch a bare host on the bundle — no `nsh run`, no dev server.
      const wrapperFile = join(WINDOWS_DIR, `pop-${name}${popV ? `-${popV}` : ""}.tsx`);
      const buildKey = createHash("sha1")
        .update(readFileSync(wrapperFile))
        .digest("hex")
        .slice(0, 10);
      const buildDir = join(WINDOWS_DIR, `.pop-${name}${popV ? `-${popV}` : ""}-${buildKey}`);
      const bundleFile = join(buildDir, "bundle.nshc");
      if (!existsSync(bundleFile)) {
        mkdirSync(buildDir, { recursive: true });
        const b = spawnSync(NSH, ["build", wrapperFile, "-o", buildDir], {
          encoding: "utf8",
        });
        if (b.status !== 0 || !existsSync(bundleFile)) {
          const err = (b.stderr || b.stdout || "").trim().split("\n").pop();
          return res.end(
            JSON.stringify({ ok: false, error: `build failed: ${err}` }),
          );
        }
      }
      const child = spawn(
        NSH_HOST,
        [
          "--bundle",
          bundleFile,
          "--name",
          `mini app · ${name}`,
          "--app-id",
          `${BUNDLE_ID}.widget`,
          "--window-size",
          `${wantW}x${wantH + 26}`,
        ],
        {
          detached: true,
          stdio: "ignore",
          env: { ...process.env },
        },
      );
      child.unref();
      child.on("exit", () => {
        openWindows.delete(key);
        if (desk) deskSlots.delete(name);
      });
      openWindows.set(key, child);
      console.log(`  /popout ${name} (${mode}) → pid ${child.pid}`);
      // AeroSpace tiles every new window; float ours once its title lands.
      // Desktop-mode windows sit at the desktop-icon level — leave them
      // out of the WM entirely, but still grab the id for snapshots.
      let tries = 0;
      const floatPoll = setInterval(() => {
        if (++tries > 40 || child.exitCode !== null)
          return clearInterval(floatPoll);
        try {
          const out = execFileSync(
            "aerospace",
            ["list-windows", "--all", "--format", "%{window-id} %{window-title}"],
            { encoding: "utf8" },
          );
          const line = out
            .split("\n")
            .find((l) => l.includes(`widget · ${name}`));
          if (!line) return;
          const wid = line.trim().split(/\s+/)[0];
          // Bring the window to the user's space: floating ones get
          // float + move-to-focused-workspace + focus; desktop-mode ones
          // still move to the focused workspace so they land on the
          // desktop the user is actually looking at.
          const ws = execFileSync(
            "aerospace", ["list-workspaces", "--focused"], { encoding: "utf8" },
          ).trim();
          if (!desk)
            execFileSync("aerospace", ["layout", "floating", "--window-id", wid]);
          execFileSync("aerospace", [
            "move-node-to-workspace", "--window-id", wid, "--focus-follows-window", ws,
          ]);
          if (!desk)
            execFileSync("aerospace", ["focus", "--window-id", wid]);
          clearInterval(floatPoll);
          console.log(`  /popout ${name} → window ${wid}`);
        } catch {}
      }, 600);
      res.end(JSON.stringify({ ok: true, pid: child.pid }));
    });
    return;
  }
  // GET /widget-shot?name=R35&w=340&h=340 — the mini-app rendered to PNG:
  // what the WidgetKit extension shows for its configured app (WidgetKit
  // can't run TSX; this is the real UI, as a picture, refreshed with the
  // timeline). 404 when the module doesn't exist or rendering fails.
  if (req.method === "GET" && req.url.startsWith("/widget-shot")) {
    const u = new URL(req.url, "http://x");
    const name = u.searchParams.get("name") || "";
    const w = Math.min(
      800,
      Math.max(80, Math.round(Number(u.searchParams.get("w")) || 340)),
    );
    const h = Math.min(
      800,
      Math.max(80, Math.round(Number(u.searchParams.get("h")) || 340)),
    );
    if (!/^R\d+$/.test(name)) {
      res.statusCode = 400;
      return res.end("bad name");
    }
    renderShot(name, w, h).then((png) => {
      if (!png) {
        res.statusCode = 404;
        return res.end("no shot");
      }
      res.setHeader("content-type", "image/png");
      res.end(png);
    });
    return;
  }
  res.statusCode = 404;
  res.end("not found");
});

persist(); // seed the DB on first boot / after a JSON→SQLite migration
writeShelf(); // sync widgets/index.tsx with the DB
// BetterOffice MCP child (office_* tools) — connected in the background; a
// failed spawn just leaves the surface out of toolList().
ensureOfficeMcp(OUT_DIR).catch(() => {});
// The editor bundle is gitignored (dist/) — build it once when the engines
// are installed and dist/editor.js is missing; until then renderFile hands
// out the read-only /office/ viewer URL instead.
{
  const editorJs = join(dirname(fileURLToPath(import.meta.url)), "office", "editor", "dist", "editor.js");
  if (!existsSync(editorJs) && officeCaps().docx) {
    execFile("npx", ["esbuild", "editor.jsx", "--bundle", "--format=esm", "--splitting", "--outdir=dist",
      "--loader:.wasm=file", "--loader:.ttf=file", "--loader:.otf=file", "--external:*.png", "--minify"],
      { cwd: join(dirname(fileURLToPath(import.meta.url)), "office", "editor") }, (e) => {
      if (e) console.error(`  office editor build: ${String(e.message || e).slice(0, 160)}`);
      else {
        const ed = join(dirname(fileURLToPath(import.meta.url)), "office", "editor");
        execFile("node", [join(ed, "patch-dist.mjs")], () => {});
        const g = join(ed, "dist", "generated");
        for (const pkg of ["docx", "xlsx", "pptx"]) {
          const src = officePkgRoot(pkg);
          if (src) execFile("cp", ["-R", join(src, "dist", "generated") + "/", g + "/"], () => {});
        }
      }
    });
  }
}

// Materialise generated/index.tsx so a fresh clone typechecks before the
// first generation lands (generated/ is gitignored — nothing tracks it).
writeIndex();

// Migrate pre-versioning artifacts: the file as it stands becomes v1, the
// head pointer opens on it. (Cards rendered before `v` existed resolve the
// bare name — the head mirror — which is what they'd show anyway.)
for (const f of readdirSync(GEN_DIR)) {
  const m = f.match(/^(R\d+)\.tsx$/);
  if (!m || headOf(m[1])) continue;
  try {
    const dir = join(GEN_DIR, m[1]);
    mkdirSync(dir, { recursive: true });
    const v1 = join(dir, `${verSlug(m[1])}.v1.tsx`);
    if (!verFile(m[1], 1)) copyFileSync(join(GEN_DIR, f), v1);
    db.prepare(
      "INSERT OR IGNORE INTO versions(name,v,parent,prompt,created_at) VALUES(?,1,NULL,NULL,?)",
    ).run(m[1], statSync(join(GEN_DIR, f)).mtimeMs);
    db.prepare("INSERT OR IGNORE INTO heads(name,v) VALUES(?,1)").run(m[1]);
  } catch {}
}
// Pre-slug version files (v<n>.tsx) rename to <slug>.v<n>.tsx.
for (const d of readdirSync(GEN_DIR)) {
  if (!/^R\d+$/.test(d)) continue;
  try {
    for (const x of readdirSync(join(GEN_DIR, d))) {
      const m = x.match(/^v(\d+)\.tsx$/);
      if (m)
        renameSync(
          join(GEN_DIR, d, x),
          join(GEN_DIR, d, `${verSlug(d)}.v${m[1]}.tsx`),
        );
    }
  } catch {}
}
writeIndex();

// Backfill runtime chunks for modules that predate them (the packaged app
// imports these through NSH_CHUNK_DIR; without a chunk its card shows
// "module missing"). Version files get their own chunks — a card pins the
// exact version it rendered.
(async () => {
  for (const dir of [GEN_DIR, WIDGETS_DIR]) {
    if (!existsSync(dir)) continue;
    for (const f of readdirSync(dir)) {
      const m = f.match(/^(R\d+)\.tsx$/);
      if (!m) continue;
      const key = dir === WIDGETS_DIR ? `${m[1]}w` : m[1];
      if (!existsSync(join(CHUNK_DIR, `${key}.js`)))
        await emitChunk(key, join(dir, f));
    }
  }
  for (const d of readdirSync(GEN_DIR)) {
    if (!/^R\d+$/.test(d)) continue;
    try {
      for (const vf of readdirSync(join(GEN_DIR, d))) {
        const vm = vf.match(/\.v(\d+)\.tsx$/);
        if (!vm) continue;
        const key = `${d}v${vm[1]}`;
        if (!existsSync(join(CHUNK_DIR, `${key}.js`)))
          await emitChunk(key, join(GEN_DIR, d, vf));
      }
    } catch {}
  }
})().catch(() => {});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`app-server on http://127.0.0.1:${PORT}`);
  console.log(
    MOCK ||
    !(PROVIDER === "anthropic"
      ? process.env.ANTHROPIC_API_KEY
      : process.env.OPENROUTER_API_KEY)
      ? "mode: MOCK (set ANTHROPIC_API_KEY or OPENROUTER_API_KEY for real generation)"
      : `mode: ${MODEL} via ${PROVIDER === "anthropic" ? "anthropic" : BASE_URL}`,
  );
});
