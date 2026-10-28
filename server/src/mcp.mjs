// server/mcp.mjs — user-connected MCP servers (stdio).
//
// app-server owns the mcp_servers table and the endpoints; this module owns
// client lifecycle: lazy spawn on first use, tool discovery, namespaced tool
// defs (mcp_<id>_<tool>), and call routing. A failed spawn caches its error
// and quietly leaves the surface out of toolList() — the model just doesn't
// see those tools. Deleting or editing a server invalidates its client.

const attempts = new Map(); // id -> Promise<{client,tools,err}>
const recs = new Map(); // id -> settled {client,tools,err}

export function statusOf(id) {
  const r = recs.get(id);
  if (!r) return { status: "idle", tools: 0 };
  // A server that lists tools without auth but whose resource metadata
  // (or a tool call's 401) says it wants OAuth isn't really connected.
  if (r.client && r.needsAuth) return { status: "needs-auth", tools: r.tools.length, err: r.needsAuth };
  if (r.client && r.unavailable) return { status: "error", tools: r.tools.length, err: r.unavailable };
  if (r.client) return { status: "connected", tools: r.tools.length };
  return { status: "error", tools: 0, err: r.err };
}

export function invalidate(id) {
  const r = recs.get(id);
  attempts.delete(id);
  recs.delete(id);
  try {
    r?.client?.close();
  } catch {}
}

// Spawn + handshake + tools/list once per row; every caller shares the one
// attempt. The row is the mcp_servers record (command/args/env as stored).
export function ensureClient(row) {
  let p = attempts.get(row.id);
  if (!p) {
    p = connect(row);
    attempts.set(row.id, p);
    p.then((r) => recs.set(row.id, r)).catch(() => {});
  }
  return p;
}

// ── OAuth for remote servers ─────────────────────────────────────────────
// MCP's Authorization spec (RFC 9728 resource metadata → RFC 8414 auth-server
// metadata → RFC 7591 dynamic client registration → browser authorize →
// localhost callback → token exchange). The SDK's client/auth.js drives it;
// we supply storage (the row's `auth` JSON column), the browser launch and
// the loopback redirect endpoint. Tokens persist, so reconnects reuse them
// and the SDK refreshes them when they expire.
import { execFile } from "node:child_process";
import os from "node:os";
import { randomBytes } from "node:crypto";
import { PORT } from "./config.mjs";
import { isName } from "./grammar.mjs";

let authHooks = null; // {loadAuth(id), saveAuth(id, obj), done(id)} — app-server wires the db
export const setAuthHooks = (h) => (authHooks = h);

const pending = new Map(); // id -> {provider, url, expiry}

function openUrl(u) {
  const cmd = os.platform() === "darwin" ? "open" : os.platform() === "win32" ? "cmd" : "xdg-open";
  const args = os.platform() === "win32" ? ["/c", "start", "", u] : [u];
  execFile(cmd, args, () => {});
}

const donePage = (title) =>
  `<body style="background:#1f1f1f;color:#f5f5f5;font:15px -apple-system,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0"><div style="text-align:center"><div style="font-size:22px;font-weight:600">${title}</div><div style="color:#f5f5f58f;margin-top:8px">You can close this window.</div></div></body>`;

// Pre-registered (static) OAuth clients redirect to the main server — a
// fixed URI the user registers with the provider (X validates it exactly);
// dynamic-registration clients keep the ephemeral loopback listener.
export const fixedRedirectUrl = (id) => `http://127.0.0.1:${PORT}/oauth/callback/${encodeURIComponent(id)}`;
const staticClient = (row) => {
  try {
    const c = JSON.parse(row.oauth_client || "null");
    return c?.client_id ? c : null;
  } catch { return null; }
};
const storedAuth = (row) => {
  try { return JSON.parse(row.auth || "{}") || {}; } catch { return {}; }
};

