// A remote MCP server behind its own OAuth authorization server, for the
// OAuth flow test (Rust broker plan 4.2), in the test's process: no SDK,
// no network beyond 127.0.0.1. It records every request it gets, so a run
// through Node and a run through buwiz-broker can be compared.
//
// MCP, each path its own resource:
//   /mcp   streamable HTTP, Bearer required (401 names its resource metadata)
//   /sse   the legacy SSE transport (GET the stream, POST /messages), Bearer
//   /open  streamable HTTP that answers without a token, though its resource
//          metadata names this authorization server (Google's servers do)
// OAuth: RFC 9728 resource metadata, RFC 8414 server metadata, RFC 7591
// registration (/register), /authorize (answers 302 to the redirect with a
// code), /token (authorization_code with PKCE S256, refresh_token that
// rotates). Clients: dynamic ones (public, "none") and STATIC (a confidential
// client with a secret, "client_secret_basic").
import { createHash, randomBytes } from "node:crypto";
import { createServer } from "node:http";

export const STATIC = { client_id: "static-client", client_secret: "static-secret" };

const rand = () => randomBytes(12).toString("hex");
const b64url = (buf) => buf.toString("base64url");

const TOOLS = [
  { name: "whoami", description: "The caller's client", inputSchema: { type: "object" } },
  { name: "denied", description: "Answers as a server whose token was revoked", inputSchema: { type: "object" } },
];

