// plugins.mjs — the plugin catalog (Grok Bot's snapshot + the legacy xAI
// marketplace), per-plugin setup values, catalog installs from git, and the
// connector configs materialized from them.
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { APP_ROOT, GROKBOT_CATALOG, OUT_DIR, PLUGIN_CATALOG, PLUGINS_DIR, git, serverFile } from "./config.mjs";
import { db } from "./state.mjs";
import { fixedRedirectUrl } from "./mcp.mjs";
import { adjustServers as adjust, repoOf, resolveEntry, sourceUrlInfo } from "./plugin_resolve.mjs";

// ————— categories (Grok Bot's marketplace browse model) —————
// Grok Bot files catalog entries under its own eleven categories, from the
// entry's categoryKeys plus a few per-plugin overrides; private-marketplace
// entries stay out of the category sections.
const CAT_ORDER = ["credentials", "productivity", "communication", "design", "code", "data", "sales", "marketing", "finance", "research", "support"];
const CAT_LABEL = {
  credentials: "Password Managers", productivity: "Productivity", communication: "Communication", design: "Design",
  code: "Code", data: "Data", sales: "Sales", marketing: "Marketing", finance: "Finance", research: "Research", support: "Support",
};
const CAT_OF_KEY = {
  LOGIN_AND_CREDENTIAL_MANAGEMENT: "credentials", PRODUCTIVITY: "productivity", INBOX_AND_COLLABORATION: "communication",
  SCHEDULING: "communication", SALES: "sales", MARKETING: "marketing", CUSTOMER_SUPPORT: "support", PAYMENTS: "finance",
  FINANCE_AND_LEGAL: "finance", DATA_ANALYTICS: "data", DESIGN: "design", CANVAS: "design", DOCUMENTS_AND_FILES: "productivity",
  INFRASTRUCTURE: "code", RESEARCH: "research",
};
const CAT_OVERRIDE = {
  slack: ["communication"], notion: ["productivity"], "notion-workspace": ["productivity"], linear: ["productivity"],
  figma: ["design"], tldraw: ["design"], github: ["code"], "github-plugin": ["code"], runlayer: ["code"], langfuse: ["data"],
  parallel: ["research", "data"], superpowers: ["code"], "compound-engineering": ["code"], "create-plugin": ["code"],
  "context7-plugin": ["research"], context7: ["research"],
};
const keyNorm = (k) => String(k).trim().toUpperCase().replace(/&/g, " AND ").replace(/[^A-Z0-9]+/g, "_").replace(/^_+|_+$/g, "");
function catsOf(e) {
  if (e.marketplace) return [];
  for (const n of [e.pluginName, e.name]) {
    const o = n && CAT_OVERRIDE[n.trim().toLowerCase()];
    if (o) return o;
  }
  const keys = e.categoryKeys ?? (e.categoryKey ? [e.categoryKey] : [e.category]);
  return [...new Set(keys.map((k) => CAT_OF_KEY[keyNorm(k || "")]).filter(Boolean))];
}
export const CATEGORIES = CAT_ORDER.map((id) => ({ id, label: CAT_LABEL[id] }));

// ————— catalog —————
const legacyByName = new Map((PLUGIN_CATALOG.plugins || []).map((p) => [p.name, p]));
const CATALOG = (() => {
  const out = (GROKBOT_CATALOG.plugins || []).map((e) => ({ ...e, cats: catsOf(e) }));
  const keys = new Set(out.flatMap((e) => [e.key, e.name, e.pluginName]));
  // xAI marketplace plugins Grok Bot doesn't list stay installable.
  for (const p of PLUGIN_CATALOG.plugins || []) {
    if (keys.has(p.name)) continue;
    out.push({
      key: p.name, name: p.name, pluginName: p.name, displayName: titleCase(p.name), description: p.description || "",
      category: p.category || "other", cats: [], websiteUrl: p.homepage || null, repositoryUrl: p.source?.url?.replace(/\.git$/, "") || null,
      connectors: [], skills: [], publisher: { name: "xai", displayName: "xAI" }, legacy: true, keywords: p.keywords || [],
    });
  }
  return out;
})();
const byKey = new Map(CATALOG.map((e) => [e.key, e]));
function titleCase(n) {
  return String(n).replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}
