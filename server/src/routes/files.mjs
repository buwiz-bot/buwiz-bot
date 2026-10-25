// routes/files.mjs — extracted from app-server.mjs
import { copyFileSync, existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { serverFile, ASSETS_DIR, AVATAR_DIR, ATTACH_DIR, outFile } from "../config.mjs";
import { sendRanged } from "../state.mjs";
import { osa } from "../avatars.mjs";
import { highlight, renderFile } from "../files.mjs";
import { officeCaps, officePkgRoot } from "../office.mjs";

export default async function handle(req, res) {
  // Image avatars (Grok's Generate / Upload tabs): files land in
  // generated/avatars and the app shows them by file:// URI.
  // Mermaid, rendered by mermaid.js in a web view (served locally: the web
  // view's grant is 127.0.0.1). The page reports its size; "full" mode zooms.
  if (req.method === "GET" && req.url === "/mermaid.js") {
    res.setHeader("content-type", "text/javascript");
    return res.end(readFileSync(serverFile("../node_modules/mermaid/dist/mermaid.min.js")));
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
    return true;
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
    return true;
  }
  // BetterOffice viewer: a WebView page that paints the engines' display
  // lists. /office/vendor/<pkg>/… statically serves the installed packages
  // (dist bundles, wasm, font assets) so the page's imports resolve.
  if (req.method === "GET" && (req.url === "/office/" || req.url.startsWith("/office/?") || req.url.startsWith("/office/editor/"))) {
    const editing = req.url.startsWith("/office/editor/");
    const page = serverFile(editing ? "office/editor/editor.html" : "office/viewer.html");
    res.setHeader("content-type", "text/html; charset=utf-8");
    res.setHeader("cache-control", "no-cache");
    let html = readFileSync(page, "utf8");
    if (editing) {
      // WebViews cache /office/dist/editor.js hard; a ?v=mtime makes every
      // bundle rebuild a fresh URL so stale chrome can never come back.
      const dist = serverFile("office/editor/dist/editor.js");
      const v = existsSync(dist) ? Math.floor(statSync(dist).mtimeMs) : 0;
      html = html.replaceAll("/office/dist/editor.js", `/office/dist/editor.js?v=${v}`);
    }
    return res.end(html);
  }
  // The bundled BetterOffice editor app + its esbuild chunks and wasm —
  // resolved relative to the bundle URL, so dist/generated/ holds the wasms.
  if (req.method === "GET" && req.url.startsWith("/office/dist/")) {
    const rest = decodeURIComponent(req.url.slice("/office/dist/".length).split("?")[0]);
    const dir = serverFile("office/editor/dist");
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
    return true;
  }
  // {path} is a reply file, {asset} a generated image (by bare file name);
  // `name` is the panel's default (the Library's display name).
  if (req.method === "POST" && req.url === "/file/save") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const { path, asset, name: as } = JSON.parse(body || "{}");
      const bare = String(asset || "").split("/").pop();
      const p = asset ? (bare ? join(ASSETS_DIR, bare) : null) : outFile(path || "");
      res.setHeader("content-type", "application/json");
      if (!p || !existsSync(p)) return res.end(JSON.stringify({ ok: false }));
      const name = String(as || p.split("/").pop()).split("/").pop().replace(/["\\]/g, "");
      osa(`POSIX path of (choose file name with prompt "Download" default name "${name}")`)
        .then((dest) => {
          copyFileSync(p, dest);
          res.end(JSON.stringify({ ok: true, dest }));
        })
        .catch(() => res.end(JSON.stringify({ ok: false, cancelled: true })));
    });
    return true;
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
  // A mini app's `asset:/generated/assets/<file>` (generated images): Rox
  // gives its mini apps http://127.0.0.1:8787/asset/ as their asset base.
  // Only that folder, by bare file name.
  if (req.method === "GET" && req.url.startsWith("/asset/generated/assets/")) {
    const name = decodeURIComponent(req.url.slice("/asset/generated/assets/".length).split("?")[0]).split("/").pop();
    const path = join(ASSETS_DIR, name);
    if (!name || !existsSync(path)) {
      res.statusCode = 404;
      return res.end();
    }
    const ext = (name.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
    const type = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", mp4: "video/mp4" }[ext] || "application/octet-stream";
    res.setHeader("cache-control", "max-age=86400");
    return sendRanged(req, res, path, type);
  }
  return false;
}
