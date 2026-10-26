// routes/market.mjs — the plugin catalog (Grok Bot's snapshot), installs,
// plugin pages, connector accounts, private skills and MCP servers.
import { execFile } from "node:child_process";
import { existsSync, mkdirSync, rmSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { APP_ROOT, PLUGINS_DIR, git } from "../config.mjs";
import { db } from "../state.mjs";
import { mcpAppCall, mcpAppList, mcpUpsertServer } from "../tools.mjs";
import {
  beginAuth as mcpBeginAuth, ensureClient as mcpEnsure, fixedRedirectUrl, handleOAuthCallback,
  invalidate as mcpInvalidate, statusOf as mcpStatus,
} from "../mcp.mjs";
import {
  CATEGORIES, SHOW_OAUTH_APP_PLUGINS, hiddenInfo, iconFile, catalogEntries, catalogFetchedAt, catalogSkills, entryOf, fetchPlugin, fieldsOf, installedMeta,
  installedRow, legacyOf, materialize, pluginInventory, pluginRoot, removePluginFiles, saveSettings, serverConfigs,
  pluginUsesGoogle, serverIdFor, settingsOf, skillDescription, skillFile,
} from "../plugins.mjs";

const readBody = (req) =>
  new Promise((ok) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try { ok(JSON.parse(body || "{}")); } catch { ok({}); }
    });
  });
const sendJson = (res, obj, status = 200) => {
  res.statusCode = status;
  res.setHeader("content-type", "application/json");
  res.end(JSON.stringify(obj));
};
const fail = (res, e) => sendJson(res, { ok: false, error: String(e?.message || e).slice(0, 300) }, 400);
const catLabel = Object.fromEntries(CATEGORIES.map((c) => [c.id, c.label]));
const parse = (s, d) => { try { return JSON.parse(s) ?? d; } catch { return d; } };