export const catalogEntries = () => CATALOG;
export const catalogFetchedAt = () => GROKBOT_CATALOG.fetchedAt || "";
export const entryOf = (key) => byKey.get(String(key || "")) || null;
export const legacyOf = (key) => legacyByName.get(String(key || "")) || null;

// Plugins whose connectors need a pre-registered OAuth app (or sit behind
// a vendor's account gateway) — tools/probe-oauth-apps.mjs. Hidden from
// the catalog unless ROX_SHOW_OAUTH_APP_PLUGINS=1; installed ones stay.
const HIDDEN = (() => {
  const m = new Map();
  try {
    for (const r of JSON.parse(readFileSync(serverFile("plugins/needs-oauth-app.json"), "utf8")).plugins || []) {
      if (!m.has(r.name)) m.set(r.name, []);
      m.get(r.name).push(r);
    }
  } catch {}
  return m;
})();
export const SHOW_OAUTH_APP_PLUGINS = /^(1|true|yes)$/i.test(process.env.ROX_SHOW_OAUTH_APP_PLUGINS || "");
export const hiddenInfo = (key) => HIDDEN.get(key) || null;

// Logos: server/plugins/icons/<catalog name>.<ext> (from the catalog's
// iconUrl). → absolute file path or null.
const ICON_DIR = serverFile("plugins/icons");
const ICON_FILES = (() => {
  const m = new Map();
  try { for (const f of readdirSync(ICON_DIR)) m.set(f.replace(/\.[^.]+$/, ""), f); } catch {}
  return m;
})();
export function iconFile(key) {
  const e = entryOf(key);
  const f = ICON_FILES.get(e?.name) || ICON_FILES.get(key);
  return f ? join(ICON_DIR, f) : null;
}

export const installedRow = (key) => db.prepare("SELECT name, meta FROM installed_plugins WHERE name=?").get(key);
const parse = (s, d) => { try { return JSON.parse(s) ?? d; } catch { return d; } };
export const installedMeta = (key) => {
  const r = installedRow(key);
  return r ? parse(r.meta, {}) : null;
};