export async function oauthServer() {
  const log = [];
  const opts = { dcr: true, failRefresh: false };
  const clients = new Map([[STATIC.client_id, { ...STATIC, redirect_uris: null, token_endpoint_auth_method: "client_secret_basic" }]]);
  const codes = new Map(); // code -> {client_id, redirect_uri, challenge, scope, resource}
  const access = new Map(); // token -> client_id
  const refresh = new Map(); // token -> client_id
  const sse = new Map(); // session -> response stream
  let base = "";

  const json = (res, status, obj, headers = {}) => {
    res.writeHead(status, { "content-type": "application/json", ...headers });
    res.end(JSON.stringify(obj));
  };

  const prmUrl = (path) => `${base}/.well-known/oauth-protected-resource${path}`;

  // the client a bearer token names, or null
  const bearer = (req) => access.get(/^Bearer (.+)$/.exec(req.headers.authorization || "")?.[1]) ?? null;

  function unauthorized(res, path) {
    res.writeHead(401, { "www-authenticate": `Bearer resource_metadata="${prmUrl(path)}"`, "content-type": "application/json" });
    res.end(JSON.stringify({ error: "invalid_token" }));
  }

  // client authentication at /token: Basic for a confidential client, the
  // client_id field for a public one
  function clientOf(req, form) {
    const basic = /^Basic (.+)$/.exec(req.headers.authorization || "")?.[1];

    if (basic) {
      const [id, secret] = Buffer.from(basic, "base64").toString().split(":").map(decodeURIComponent);
      const c = clients.get(id);

      return c && c.client_secret === secret ? c : null;
    }

    const c = clients.get(form.get("client_id"));

    return c && !c.client_secret ? c : null;
  }

  function issue(client_id, scope) {
    const a = `at-${rand()}`;
    const r = `rt-${rand()}`;

    access.set(a, client_id);
    refresh.set(r, client_id);

    return { access_token: a, token_type: "Bearer", expires_in: 3600, refresh_token: r, ...(scope ? { scope } : {}) };
  }

  function rpc(msg, who) {
    const ok = (result) => ({ jsonrpc: "2.0", id: msg.id, result });

    switch (msg.method) {
      case "initialize":
        return ok({ protocolVersion: msg.params?.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: "oauth-fixture", version: "0.0.0" } });
      case "tools/list":
        return ok({ tools: TOOLS });
      case "tools/call":
        if (msg.params?.name === "denied") return ok({ content: [{ type: "text", text: "401 Unauthorized: the token was revoked" }], isError: true });

        return ok({ content: [{ type: "text", text: JSON.stringify({ client: who }) }], structuredContent: { client: who } });
      case "ping":
        return ok({});
      default:
        return { jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: `no method ${msg.method}` } };
    }
  }

  async function handle(req, res) {
    const u = new URL(req.url, base);
    const chunks = [];

    for await (const c of req) chunks.push(c);
    const body = Buffer.concat(chunks).toString();

    log.push({ method: req.method, path: u.pathname, query: Object.fromEntries(u.searchParams), headers: { ...req.headers }, body });

    // RFC 9728: per resource path; the root names /mcp
    const prm = /^\/\.well-known\/oauth-protected-resource(\/.*)?$/.exec(u.pathname);

    if (req.method === "GET" && prm) {
      const path = prm[1] || "/mcp";

      if (!["/mcp", "/sse", "/open"].includes(path)) return json(res, 404, { error: "not_found" });

      return json(res, 200, { resource: `${base}${path}`, authorization_servers: [base], scopes_supported: ["mcp:tools"], bearer_methods_supported: ["header"] });
    }

    if (req.method === "GET" && u.pathname === "/.well-known/oauth-authorization-server")
      return json(res, 200, {
        issuer: base,
        authorization_endpoint: `${base}/authorize`,
        token_endpoint: `${base}/token`,
        ...(opts.dcr ? { registration_endpoint: `${base}/register` } : {}),
        response_types_supported: ["code"],
        grant_types_supported: ["authorization_code", "refresh_token"],
        code_challenge_methods_supported: ["S256"],
        token_endpoint_auth_methods_supported: ["none", "client_secret_basic"],
        scopes_supported: ["mcp:tools", "offline_access"],
      });

    if (req.method === "POST" && u.pathname === "/register") {
      if (!opts.dcr) return json(res, 404, { error: "not_found" });
      const meta = JSON.parse(body || "{}");
      const c = { ...meta, client_id: `dyn-${rand()}`, client_id_issued_at: 1700000000 };

      clients.set(c.client_id, c);

      return json(res, 201, c);
    }

    if (req.method === "GET" && u.pathname === "/authorize") {
      const q = u.searchParams;
      const c = clients.get(q.get("client_id"));
      const redirect = q.get("redirect_uri") || "";

      if (!c || (c.redirect_uris && !c.redirect_uris.includes(redirect)) || q.get("code_challenge_method") !== "S256" || !q.get("code_challenge"))
        return json(res, 400, { error: "invalid_request" });
      const code = `code-${rand()}`;

      codes.set(code, { client_id: c.client_id, redirect_uri: redirect, challenge: q.get("code_challenge"), scope: q.get("scope") || "" });
      const to = new URL(redirect);

      to.searchParams.set("code", code);
      if (q.get("state") !== null) to.searchParams.set("state", q.get("state"));
      res.writeHead(302, { location: String(to) });

      return res.end();
    }

    if (req.method === "POST" && u.pathname === "/token") {
      const form = new URLSearchParams(body);
      const c = clientOf(req, form);

      if (!c) return json(res, 401, { error: "invalid_client" });

      if (form.get("grant_type") === "authorization_code") {
        const g = codes.get(form.get("code"));

        codes.delete(form.get("code"));
        const verifier = form.get("code_verifier") || "";

        if (!g || g.client_id !== c.client_id || g.redirect_uri !== form.get("redirect_uri") || b64url(createHash("sha256").update(verifier).digest()) !== g.challenge)
          return json(res, 400, { error: "invalid_grant" });

        return json(res, 200, issue(c.client_id, g.scope));
      }

      if (form.get("grant_type") === "refresh_token") {
        const r = form.get("refresh_token");

        if (opts.failRefresh || refresh.get(r) !== c.client_id) return json(res, 400, { error: "invalid_grant", error_description: "refresh token revoked" });
        refresh.delete(r);

        return json(res, 200, issue(c.client_id, form.get("scope") || ""));
      }

      return json(res, 400, { error: "unsupported_grant_type" });
    }

    // streamable HTTP
    if (u.pathname === "/mcp" || u.pathname === "/open") {
      const who = bearer(req);

      if (u.pathname === "/mcp" && !who) return unauthorized(res, "/mcp");

      if (req.method === "GET") return json(res, 405, { error: "no stream" });

      if (req.method === "DELETE") return json(res, 200, {});
      const msg = JSON.parse(body || "{}");

      if (msg.id === undefined) {
        res.writeHead(202);

        return res.end();
      }

      return json(res, 200, rpc(msg, who), msg.method === "initialize" ? { "mcp-session-id": `s-${rand()}` } : {});
    }

    // legacy SSE: the stream, then messages POSTed to the endpoint it names
    if (req.method === "GET" && u.pathname === "/sse") {
      if (!bearer(req)) return unauthorized(res, "/sse");
      const id = rand();

      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      res.write(`event: endpoint\ndata: /messages?sessionId=${id}\n\n`);
      sse.set(id, res);
      req.on("close", () => sse.delete(id));

      return;
    }

    if (req.method === "POST" && u.pathname === "/messages") {
      const who = bearer(req);

      if (!who) return unauthorized(res, "/sse");
      const stream = sse.get(u.searchParams.get("sessionId"));

      if (!stream) return json(res, 404, { error: "no session" });
      const msg = JSON.parse(body || "{}");

      res.writeHead(202);
      res.end();
      if (msg.id !== undefined) stream.write(`event: message\ndata: ${JSON.stringify(rpc(msg, who))}\n\n`);

      return;
    }

    json(res, 404, { error: "not_found" });
  }

  const server = createServer((req, res) => handle(req, res).catch((e) => json(res, 500, { error: String(e) })));

  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${server.address().port}`;

  return {
    base,
    opts,
    log,
    clients,
    /** Every access token stops working (as if expired). */
    expireAccess: () => access.clear(),
    close: () => {
      for (const s of sse.values()) s.end();
      server.closeAllConnections?.();

      return new Promise((r) => server.close(r));
    },
  };
}
