// routes/apps.mjs — extracted from app-server.mjs
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, rmSync, readFileSync, readdirSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { homedir } from "node:os";
import { createHash } from "node:crypto";
import { DATA_DIR, GEN_DIR, MODEL, NSH, PORT, NSH_HOST, BUNDLE_ID, OPENCODEX, WIDGETS_DIR, POP_W, POP_H, WINDOWS_DIR, CHUNK_DIR, OUT_DIR } from "../config.mjs";
import { uuidv7, openWindows, db, metaGet, metaSet, slugFile, store, activeThread, getLinearKey, writeIndex, writeShelf, widgetRow } from "../state.mjs";
import { emitChunk, walkFiles, headOf, verFile, versionsOf, setHead } from "../pipeline.mjs";
import { desktopBounds, raiseWidget } from "../windows.mjs";
import { imageCapable, imageModels } from "../avatars.mjs";
import { appInfo, appsMeta, artHints, genAppIcon, iconPath, iconUrl, originOf, updateAppMeta } from "../appmeta.mjs";
import { forgetArtifact } from "../artifacts.mjs";
import { mcpAppModule } from "../tools.mjs";


// opencodex's providers and how each authenticates (its config file):
// a ChatGPT / OAuth login is a subscription, a key an API connection.
let connCache = { at: 0, conns: { providers: {}, def: "openai" } };
function opencodexConnections() {
  if (Date.now() - connCache.at < 30_000) return connCache.conns;
  try {
    const c = JSON.parse(readFileSync(join(homedir(), ".opencodex", "config.json"), "utf8"));
    const providers = {};
    for (const [id, p] of Object.entries(c.providers || {}))
      providers[id] = { auth: p.authMode || "", label: p.label || p.name || "" };
    connCache = { at: Date.now(), conns: { providers, def: c.defaultProvider || "openai" } };
  } catch {
    connCache.at = Date.now();
  }
  return connCache.conns;
}
const CONN_LABELS = {
  "anthropic-apikey": "Anthropic", anthropic: "Anthropic", openai: "OpenAI", xai: "xAI",
  openrouter: "OpenRouter", google: "Google", gemini: "Google", deepseek: "DeepSeek",
  moonshot: "Moonshot", groq: "Groq", together: "Together", fireworks: "Fireworks",
};
function connectionOf(id, owner, conns) {
  const slash = id.indexOf("/");
  const conn = slash > 0 ? id.slice(0, slash) : String(owner || conns.def).toLowerCase();
  const p = conns.providers[conn] || {};
  const auth = p.auth || (/apikey|key/.test(conn) ? "key" : "forward");
  const label =
    p.label || CONN_LABELS[conn] || conn.replace(/-?apikey$/i, "").replace(/^\w/, (c) => c.toUpperCase());
  return { connection: conn, group: label, subscription: auth !== "key" };
}

