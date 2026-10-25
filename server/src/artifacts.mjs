// artifacts.mjs — the Library's one record per artifact. Mini apps and image
// cards (id R<n>), the files a reply made (id file:<path>) and the Drive
// files it made or changed (id gdrive:<file id>, drive.mjs) share a shape:
//   { id, kind, title, filename, description, icon, meta, origin, storage,
//     versioning, versions, v, mtime, published }
// plus the fields /thread/apps always carried (name, file, w, h, attach, i,
// subtitle). Apps keep their details in `apps` (appmeta.mjs); images and
// files keep the user's edits in `artifact_meta`, and `library_hidden`
// archives any of them: still listed, flagged `archived` — the Library's
// Archived chip (the file and its chat card stay).
//
// `storage` says where an artifact lives, `versioning` who keeps its
// versions:
//   {where: "mac", uri: file://…}            Rox wrote it here; "rox"
//   {where: "connector", provider, link, uri} a connected app's file (Google
//                                            Drive); "provider" — its own
//                                            history
//   {where: "agent" | "cloud", …}            recognised, not yet made: a
//                                            remote agent's machine, Rox's
//                                            cloud
// versioning "none" = nobody keeps versions. Only "mac" has a local path —
// open, download and thumbnails branch on `where`.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, statSync, unlinkSync } from "node:fs";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";
import { ASSETS_DIR, GEN_DIR, WIDGETS_DIR, outFile } from "./config.mjs";
import { db, store, writeIndex } from "./state.mjs";
import { headOf, versionsOf } from "./pipeline.mjs";
import { adoptIcon, appSummary, artHints, originOf, updateAppMeta } from "./appmeta.mjs";
import { docMeta, extOf, viewerKind } from "./files.mjs";
import { imageFileOf } from "./tools.mjs";
import { callTool } from "./mcp.mjs";
import { DRIVE_SERVER, driveType, driveWrite } from "./drive.mjs";
import { versionsOfFile } from "./routes/files.mjs";

// Library kinds by extension; audio/video are media, anything else a file.
const KIND_BY_EXT = {
  docx: "document", doc: "document", odt: "document", rtf: "document", md: "document", markdown: "document", txt: "document", pages: "document",
  pptx: "presentation", ppt: "presentation", key: "presentation", odp: "presentation",
  xlsx: "spreadsheet", xlsm: "spreadsheet", xls: "spreadsheet", csv: "spreadsheet", tsv: "spreadsheet", numbers: "spreadsheet", ods: "spreadsheet",
  pdf: "pdf",
};
export function fileKind(f) {
  const ext = f.ext || extOf(f.name || f.path || "");
  if (KIND_BY_EXT[ext]) return KIND_BY_EXT[ext];
  const vk = f.kind || viewerKind(f.name || f.path || "");
  if (vk === "audio" || vk === "video") return "media";
  if (vk === "image") return "image";
  return "file";
}

// An image's or file's own icon (an app's is generated/<name>/icon.png).
const ICON_DIR = join(GEN_DIR, "icons");
export const artIconPath = (id) =>
  join(ICON_DIR, `${createHash("sha1").update(String(id)).digest("hex").slice(0, 16)}.png`);
function artIconUrl(id, origin) {
  try {
    const st = statSync(artIconPath(id));
    return `${origin}/artifact/icon?id=${encodeURIComponent(id)}&t=${Math.round(st.mtimeMs)}`;
  } catch {
    return "";
  }
}

const editRow = (id) => db.prepare("SELECT * FROM artifact_meta WHERE id=?").get(id) || null;
const mtimeOf = (p) => {
  try { return statSync(p).mtimeMs; } catch { return 0; }
};
const storage = (p) => ({ where: "mac", uri: p ? pathToFileURL(p).href : "" });
const isDrive = (id) => /^gdrive:./.test(id);


// Lookups one Library build shares across its records.
function context(origin) {
  return {
    origin,
    published: new Set(db.prepare("SELECT name FROM widgets").all().map((w) => w.name)),
    apps: new Map(db.prepare("SELECT * FROM apps").all().map((r) => [r.name, r])),
    edits: new Map(db.prepare("SELECT * FROM artifact_meta").all().map((r) => [r.id, r])),
    hints: artHints(),
  };
}

