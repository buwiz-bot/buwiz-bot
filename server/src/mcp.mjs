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
import http from "node:http";
import { execFile } from "node:child_process";
import os from "node:os";

let authHooks = null; // {loadAuth(id), saveAuth(id, obj), done(id)} — app-server wires the db
export const setAuthHooks = (h) => (authHooks = h);

const pending = new Map(); // id -> {provider, url, expiry}
let cbServer = null;
let cbPort = 0;

function openUrl(u) {
  const cmd = os.platform() === "darwin" ? "open" : os.platform() === "win32" ? "cmd" : "xdg-open";
  const args = os.platform() === "win32" ? ["/c", "start", "", u] : [u];
  execFile(cmd, args, () => {});
}

const donePage = (title) =>
  `<body style="background:#1f1f1f;color:#f5f5f5;font:15px -apple-system,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0"><div style="text-align:center"><div style="font-size:22px;font-weight:600">${title}</div><div style="color:#f5f5f58f;margin-top:8px">You can close this window.</div></div></body>`;

async function finishAuth(id, code) {
  const p = pending.get(id);
  if (!p) throw new Error("no pending flow");
  const { auth } = await import("@modelcontextprotocol/sdk/client/auth.js");
  await auth(p.provider, { serverUrl: p.url, authorizationCode: code });
  pending.delete(id);
  authHooks?.done(id);
}

function ensureCallbackServer() {
  if (cbServer) return Promise.resolve();
  cbServer = http.createServer((req, res) => {
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
    if (errTxt) {
      pending.delete(id);
      res.end(donePage("Authentication failed"));
      return;
    }
    res.end(donePage("Connected"));
    finishAuth(id, code).catch(() => {});
  });
  return new Promise((resolve) =>
    cbServer.listen(0, "127.0.0.1", () => {
      cbPort = cbServer.address().port;
      resolve();
    }),
  );
}

// OAuthClientProvider backed by the row's `auth` JSON column. Reads go
// through loadAuth each call so sequential SDK writes never overwrite each
// other (client_info first, verifier and tokens later).
function providerFor(row) {
  const state = () => authHooks?.loadAuth(row.id) || {};
  const save = (patch) => authHooks?.saveAuth(row.id, { ...state(), ...patch });
  return {
    get redirectUrl() {
      return `http://127.0.0.1:${cbPort}/oauth/callback/${encodeURIComponent(row.id)}`;
    },
    get clientMetadata() {
      return {
        client_name: "Grok Shell",
        redirect_uris: [this.redirectUrl],
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        token_endpoint_auth_method: "none",
      };
    },
    clientInformation() { return state().client_info; },
    async saveClientInformation(i) { save({ client_info: i }); },
    tokens() { return state().tokens; },
    async saveTokens(t) { save({ tokens: t }); },
    redirectToAuthorization(u) { openUrl(String(u)); },
    async saveCodeVerifier(v) { save({ verifier: v }); },
    codeVerifier() { return state().verifier || ""; },
  };
}

// POST /mcp/auth — kick the browser flow for a remote server. AUTHORIZED
// means stored tokens already satisfy the server; REDIRECT means the tab is
// open and finishAuth() completes it from the loopback callback.
export async function beginAuth(row) {
  if (!row.url) throw new Error("not a remote server");
  await ensureCallbackServer();
  const provider = providerFor(row);
  const { auth } = await import("@modelcontextprotocol/sdk/client/auth.js");
  const result = await auth(provider, { serverUrl: row.url });
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
  return { ok: true, redirected: true };
}

async function connect(row) {
  const rec = { client: null, tools: [], err: null };
  try {
    const [{ Client }] = await Promise.all([import("@modelcontextprotocol/sdk/client/index.js")]);
    if (row.url) {
      const { StreamableHTTPClientTransport } = await import("@modelcontextprotocol/sdk/client/streamableHttp.js");
      const transport = new StreamableHTTPClientTransport(new URL(row.url), {
        // A provider makes the transport drive the full auth flow on a 401
        // (which opens a browser tab) — only supply it once tokens exist so
        // plain probes just surface UnauthorizedError instead of popping
        // a tab on every status check.
        authProvider: row.auth ? providerFor(row) : undefined,
      });
      const client = new Client({ name: "grok-shell", version: "1.0.0" });
      await client.connect(transport);
      const { tools } = await client.listTools();
      rec.client = client;
      rec.tools = tools || [];
      console.log(`  mcp ${row.id}: connected (http), ${rec.tools.length} tools`);
      return rec;
    }
    const [{ StdioClientTransport }] = await Promise.all([
      import("@modelcontextprotocol/sdk/client/stdio.js"),
    ]);
    const transport = new StdioClientTransport({
      command: row.command,
      args: JSON.parse(row.args || "[]"),
      env: { ...process.env, ...JSON.parse(row.env || "{}") },
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
    rec.err = String(e?.message || e).slice(0, 300);
    console.error(`  mcp ${row.id}: ${rec.err}`);
  }
  return rec;
}

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
  const result = await rec.client.callTool(
    { name: toolName, arguments: input || {} },
    undefined,
    { timeout: 60_000 },
  );
  const parts = (result.content || []).map((c) => {
    if (c.type === "text") return c.text;
    if (c.type === "image")
      return `[image: ${c.mimeType || "image/png"}, ${Math.round((c.data?.length || 0) * 0.75)} bytes]`;
    return JSON.stringify(c);
  });
  return { content: parts.join("\n") || "(empty)", is_error: !!result.isError };
}
