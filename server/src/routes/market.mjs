// routes/market.mjs — extracted from app-server.mjs
import { existsSync, mkdirSync, rmSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { APP_ROOT, PLUGIN_CATALOG, PLUGINS_DIR, git } from "../config.mjs";
import { db } from "../state.mjs";
import { mcpUpsertServer } from "../tools.mjs";
import { beginAuth as mcpBeginAuth, ensureClient as mcpEnsure, invalidate as mcpInvalidate, statusOf as mcpStatus } from "../mcp.mjs";

export default async function handle(req, res) {
  // ————— Plugin marketplace (agent-plugins.org catalog) —————
  // GET /plugins/catalog → marketplace entries + installed flags.
  if (req.method === "GET" && req.url === "/plugins/catalog") {
    res.setHeader("content-type", "application/json");
    const installed = new Map(
      db.prepare("SELECT name, meta FROM installed_plugins").all().map((r) => [r.name, r.meta]),
    );
    res.end(
      JSON.stringify({
        marketplace: PLUGIN_CATALOG.name || "marketplace",
        plugins: (PLUGIN_CATALOG.plugins || []).map((p) => ({
          name: p.name,
          description: p.description || "",
          category: p.category || "other",
          homepage: p.homepage || null,
          domains: p.domains || [],
          keywords: p.keywords || [],
          installed: installed.has(p.name),
          meta: installed.has(p.name) ? JSON.parse(installed.get(p.name) || "{}") : null,
        })),
      }),
    );
    return true;
  }
  // POST /plugins/install {name} — clone the plugin repo into
  // plugins/<name>, register its .mcp.json servers, inventory the pieces
  // (skills/commands/agents/hooks/lsp) so later steps can wire them up.
  if (req.method === "POST" && req.url === "/plugins/install") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const { name } = JSON.parse(body || "{}");
        const p = (PLUGIN_CATALOG.plugins || []).find((x) => x.name === name);
        if (!p) throw new Error(`unknown plugin ${name}`);
        const dir = join(PLUGINS_DIR, p.name);
        let root;
        if (p.source?.type === "local" || (p.source?.path && !p.source?.url)) {
          // Vendored inside the marketplace catalog itself (external_plugins/<name>).
          root = join(APP_ROOT, "server/plugins", String(p.source.path).replace(/^\.\//, ""));
          if (!existsSync(root)) throw new Error(`vendored source missing for ${name}`);
        } else {
          if (!existsSync(join(dir, ".git"))) {
            rmSync(dir, { recursive: true, force: true });
            mkdirSync(dirname(dir), { recursive: true });
            const url = String(p.source?.url || "").replace(/\.git$/, "");
            await git(["clone", "--depth", "1", url, dir]);
            if (p.source?.sha) {
              try {
                await git(["-C", dir, "fetch", "--depth", "1", "origin", p.source.sha]);
                await git(["-C", dir, "checkout", "-q", p.source.sha]);
              } catch {}
            }
          }
          root = p.source?.path ? join(dir, p.source.path) : dir;
        }
        // Plugin repos declare MCP servers under different manifest names:
        // .mcp.json (claude/mcp convention), gemini-extension.json, or mcp.json.
        const added = [];
        for (const manifest of [".mcp.json", "gemini-extension.json", "mcp.json"]) {
          const mcpFile = join(root, manifest);
          if (!existsSync(mcpFile)) continue;
          let mcp;
          try { mcp = JSON.parse(readFileSync(mcpFile, "utf8")); } catch { continue; }
          const servers = mcp.mcpServers || mcp.mcp || {};
          for (const [srvName, cfg] of Object.entries(servers)) {
            if (!cfg || typeof cfg !== "object" || (!cfg.command && !cfg.url)) continue;
            const r = await mcpUpsertServer({
              name: srvName,
              url: cfg.url || (cfg.type === "http" ? cfg.url : undefined),
              command: cfg.command,
              args: cfg.args,
              env: cfg.env,
            });
            added.push(r.id);
          }
          if (Object.keys(servers).length) break;
        }
        // Inventory the pieces the plugin ships.
        const kinds = [];
        for (const k of ["skills", "commands", "agents"])
          if (existsSync(join(root, k))) kinds.push(k);
        for (const f of ["hooks/hooks.json", ".lsp.json", "settings.json"])
          if (existsSync(join(root, f))) kinds.push(f);
        db.prepare(
          "INSERT OR REPLACE INTO installed_plugins(name, meta, created_at) VALUES(?,?,COALESCE((SELECT created_at FROM installed_plugins WHERE name=?),?))",
        ).run(
          p.name,
          JSON.stringify({ mcp: added, kinds, path: p.source?.path || "" }),
          p.name,
          Date.now(),
        );
        res.end(JSON.stringify({ ok: true, name: p.name, mcp: added, kinds }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e).slice(0, 300) }));
      }
    });
    return true;
  }
  // POST /plugins/uninstall {name} — remove the record + cloned files; its
  // .mcp.json servers were registered under their own ids and stay (they are
  // ordinary entries in mcp_servers — the Connect tab manages them).
  if (req.method === "POST" && req.url === "/plugins/uninstall") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { name } = JSON.parse(body || "{}");
        const n = String(name || "");
        db.prepare("DELETE FROM installed_plugins WHERE name=?").run(n);
        if (/^[\w.-]+$/.test(n)) rmSync(join(PLUGINS_DIR, n), { recursive: true, force: true });
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // Where a plugin's content lives: its clone for url sources, the vendored
  // catalog dir for local sources. Null when a url plugin isn't installed.
  function pluginRoot(p) {
    if (p.source?.type === "local" || (p.source?.path && !p.source?.url)) {
      const root = join(APP_ROOT, "server/plugins", String(p.source.path).replace(/^\.\//, ""));
      return existsSync(root) ? root : null;
    }
    const dir = join(PLUGINS_DIR, p.name);
    if (!existsSync(dir)) return null;
    return p.source?.path ? join(dir, p.source.path) : dir;
  }
  // What a plugin ships: skills/commands/agents (name + description from
  // SKILL.md-style frontmatter), hook/lsp/settings presence, declared MCP
  // server names. Powers the detail page.
  function pluginInventory(root) {
    const listDir = (sub, { md = false } = {}) => {
      const base = join(root, sub);
      if (!existsSync(base)) return [];
      try {
        return readdirSync(base).map((n) => {
          const entry = { name: n.replace(/\.md$/, ""), description: "" };
          const file = md ? join(base, n) : join(base, n, "SKILL.md");
          try {
            const head = readFileSync(file, "utf8").slice(0, 3000);
            entry.description =
              head.match(/^description:\s*>?-?\s*\n?((?:\s+.+\n?)+)/m)?.[1]?.replace(/\s+/g, " ").trim() ||
              head.match(/^description:\s*(.+)$/m)?.[1]?.trim() ||
              "";
          } catch {}
          return entry;
        });
      } catch { return []; }
    };
    const manifest = [".mcp.json", "gemini-extension.json", "mcp.json"]
      .map((m) => { try { return JSON.parse(readFileSync(join(root, m), "utf8")); } catch { return null; } })
      .find((m) => m && Object.keys(m.mcpServers || m.mcp || {}).length);
    let meta = {};
    try { meta = JSON.parse(readFileSync(join(root, ".grok-plugin/plugin.json"), "utf8")); } catch {}
    try { if (!meta.name) meta = JSON.parse(readFileSync(join(root, "plugin.json"), "utf8")); } catch {}
    return {
      skills: listDir("skills"),
      commands: listDir("commands", { md: true }),
      agents: listDir("agents", { md: true }),
      hooks: existsSync(join(root, "hooks/hooks.json")),
      lsp: existsSync(join(root, ".lsp.json")),
      mcp: Object.entries(manifest?.mcpServers || manifest?.mcp || {}).map(([n, c]) => ({
        name: n, transport: c?.url || c?.type === "http" ? "remote" : "command",
      })),
      manifest: { author: meta.author?.name || null, version: meta.version || null, license: meta.license || null },
    };
  }
  // GET /plugins/detail?name=<name> — catalog entry + content inventory +
  // live status of its registered MCP servers.
  if (req.method === "GET" && req.url?.startsWith("/plugins/detail")) {
    const name = new URL(req.url, "http://x").searchParams.get("name");
    const p = (PLUGIN_CATALOG.plugins || []).find((x) => x.name === name);
    res.setHeader("content-type", "application/json");
    if (!p) { res.statusCode = 404; res.end('{"error":"unknown plugin"}'); return; }
    const inst = db.prepare("SELECT meta FROM installed_plugins WHERE name=?").get(name);
    const root = pluginRoot(p);
    const inv = root ? pluginInventory(root) : { skills: [], commands: [], agents: [], hooks: false, lsp: false, mcp: [], manifest: {} };
    const mcp = inv.mcp.map((s) => {
      const row = db.prepare("SELECT * FROM mcp_servers WHERE name=?").get(s.name);
      const st = row ? mcpStatus(row.id) : { status: "off", tools: 0 };
      return { ...s, id: row?.id || null, status: row ? (row.enabled ? st.status : "off") : "not-added", tools: st.tools, err: st.err || null };
    });
    res.end(JSON.stringify({
      name: p.name, description: p.description, category: p.category || "other",
      homepage: p.homepage || null, source: p.source?.type === "local" ? "marketplace" : (p.source?.url || null),
      installed: !!inst, meta: inst ? JSON.parse(inst.meta || "{}") : null,
      ...inv, mcp,
    }));
    return true;
  }
  // GET /plugins/skill?plugin=<name>&skill=<skill> — the SKILL.md body.
  if (req.method === "GET" && req.url?.startsWith("/plugins/skill")) {
    const q = new URL(req.url, "http://x").searchParams;
    const p = (PLUGIN_CATALOG.plugins || []).find((x) => x.name === q.get("plugin"));
    res.setHeader("content-type", "application/json");
    const root = p && pluginRoot(p);
    const file = root && join(root, "skills", String(q.get("skill") || ""), "SKILL.md");
    if (!file || !existsSync(file)) { res.statusCode = 404; res.end('{"error":"no such skill"}'); return; }
    const body = readFileSync(file, "utf8");
    res.end(JSON.stringify({
      name: q.get("skill"), body,
      description: body.match(/^description:\s*>?-?\s*\n?((?:\s+.+\n?)+)/m)?.[1]?.replace(/\s+/g, " ").trim() || body.match(/^description:\s*(.+)$/m)?.[1]?.trim() || "",
    }));
    return true;
  }
  // ————— Private skills —————
  // GET /skills → the user's own skills (what the agent writes with
  // skill_* tools; not plugin-shipped skills).
  if (req.method === "GET" && req.url === "/skills") {
    const rows = db.prepare("SELECT * FROM skills ORDER BY created_at").all();
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({
      skills: rows.map((r) => ({ id: r.id, name: r.name, description: r.description, body: r.body, enabled: !!r.enabled })),
    }));
    return true;
  }
  // POST /skills {id?, name, description, instructions} — create or edit.
  if (req.method === "POST" && req.url === "/skills") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const b = JSON.parse(body || "{}");
        const id = b.id ? String(b.id) : String(b.name || "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48) || "skill";
        const cur = db.prepare("SELECT * FROM skills WHERE id=?").get(id);
        if (!cur && !String(b.instructions || b.body || "").trim()) throw new Error("skill needs instructions");
        db.prepare(
          "INSERT OR REPLACE INTO skills(id,name,description,body,enabled,created_at) VALUES(?,?,?,?,COALESCE((SELECT enabled FROM skills WHERE id=?),1),COALESCE((SELECT created_at FROM skills WHERE id=?),?))",
        ).run(
          id,
          String(b.name ?? cur?.name ?? id).slice(0, 80),
          String(b.description ?? cur?.description ?? "").slice(0, 400),
          String(b.instructions ?? b.body ?? cur?.body ?? ""),
          id, id, Date.now(),
        );
        res.end(JSON.stringify({ ok: true, id }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e).slice(0, 300) }));
      }
    });
    return true;
  }
  // POST /skills/state {id, enabled}
  if (req.method === "POST" && req.url === "/skills/state") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { id, enabled } = JSON.parse(body || "{}");
        db.prepare("UPDATE skills SET enabled=? WHERE id=?").run(enabled ? 1 : 0, String(id || ""));
        res.end('{"ok":true}');
      } catch (e) { res.statusCode = 400; res.end('{"ok":false}'); }
    });
    return true;
  }
  // POST /skills/delete {id}
  if (req.method === "POST" && req.url === "/skills/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      const { id } = JSON.parse(body || "{}");
      db.prepare("DELETE FROM skills WHERE id=?").run(String(id || ""));
      res.end('{"ok":true}');
    });
    return true;
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
            url: r.url || null,
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
    return true;
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
    return true;
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
    return true;
  }
  // POST /mcp/auth {id} — kick the OAuth browser flow for a remote server.
  if (req.method === "POST" && req.url === "/mcp/auth") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      (async () => {
        res.setHeader("content-type", "application/json");
        try {
          const { id } = JSON.parse(body || "{}");
          const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(id || ""));
          if (!row) throw new Error("no such server");
          res.end(JSON.stringify(await mcpBeginAuth(row)));
        } catch (e) {
          res.statusCode = 400;
          res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
        }
      })();
    });
    return true;
  }
  // GET /mcp/tools?id=<id> — discovered tools with per-tool enabled flags.
  if (req.method === "GET" && req.url?.startsWith("/mcp/tools")) {
    const id = new URL(req.url, "http://x").searchParams.get("id");
    const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(id || ""));
    res.setHeader("content-type", "application/json");
    if (!row) { res.statusCode = 404; res.end('{"error":"no such server"}'); return; }
    const off = new Set(JSON.parse(row.disabled_tools || "[]"));
    mcpEnsure(row).then((rec) => {
      res.end(JSON.stringify({
        tools: rec.tools.map((t) => ({ name: t.name, description: t.description || "", enabled: !off.has(t.name) })),
        status: rec.client ? "connected" : "error",
      }));
    });
    return true;
  }
  // POST /mcp/tools/state {id, tool, enabled} — per-tool switch.
  if (req.method === "POST" && req.url === "/mcp/tools/state") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { id, tool, enabled } = JSON.parse(body || "{}");
        const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(id || ""));
        if (!row) throw new Error("no such server");
        const off = new Set(JSON.parse(row.disabled_tools || "[]"));
        if (enabled) off.delete(String(tool)); else off.add(String(tool));
        db.prepare("UPDATE mcp_servers SET disabled_tools=? WHERE id=?").run(JSON.stringify([...off]), row.id);
        res.end('{"ok":true}');
      } catch (e) { res.statusCode = 400; res.end(JSON.stringify({ ok: false, error: String(e?.message || e) })); }
    });
    return true;
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
    return true;
  }
  return false;
}
