// Remote MCP servers and their OAuth sign-in (Rust broker plan 4.2): the
// flow as the user runs it, against a fake server and authorization server
// (fixtures/oauth-server.mjs). Written against Node first; the same cases
// then run through rox-broker (BUWIZ_BROKER_CMD, ROX_BROKER_MCP_HOST=1).
//
// No browser opens: `open` on the broker's PATH is a stub that records its
// arguments, and every sign-in here asks for the authorize URL instead
// (open:false). With OAUTH_LOG set, the fixture's requests are written
// there, so a Node run and a Rust run can be compared.
import assert from "node:assert/strict";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { STATIC, oauthServer } from "./fixtures/oauth-server.mjs";
import { PORT, postJson, request, startServer, tempDir } from "./helpers.mjs";

const dataDir = tempDir("mcp-oauth");
const bin = tempDir("mcp-oauth-bin");
const opened = join(bin, "opened");

process.env.GROK_DATA_DIR = dataDir;
process.env.MOCK = "1";
process.env.PORT = String(PORT);
writeFileSync(join(bin, "open"), `#!/bin/sh\necho "$@" >> '${opened}'\n`);
chmodSync(join(bin, "open"), 0o755);
process.env.PATH = `${bin}:${process.env.PATH}`;

const { db } = await import("../src/state.mjs");

let server;
let fx;

const rowOf = (id) => db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(id);
const authOf = (id) => JSON.parse(rowOf(id)?.auth || "{}") || {};
const callback = (id) => `http://127.0.0.1:${PORT}/oauth/callback/${id}`;

function addRow(id, { path = "/mcp", transport = null, headers = null, oauthClient = null, auth = null } = {}) {
  db.prepare(
    "INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at, url, headers, oauth_client, transport, auth) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
  ).run(id, id, "", "[]", "{}", 1, Date.now(), `${fx.base}${path}`, headers && JSON.stringify(headers), oauthClient && JSON.stringify(oauthClient), transport, auth && JSON.stringify(auth));
}

const begin = (id) => postJson("/mcp/auth", { id, open: false });

/** The browser's part: the authorize URL, its redirect, the broker's callback. */
async function approve(authorizeUrl) {
  const r = await fetch(authorizeUrl, { redirect: "manual" });

  assert.equal(r.status, 302, `the authorization server refused: ${await r.text()}`);
  const to = new URL(r.headers.get("location"));

  return request("GET", to.pathname + to.search);
}

async function signIn(id) {
  const b = await begin(id);

  assert.equal(b.json.ok, true, JSON.stringify(b.json));
  assert.equal(b.json.redirected, true);
  const cb = await approve(b.json.authorizeUrl);

  assert.equal(cb.status, 200);
  assert.match(cb.text, /Connected/);

  return new URL(b.json.authorizeUrl);
}

const tools = (id) => request("GET", `/mcp/tools?id=${id}`);
const listed = async (id) => (await request("GET", "/mcp/servers")).json.servers.find((s) => s.id === id);
const call = (id) => postJson("/mcp/call", { server: id, tool: "whoami", args: {} });
const since = (n, path) => fx.log.slice(n).filter((r) => r.path === path);
const tabs = () => (existsSync(opened) ? readFileSync(opened, "utf8").split("\n").filter(Boolean) : []);

before(async () => {
  fx = await oauthServer();
  server = await startServer(dataDir);
});

after(async () => {
  await server?.stop();
  if (process.env.OAUTH_LOG) writeFileSync(process.env.OAUTH_LOG, JSON.stringify(fx.log, null, 1));
  await fx?.close();
});