// The record of reply file `f` on turn i of thread t (p = its path on disk).
async function fileRecord(t, i, u, f, p, ctx) {
  const id = `file:${f.path}`;
  const e = ctx.edits.get(id) || {};
  const filename = e.file || f.name || basename(f.path);
  const size = f.size || (existsSync(p) ? statSync(p).size : 0);
  return {
    id,
    name: id,
    kind: fileKind(f),
    title: e.title || filename,
    filename,
    file: f.path,
    description: e.description || "",
    icon: artIconUrl(id, ctx.origin),
    meta: {
      size,
      ext: f.ext || extOf(f.name || f.path),
      ...(f.width && f.height ? { w: f.width, h: f.height } : {}),
      ...(f.duration ? { duration: f.duration } : {}),
      ...(await docMeta(p)),
    },
    // the viewer's record, under its display name
    attach: e.file ? { ...f, name: e.file } : f,
    i,
    origin: { thread: t.id, uid: t.uid || "", i, turn: u.id || "" },
    storage: storage(p),
    versioning: "rox",
    published: false,
    mtime: mtimeOf(p),
    // a file saved from Rox keeps versions (routes/files.mjs); the viewer
    // lists them, the Library shows the current number
    v: versionsOfFile(f.path).at(-1)?.v || 0,
    versions: [],
  };
}

// Thread t's Drive links by file id, the newest first with what older ones
// knew filled in (a later edit may not know the name or the type).
function driveLinks(t) {
  const out = new Map();
  for (let i = t.turns.length - 1; i >= 0; i--)
    for (const l of t.turns[i].links || []) {
      if (l?.provider !== "gdrive" || !l.id) continue;
      const cur = out.get(l.id);
      if (!cur) out.set(l.id, { ...l, first: l.at || 0 });
      else {
        for (const k of ["name", "mime", "link"]) cur[k] ||= l[k] || "";
        cur.first = l.at || cur.first;
      }
    }
  return out;
}

// The record of Drive file link `l` (merged across the thread) on turn i.
function linkRecord(t, i, u, l, ctx) {
  const id = `gdrive:${l.id}`;
  const e = ctx.edits.get(id) || {};
  const { kind, type } = driveType(l.mime, l.name);
  const ext = extOf(l.name || "");
  return {
    id,
    name: id,
    kind,
    title: l.name || `Untitled ${type}`,
    filename: l.name || "",
    file: "",
    description: e.description || "",
    icon: artIconUrl(id, ctx.origin),
    meta: {
      provider: "Google Drive",
      type,
      mime: l.mime || "",
      action: l.action || "",
      ...(ext && !/google-apps/.test(l.mime || "") ? { ext } : {}),
    },
    i,
    origin: { thread: t.id, uid: t.uid || "", i, turn: u.id || "" },
    storage: { where: "connector", provider: "gdrive", link: l.link || "", uri: l.link || "" },
    versioning: "provider",
    published: false,
    mtime: l.at || 0,
    created_at: l.first || l.at || 0,
    v: 0,
    versions: [],
  };
}

const promptBefore = (t, i) => {
  for (let j = i - 1; j >= 0; j--) if (t.turns[j].role === "user") return t.turns[j].text || "";
  return "";
};