async function finishAuth(id, code) {
  const p = pending.get(id);
  if (!p) throw new Error("no pending flow");
  const { auth } = await import("@modelcontextprotocol/sdk/client/auth.js");
  await auth(p.provider, { serverUrl: p.url, authorizationCode: code });
  pending.delete(id);
  authHooks?.done(id);
}

// GET /oauth/callback/<id>?code=… — the main server's fixed route.
export function handleOAuthCallback(req, res) {
  {
    const u = new URL(req.url || "/", "http://x");
    const id = decodeURIComponent(u.pathname.match(/^\/oauth\/callback\/(.+)$/)?.[1] || "");
    res.setHeader("content-type", "text/html");
    const code = u.searchParams.get("code");
    const errTxt = u.searchParams.get("error");
    if (!id || (!code && !errTxt) || !pending.has(id)) {
      res.statusCode = 400;
      res.end(donePage("Authentication failed"));
      return;
    }
    const want = authHooks?.loadAuth(id)?.state;
    if (code && want && u.searchParams.get("state") !== want) {
      res.statusCode = 400;
      res.end(donePage("Authentication failed"));
      return;
    }
    if (errTxt) {
      pending.delete(id);
      res.end(donePage("Authentication failed"));
      return;
    }
    // Respond once the token exchange settles so a failure is visible.
    finishAuth(id, code).then(
      () => res.end(donePage("Connected")),
      (e) => {
        console.error(`  mcp ${id}: token exchange failed: ${String(e?.message || e).slice(0, 200)}`);
        res.end(donePage("Authentication failed"));
      },
    );
  }
}

// OAuthClientProvider backed by the row's `auth` JSON column. Reads go
// through loadAuth each call so sequential SDK writes never overwrite each
// other (client_info first, verifier and tokens later).
// background: the transport's own re-auth on a 401 (a probe or a tool
// call) only records the authorize URL — a browser tab opens solely from
// the user's Authenticate.
function providerFor(row, { background = false } = {}) {
  const state = () => authHooks?.loadAuth(row.id) || {};
  const save = (patch) => authHooks?.saveAuth(row.id, { ...state(), ...patch });
  const fixed = staticClient(row);
  return {
    // Always the main server's fixed route, so a registered client's
    // redirect stays valid across restarts.
    get redirectUrl() {
      return fixedRedirectUrl(row.id);
    },
    get clientMetadata() {
      return {
        client_name: "Grok Shell",
        redirect_uris: [this.redirectUrl],
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        token_endpoint_auth_method: fixed?.client_secret ? "client_secret_basic" : "none",
        ...(fixed?.scope ? { scope: fixed.scope } : {}),
      };
    },
    // A pre-registered client is the user's own app: its id (and secret,
    // for confidential "Web App" clients → HTTP Basic on the token call)
    // stand in for dynamic registration, which is never attempted.
    clientInformation() {
      if (fixed) return { client_id: fixed.client_id, ...(fixed.client_secret ? { client_secret: fixed.client_secret } : {}) };
      // A client registered for another redirect (the old per-run port)
      // can't complete a flow: register again.
      const info = state().client_info;
      return info?.redirect_uris && !info.redirect_uris.includes(fixedRedirectUrl(row.id)) ? undefined : info;
    },
    async saveClientInformation(i) { if (!fixed) save({ client_info: i }); },
    tokens() { return state().tokens; },
    async saveTokens(t) { save({ tokens: t }); },
    redirectToAuthorization(u) {
      // Provider-specific authorize params (Google: access_type=offline +
      // prompt=consent so a refresh token is issued).
      if (fixed?.extra) {
        u = new URL(String(u));
        for (const [k, v] of Object.entries(fixed.extra)) u.searchParams.set(k, v);
      }
      lastAuthorizeUrl.set(row.id, String(u));
      console.log(`  mcp ${row.id}: authorize ${String(u).slice(0, 400)}`);
      if (!background && !noOpen.has(row.id)) openUrl(String(u));
    },
    // CSRF state (X requires one): checked against the callback's.
    async state() {
      const st = randomBytes(16).toString("hex");
      save({ state: st });
      return st;
    },
    async saveCodeVerifier(v) { save({ verifier: v }); },
    codeVerifier() { return state().verifier || ""; },
  };
}

