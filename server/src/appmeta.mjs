// appmeta.mjs — a mini app's details (title, file label, description, size,
// icon) and its generated icon. Shared by routes/apps.mjs (/app/meta,
// /app/info, /apps/meta, /app/icon*) and the agent tools in tools.mjs.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, renameSync, statSync, unlinkSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { GEN_DIR, PORT } from "./config.mjs";
import { db, store, slugFile, widgetRow, writeIndex } from "./state.mjs";
import { headOf, verFile, versionsOf } from "./pipeline.mjs";
import { generateImage, imageCapable } from "./avatars.mjs";

export const iconPath = (name) => join(GEN_DIR, name, "icon.png");
export const originOf = (req) =>
  req?.headers?.host ? `http://${req.headers.host}` : `http://127.0.0.1:${PORT}`;
export function iconUrl(name, origin = originOf()) {
  try {
    const st = statSync(iconPath(name));
    return `${origin}/app/icon?name=${name}&t=${Math.round(st.mtimeMs)}`;
  } catch {
    return "";
  }
}
export const isArtifact = (name) =>
  /^R\d+$/.test(String(name || "")) && existsSync(join(GEN_DIR, `${name}.tsx`));

const appRow = (name) =>
  db.prepare("SELECT * FROM apps WHERE name=?").get(name) || {};
// Upsert that never clobbers the columns a caller didn't name.
export function setAppCols(name, cols) {
  db.prepare("INSERT INTO apps(name) VALUES(?) ON CONFLICT(name) DO NOTHING").run(name);
  const keys = Object.keys(cols);
  if (keys.length)
    db.prepare(`UPDATE apps SET ${keys.map((k) => `${k}=?`).join(",")} WHERE name=?`)
      .run(...keys.map((k) => cols[k] ?? null), name);
}

// The size hints and one-line description the model declared on the card
// (turn.art), latest card wins — the fallback when apps.w/h/description are
// unset. One pass over the store for every artifact.
export function artHints() {
  const out = new Map();
  for (const t of store.list)
    for (const u of t.turns || []) {
      if (u.role !== "ai" || !/^R\d+$/.test(u.name || "") || !u.art) continue;
      const cur = out.get(u.name) || { w: 0, h: 0, desc: "" };
      if (u.art.w && u.art.h) Object.assign(cur, { w: u.art.w, h: u.art.h });
      if (u.art.desc && !cur.desc) cur.desc = u.art.desc;
      out.set(u.name, cur);
    }
  return out;
}
// The module's own declared size (export const popoutSize = { w, h }).
function declaredSize(name) {
  try {
    const src = readFileSync(join(GEN_DIR, `${name}.tsx`), "utf8");
    const m = src.match(
      /export\s+const\s+popoutSize\s*=\s*\{[^}]*?w\s*:\s*(\d+(?:\.\d+)?)[^}]*?h\s*:\s*(\d+(?:\.\d+)?)/,
    );
    return m ? { w: Number(m[1]), h: Number(m[2]) } : null;
  } catch {
    return null;
  }
}

// The common fields /apps/meta, /artifacts and /app/info share.
export function appSummary(name, origin, row = appRow(name), hints = artHints()) {
  const hint = hints.get(name) || {};
  const decl = row.w && row.h ? null : declaredSize(name);
  const vers = db
    .prepare("SELECT MAX(created_at) AS m FROM versions WHERE name=?")
    .get(name);
  return {
    name,
    title: row.title || name,
    file: row.file || `${name}.tsx`,
    description: row.description ?? hint.desc ?? "",
    icon: iconUrl(name, origin),
    w: Number(row.w) || hint.w || decl?.w || 0,
    h: Number(row.h) || hint.h || decl?.h || 0,
    v: headOf(name),
    updated_at: Math.round(Math.max(Number(row.updated_at) || 0, Number(vers?.m) || 0)) || null,
  };
}

export function appsMeta(origin) {
  const tomb = new Set(db.prepare("SELECT name FROM tombstones").all().map((r) => r.name));
  const rows = new Map(db.prepare("SELECT * FROM apps").all().map((r) => [r.name, r]));
  const hints = artHints();
  return readdirSync(GEN_DIR)
    .map((f) => f.match(/^(R\d+)\.tsx$/)?.[1])
    .filter((n) => n && !tomb.has(n))
    .sort((a, b) => Number(a.slice(1)) - Number(b.slice(1)))
    .map((n) => appSummary(n, origin, rows.get(n) || {}, hints));
}

