// routes/artifacts.mjs — the Library's per-artifact edits (artifacts.mjs):
// details, icons and hiding for images and files; a mini app's details go
// through to /app/meta's updateAppMeta; a Drive file's rename goes to Drive.
import { existsSync, readFileSync } from "node:fs";
import { appInfo, originOf } from "../appmeta.mjs";
import { artIconPath, artifactInfo, deleteArtifact, hideArtifact, renameRemote, updateArtifactMeta } from "../artifacts.mjs";
import { imageFileOf } from "../tools.mjs";

const isApp = (id) => /^R\d+$/.test(id) && !imageFileOf(id);
const info = (id, req) => (isApp(id) ? appInfo(id, originOf(req)) : artifactInfo(id, originOf(req)));

export default async function handle(req, res) {
  // POST /artifact/meta {id, title?, filename?, description?, icon?} — Edit
  // Details' save; icon "" removes it, a file:// uri or path is adopted.
  // Replies with the artifact's info (an app's /app/info object).
  if (req.method === "POST" && req.url === "/artifact/meta") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const j = JSON.parse(body || "{}");
        const id = String(j.id || "");
        await updateArtifactMeta(id, j);
        res.end(JSON.stringify(await info(id, req)));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // POST /artifact/rename-remote {id: gdrive:<file id>, name} — renames the
  // file in Drive (update_drive_file). Replies with the artifact's info.
  if (req.method === "POST" && req.url === "/artifact/rename-remote") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const j = JSON.parse(body || "{}");
        res.end(JSON.stringify(await renameRemote(j.id, j.name, originOf(req))));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // GET /artifact/info?id=R<n>|file:<path>|gdrive:<id> — everything Edit
  // Details shows.
  if (req.method === "GET" && req.url.split("?")[0] === "/artifact/info") {
    res.setHeader("content-type", "application/json");
    const id = new URL(req.url, "http://x").searchParams.get("id") || "";
    try {
      return res.end(JSON.stringify(await info(id, req)));
    } catch (e) {
      res.statusCode = 404;
      return res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
    }
  }
  // POST /artifact/hide {id, hidden = true} — Remove from Library (and back).
  if (req.method === "POST" && req.url === "/artifact/hide") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { id, hidden } = JSON.parse(body || "{}");
        hideArtifact(id, hidden !== false);
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // POST /artifact/delete {id: file:<path>} — the file, its cards, groups.
  if (req.method === "POST" && req.url === "/artifact/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        deleteArtifact(JSON.parse(body || "{}").id);
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // GET /artifact/icon?id=…[&t=…] — an image's or file's icon (404 when none).
  if (req.method === "GET" && req.url.split("?")[0] === "/artifact/icon") {
    const id = new URL(req.url, "http://x").searchParams.get("id") || "";
    const p = id ? artIconPath(id) : "";
    if (!p || !existsSync(p)) {
      res.statusCode = 404;
      return res.end("no icon");
    }
    res.setHeader("content-type", "image/png");
    res.setHeader("cache-control", "no-cache");
    return res.end(readFileSync(p));
  }
  return false;
}