// POST /mcp/auth — kick the browser flow for a remote server. AUTHORIZED
// means stored tokens already satisfy the server; REDIRECT means the tab is
// open and finishAuth() completes it from the loopback callback.
const lastAuthorizeUrl = new Map(); // id -> the authorize URL last opened
const noOpen = new Set(); // ids whose next flow returns the URL without a tab

// open:false returns the authorize URL instead of opening a browser tab.
export async function beginAuth(row, { open = true } = {}) {
  if (!row.url) throw new Error("not a remote server");
  const fixed = staticClient(row);
  const provider = providerFor(row);
  const { auth } = await import("@modelcontextprotocol/sdk/client/auth.js");
  let result;
  lastAuthorizeUrl.delete(row.id);
  if (!open) noOpen.add(row.id);
  try {
    const scope = fixed?.scope || (await offlineScope(row.url));
    result = await auth(provider, { serverUrl: row.url, ...(scope ? { scope } : {}) });
  } catch (e) {
    const msg = String(e?.message || e);
    // No registration endpoint and no client of our own: remember it so
    // the plugin page asks for a pre-registered Client ID.
    if (!fixed && /dynamic client registration|registration_endpoint|does not support/i.test(msg)) {
      authHooks?.saveAuth(row.id, { ...storedAuth(row), no_dcr: true });
      throw new Error("This app's sign-in has no automatic client registration — add your own OAuth Client ID in Setup");
    }
    throw e;
  } finally {
    noOpen.delete(row.id);
  }
  if (result === "AUTHORIZED") {
    authHooks?.done(row.id);
    return { ok: true, authorized: true };
  }
  clearTimeout(pending.get(row.id)?.timer);
  pending.set(row.id, {
    provider,
    url: row.url,
    timer: setTimeout(() => pending.delete(row.id), 10 * 60 * 1000),
  });
  return { ok: true, redirected: true, ...(open ? {} : { authorizeUrl: lastAuthorizeUrl.get(row.id) || null }) };
}

async function connect(row) {
  const rec = { client: null, tools: [], err: null };
  try {
    const [{ Client }] = await Promise.all([import("@modelcontextprotocol/sdk/client/index.js")]);
    if (row.url) {
      // A provider makes the transport drive the full auth flow on a 401
      // (which opens a browser tab) — only supply it once tokens exist so
      // plain probes just surface UnauthorizedError instead of popping
      // a tab on every status check. Signed-in OAuth tokens win over a
      // static Authorization header (an app token) from the plugin setup.
      const hasTokens = !!storedAuth(row).tokens;
      let headers = {};
      try { headers = JSON.parse(row.headers || "{}") || {}; } catch {}
      if (hasTokens)
        for (const k of Object.keys(headers)) if (k.toLowerCase() === "authorization") delete headers[k];
      const opts = { authProvider: hasTokens ? providerFor(row, { background: true }) : undefined, requestInit: { headers } };
      let transport;
      if (row.transport === "sse") {
        const { SSEClientTransport } = await import("@modelcontextprotocol/sdk/client/sse.js");
        transport = new SSEClientTransport(new URL(row.url), opts);
      } else {
        const { StreamableHTTPClientTransport } = await import("@modelcontextprotocol/sdk/client/streamableHttp.js");
        transport = new StreamableHTTPClientTransport(new URL(row.url), opts);
      }
      const client = new Client({ name: "grok-shell", version: "1.0.0" });
      await client.connect(transport);
      const { tools } = await client.listTools();
      rec.client = client;
      rec.tools = tools || [];
      // Some servers (Google's) serve tools/list without auth while every
      // call needs a token: without tokens or credential headers, a server
      // that publishes OAuth resource metadata needs sign-in.
      const credHeader = Object.entries(headers).some(([k, v]) => v && /auth|key|token|secret/i.test(k));
      if (!hasTokens && !credHeader && (await wantsOAuth(row.url))) rec.needsAuth = "Sign-in required";
      console.log(`  mcp ${row.id}: connected (http), ${rec.tools.length} tools${rec.needsAuth ? " — needs sign-in" : ""}`);
      return rec;
    }
    const [{ StdioClientTransport }] = await Promise.all([
      import("@modelcontextprotocol/sdk/client/stdio.js"),
    ]);
    const transport = new StdioClientTransport({
      command: row.command,
      args: JSON.parse(row.args || "[]"),
      env: { ...process.env, PATH: toolPath(), ...JSON.parse(row.env || "{}") },
      stderr: "pipe",
    });
    const client = new Client({ name: "grok-shell", version: "1.0.0" });
    transport.stderr?.on("data", (d) => {
      const line = String(d).trim();
      if (
        line &&
        !line.startsWith("(node:") &&
        !line.includes("ExperimentalWarning")
      )
        console.error(`  mcp ${row.id}: ${line.slice(0, 200)}`);
    });
    await client.connect(transport);
    const { tools } = await client.listTools();
    rec.client = client;
    rec.tools = tools || [];
    console.log(`  mcp ${row.id}: connected, ${rec.tools.length} tools`);
  } catch (e) {
    rec.err = googleApiOff(row, e) || String(e?.message || e).slice(0, 300);
    console.error(`  mcp ${row.id}: ${rec.err}`);
  }
  return rec;
}