export function appInfo(name, origin) {
  if (!isArtifact(name)) throw new Error(`no such app ${name}`);
  const row = appRow(name);
  const base = appSummary(name, origin, row);
  const stats = db
    .prepare("SELECT COUNT(*) AS n, MIN(created_at) AS c FROM versions WHERE name=?")
    .get(name);
  const first = db
    .prepare("SELECT prompt FROM versions WHERE name=? ORDER BY v LIMIT 1")
    .get(name);
  const path = (base.v && verFile(name, base.v)) || join(GEN_DIR, `${name}.tsx`);
  let bytes = 0;
  try { bytes = statSync(path).size; } catch {}
  // The creating thread: the earliest ai turn naming this artifact (turn
  // uids are UUID v7 — time-ordered).
  const turn = db
    .prepare("SELECT thread_id FROM turns WHERE name=? AND role='ai' ORDER BY uid, seq LIMIT 1")
    .get(name);
  const threadId = turn?.thread_id || "";
  const thread = threadId
    ? db.prepare("SELECT title FROM threads WHERE id=?").get(threadId)
    : null;
  const bot = threadId
    ? db.prepare("SELECT name FROM bots WHERE thread_id=?").get(threadId)
    : null;
  return {
    ok: true,
    generating: inflight.has(name),
    icon_error: iconErrors.get(name) || "",
    ...base,
    icon_prompt: row.icon_prompt || "",
    versions: Number(stats?.n) || 0,
    history: versionsOf(name),
    created_at: Math.round(Number(stats?.c) || 0) || null,
    prompt: first?.prompt || "",
    path,
    bytes,
    published: !!widgetRow(name),
    thread_id: threadId,
    thread_title: thread?.title || "",
    bot_name: bot?.name || "",
  };
}

const fromUri = (src) => {
  let p = String(src || "").trim();
  if (p.startsWith("file://")) p = decodeURIComponent(p.slice(7));
  return p;
};
// Any image sips can read becomes the square 512×512 icon.png (`out`:
// a Library image's or file's icon lives elsewhere — artifacts.mjs).
export function adoptIcon(name, src, out = iconPath(name)) {
  const p = fromUri(src);
  if (!isAbsolute(p) || !existsSync(p)) throw new Error(`no such image ${src}`);
  const tmp = `${out}.${Date.now().toString(36)}.tmp.png`;
  try {
    execFileSync("sips", ["-s", "format", "png", "-z", "512", "512", p, "--out", tmp], { stdio: "ignore" });
    renameSync(tmp, out);
  } catch (e) {
    try { unlinkSync(tmp); } catch {}
    throw new Error(`could not convert ${p} to a png`);
  }
  return out;
}

const clampSize = (n) => {
  if (n === null || n === undefined || n === "" || Number(n) === 0) return null;
  const v = Number(n);
  if (!Number.isFinite(v)) throw new Error(`bad size ${n}`);
  return Math.min(2400, Math.max(120, Math.round(v)));
};

// POST /app/meta and app_details_update: every field optional; omitted
// fields keep their values.
export function updateAppMeta(name, { title, file, description, w, h, icon } = {}) {
  if (!isArtifact(name)) throw new Error(`no such module ${name}`);
  const cur = appRow(name);
  const cols = { updated_at: Date.now() };
  const t =
    title === undefined
      ? (cur.title ?? null)
      : String(title ?? "").trim().slice(0, 60) || null;
  const f = file === undefined ? (cur.file ?? null) : slugFile(file ?? "") || null;
  cols.title = t;
  cols.file = f;
  if (description !== undefined)
    cols.description = String(description ?? "").trim().slice(0, 280);
  if (w !== undefined) cols.w = clampSize(w);
  if (h !== undefined) cols.h = clampSize(h);
  if (icon !== undefined) {
    if (!String(icon ?? "").trim()) {
      try { unlinkSync(iconPath(name)); } catch {}
      cols.icon = null;
      cols.icon_prompt = null;
    } else {
      cols.icon = adoptIcon(name, icon);
      cols.icon_prompt = null;
    }
  }
  setAppCols(name, cols);
  // A rename moves the whole batch: every <old>.v<n>.tsx in the artifact's
  // dir becomes <new-slug>.v<n>.tsx, and the index re-points at the new
  // paths. (Chunk ids stay flat R<n>v<k>.js — name-stable.)
  const oldSlug = (cur.file || slugFile(cur.title) || `${name}.tsx`).replace(/\.tsx$/i, "");
  const newSlug = (f || slugFile(t) || `${name}.tsx`).replace(/\.tsx$/i, "");
  if (newSlug !== oldSlug) {
    const dir = join(GEN_DIR, name);
    try {
      for (const x of readdirSync(dir)) {
        const m = x.match(/\.v(\d+)\.tsx$/);
        if (m) renameSync(join(dir, x), join(dir, `${newSlug}.v${m[1]}.tsx`));
      }
    } catch {}
    writeIndex();
  }
}