// Legacy xAI-marketplace install (plugins Grok Bot doesn't list): clone the
// repo at its pinned sha into plugins/<name>.
async function installLegacy(p) {
  const dir = join(PLUGINS_DIR, p.name);
  let root;
  if (p.source?.type === "local" || (p.source?.path && !p.source?.url)) {
    root = join(APP_ROOT, "server/plugins", String(p.source.path).replace(/^\.\//, ""));
    if (!existsSync(root)) throw new Error(`vendored source missing for ${p.name}`);
  } else {
    if (!existsSync(join(dir, ".git"))) {
      rmSync(dir, { recursive: true, force: true });
      mkdirSync(dirname(dir), { recursive: true });
      await git(["clone", "--depth", "1", String(p.source?.url || "").replace(/\.git$/, ""), dir]);
      if (p.source?.sha) {
        try {
          await git(["-C", dir, "fetch", "--depth", "1", "origin", p.source.sha]);
          await git(["-C", dir, "checkout", "-q", p.source.sha]);
        } catch {}
      }
    }
    root = p.source?.path ? join(dir, p.source.path) : dir;
  }
  return root;
}

// Register (or refresh) one connector row from the plugin's config and
// setup values. Reuses the plugin's row, else any row of that name.
async function upsertConnector(key, server, { id, probe = false } = {}) {
  const m = materialize(key, server);
  if (!m) throw new Error(`${key} declares no server ${server}`);
  if (!m.url && !m.command) throw new Error(`${server} has no url or command`);
  const rowId = id || serverIdFor(key, server);
  const exists = db.prepare("SELECT id, name FROM mcp_servers WHERE id=?").get(rowId);
  const r = await mcpUpsertServer({
    id: exists && exists.name !== server ? undefined : rowId,
    name: server, url: m.url, command: m.command, args: m.args, env: m.env,
    headers: m.headers, oauth_client: m.oauth_client, transport: m.transport, probe,
  });
  return { ...r, created: !exists, missing: m.missing };
}

// Account status note: what's missing, or how a connected account signs in.
function accountNote(key, server, row, st) {
  const m = materialize(key, server);
  if (!m) return "";
  if (m.missing.length) return `Add ${m.missing.join(", ")} in Setup to connect`;
  const authed = !!parse(row.auth, {}).tokens;
  const hasHeader = Object.keys(parse(row.headers, {})).some((k) => k.toLowerCase() === "authorization");
  if (st.status === "connected" && !authed && hasHeader && m.own) return "Connected · read-only (app token)";
  if (st.status === "connected" && !authed && hasHeader) return "Connected · token";
  return "";
}

export default async function handle(req, res) {
  // ————— Plugin marketplace (Grok Bot's catalog) —————
  // GET /plugins/catalog[?rev=<rev>] → catalog rows + installed flags; a
  // matching rev answers {unchanged:true} so the open modal's poll is cheap.
  if (req.method === "GET" && req.url?.startsWith("/plugins/catalog")) {
    const installed = new Map(db.prepare("SELECT name, meta FROM installed_plugins").all().map((r) => [r.name, r.meta]));
    const rev = `${catalogFetchedAt()}|${[...installed.keys()].sort().join(",")}|${installed.size ? [...installed.values()].join("").length : 0}`;
    const q = new URL(req.url, "http://x").searchParams;
    if (q.get("rev") === rev) return sendJson(res, { rev, unchanged: true }), true;
    sendJson(res, {
      marketplace: "grok-bot",
      rev,
      categories: CATEGORIES,
      plugins: catalogEntries().filter((e) => SHOW_OAUTH_APP_PLUGINS || installed.has(e.key) || !hiddenInfo(e.key)).map((e) => ({
        name: e.key,
        displayName: e.displayName || e.name,
        description: e.description || "",
        category: e.cats?.length ? catLabel[e.cats[0]] : e.category || "other",
        cats: (e.cats || []).map((c) => catLabel[c]),
        homepage: e.websiteUrl || null,
        iconUrl: e.iconUrl || null,
        icon: iconFile(e.key) ? `/plugins/icon/${encodeURIComponent(e.key)}` : null,
        publisher: e.publisher?.displayName || e.publisher?.name || "",
        keywords: [
          ...(e.keywords || []), e.name, e.pluginName,
          ...(e.connectors || []).map((c) => c.name), ...(e.skills || []).map((s) => s.name),
        ].filter(Boolean),
        domains: [],
        private: !!e.marketplace,
        setup: fieldsOf(e.key).length > 0,
        unavailable: e.install?.unavailable || null,
        installed: installed.has(e.key),
        meta: installed.has(e.key) ? parse(installed.get(e.key), {}) : null,
      })),
    });
    return true;
  }

  // POST /plugins/install {name, values?} — fetch the plugin's directory
  // from its repository, store setup values, register its connectors as
  // mcp_servers rows (connectors still missing values wait for Setup).
  if (req.method === "POST" && req.url === "/plugins/install") {
    const b = await readBody(req);
    try {
      const e = entryOf(b.name);
      if (!e) throw new Error(`unknown plugin ${b.name}`);
      if (b.values) saveSettings(e.key, { ...settingsOf(e.key), ...b.values });
      const vals = settingsOf(e.key);
      const missing = (e.fields || []).filter((f) => f.isRequired && !vals[f.key]).map((f) => f.label);
      if (missing.length) throw new Error(`${e.displayName} needs ${missing.join(", ")} before it can be added`);
      const prev = installedMeta(e.key);
      let meta;
      if (e.legacy) {
        const p = legacyOf(e.key);
        await installLegacy(p);
        meta = { path: p.source?.path || "" };
      } else {
        const got = await fetchPlugin(e);
        meta = { root: got.root, skills: got.skills, repo: e.install?.repo || null, sha: e.install?.sha || null, catalogId: e.id };
      }
      // Record first so pluginRoot / materialize see the new files.
      db.prepare(
        "INSERT OR REPLACE INTO installed_plugins(name, meta, created_at) VALUES(?,?,COALESCE((SELECT created_at FROM installed_plugins WHERE name=?),?))",
      ).run(e.key, JSON.stringify({ ...meta, mcp: prev?.mcp || [], created: prev?.created || [] }), e.key, Date.now());
      const mcp = [], created = [...(prev?.created || [])], waiting = [];
      for (const server of Object.keys(serverConfigs(e.key))) {
        const m = materialize(e.key, server);
        if (!m || (!m.url && !m.command)) continue;
        if (m.missing.length) { waiting.push(server); continue; }
        const r = await upsertConnector(e.key, server);
        mcp.push(r.id);
        if (r.created) created.push(r.id);
      }
      const root = pluginRoot(e.key);
      const kinds = [];
      for (const k of ["skills", "commands", "agents"]) if (root && existsSync(join(root, k))) kinds.push(k);
      for (const f of ["hooks/hooks.json", ".lsp.json", "settings.json"]) if (root && existsSync(join(root, f))) kinds.push(f);
      const full = { ...meta, mcp: [...new Set([...(prev?.mcp || []), ...mcp])], created: [...new Set(created)], kinds };
      db.prepare("UPDATE installed_plugins SET meta=? WHERE name=?").run(JSON.stringify(full), e.key);
      sendJson(res, { ok: true, name: e.key, mcp: full.mcp, kinds, waiting });
    } catch (err) {
      fail(res, err);
    }
    return true;
  }

  // POST /plugins/uninstall {name} — drop the record, its files, and the
  // connector rows its install created (rows that predate it stay).
  if (req.method === "POST" && req.url === "/plugins/uninstall") {
    const b = await readBody(req);
    try {
      const n = String(b.name || "");
      const meta = installedMeta(n);
      for (const id of meta?.created || []) {
        mcpInvalidate(id);
        db.prepare("DELETE FROM mcp_servers WHERE id=?").run(id);
      }
      db.prepare("DELETE FROM installed_plugins WHERE name=?").run(n);
      db.prepare("DELETE FROM plugin_settings WHERE plugin=?").run(n);
      removePluginFiles(n);
      sendJson(res, { ok: true });
    } catch (err) {
      fail(res, err);
    }
    return true;
  }

  // POST /plugins/settings {plugin, values, clear?} — save setup values
  // (blank secret = keep the stored one, blank plain value = clear; clear
  // lists keys to drop) and re-apply them to the plugin's connector rows.
  if (req.method === "POST" && req.url === "/plugins/settings") {
    const b = await readBody(req);
    try {
      const e = entryOf(b.plugin);
      if (!e) throw new Error(`unknown plugin ${b.plugin}`);
      const cur = settingsOf(e.key);
      const next = { ...cur };
      for (const f of fieldsOf(e.key)) {
        if (!(f.key in (b.values || {}))) continue;
        const v = String(b.values[f.key] ?? "").trim();
        if (v) next[f.key] = v;
        else if (!f.secret) delete next[f.key];
      }
      for (const k of b.clear || []) delete next[k];
      saveSettings(e.key, next);
      // The shared Google client re-applies to every installed Google plugin.
      const keys = pluginUsesGoogle(e.key)
        ? [e.key, ...db.prepare("SELECT name FROM installed_plugins").all().map((r) => r.name).filter((k) => k !== e.key && pluginUsesGoogle(k))]
        : [e.key];
      for (const key of keys) await reapply(key);
      sendJson(res, { ok: true });
    } catch (err) {
      fail(res, err);
    }
    return true;
  }
  async function reapply(key) {
    const e = entryOf(key);
    {
      const meta = installedMeta(e.key);
      if (meta) {
        for (const server of Object.keys(serverConfigs(e.key))) {
          const rows = db.prepare("SELECT id FROM mcp_servers WHERE name=?").all(server).filter((r) => (meta.mcp || []).includes(r.id));
          if (rows.length) {
            for (const r of rows) await upsertConnector(e.key, server, { id: r.id });
          } else if (!materialize(e.key, server).missing.length) {
            const r = await upsertConnector(e.key, server);
            meta.mcp = [...new Set([...(meta.mcp || []), r.id])];
            if (r.created) meta.created = [...new Set([...(meta.created || []), r.id])];
          }
        }
        db.prepare("UPDATE installed_plugins SET meta=? WHERE name=?").run(JSON.stringify(meta), e.key);
      }
    }
  }

  // GET /plugins/detail?name=<key> — catalog entry + (installed) content
  // inventory + live status of its connectors + setup fields.
  if (req.method === "GET" && req.url?.startsWith("/plugins/detail")) {
    const key = new URL(req.url, "http://x").searchParams.get("name");
    const e = entryOf(key);
    if (!e) return sendJson(res, { error: "unknown plugin" }, 404), true;
    const meta = installedMeta(e.key);
    const root = meta ? pluginRoot(e.key) : null;
    const inv = root
      ? pluginInventory(e.key, root)
      : { skills: catalogSkills(e), commands: [], agents: [], hooks: false, lsp: false, servers: serverConfigs(e.key), manifest: {} };
    const servers = inv.servers || {};
    const names = [...new Set([...(e.connectors || []).map((c) => c.name), ...Object.keys(servers)])];
    const vals = settingsOf(e.key);
    // Each connector's accounts are the rows sharing its name (first =
    // "default"). An idle enabled row is probed in the background so its
    // status settles (after OAuth lands, done() drops the client to idle).
    const mcp = names.map((name) => {
      const cfg = servers[name];
      const rows = db.prepare("SELECT * FROM mcp_servers WHERE name=? ORDER BY created_at").all(name);
      const m = cfg ? materialize(e.key, name, cfg) : null;
      const accounts = rows.map((r, i) => {
        const st = mcpStatus(r.id);
        if (r.enabled && st.status === "idle" && !(m?.missing.length)) mcpEnsure(r).catch(() => {});
        return {
          id: r.id, label: r.account || (i ? r.id : "default"),
          // Missing setup values: it can't sign in yet, whatever it lists.
          status: !r.enabled ? "off" : m?.missing.length && st.status !== "connected" ? "needs-auth" : st.status, tools: st.tools, err: st.err || null, authed: !!parse(r.auth, {}).tokens,
          note: meta && cfg ? accountNote(e.key, name, r, st) : "",
        };
      });
      const row = rows[0];
      const st = row ? mcpStatus(row.id) : { status: "off", tools: 0 };
      return {
        name,
        transport: cfg ? (cfg.url ? "remote" : "command") : "unknown",
        id: row?.id || null,
        status: row ? (!row.enabled ? "off" : m?.missing.length && st.status !== "connected" ? "needs-auth" : st.status) : "not-added",
        tools: st.tools, err: st.err || null, accounts,
        unavailable: cfg ? null : "Its configuration isn't published in the plugin's repository",
        needs: m?.missing || [],
        callbackUrl: m?.own ? fixedRedirectUrl(row?.id || serverIdFor(e.key, name)) : null,
      };
    });
    const fields = fieldsOf(e.key).map((f) => ({ ...f, set: !!vals[f.key], value: f.secret ? "" : vals[f.key] || "" }));
    sendJson(res, {
      name: e.key, displayName: e.displayName || e.name, description: e.description || "",
      category: e.cats?.length ? catLabel[e.cats[0]] : e.category || "other",
      homepage: e.websiteUrl || null, source: e.repositoryUrl || e.install?.repo || null, iconUrl: e.iconUrl || null,
      icon: iconFile(e.key) ? `/plugins/icon/${encodeURIComponent(e.key)}` : null,
      needsOAuthApp: hiddenInfo(e.key),
      publisher: e.publisher?.displayName || e.publisher?.name || "",
      availability: e.marketplace ? `Private marketplace (${e.marketplace.displayName || e.marketplace.name})` : "Public",
      unavailable: e.install?.unavailable || null,
      installed: !!meta, meta,
      skills: inv.skills, commands: inv.commands, agents: inv.agents, hooks: inv.hooks, lsp: inv.lsp,
      manifest: { ...inv.manifest, author: inv.manifest?.author || e.publisher?.displayName || null },
      mcp, fields,
    });
    return true;
  }

  // POST /plugins/connect {plugin, server, label?, open?} — register one of
  // a plugin's connectors as an account and start its auth: no label = the
  // default account (reuses the existing row), a label adds another (a new
  // row under the same name, id <server>-<label>). Remote servers kick the
  // browser OAuth flow unless an app token in Setup already signs them in;
  // open:false returns the authorize URL instead of opening a tab.
  if (req.method === "POST" && req.url === "/plugins/connect") {
    const b = await readBody(req);
    try {
      const { plugin, server, label } = b;
      const e = entryOf(plugin);
      if (!e || !installedRow(e.key)) throw new Error(`plugin ${plugin} is not installed`);
      const m = materialize(e.key, server);
      if (!m || (!m.url && !m.command)) throw new Error(`${plugin} declares no server ${server}`);
      if (m.missing.length) throw new Error(`Add ${m.missing.join(", ")} in Setup first`);
      const lab = String(label || "").trim().slice(0, 40);
      if (/^(grok|default)$/i.test(lab)) throw new Error(`${lab} is a reserved account label`);
      const rows = db.prepare("SELECT * FROM mcp_servers WHERE name=? ORDER BY created_at").all(server);
      if (lab && rows.some((r) => (r.account || "").toLowerCase() === lab.toLowerCase()))
        throw new Error(`an account labeled ${lab} already exists`);
      let id = lab ? null : rows[0]?.id;
      if (lab) {
        const base = `${server}-${lab}`.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24) || "srv";
        id = base;
        for (let i = 2; db.prepare("SELECT id FROM mcp_servers WHERE id=?").get(id); i++) id = `${base}-${i}`;
      }
      const r = await upsertConnector(e.key, server, { id: id || undefined });
      if (lab) db.prepare("UPDATE mcp_servers SET account=? WHERE id=?").run(lab, r.id);
      db.prepare("UPDATE mcp_servers SET enabled=1 WHERE id=?").run(r.id);
      const meta = installedMeta(e.key);
      meta.mcp = [...new Set([...(meta.mcp || []), r.id])];
      if (r.created) meta.created = [...new Set([...(meta.created || []), r.id])];
      db.prepare("UPDATE installed_plugins SET meta=? WHERE name=?").run(JSON.stringify(meta), e.key);
      const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(r.id);
      const hasHeader = Object.keys(m.headers).some((k) => k.toLowerCase() === "authorization");
      // Probe first: stored tokens or setup-provided credential headers
      // (an API key, an app token) may already sign the account in. Only
      // a remote server that still refuses — and isn't on a static
      // Authorization header alone — goes to the browser sign-in.
      mcpInvalidate(row.id);
      const rec = await mcpEnsure(row);
      const live = rec.client && !rec.needsAuth;
      let auth = { ok: true, status: live ? "connected" : rec.needsAuth ? "needs-auth" : "error", err: rec.err || null, token: hasHeader };
      if (row.url && !live && (m.oauth_client || !hasHeader)) auth = await mcpBeginAuth(row, { open: b.open !== false });
      sendJson(res, { ...auth, ok: true, id: row.id });
    } catch (err) {
      fail(res, err);
    }
    return true;
  }

  // GET /plugins/icon/<key> — the plugin's logo file.
  if (req.method === "GET" && req.url?.startsWith("/plugins/icon/")) {
    const key = decodeURIComponent(req.url.slice("/plugins/icon/".length).split("?")[0]);
    const f = iconFile(key);
    if (!f) return (res.statusCode = 404), res.end(), true;
    const ext = f.split(".").pop().toLowerCase();
    res.setHeader("content-type", { svg: "image/svg+xml", png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", webp: "image/webp", gif: "image/gif", ico: "image/x-icon" }[ext] || "application/octet-stream");
    res.setHeader("cache-control", "max-age=86400");
    res.end(readFileSync(f));
    return true;
  }

  // GET /oauth/callback/<id> — the fixed redirect for pre-registered OAuth
  // clients (e.g. http://127.0.0.1:8787/oauth/callback/x).
  if (req.method === "GET" && req.url?.startsWith("/oauth/callback/")) {
    handleOAuthCallback(req, res);
    return true;
  }

  // POST /mcp/account {id, label?, disconnect?} — rename an account
  // (label) or sign it out (disconnect drops the stored OAuth tokens).
  if (req.method === "POST" && req.url === "/mcp/account") {
    const b = await readBody(req);
    try {
      const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(b.id || ""));
      if (!row) throw new Error("no such server");
      if (b.label !== undefined) {
        const lab = String(b.label || "").trim().slice(0, 40);
        if (/^grok$/i.test(lab)) throw new Error("Grok is a reserved account label");
        db.prepare("UPDATE mcp_servers SET account=? WHERE id=?").run(lab || null, row.id);
      }
      if (b.disconnect) {
        db.prepare("UPDATE mcp_servers SET auth=NULL WHERE id=?").run(row.id);
        mcpInvalidate(row.id);
      }
      sendJson(res, { ok: true });
    } catch (err) {
      fail(res, err);
    }
    return true;
  }

  // GET /plugins/skill?plugin=<key>&skill=<skill> — the SKILL.md body.
  if (req.method === "GET" && req.url?.startsWith("/plugins/skill")) {
    const q = new URL(req.url, "http://x").searchParams;
    const e = entryOf(q.get("plugin"));
    const skill = String(q.get("skill") || "");
    const file = e && installedRow(e.key) ? skillFile(e.key, skill) : null;
    if (!file || !existsSync(file)) {
      // Not installed: the catalog's description (and its source link).
      const c = e?.skills?.find((s) => s.name === skill);
      if (!c) return sendJson(res, { error: "no such skill" }, 404), true;
      return sendJson(res, { name: skill, description: c.description || "", body: "", source: c.sourceUrl || null }), true;
    }
    const body = readFileSync(file, "utf8");
    sendJson(res, { name: skill, body, description: skillDescription(body) });
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
  // POST /mcp/call {server, tool, args} — a shipped mini app calling its
  // server: {ok, result} (the tool's data) or {ok:false, error}.
  if (req.method === "POST" && req.url === "/mcp/call") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        res.end(JSON.stringify(await mcpAppCall(JSON.parse(body || "{}"))));
      } catch (e) {
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // POST /mcp/open {url} — a mini app opening another: nsh has no open-URL
  // API, so the server hands a `rox://app/…` link to the system, which
  // routes it back to Rox (lib.rs on_open_url). Nothing else is opened.
  if (req.method === "POST" && req.url === "/mcp/open") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      let url = "";
      try { url = String(JSON.parse(body || "{}").url || ""); } catch {}
      if (!/^rox:\/\/app\/[a-z0-9-]+\/[A-Za-z0-9._-]+(\?[^\s]*)?$/.test(url)) {
        res.statusCode = 400;
        return res.end(JSON.stringify({ ok: false, error: "only rox://app/<server>/<app> links" }));
      }
      execFile("open", [url], (e) => res.end(JSON.stringify(e ? { ok: false, error: String(e.message) } : { ok: true })));
    });
    return true;
  }
  // GET /mcp/apps — the mini apps every enabled server ships.
  if (req.method === "GET" && req.url === "/mcp/apps") {
    mcpAppList().then((apps) => {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ apps }));
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