// Google's MCP hosts answer a signed-in request with a bare 403 (the body
// is the normal result) when the service is off in the project that owns
// the client; only a tool call says why. Point at the enable page — the
// client id's leading digits are that project's number. The wording stays
// clear of "auth"/"403" so Rox offers Reconnect, not another sign-in.
function googleApiOff(row, e) {
  if (e?.code !== 403 || !storedAuth(row).tokens) return null;
  let host;
  try { host = new URL(row.url).hostname; } catch { return null; }
  if (!host.endsWith(".googleapis.com")) return null;
  const project = staticClient(row)?.client_id?.match(/^(\d+)-/)?.[1];
  const link = `https://console.developers.google.com/apis/api/${host}/overview${project ? `?project=${project}` : ""}`;
  return `Google refused the request: the ${host} service is probably turned off in your Google Cloud project${project ? ` ${project}` : ""}. Enable it at ${link}, wait a minute, then Reconnect.`;
}

// PATH for stdio servers: an app launched from Finder inherits launchd's
// bare PATH, without the dirs npx / uvx / bunx usually live in.
function toolPath() {
  const home = process.env.HOME || "";
  const extra = [`${home}/.local/bin`, `${home}/.cargo/bin`, `${home}/.bun/bin`, "/opt/homebrew/bin", "/usr/local/bin"];
  const have = (process.env.PATH || "").split(":").filter(Boolean);
  return [...have, ...extra.filter((d) => !have.includes(d))].join(":");
}

// The resource's scopes plus offline_access when its authorization server
// offers it — without that some (Vercel) issue no refresh token, and the
// one-hour access token can't renew. Cached per URL; undefined leaves the
// SDK's own choice.
const scopeCache = new Map();
function offlineScope(url) {
  if (!scopeCache.has(url))
    scopeCache.set(url, (async () => {
      const { discoverOAuthProtectedResourceMetadata, discoverAuthorizationServerMetadata } = await import("@modelcontextprotocol/sdk/client/auth.js");
      const prm = await discoverOAuthProtectedResourceMetadata(url).catch(() => null);
      const as = await discoverAuthorizationServerMetadata(prm?.authorization_servers?.[0] || new URL("/", url).href);
      if (!as?.scopes_supported?.includes("offline_access")) return undefined;
      const base = prm?.scopes_supported?.length ? prm.scopes_supported : [];
      return [...new Set([...base, "offline_access"])].join(" ");
    })().catch(() => undefined));
  return scopeCache.get(url);
}