test("dynamic registration: sign-in end to end, the redirect on the broker's port, tokens stamped and used", async () => {
  addRow("dcr");
  const n = fx.log.length;
  const url = await signIn("dcr");

  assert.equal(url.searchParams.get("redirect_uri"), callback("dcr"));
  assert.equal(url.searchParams.get("code_challenge_method"), "S256");
  assert.match(url.searchParams.get("client_id"), /^dyn-/);
  assert.equal(url.searchParams.get("scope"), "mcp:tools offline_access");
  assert.equal(url.searchParams.get("resource"), `${fx.base}/mcp`);
  assert.equal(since(n, "/register").length, 1);
  const reg = JSON.parse(since(n, "/register")[0].body);

  assert.deepEqual(reg.redirect_uris, [callback("dcr")]);
  assert.equal(reg.client_name, "Buwiz Bot");
  assert.equal(reg.token_endpoint_auth_method, "none");
  const a = authOf("dcr");

  assert.equal(a.client_info.client_id, url.searchParams.get("client_id"));
  assert.ok(a.client_info.issuer.startsWith(fx.base), a.client_info.issuer);
  assert.ok(a.tokens.access_token && a.tokens.refresh_token);
  assert.ok(a.tokens.issuer.startsWith(fx.base));
  assert.equal(a.state, url.searchParams.get("state"));
  const t = await tools("dcr");

  assert.equal(t.json.status, "connected");
  assert.deepEqual(t.json.tools.map((x) => x.name), ["whoami", "denied"]);
  assert.equal((await listed("dcr")).status, "connected");
  const c = await call("dcr");

  assert.deepEqual(c.json, { ok: true, result: { client: a.client_info.client_id } });
  assert.deepEqual(tabs(), []);
});

test("an expired access token is refreshed on the 401, the rotated refresh token saved, no browser", async () => {
  const before = authOf("dcr").tokens;
  const n = fx.log.length;

  fx.expireAccess();
  const c = await call("dcr");

  assert.equal(c.json.ok, true, JSON.stringify(c.json));
  const grants = since(n, "/token").map((r) => new URLSearchParams(r.body).get("grant_type"));

  assert.deepEqual(grants, ["refresh_token"]);
  const now = authOf("dcr").tokens;

  assert.notEqual(now.access_token, before.access_token);
  assert.notEqual(now.refresh_token, before.refresh_token);
  assert.deepEqual(tabs(), []);
});

test("a refused refresh fails the call and opens no browser", async () => {
  const n = fx.log.length;

  fx.opts.failRefresh = true;
  fx.expireAccess();
  try {
    const c = await call("dcr");

    assert.equal(c.json.ok, false);
    assert.match(c.json.error, /refresh token revoked|invalid_grant/i);
  } finally {
    fx.opts.failRefresh = false;
  }

  assert.ok(since(n, "/token").length >= 1);
  assert.equal(since(n, "/authorize").length, 0);
  assert.deepEqual(tabs(), []);
});

test("a pre-registered client: no registration, its extra authorize parameters, Basic at the token endpoint", async () => {
  addRow("stat", { oauthClient: { ...STATIC, scope: "mcp:tools", extra: { access_type: "offline", prompt: "consent" } } });
  const n = fx.log.length;
  const url = await signIn("stat");

  assert.equal(url.searchParams.get("client_id"), STATIC.client_id);
  assert.equal(url.searchParams.get("access_type"), "offline");
  assert.equal(url.searchParams.get("prompt"), "consent");
  assert.equal(url.searchParams.get("scope"), "mcp:tools");
  assert.equal(since(n, "/register").length, 0);
  const token = since(n, "/token")[0];

  assert.equal(token.headers.authorization, `Basic ${Buffer.from(`${STATIC.client_id}:${STATIC.client_secret}`).toString("base64")}`);
  assert.equal(new URLSearchParams(token.body).get("client_secret"), null);
  assert.equal(authOf("stat").client_info, undefined);
  assert.deepEqual((await call("stat")).json, { ok: true, result: { client: STATIC.client_id } });
});

test("no registration endpoint and no client of our own: asks for a Client ID and remembers it", async () => {
  addRow("nodcr");
  fx.opts.dcr = false;
  try {
    const b = await begin("nodcr");

    assert.equal(b.status, 400);
    assert.equal(b.json.ok, false);
    assert.match(b.json.error, /add your own OAuth Client ID in Setup/);
  } finally {
    fx.opts.dcr = true;
  }

  assert.equal(authOf("nodcr").no_dcr, true);
  assert.deepEqual(tabs(), []);
});