// The record of the image card or mini app R<n> on turn i of thread t.
function cardRecord(t, i, u, ctx) {
  const name = u.name;
  const origin = { thread: t.id, uid: t.uid || "", i, turn: u.id || "" };
  const prompt = promptBefore(t, i);
  const app = ctx.apps.get(name) || {};
  const img = imageFileOf(name);
  if (img) {
    const e = ctx.edits.get(name) || {};
    const path = join(ASSETS_DIR, img);
    // the card's size, else the first card of it that declared one
    const art = u.art?.w ? u.art : t.turns.find((x) => x.name === name && x.art?.w)?.art || u.art || {};
    return {
      id: name,
      name,
      kind: "image",
      title: e.title || app.title || t.turns.find((x) => x.name === name && x.art?.title)?.art.title || prompt.slice(0, 60) || "Image",
      filename: e.file || img,
      file: img,
      description: e.description ?? art.desc ?? "",
      icon: artIconUrl(name, ctx.origin),
      meta: { w: art.w || 0, h: art.h || 0, ...(art.via ? { made_with: art.via } : {}) },
      w: art.w || 0,
      h: art.h || 0,
      i,
      origin,
      storage: storage(path),
      versioning: "rox",
      published: false,
      mtime: mtimeOf(path),
      v: headOf(name),
      versions: versionsOf(name),
    };
  }
  const caption = u.caption && !/error|implicit|rejected/i.test(u.caption) ? u.caption : "";
  const sum = appSummary(name, ctx.origin, app, ctx.hints);
  const path = join(GEN_DIR, `${name}.tsx`);
  return {
    id: name,
    name,
    kind: "app",
    title: app.title || u.art?.title || u.label || caption || prompt.slice(0, 60) || name,
    filename: sum.file,
    file: sum.file,
    description: sum.description,
    icon: sum.icon,
    meta: { w: sum.w, h: sum.h },
    subtitle: prompt.replace(/\s+/g, " ").slice(0, 120),
    i,
    origin,
    storage: storage(existsSync(path) ? path : join(WIDGETS_DIR, `${name}.tsx`)),
    versioning: "rox",
    published: ctx.published.has(name),
    mtime: mtimeOf(path),
    v: headOf(name),
    versions: versionsOf(name),
  };
}

/** GET /thread/apps — thread `t`'s artifacts, newest first, one record
 *  each; archived ones flagged `archived`, deleted and missing ones left
 *  out. */
export async function libraryOf(t, origin = originOf()) {
  const ctx = context(origin);
  const tomb = new Set(db.prepare("SELECT name FROM tombstones").all().map((r) => r.name));
  const hidden = new Set(db.prepare("SELECT id FROM library_hidden").all().map((r) => r.id));
  const seen = new Set();
  const out = [];
  const links = driveLinks(t);
  for (let i = t.turns.length - 1; i >= 0; i--) {
    const u = t.turns[i];
    if (u.role !== "ai" || u.building) continue;
    for (const l of u.links || []) {
      const id = `gdrive:${l?.id}`;
      if (!links.has(l?.id) || seen.has(id)) continue;
      seen.add(id);
      out.push(archived(linkRecord(t, i, u, links.get(l.id), ctx), hidden));
    }
    for (const f of u.files || []) {
      const id = `file:${f?.path}`;
      if (!f?.path || seen.has(id)) continue;
      seen.add(id);
      const p = outFile(f.path);
      if (!p || !existsSync(p)) continue;
      out.push(archived(await fileRecord(t, i, u, f, p, ctx), hidden));
    }
    if (!u.name || seen.has(u.name) || tomb.has(u.name)) continue;
    seen.add(u.name);
    // A name with no file anywhere was deleted before tombstones
    // existed — same zombie, same filter.
    const gone =
      !existsSync(join(GEN_DIR, `${u.name}.tsx`)) &&
      !existsSync(join(WIDGETS_DIR, `${u.name}.tsx`));
    if (gone) continue;
    out.push(archived(cardRecord(t, i, u, ctx), hidden));
  }
  return out;
}

// A record, flagged when the user archived it.
const archived = (rec, hidden) => (hidden.has(rec.id) ? { ...rec, archived: true } : rec);

/** POST /artifact/delete {id} — a file a reply made (file:<path>): its
 *  cards leave every chat, the file leaves the disk, and it leaves every
 *  group and the Library. (Mini apps and image cards go by /app/delete.) */
export function deleteArtifact(id) {
  id = String(id || "");
  if (!id.startsWith("file:")) throw new Error(`${id} isn't a file — use /app/delete`);
  const path = id.slice(5);
  for (const t of store.list) {
    for (const u of t.turns) if (u.files?.length) u.files = u.files.filter((f) => f?.path !== path);
    // a reply that held only this file has nothing left to show
    t.turns = t.turns.filter((u) => u.role !== "ai" || u.building || u.name || (u.text || "").trim() || u.files?.length || u.links?.length);
  }
  writeIndex();
  forgetArtifact(id);
  const p = outFile(path);
  if (p) try { unlinkSync(p); } catch {}
}

/** A deleted artifact's rows: its group memberships (any version's ref),
 *  archive flag and edited details. */