// Does the server advertise OAuth (RFC 9728 protected-resource metadata
// naming an authorization server)? Cached per URL.
const prmCache = new Map();
function wantsOAuth(url) {
  if (!prmCache.has(url))
    prmCache.set(url, (async () => {
      const { discoverOAuthProtectedResourceMetadata } = await import("@modelcontextprotocol/sdk/client/auth.js");
      const prm = await discoverOAuthProtectedResourceMetadata(url);
      return !!prm?.authorization_servers?.length;
    })().catch(() => false));
  return prmCache.get(url);
}
const AUTH_ERR = /\b401\b|unauthori[sz]ed|unauthenticated|invalid[_ ]token|invalid_grant|login required|re-?authenticate|sign in again/i;

// Tool names the model sees: mcp_<id>_<tool>. Ids are [a-z0-9-] (no
// underscores) so the name parses back unambiguously; Anthropic's charset is
// [a-zA-Z0-9_-] ≤64.
export const mcpName = (id, tool) =>
  `mcp_${id}_${tool}`.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64);
export const parseMcpName = (n) =>
  n.match(/^mcp_([a-z0-9-]+)_(.+)$/)?.slice(1) || null;

// Anthropic-shaped defs for a server's discovered tools.
export function anthropicTools(id, tools) {
  return tools.map((t) => ({
    name: mcpName(id, t.name),
    description: `[${id}] ${t.description || t.name}`.slice(0, 1024),
    input_schema: t.inputSchema || { type: "object", properties: {} },
  }));
}

// Fold MCP content blocks into the single string the model reads.
export async function callTool(row, toolName, input) {
  const rec = await ensureClient(row);
  if (!rec.client)
    return {
      content: `mcp server ${row.id} is not connected (${rec.err || "failed to start"})`,
      is_error: true,
    };
  if (rec.needsAuth)
    return { content: `${row.id} needs sign-in — ask the user to open Connect Apps, find this app and press Authenticate`, is_error: true };
  let result;
  try {
    result = await rec.client.callTool({ name: toolName, arguments: input || {} }, undefined, { timeout: 60_000 });
  } catch (e) {
    if (row.url && AUTH_ERR.test(String(e?.message || e))) rec.needsAuth = "Sign-in expired or was rejected";
    throw e;
  }
  const parts = (result.content || []).map((c) => {
    if (c.type === "text") return c.text;
    if (c.type === "image")
      return `[image: ${c.mimeType || "image/png"}, ${Math.round((c.data?.length || 0) * 0.75)} bytes]`;
    return JSON.stringify(c);
  });
  // A tool error that is really an auth failure flips the server to
  // needs-auth so the plugin page offers Authenticate.
  if (row.url && result.isError && AUTH_ERR.test(parts.join("\n").slice(0, 2000))) rec.needsAuth = "Sign-in expired or was rejected";
  // Google's MCP servers are a preview: until the client's project is
  // enrolled every call fails, so drop the server's tools until Reconnect.
  if (row.url && result.isError && /Developer Preview Program/i.test(parts.join("\n").slice(0, 2000)))
    rec.unavailable = `Google only serves this to projects in the Google Workspace Developer Preview Program — enroll at https://developers.google.com/workspace/preview, then Reconnect.`;
  // `meta` is the result's `_meta` (a server's `rox/open` request); the
  // caller strips it before the result goes back to the model.
  return { content: parts.join("\n") || "(empty)", is_error: !!result.isError, meta: result._meta };
}

// ── Mini apps a server ships ─────────────────────────────────────────────
// A server can ship Native Shell mini apps as resources of this type (the
// TSX module text, `export default function R()`); Rox compiles and runs
// them like its own and lets them call the server back (`callToolJson`).
export const APP_MIME = "text/x-nsh-tsx";