export default async function handle(req, res) {
  if (req.method === "GET" && req.url.startsWith("/module")) {
    // name is "R<n>" (current) or "R<n>@v<k>" (a pinned version).
    const raw = new URL(req.url, "http://x").searchParams.get("name") || "";
    // a mini app an MCP server ships: `mcp:<server>/<app>[|<props>]`
    if (raw.startsWith("mcp:")) {
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify(await mcpAppModule(raw)));
    }
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
            head: headOf(name),
            versions: versionsOf(name),
            title:
              db.prepare("SELECT title FROM apps WHERE name=?").get(name)
                ?.title || null,
          })
        : JSON.stringify({ source: "", published: !!widgetRow(name) }),
    );
  }
  // POST /app/meta {name, title?, file?, description?, w?, h?, icon?} — the
  // Library's renames and the details modal's edits. `file` is a display
  // label: the version files batch-rename to the new slug. icon "" removes
  // the icon; a file:// uri or absolute path is adopted as icon.png.
  // Replies with the /app/info object.
  if (req.method === "POST" && req.url === "/app/meta") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const j = JSON.parse(body || "{}");
        const name = String(j.name || "");
        updateAppMeta(name, j);
        res.end(JSON.stringify(appInfo(name, originOf(req))));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // GET /app/info?name=R<n> — everything the details modal shows.
  if (req.method === "GET" && req.url.split("?")[0] === "/app/info") {
    res.setHeader("content-type", "application/json");
    const name = new URL(req.url, "http://x").searchParams.get("name") || "";
    try {
      return res.end(JSON.stringify(appInfo(name, originOf(req))));
    } catch (e) {
      res.statusCode = 404;
      return res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
    }
  }
  // GET /apps/meta — details for every live (non-tombstoned) artifact.
  if (req.method === "GET" && req.url.split("?")[0] === "/apps/meta") {
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ apps: appsMeta(originOf(req)) }));
  }
  // GET /app/icon?name=R<n>[&t=…] — the app's icon.png (404 when none).
  if (req.method === "GET" && req.url.split("?")[0] === "/app/icon") {
    const name = new URL(req.url, "http://x").searchParams.get("name") || "";
    const p = /^R\d+$/.test(name) ? iconPath(name) : "";
    if (!p || !existsSync(p)) {
      res.statusCode = 404;
      return res.end("no icon");
    }
    res.setHeader("content-type", "image/png");
    res.setHeader("cache-control", "no-cache");
    return res.end(readFileSync(p));
  }
  // POST /app/icon/gen {name, prompt?, model} — start drawing the app's
  // icon with an image model the bot's `model` reaches (prompt defaults to
  // its title + description + first build prompt). Answers at once; the
  // client polls /app/info (`generating`, `icon_error`).
  if (req.method === "POST" && req.url === "/app/icon/gen") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const { name, prompt, model } = JSON.parse(body || "{}");
        const n = String(name || "");
        if (!(await imageCapable(model)))
          throw new Error(`${model || "This model"} can't generate images`);
        genAppIcon(n, prompt, model).catch(() => {});
        res.end(JSON.stringify(appInfo(n, originOf(req))));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
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
        // An open pop-out of the app shows the old head: reopen it on v.
        const open = openWindows.get(String(name));
        if (open && open.exitCode === null && !open.killed && open.shownV !== Number(v)) {
          open.kill();
          openWindows.delete(String(name));
          fetch(`http://127.0.0.1:${PORT}/popout`, {
            method: "POST",
            body: JSON.stringify({ name, w: open.hintW, h: open.hintH }),
          }).catch(() => {});
        }
        res.end(JSON.stringify({ ok: true, v: Number(v) }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
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
    return true;
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
      db.prepare("SELECT name,title,file,description FROM apps").all().map((r) => [r.name, r]),
    );
    const hints = artHints();
    const origin = originOf(req);
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
        description: meta.description ?? hints.get(m[0])?.desc ?? "",
        icon: iconUrl(m[0], origin),
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
      .then(async (j) => {
        const draws = await imageModels();
        const conns = opencodexConnections();
        const ids = new Set(j.data.map((m) => m.id));
        const models = j.data
          .filter((m) => !m.id.endsWith("--fast"))
          .map((m) => ({
            id: m.id,
            owner: m.owned_by || "",
            fast: ids.has(m.id + "--fast"),
            efforts: (m.reasoning_efforts || []).map((e) => ({ value: e.value, label: String(e.label || e.value).replace(/ Effort$/i, "") })),
            defaultEffort: m.reasoning_effort || "",
            // reaches an image model (App Details' Generate)
            image: !!draws.get(m.id),
            // the route: opencodex's provider (the id's prefix, else its
            // default), a subscription login or an API connection
            ...connectionOf(m.id, m.owned_by, conns),
          }));
        res.end(JSON.stringify({ ok: true, default: MODEL, models }));
      })
      .catch(() => res.end(JSON.stringify({ ok: false, default: MODEL, models: [], error: `opencodex at ${OPENCODEX} unreachable` })));
    return true;
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
        // the sidebar's two fixed sections, renamable (blank = default)
        unassigned_name: metaGet("unassigned_name") || "Unassigned",
        chats_name: metaGet("chats_name") || "Chats",
      }));
    }
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const { theme, model, effort, fast, unassigned_name, chats_name } = JSON.parse(body || "{}");
      if (["system", "dark", "light"].includes(theme)) metaSet("theme", theme);
      if (model !== undefined) metaSet("model", String(model));
      if (effort !== undefined) metaSet("effort", String(effort));
      if (fast !== undefined) metaSet("fast", fast ? "1" : "0");
      // a section keeps a name: a blank one is refused
      for (const [k, v] of [["unassigned_name", unassigned_name], ["chats_name", chats_name]]) {
        if (v === undefined) continue;
        const n = String(v).replace(/\s+/g, " ").trim().slice(0, 60);
        if (!n) {
          res.statusCode = 400;
          res.setHeader("content-type", "application/json");
          return res.end(JSON.stringify({ ok: false, error: "a section needs a name" }));
        }
        metaSet(k, n);
      }
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ ok: true }));
    });
    return true;
  }
  // The user's keyboard shortcuts: DATA_DIR/keymap.json, a map of command
  // id → its keystrokes (["mod+h"]; [] unbinds it). Only the user's
  // changes live there — the app ships the defaults — and the file can be
  // edited by hand: a GET always reads it fresh.
  if (req.url === "/keymap") {
    const file = join(DATA_DIR, "keymap.json");
    res.setHeader("content-type", "application/json");
    if (req.method === "GET") {
      let map = {};
      try { map = JSON.parse(readFileSync(file, "utf8")); } catch {}
      return res.end(JSON.stringify({ keymap: map && typeof map === "object" && !Array.isArray(map) ? map : {} }));
    }
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const map = JSON.parse(body || "{}").keymap;
        if (!map || typeof map !== "object" || Array.isArray(map)) throw new Error("keymap must be an object");
        const clean = {};
        for (const [id, keys] of Object.entries(map)) {
          if (!/^[a-z0-9_.-]+$/i.test(id) || !Array.isArray(keys)) throw new Error(`bad entry ${id}`);
          clean[id] = keys.map((k) => String(k).trim().toLowerCase()).filter(Boolean).slice(0, 4);
        }
        writeFileSync(file, JSON.stringify(clean, null, 2) + "\n");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
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
    return true;
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
    return true;
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
        // …and out of every group, the archive and its edited details
        forgetArtifact(name);
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
    return true;
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
          id: uuidv7(),
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
    return true;
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
    return true;
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

  // GET /catalog — the Mini Apps tab's data: every published widget as a
  // metadata card. kinds: "db" (persists via /state), "api" (talks to a
  // /data source), "mem" (in-memory only — fresh state per surface).
  // tags are user-set via /widget-meta.
  if (req.method === "GET" && req.url === "/catalog") {
    res.setHeader("content-type", "application/json");
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
    return true;
  }

  // POST /popout {name, v?, w?, h?} — freeze a mount wrapper and spawn
  // `nsh run` on it: the widget gets its own real macOS window.
  if (req.method === "POST" && req.url === "/popout") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      let name = "", popV = null, hintW = 0, hintH = 0;
      try {
        const j = JSON.parse(body || "{}");
        name = j.name || "";
        popV = j.v ? Number(j.v) : null;
        // The size the caller already laid the app out at (a chat card:
        // its declared width, its measured height), so the window opens
        // at it instead of POP_H and then hugging down.
        hintW = Number(j.w) > 0 ? Number(j.w) : 0;
        hintH = Number(j.h) > 0 ? Number(j.h) : 0;
      } catch {}
      res.setHeader("content-type", "application/json");
      if (!/^R\d+$/.test(name)) return res.end(JSON.stringify({ ok: false }));
      const key = `${name}${popV ? `@v${popV}` : ""}`;
      // The version the window shows: the pinned one, else the head.
      const shownV = popV || headOf(name) || 0;
      // Already open: raise the live window instead of rewriting the
      // wrapper under it — a rewrite hot-reloads the dev session and the
      // module remounts, wiping its in-memory state. A window still on an
      // older head is replaced.
      const prev = openWindows.get(key);
      if (prev && prev.exitCode === null && !prev.killed) {
        if (prev.shownV === shownV) {
          raiseWidget(`widget · ${name}`);
          return res.end(JSON.stringify({ ok: true, already: true }));
        }
        prev.kill();
        openWindows.delete(key);
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
      // The window hugs content height up to three quarters of the screen.
      const capH = Math.max(560, Math.floor(desktopBounds().h * 0.75));
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
        Math.max(240, Math.round(Number(sizeM?.[1] ?? widthM?.[1] ?? (hintW || POP_W)))),
      );
      const wantH = sizeM
        ? Math.min(capH, Math.max(120, Math.round(Number(sizeM[2]))))
        : hintH
          ? Math.min(capH, Math.max(120, Math.round(hintH)))
          : Number(POP_H);
      const popName = popV ? `${name}@${popV}` : name;
      const appTitle =
        db.prepare("SELECT title FROM apps WHERE name=?").get(name)?.title || name;
      const verLabel = shownV
        ? db.prepare("SELECT label FROM versions WHERE name=? AND v=?").get(name, shownV)?.label || ""
        : "";
      const badge = shownV ? `v${shownV}${verLabel ? ` · ${verLabel}` : ""}` : "";
      mkdirSync(WINDOWS_DIR, { recursive: true });
      writeFileSync(
        join(WINDOWS_DIR, `pop-${name}${popV ? `-${popV}` : ""}.tsx`),
        `import { mount, Text, View } from "nsh";
import { isAvailable, Window } from "nsh/desktop";
import { colorScheme } from "nsh/device";
import R, * as mod from "${rel}";
// Sizing contract: \`export const popoutSize = { w, h }\` fixes both axes
// (canvas apps, games); \`export const popoutWidth\` sets the width and
// the height hugs the content; neither → ${POP_W}pt wide, hugged height.
const exact = (mod as any).popoutSize as { w?: number; h?: number } | undefined;
const wantW = Math.min(720, Math.max(240, Math.round(exact?.w ?? (mod as any).popoutWidth ?? ${hintW || POP_W})));
const capH = ${capH};
// The hidden title bar still paints the traffic lights over the top ~26pt —
// the strip is real chrome: pad the module below it, add it to the frame.
const STRIP = 26;
if (isAvailable) {
  Window.setTitle("widget · " + ${JSON.stringify(popName)});
  Window.setTitleBar("hidden", 32); // natural height: the lights land at AppKit's own (9,9) spot
  // A regular window level: aerospace keeps it out of the tile grid (a
  // free-standing window), but it stacks normally — it can sit behind
  // other windows and comes forward when focused, not pinned on top.
  Window.setLevel("normal");
  Window.setFrame({ width: wantW, height: ${wantH} + STRIP });
  // A spawned host process has no activation context — without this the
  // window orders in behind the front app, never key (grayed lights).
  Window.orderFront();
}
// The window hugs the widget's real height — with manners. A resize is
// applied only once the measured height has been stable for ~150ms, so a
// transient "loading" or empty render can never collapse it; a change
// under 12pt is wobble, not layout. anchorTop keeps the top edge fixed —
// the window grows and shrinks at its bottom edge. popoutSize-h modules
// opt out of hugging entirely: their frame is exactly what they asked.
let lastH = ${hintH ? wantH : 0}, pendingH = 0, pendingT = 0, timer: any = null;
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
    {/* The version badge, top right in the title strip: which version
        of the app this window runs. */}
    {${JSON.stringify(badge)} ? (
      <View
        style={{
          position: "absolute", top: 6, right: 8, height: 17, paddingHorizontal: 7,
          borderRadius: 8.5, justifyContent: "center",
          backgroundColor: colorScheme() === "dark" ? "rgba(245,245,245,0.10)" : "rgba(28,28,28,0.07)",
        }}
      >
        <Text
          style={{
            fontSize: 11, fontWeight: "600",
            color: colorScheme() === "dark" ? "rgba(245,245,245,0.75)" : "rgba(28,28,28,0.65)",
          }}
        >
          {${JSON.stringify(badge)}}
        </Text>
      </View>
    ) : null}
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
          shownV ? `${appTitle} · v${shownV}` : appTitle,
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
      // A replaced window exits after its successor took the key.
      child.on("exit", () => openWindows.get(key) === child && openWindows.delete(key));
      child.shownV = shownV;
      child.hintW = hintW;
      child.hintH = hintH;
      openWindows.set(key, child);
      console.log(`  /popout ${name} → pid ${child.pid}`);
      // AeroSpace tiles every new window; float ours once its title lands.
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
          // Bring the window to the user's space: float it, move it to the
          // focused workspace, focus it.
          const ws = execFileSync(
            "aerospace", ["list-workspaces", "--focused"], { encoding: "utf8" },
          ).trim();
          execFileSync("aerospace", ["layout", "floating", "--window-id", wid]);
          execFileSync("aerospace", [
            "move-node-to-workspace", "--window-id", wid, "--focus-follows-window", ws,
          ]);
          execFileSync("aerospace", ["focus", "--window-id", wid]);
          clearInterval(floatPoll);
          console.log(`  /popout ${name} → window ${wid}`);
        } catch {}
      }, 600);
      res.end(JSON.stringify({ ok: true, pid: child.pid }));
    });
    return true;
  }
  return false;
}