// The subject the icon depicts: the caller's prompt, else the app's title,
// description and first build prompt.
function iconSubject(name, prompt) {
  const p = String(prompt || "").trim();
  if (p) return p.slice(0, 600);
  const row = appRow(name);
  const desc = row.description ?? artHints().get(name)?.desc ?? "";
  const first = db
    .prepare("SELECT prompt FROM versions WHERE name=? ORDER BY v LIMIT 1")
    .get(name)?.prompt || "";
  return [
    row.title || name,
    desc && `— ${desc}`,
    first && `(an app built for: ${first.slice(0, 200)})`,
  ].filter(Boolean).join(" ");
}

const MAGICK = ["/opt/homebrew/bin/magick", "/usr/local/bin/magick"].find((p) => existsSync(p));

/** A generated icon that still came back as a shape on a white (or
 * empty) canvas: the corners go transparent and the shape is trimmed to
 * fill the 512 square. Full-bleed art (dark or colored corners) is left
 * as drawn. No-op without ImageMagick. */
export function normalizeIcon(file) {
  if (!MAGICK) return;
  try {
    const corners = execFileSync(MAGICK, [
      file, "-alpha", "set", "-format",
      "%[pixel:p{1,1}] %[pixel:p{510,1}] %[pixel:p{1,510}] %[pixel:p{510,510}]", "info:",
    ], { encoding: "utf8" }).trim().split(" ");
    const blank = (c) => {
      const n = (c.match(/[\d.]+/g) || []).map(Number);
      const [r, g, b, a = 1] = n;
      return a < 0.1 || (r > 235 && g > 235 && b > 235);
    };
    if (!corners.some(blank)) return;
    const fill = [];
    for (const [x, y] of [[0, 0], [511, 0], [0, 511], [511, 511]])
      fill.push("-draw", `alpha ${x},${y} floodfill`);
    execFileSync(MAGICK, [
      file, "-alpha", "set", "-fuzz", "8%", "-fill", "none", ...fill,
      "-trim", "+repage", "-background", "none", "-gravity", "center",
      "-resize", "512x512^", "-extent", "512x512", file,
    ], { stdio: "ignore" });
  } catch (e) {
    console.log(`  icon normalize failed: ${e?.message || e}`);
  }
}

const inflight = new Map(); // name → Promise
const iconErrors = new Map(); // name → the last generation's failure
export const iconGenerating = (name) => inflight.has(name);
export const iconError = (name) => iconErrors.get(name) || "";
/** Draws the app's icon with an image model `model` can reach (the bot's
 * chat model; see `imageCapable`). */
export function genAppIcon(name, prompt, model) {
  if (!isArtifact(name)) return Promise.reject(new Error(`no such app ${name}`));
  if (inflight.has(name)) return inflight.get(name);
  iconErrors.delete(name);
  const run = (async () => {
    if (!(await imageCapable(model)))
      throw new Error(`${model || "this model"} can't generate images`);
    const subject = iconSubject(name, prompt);
    const out = iconPath(name);
    const tmp = `${out}.${Date.now().toString(36)}.gen.png`;
    try {
      // Rox masks the corners itself, so the art must fill the square:
      // a drawn squircle leaves its canvas (white) in the corners.
      const backend = await generateImage(
        `App icon artwork, a perfect square filled edge to edge: a vivid gradient background that reaches all four edges and corners, with a single bold centered symbol representing: ${subject}. The symbol stays inside the central 70% of the square. Do not draw a rounded-square, squircle, tile, frame or device; no white or empty margins, no text, no letters, no border, no drop shadow.`,
        tmp,
        {
          background: "opaque",
          svgHint: `Draw app icon artwork as a single SVG with viewBox="0 0 512 512". The first element must be <rect width="512" height="512"/> filled with a vivid gradient so the background covers every pixel, corners included (no rx/ry, no rounded-square or squircle shape, no frame, no margin; the app applies its own corner mask). On it, a single bold centered symbol with soft depth and lighting, kept inside the central 360×360, representing: ${subject}. No text, no letters, no border, no drop shadow.`,
        },
      );
      execFileSync("sips", ["-s", "format", "png", "-z", "512", "512", tmp, "--out", out], { stdio: "ignore" });
      normalizeIcon(out);
      setAppCols(name, { icon: out, icon_prompt: subject, updated_at: Date.now() });
      console.log(`  icon ${name} via ${backend}`);
      return backend;
    } finally {
      try { unlinkSync(tmp); } catch {}
    }
  })();
  inflight.set(name, run);
  run
    .catch((e) => iconErrors.set(name, String(e?.message || e).slice(0, 200)))
    .finally(() => inflight.delete(name));
  return run;
}