// The server's resources, listed once per connection.
async function listResources(row) {
  const rec = await ensureClient(row);
  if (!rec.client?.getServerCapabilities()?.resources) return [];
  rec.resources ||= rec.client
    .listResources()
    .then(({ resources }) => resources || [])
    .catch(() => []);
  return rec.resources;
}

// The server's mini-app resources.
export async function listApps(row) {
  return (await listResources(row)).filter((r) => r.mimeType === APP_MIME);
}

// The commands and `@` scopes a server announces: its `rox://manifest`
// resource (JSON, every field optional), read once per connection; {} when
// it ships none or it doesn't parse. Commands and scopes whose name breaks
// the grammar (grammar.mjs) are dropped with a warning: no `@` token or
// `/` command could name them.
export const MANIFEST_URI = "rox://manifest";
export async function readManifest(row) {
  const rec = await ensureClient(row);
  if (!(await listResources(row)).some((r) => r.uri === MANIFEST_URI)) return {};
  rec.manifest ||= rec.client
    .readResource({ uri: MANIFEST_URI })
    .then(({ contents }) => {
      const m = JSON.parse(contents?.find((c) => typeof c.text === "string")?.text || "{}");
      if (!m || typeof m !== "object" || Array.isArray(m)) return {};
      for (const kind of ["commands", "scopes"]) {
        if (m[kind] === undefined) continue;
        const list = Array.isArray(m[kind]) ? m[kind] : [];
        m[kind] = list.filter((x) => {
          if (x && typeof x === "object" && isName(x.name)) return true;
          console.warn(`  mcp ${row.id}: manifest ${kind} entry ${JSON.stringify(x?.name ?? x).slice(0, 80)} dropped: names are [A-Za-z0-9_-]+`);
          return false;
        });
      }
      return m;
    })
    .catch(() => ({}));
  return rec.manifest;
}

// One mini app's module text, or "" when the server has no such app.
export async function readApp(row, uri) {
  const rec = await ensureClient(row);
  if (!rec.client) return "";
  const { contents } = await rec.client.readResource({ uri });
  return contents?.find((c) => typeof c.text === "string")?.text || "";
}

// A mini app's tool call: the result as data — structuredContent, else the
// text parsed as JSON, else the text.
export async function callToolJson(row, toolName, input) {
  const rec = await ensureClient(row);
  if (!rec.client) return { ok: false, error: `${row.id} is not connected (${rec.err || "failed to start"})` };
  const result = await rec.client.callTool({ name: toolName, arguments: input || {} }, undefined, { timeout: 120_000 });
  const text = (result.content || []).filter((c) => c.type === "text").map((c) => c.text).join("\n");
  if (result.isError) return { ok: false, error: text || "tool failed" };
  // MCP wants structuredContent to be an object, so a bare value comes
  // wrapped as {result: …}
  const sc = result.structuredContent;
  if (sc && typeof sc === "object" && !Array.isArray(sc) && Object.keys(sc).length === 1 && "result" in sc)
    return { ok: true, result: sc.result };
  if (sc !== undefined) return { ok: true, result: sc };
  try {
    return { ok: true, result: JSON.parse(text) };
  } catch {
    return { ok: true, result: text };
  }
}

// `mcp:<server>/<app>[|<props JSON>]` — a shipped mini app's module name.
export const appKey = (server, app, props) =>
  `mcp:${server}/${app}${props && Object.keys(props).length ? `|${JSON.stringify(props)}` : ""}`;
export function parseAppKey(key) {
  const m = String(key || "").match(/^mcp:([a-z0-9-]+)\/([A-Za-z0-9._-]+)(?:\|(.*))?$/s);
  if (!m) return null;
  let props = {};
  try { props = m[3] ? JSON.parse(m[3]) : {}; } catch {}
  return { server: m[1], app: m[2], props };
}
// `nsh://<anything>/<app>` → `<app>`.
export const appOfUri = (uri) => String(uri || "").split("/").pop();