export function forgetArtifact(id) {
  db.prepare("DELETE FROM group_items WHERE kind='artifact' AND (ref=? OR ref LIKE ?)").run(id, `${id}@v%`);
  db.prepare("DELETE FROM library_hidden WHERE id=?").run(id);
  db.prepare("DELETE FROM artifact_meta WHERE id=?").run(id);
}

// Where artifact `id` was made: the newest turn holding it — in `prefer`
// (a thread) when it has one, else in any thread.
function locate(id, prefer = null) {
  const path = id.startsWith("file:") ? id.slice(5) : null;
  const drive = isDrive(id) ? id.slice(7) : null;
  const inThread = (t) => {
    for (let i = t.turns.length - 1; i >= 0; i--) {
      const u = t.turns[i];
      if (u.role !== "ai" || u.building) continue;
      if (drive) {
        if ((u.links || []).some((x) => x?.id === drive)) return { t, i, u, l: driveLinks(t).get(drive) };
      } else if (path) {
        const f = (u.files || []).find((x) => x?.path === path);
        if (f) return { t, i, u, f };
      } else if (u.name === id) return { t, i, u };
    }
    return null;
  };
  if (prefer) {
    const at = inThread(prefer);
    if (at) return at;
  }
  for (const t of store.list) {
    const at = inThread(t);
    if (at) return at;
  }
  return null;
}

/** The Library records of `refs` ([{id, thread}]), in order — each from
 *  its own thread when that still holds it. null for one that is gone
 *  (deleted, tombstoned, its file missing); Remove from Library doesn't
 *  count — the sidebar's groups still show it. */
export async function artifactRecords(refs, origin = originOf()) {
  const ctx = context(origin);
  const tomb = new Set(db.prepare("SELECT name FROM tombstones").all().map((r) => r.name));
  const out = [];
  for (const { id, thread } of refs) {
    const at = locate(String(id || ""), store.list.find((t) => t.id === thread) || null);
    if (!at) {
      out.push(null);
      continue;
    }
    const { t, i, u, f, l } = at;
    if (l) {
      out.push(linkRecord(t, i, u, l, ctx));
      continue;
    }
    if (f) {
      const p = outFile(f.path);
      out.push(p && existsSync(p) ? await fileRecord(t, i, u, f, p, ctx) : null);
      continue;
    }
    const gone = tomb.has(u.name) ||
      (!existsSync(join(GEN_DIR, `${u.name}.tsx`)) && !existsSync(join(WIDGETS_DIR, `${u.name}.tsx`)));
    out.push(gone ? null : cardRecord(t, i, u, ctx));
  }
  return out;
}

/** GET /artifact/info?id= — an image's or file's record plus what Edit
 *  Details shows (where it was made, when, its path and size). */