// Where an installed plugin's files live: catalog installs record `root`
// (relative to PLUGINS_DIR); legacy installs are the clone + source path,
// vendored ones the catalog's external_plugins dir.
export function pluginRoot(key) {
  const meta = installedMeta(key);
  if (meta?.root != null) {
    const r = join(PLUGINS_DIR, meta.root);
    return existsSync(r) ? r : null;
  }
  const p = legacyOf(key);
  if (!p) return null;
  if (p.source?.type === "local" || (p.source?.path && !p.source?.url)) {
    const root = join(APP_ROOT, "server/plugins", String(p.source.path).replace(/^\.\//, ""));
    return existsSync(root) ? root : null;
  }
  const dir = join(PLUGINS_DIR, p.name);
  if (!existsSync(dir)) return null;
  return p.source?.path ? join(dir, p.source.path) : dir;
}

// ————— setup values —————
const rawSettings = (key) => parse(db.prepare("SELECT vals FROM plugin_settings WHERE plugin=?").get(key)?.vals, {});
export const settingsOf = (key) => (key !== GOOGLE_GROUP && pluginUsesGoogle(key) ? { ...rawSettings(key), ...rawSettings(GOOGLE_GROUP) } : rawSettings(key));
export function saveSettings(key, vals) {
  const clean = {};
  for (const [k, v] of Object.entries(vals || {}))
    if (/^[A-Za-z_][\w.]*$/.test(k) && v != null && String(v).trim() !== "") clean[k] = String(v).trim();
  // The shared Google client lives in its own row.
  if (key !== GOOGLE_GROUP && pluginUsesGoogle(key)) {
    const g = {};
    for (const k of Object.keys(clean)) if (isGoogleKey(k)) { g[k] = clean[k]; delete clean[k]; }
    saveSettings(GOOGLE_GROUP, g);
  }
  if (!Object.keys(clean).length) db.prepare("DELETE FROM plugin_settings WHERE plugin=?").run(key);
  else db.prepare("INSERT OR REPLACE INTO plugin_settings(plugin, vals, updated_at) VALUES(?,?,?)").run(key, JSON.stringify(clean), Date.now());
  return clean;
}

// Builtins a plugin config may reference besides its own fields
// (ROX_FILES_DIR: the generated-files folder the office tools work in).
const BUILTIN = new Set(["CLAUDE_PLUGIN_ROOT", "CURSOR_PLUGIN_ROOT", "workspaceFolder", "userHome", "HOME", "USERPROFILE", "ROX_FILES_DIR"]);
const VAR_RE = /\$\{([^}]+)\}/g;
const isSecretKey = (k) => /KEY|TOKEN|SECRET|PASSWORD|PAT\b/i.test(k);

// The MCP server configs a catalog entry declares (install hints from the
// snapshot, else the installed copy's manifests).
export function serverConfigs(key) {
  const e = entryOf(key);
  if (e?.install?.servers && Object.keys(e.install.servers).length) return adjust(e.install.servers);
  const root = pluginRoot(key);
  if (!root) return {};
  for (const m of [".mcp.json", "gemini-extension.json", "mcp.json"]) {
    const j = parse(existsSync(join(root, m)) ? readFileSync(join(root, m), "utf8") : null, null);
    const s = j && (j.mcpServers || j.mcp);
    if (s && typeof s === "object" && Object.keys(s).length) return adjust(s);
  }
  return {};
}

// ————— Google: one shared OAuth client for every Google connector —————
// accounts.google.com has no dynamic client registration; the user's own
// "Desktop app" OAuth client (id + secret) is stored once under "@google"
// and used by Gmail, Calendar, Drive, Docs, Sheets and Slides. Desktop
// clients accept any loopback redirect, so nothing has to be registered.
export const GOOGLE_GROUP = "@google";
const isGoogle = (cfg) => typeof cfg?.url === "string" && /^https:\/\/[a-z]+mcp\.googleapis\.com\//.test(cfg.url);
const GOOGLE_SCOPES = {
  gmailmcp: ["gmail.modify", "gmail.compose", "gmail.labels"],
  calendarmcp: ["calendar.events", "calendar.readonly"],
  drivemcp: ["drive.readonly", "drive.file"],
  docsmcp: ["documents", "drive.readonly"],
  sheetsmcp: ["spreadsheets", "drive.readonly"],
  slidesmcp: ["presentations", "drive.readonly"],
};
const googleScope = (url) => {
  const host = new URL(url).hostname.split(".")[0];
  return (GOOGLE_SCOPES[host] || []).map((s) => `https://www.googleapis.com/auth/${s}`).join(" ");
};
export const isGoogleKey = (k) => k === "GOOGLE_CLIENT_ID" || k === "GOOGLE_CLIENT_SECRET";
// A local server (google-workspace's workspace-mcp) that takes the shared
// client through ${GOOGLE_CLIENT_ID} / ${GOOGLE_CLIENT_SECRET} in its env.
const usesGoogleVars = (cfg) => /\$\{GOOGLE_CLIENT_(ID|SECRET)\}/.test(JSON.stringify(cfg || {}));
const sharesGoogle = (cfg) => isGoogle(cfg) || usesGoogleVars(cfg);
export const pluginUsesGoogle = (key) => Object.values(serverConfigs(key)).some(sharesGoogle);

// A connector whose config pins a vendor's own OAuth client id (Cursor's,
// registered to its cursor:// redirect) can't use that id here: the user
// brings their own app's client id (and optionally an app token).
const literalClient = (cfg) => !!(cfg?.url && cfg.auth?.CLIENT_ID && !/\$\{/.test(String(cfg.auth.CLIENT_ID)));

// DCR availability per server URL (RFC 9728 → RFC 8414 metadata). null
// while unknown; probes are cached for the process lifetime.
const dcrCache = new Map();
export function dcrSupport(url) {
  if (!url) return null;
  if (!dcrCache.has(url)) {
    dcrCache.set(url, { value: null });
    (async () => {
      const { discoverOAuthProtectedResourceMetadata, discoverAuthorizationServerMetadata } = await import("@modelcontextprotocol/sdk/client/auth.js");
      let issuer = new URL("/", url).href;
      try {
        const prm = await discoverOAuthProtectedResourceMetadata(url);
        if (prm?.authorization_servers?.[0]) issuer = prm.authorization_servers[0];
      } catch {}
      const md = await discoverAuthorizationServerMetadata(issuer);
      dcrCache.set(url, { value: md ? !!md.registration_endpoint : null });
    })().catch(() => dcrCache.set(url, { value: null }));
  }
  return dcrCache.get(url).value;
}

// The setup fields a plugin shows: the catalog's own `fields`, then every
// ${KEY} its MCP config references without a field (Grok Bot does the
// same from the plugin's variable names), then — for connectors that need
// a pre-registered OAuth client — Client ID / Client Secret / Bearer Token.
export function fieldsOf(key) {
  const e = entryOf(key);
  const out = [];
  const seen = new Set();
  for (const f of e?.fields || []) {
    seen.add(f.key);
    out.push({
      key: f.key, label: f.label || f.key, type: f.type || "string", placeholder: f.placeholder || f.key,
      required: !!f.isRequired, secret: !!f.isSecret, hint: f.hint || "",
      options: (f.options || []).map((o) => ({ label: o.label, value: String(parse(o.rawValue, o.rawValue)) })),
    });
  }
  const servers = serverConfigs(key);
  for (const cfg of Object.values(servers)) {
    if (literalClient(cfg)) continue; // its auth block is handled below
    for (const m of JSON.stringify(cfg).matchAll(VAR_RE)) {
      const [name, dflt] = m[1].split(":-");
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || BUILTIN.has(name) || seen.has(name) || isGoogleKey(name)) continue;
      seen.add(name);
      out.push({
        key: name, label: titleCase(name.toLowerCase()).replace(/\b(Api|Mcp|Url|Id|Pat|Dd)\b/g, (w) => w.toUpperCase()),
        type: "string", placeholder: name, required: false, secret: isSecretKey(name),
        hint: `Referenced as \`\${${name}}\` in this plugin's MCP configuration.${dflt !== undefined && dflt !== "" ? ` Defaults to \`${dflt}\` when left blank.` : ""}`,
        options: [],
      });
    }
  }
  if (Object.values(servers).some(sharesGoogle)) {
    for (const f of [
      { key: "GOOGLE_CLIENT_ID", label: "Google Client ID", placeholder: "xxxxxxxx.apps.googleusercontent.com", secret: false,
        hint: "Shared by every Google plugin (Gmail, Google Calendar, Drive, Docs, Sheets, Slides) — saving it here updates all of them. Create it in Google Cloud Console → APIs & Services → Credentials → OAuth client ID, type Desktop app." },
      { key: "GOOGLE_CLIENT_SECRET", label: "Google Client Secret", placeholder: "GOCSPX-…", secret: true,
        hint: "The Desktop app client's secret (Google issues one; it's sent on the token request). Shared by every Google plugin." },
    ])
      if (!seen.has(f.key)) { seen.add(f.key); out.push({ ...f, type: "string", required: true, options: [], oauth: true, shared: "google" }); }
  }
  const clientNeeds = Object.entries(servers).filter(([n, cfg]) => !isGoogle(cfg) && needsOwnClient(key, n, cfg));
  if (clientNeeds.length) {
    const cb = clientNeeds.map(([n]) => fixedRedirectUrl(serverIdFor(key, n)));
    const add = (f) => { if (!seen.has(f.key)) { seen.add(f.key); out.push(f); } };
    add({ key: "CLIENT_ID", label: "Client ID", type: "string", placeholder: "Your app's OAuth 2.0 Client ID", required: true, secret: false, options: [], oauth: true,
      hint: `From your own developer app (OAuth 2.0, PKCE). Register ${cb.join(" and ")} as its callback URL.` });
    add({ key: "CLIENT_SECRET", label: "Client Secret", type: "string", placeholder: "Only for confidential (Web App) clients", required: false, secret: true, options: [], oauth: true,
      hint: "Leave blank for a public (Native App) client — sign-in then uses PKCE only." });
    if (clientNeeds.some(([, cfg]) => literalClient(cfg)))
      add({ key: "BEARER_TOKEN", label: "Bearer Token", type: "string", placeholder: "App-only bearer token", required: false, secret: true, options: [], oauth: true,
        hint: "Optional. Connects without signing in, sending Authorization: Bearer <token> (read-only app access). Signed-in OAuth tokens take precedence." });
  }
  return out;
}

// Does this connector need the user's own OAuth client? Vendor-pinned
// client ids do unless the server supports dynamic registration; any
// remote connector whose sign-in failed for lack of registration does too.
export function needsOwnClient(key, server, cfg) {
  if (!cfg?.url || cfg.unavailable) return false;
  if (isGoogle(cfg)) return true;
  if (literalClient(cfg)) return dcrSupport(cfg.url) !== true;
  const row = db.prepare("SELECT auth FROM mcp_servers WHERE id=?").get(serverIdFor(key, server));
  return !!parse(row?.auth, {}).no_dcr;
}

// Default mcp_servers id for a plugin connector: the server name slugged
// (X → "x", so its fixed callback is /oauth/callback/x), reusing a row a
// plugin install already created.
export function serverIdFor(key, server) {
  const meta = installedMeta(key);
  const rows = db.prepare("SELECT id FROM mcp_servers WHERE name=? ORDER BY created_at").all(server);
  const mine = rows.find((r) => (meta?.mcp || []).includes(r.id));
  if (mine) return mine.id;
  if (rows[0]) return rows[0].id;
  return server.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24) || "srv";
}

// Substitute setup values into one server config → the mcp_servers row
// fields: {url|command, args, env, headers, oauth_client, transport,
// missing:[labels of required values not set], authMode}.
export function materialize(key, server, cfg = serverConfigs(key)[server]) {
  if (!cfg) return null;
  const vals = settingsOf(key);
  const root = pluginRoot(key) || "";
  const fields = fieldsOf(key);
  const sub = (s) =>
    String(s).replace(VAR_RE, (all, inner) => {
      if (inner.startsWith("env:")) { const n = inner.slice(4); return vals[n] ?? process.env[n] ?? ""; }
      const [name, dflt] = inner.split(":-");
      if (name === "CLAUDE_PLUGIN_ROOT" || name === "CURSOR_PLUGIN_ROOT") return root;
      if (name === "workspaceFolder" || name === "userHome") return vals[name] ?? homedir();
      if (name === "HOME" || name === "USERPROFILE") return vals[name] ?? process.env[name] ?? "";
      if (name === "ROX_FILES_DIR") return OUT_DIR;
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) return all;
      return vals[name] ?? (dflt !== undefined ? dflt : "");
    });
  const headers = {};
  for (const [k, v] of Object.entries(cfg.headers || {})) {
    const val = sub(v).trim();
    if (val && !/^(Bearer|Basic|Token)\s*$/i.test(val)) headers[k] = val;
  }
  let oauth_client = null;
  const own = needsOwnClient(key, server, cfg);
  if (isGoogle(cfg)) {
    if (vals.GOOGLE_CLIENT_ID)
      oauth_client = {
        client_id: vals.GOOGLE_CLIENT_ID,
        ...(vals.GOOGLE_CLIENT_SECRET ? { client_secret: vals.GOOGLE_CLIENT_SECRET } : {}),
        scope: (Array.isArray(cfg.auth?.scopes) ? cfg.auth.scopes.join(" ") : "") || googleScope(cfg.url),
        // Redirect: the fixed loopback http://127.0.0.1:8787/oauth/callback/<id>
        // (Desktop clients accept any loopback port/path); a refresh token
        // on every consent.
        extra: { access_type: "offline", prompt: "consent" },
      };
  } else if (own || cfg.auth?.CLIENT_ID) {
    const id = own ? vals.CLIENT_ID || "" : literalClient(cfg) ? "" : sub(cfg.auth.CLIENT_ID);
    const secret = own ? vals.CLIENT_SECRET || "" : cfg.auth?.CLIENT_SECRET ? sub(cfg.auth.CLIENT_SECRET) : "";
    const scope = Array.isArray(cfg.auth?.scopes) ? cfg.auth.scopes.join(" ") : cfg.auth?.scopes || "";
    if (id) oauth_client = { client_id: id, ...(secret ? { client_secret: secret } : {}), ...(scope ? { scope } : {}) };
  }
  if (own && literalClient(cfg) && vals.BEARER_TOKEN && !Object.keys(headers).some((k) => k.toLowerCase() === "authorization"))
    headers.Authorization = `Bearer ${vals.BEARER_TOKEN}`;
  const missing = fields.filter((f) => f.required && !f.oauth && !vals[f.key]).map((f) => f.label);
  // Without a client id, a vendor-pinned connector can still connect with
  // an app token; otherwise it is blocked until the Client ID is saved.
  const hasAuthHeader = Object.keys(headers).some((k) => k.toLowerCase() === "authorization");
  if (sharesGoogle(cfg)) {
    if (!vals.GOOGLE_CLIENT_ID) missing.push("Google Client ID");
    if (!vals.GOOGLE_CLIENT_SECRET) missing.push("Google Client Secret");
  } else if (own && !oauth_client && !hasAuthHeader) missing.push("Client ID");
  const t = String(cfg.type || "").toLowerCase();
  return {
    url: cfg.url ? sub(cfg.url) : undefined,
    command: cfg.command ? sub(cfg.command) : undefined,
    args: (cfg.args || []).map(sub),
    env: Object.fromEntries(Object.entries(cfg.env || {}).map(([k, v]) => [k, sub(v)]).filter(([, v]) => v !== "")),
    headers,
    oauth_client,
    transport: t === "sse" ? "sse" : null,
    missing,
    own,
  };
}

// ————— install from the plugin's repository —————
const repoCache = (url) => join(PLUGINS_DIR, ".repos", url.replace(/^https:\/\/github\.com\//, "").replace(/[^\w.-]+/g, "__"));
async function repoView(url) {
  const dir = repoCache(url);
  if (!existsSync(join(dir, ".git"))) {
    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dirname(dir), { recursive: true });
    await git(["clone", "-q", "--depth", "1", "--filter=blob:none", "--no-checkout", url, dir]);
  }
  const files = String(await git(["-C", dir, "ls-tree", "-r", "--name-only", "HEAD"])).split("\n").filter(Boolean);
  const set = new Set(files);
  const read = async (p) => (set.has(p) ? String(await git(["-C", dir, "show", `HEAD:${p}`]).catch(() => "")) || null : null);
  return { dir, files, read };
}

// Fetch the plugin's directory (plus any catalog skills living elsewhere
// in the repo) into PLUGINS_DIR/<key>. → {root (relative), skills, servers}
export async function fetchPlugin(e) {
  const hint = e.install || {};
  if (hint.unavailable) throw new Error(`${e.displayName} can't be installed: ${hint.unavailable}`);
  const repo = hint.repo || repoOf(e);
  if (!repo) throw new Error(`${e.displayName} has no public repository`);
  const v = await repoView(repo);
  let path = hint.path, skills = hint.skills, servers = hint.servers;
  const has = (p) => p === "" || v.files.some((f) => f.startsWith(p + "/"));
  if (path == null || !has(path)) {
    const r = await resolveEntry(e, v);
    if (r.error) throw new Error(r.error);
    ({ path } = r);
    skills = r.skills.map((s) => s.file);
    servers = r.mcpServers;
  }
  const dest = join(PLUGINS_DIR, e.key);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  // Small plugin dirs come over whole; a plugin that is a big repo's root
  // (shadcn-ui/ui, …) only brings its plugin parts plus whatever its MCP
  // config references under the plugin root.
  const pre = path ? path + "/" : "";
  const under = v.files.filter((f) => f.startsWith(pre));
  const specs = new Set();
  if (under.length <= 3000) specs.add(path || ".");
  else {
    const top = new Set(under.map((f) => f.slice(pre.length).split("/")[0]));
    for (const p of [".cursor-plugin", ".claude-plugin", ".grok-plugin", ".codex-plugin", ".plugin", "plugin.json", "mcp.json", ".mcp.json",
      "skills", "commands", "agents", "hooks", "rules", "assets", ".lsp.json", "settings.json"])
      if (top.has(p)) specs.add(pre + p);
    for (const m of JSON.stringify(servers || {}).matchAll(/\$\{(?:CLAUDE|CURSOR)_PLUGIN_ROOT\}\/([\w.\/-]+)/g)) {
      const first = m[1].split("/")[0];
      if (top.has(first)) specs.add(pre + first);
    }
    if (!specs.size) specs.add(path || ".");
  }
  for (const f of skills || []) {
    const d = dirname(f);
    if (path && !d.startsWith(path + "/")) specs.add(d);
  }
  // Blobs arrive lazily from the promisor remote; a dropped fetch fails
  // the checkout, so give it one more try.
  const co = () => git(["-C", v.dir, `--work-tree=${dest}`, "checkout", "-q", "HEAD", "--", ...specs]);
  await co().catch(() => co());
  return {
    root: path ? join(e.key, path) : e.key,
    skills: (skills || []).map((f) => ({ name: f.split("/").slice(-2, -1)[0], file: join(e.key, f) })),
    servers: servers || {},
  };
}

export function removePluginFiles(key) {
  if (/^[\w.@-]+$/.test(key) && !key.startsWith(".")) rmSync(join(PLUGINS_DIR, key), { recursive: true, force: true });
}

// ————— inventory —————
const frontDesc = (head) =>
  head.match(/^description:\s*[>|]-?\s*\n((?:[ \t]+.+\n?)+)/m)?.[1]?.replace(/\s+/g, " ").trim() ||
  head.match(/^description:\s*["']?(.+?)["']?\s*$/m)?.[1]?.trim() ||
  "";
export const skillDescription = frontDesc;

// What an installed plugin ships: skills/commands/agents, hooks/lsp, the
// MCP servers its manifests declare, and manifest metadata.
export function pluginInventory(key, root) {
  const meta = installedMeta(key) || {};
  const listDir = (sub, { md = false } = {}) => {
    const base = join(root, sub);
    if (!existsSync(base)) return [];
    try {
      return readdirSync(base).filter((n) => !n.startsWith(".")).map((n) => {
        const entry = { name: n.replace(/\.md$/, ""), description: "" };
        try { entry.description = frontDesc(readFileSync(md ? join(base, n) : join(base, n, "SKILL.md"), "utf8").slice(0, 3000)); } catch {}
        return entry;
      });
    } catch { return []; }
  };
  let skills;
  if (Array.isArray(meta.skills) && meta.skills.length) {
    const seen = new Set();
    skills = meta.skills.filter((s) => !seen.has(s.file) && seen.add(s.file)).map((s) => {
      let description = "";
      try { description = frontDesc(readFileSync(join(PLUGINS_DIR, s.file), "utf8").slice(0, 3000)); } catch {}
      return { name: s.name, description };
    });
  } else skills = listDir("skills");
  let manifest = {};
  for (const m of [".cursor-plugin/plugin.json", ".claude-plugin/plugin.json", ".grok-plugin/plugin.json", "plugin.json"]) {
    const j = parse(existsSync(join(root, m)) ? readFileSync(join(root, m), "utf8") : null, null);
    if (j?.name) { manifest = j; break; }
  }
  const servers = serverConfigs(key);
  return {
    skills,
    commands: listDir("commands", { md: true }),
    agents: listDir("agents", { md: true }),
    hooks: existsSync(join(root, "hooks/hooks.json")),
    lsp: existsSync(join(root, ".lsp.json")),
    servers,
    manifest: {
      author: manifest.author?.name || (typeof manifest.author === "string" ? manifest.author : null),
      version: manifest.version || null,
      license: manifest.license || null,
    },
  };
}

// The SKILL.md of one of an installed plugin's skills.
export function skillFile(key, skill) {
  const meta = installedMeta(key) || {};
  const hit = (meta.skills || []).find((s) => s.name === skill);
  if (hit) return join(PLUGINS_DIR, hit.file);
  const root = pluginRoot(key);
  return root && /^[\w.@-]+$/.test(skill) ? join(root, "skills", skill, "SKILL.md") : null;
}

// ————— agent prompt —————
// Installed plugin skills as an index the agent reads on demand (Grok Bot
// loads a plugin skill's SKILL.md when its description matches the task).
export function pluginSkillsInject() {
  try {
    const lines = [];
    for (const { name } of db.prepare("SELECT name FROM installed_plugins ORDER BY created_at").all()) {
      const root = pluginRoot(name);
      if (!root) continue;
      const inv = pluginInventory(name, root);
      for (const s of inv.skills) {
        const f = skillFile(name, s.name);
        if (f && existsSync(f)) lines.push(`- ${name}/${s.name}: ${(s.description || "").slice(0, 160)} (file: ${f})`);
      }
    }
    if (!lines.length) return "";
    return `\nPlugin skills (installed): when a request matches a skill's description, read its SKILL.md with shell_run (cat the file) and follow it; files it references sit beside it.\n${lines.join("\n")}`.slice(0, 24000);
  } catch { return ""; }
}

// Catalog skill descriptions/links for an entry that isn't installed.
export const catalogSkills = (e) => (e?.skills || []).map((s) => ({ name: s.name, description: s.description || "", source: sourceUrlInfo(s.sourceUrl) ? s.sourceUrl : null }));