test("the callback refuses a wrong state, an unknown server, an error and a bad code", async () => {
  addRow("cb");
  let b = await begin("cb");

  assert.equal(b.json.redirected, true);
  const wrong = await request("GET", "/oauth/callback/cb?code=x&state=not-the-state");

  assert.equal(wrong.status, 400);
  assert.match(wrong.text, /Authentication failed/);
  const unknown = await request("GET", "/oauth/callback/nobody?code=x");

  assert.equal(unknown.status, 400);
  const denied = await request("GET", "/oauth/callback/cb?error=access_denied");

  assert.equal(denied.status, 200);
  assert.match(denied.text, /Authentication failed/);
  // the flow ended: a code now has nothing pending
  const late = await request("GET", `/oauth/callback/cb?code=x&state=${authOf("cb").state}`);

  assert.equal(late.status, 400);
  b = await begin("cb");
  const bad = await request("GET", `/oauth/callback/cb?code=bogus&state=${authOf("cb").state}`);

  assert.equal(bad.status, 200);
  assert.match(bad.text, /Authentication failed/);
  assert.match(server.log(), /mcp cb: token exchange failed/);
  assert.equal(authOf("cb").tokens, undefined);
});

test("a server that answers without a token but publishes OAuth metadata needs sign-in; a credential header counts as signed in", async () => {
  addRow("open", { path: "/open" });
  assert.equal((await tools("open")).json.status, "connected");
  const s = await listed("open");

  assert.equal(s.status, "needs-auth");
  assert.equal(s.err, "Sign-in required");
  addRow("openkey", { path: "/open", headers: { "x-api-key": "k" } });
  await tools("openkey");
  assert.equal((await listed("openkey")).status, "connected");
});

test("signed-in tokens win over a static Authorization header", async () => {
  addRow("hdr", { headers: { Authorization: "Bearer static-header" } });
  assert.equal((await tools("hdr")).json.status, "error");
  await signIn("hdr");
  const n = fx.log.length;
  const c = await call("hdr");

  assert.equal(c.json.ok, true, JSON.stringify(c.json));
  const sent = since(n, "/mcp").map((r) => r.headers.authorization);

  assert.ok(sent.length > 0);
  assert.ok(sent.every((h) => h === `Bearer ${authOf("hdr").tokens.access_token}`), JSON.stringify(sent));
});

test("the legacy SSE transport signs in and calls the same way", async () => {
  addRow("sse", { path: "/sse", transport: "sse" });
  const url = await signIn("sse");

  assert.equal(url.searchParams.get("resource"), `${fx.base}/sse`);
  assert.equal((await tools("sse")).json.status, "connected");
  const c = await call("sse");

  assert.deepEqual(c.json, { ok: true, result: { client: authOf("sse").client_info.client_id } });
});

test("disconnecting an account drops its tokens: the server refuses again", async () => {
  const r = await postJson("/mcp/account", { id: "stat", disconnect: true });

  assert.equal(r.json.ok, true);
  assert.equal(rowOf("stat").auth, null);
  assert.equal((await tools("stat")).json.status, "error");
  assert.equal((await call("stat")).json.ok, false);
});

test("client information stamped for another authorization server registers again", async () => {
  addRow("iss", { auth: { client_info: { client_id: "dyn-elsewhere", redirect_uris: [callback("iss")], issuer: "http://elsewhere.invalid/" } } });
  const n = fx.log.length;
  const url = await signIn("iss");

  assert.equal(since(n, "/register").length, 1);
  assert.notEqual(url.searchParams.get("client_id"), "dyn-elsewhere");
});

test("a client registered for another redirect (an old port) registers again", async () => {
  addRow("oldport", { auth: { client_info: { client_id: "dyn-oldport", redirect_uris: ["http://127.0.0.1:60031/oauth/callback/oldport"] } } });
  const n = fx.log.length;
  const url = await signIn("oldport");

  assert.equal(since(n, "/register").length, 1);
  assert.notEqual(url.searchParams.get("client_id"), "dyn-oldport");
  assert.equal(url.searchParams.get("redirect_uri"), callback("oldport"));
  assert.deepEqual(tabs(), []);
});