export async function artifactInfo(id, origin = originOf()) {
  const at = locate(String(id || ""));
  if (!at) throw new Error(`no such artifact ${id}`);
  const { t, i, u, f, l } = at;
  const ctx = context(origin);
  const bot = store.bots.find((b) => b.threadId === t.id);
  // a connector's file: no path or bytes here — Drive keeps it
  if (l) {
    const rec = linkRecord(t, i, u, l, ctx);
    return {
      ok: true,
      ...rec,
      versions: 0,
      history: [],
      thread_id: t.id,
      thread_title: t.title || "",
      bot_name: bot?.name || "",
      created_at: rec.created_at || null,
      updated_at: rec.mtime || null,
      prompt: "",
      path: "",
      bytes: 0,
    };
  }
  let rec;
  if (f) {
    const p = outFile(f.path);
    if (!p || !existsSync(p)) throw new Error(`no such file ${f.path}`);
    rec = await fileRecord(t, i, u, f, p, ctx);
  } else rec = cardRecord(t, i, u, ctx);
  const path = decodeURIComponent(rec.storage.uri.replace(/^file:\/\//, ""));
  let bytes = 0, born = 0;
  try {
    const st = statSync(path);
    bytes = st.size;
    born = st.birthtimeMs || st.mtimeMs;
  } catch {}
  const first = f ? null : db
    .prepare("SELECT MIN(created_at) AS c FROM versions WHERE name=?")
    .get(rec.id);
  return {
    ok: true,
    ...rec,
    versions: rec.versions.length,
    history: rec.versions,
    thread_id: t.id,
    thread_title: t.title || "",
    bot_name: bot?.name || "",
    created_at: Math.round(Number(first?.c) || born) || null,
    updated_at: Number(editRow(rec.id)?.updated_at) || null,
    prompt: f ? "" : promptBefore(t, i),
    path,
    bytes,
  };
}

/** POST /artifact/meta — every field optional, omitted ones keep their
 *  values. A mini app's edit goes to `apps` (updateAppMeta); an image's or
 *  file's to artifact_meta. `filename` is a display name with the
 *  artifact's own extension kept — files on disk are never renamed. icon ""
 *  removes the icon; a file:// uri or absolute path is adopted. */
export async function updateArtifactMeta(id, { title, filename, file, description, icon } = {}) {
  id = String(id || "");
  if (filename === undefined) filename = file;
  if (/^R\d+$/.test(id) && !imageFileOf(id))
    return updateAppMeta(id, { title, file: filename, description, icon });
  const at = locate(id);
  if (!at) throw new Error(`no such artifact ${id}`);
  const ext = at.f ? at.f.ext || extOf(at.f.name || at.f.path) : at.l ? "" : extOf(imageFileOf(id) || "");
  const cols = { updated_at: Date.now() };
  if (title !== undefined) cols.title = String(title ?? "").trim().slice(0, 80) || null;
  if (filename !== undefined) {
    let stem = String(filename ?? "").trim().replace(/[/\\:]/g, "-");
    if (ext && stem.toLowerCase().endsWith(`.${ext}`)) stem = stem.slice(0, -ext.length - 1);
    stem = stem.trim().slice(0, 120);
    cols.file = stem ? (ext ? `${stem}.${ext}` : stem) : null;
  }
  if (description !== undefined) cols.description = String(description ?? "").trim().slice(0, 280);
  if (icon !== undefined) {
    if (!String(icon ?? "").trim()) {
      try { unlinkSync(artIconPath(id)); } catch {}
      cols.icon = null;
    } else {
      mkdirSync(ICON_DIR, { recursive: true });
      cols.icon = adoptIcon(id, icon, artIconPath(id));
    }
  }
  db.prepare("INSERT INTO artifact_meta(id) VALUES(?) ON CONFLICT(id) DO NOTHING").run(id);
  const keys = Object.keys(cols);
  db.prepare(`UPDATE artifact_meta SET ${keys.map((k) => `${k}=?`).join(",")} WHERE id=?`)
    .run(...keys.map((k) => cols[k] ?? null), id);
}

/** POST /artifact/hide — out of (or back into) the Library. */
export function hideArtifact(id, hidden = true) {
  id = String(id || "");
  if (!/^R\d+$/.test(id) && !id.startsWith("file:") && !isDrive(id)) throw new Error(`bad artifact id ${id}`);
  if (hidden) db.prepare("INSERT OR REPLACE INTO library_hidden(id,at) VALUES(?,?)").run(id, Date.now());
  else db.prepare("DELETE FROM library_hidden WHERE id=?").run(id);
}

/** POST /artifact/rename-remote — renames a connector's file where it
 *  lives (Drive's update_drive_file, on the user's behalf) and records the
 *  rename on the newest turn holding it. Replies with the artifact's info. */
export async function renameRemote(id, name, origin = originOf()) {
  id = String(id || "");
  name = String(name ?? "").trim().slice(0, 200);
  if (!isDrive(id)) throw new Error(`${id} is not stored in a connected app`);
  if (!name) throw new Error("a file needs a name");
  const at = locate(id);
  if (!at) throw new Error(`no such artifact ${id}`);
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(DRIVE_SERVER);
  if (!row) throw new Error("Google Drive isn't connected");
  const input = { file_id: at.l.id, name };
  const account = at.u.links.find((x) => x.id === at.l.id)?.account;
  if (account) input.user_google_email = account;
  const r = await callTool(row, "update_drive_file", input);
  if (r.is_error || /^\s*Error\b/i.test(r.content)) throw new Error(String(r.content).slice(0, 300));
  const link = driveWrite("update_drive_file", input, r.content)?.link;
  const cur = at.u.links.find((x) => x.id === at.l.id);
  Object.assign(cur, { name, action: "renamed", at: Date.now() }, link ? { link } : {});
  writeIndex();
  return artifactInfo(id, origin);
}
