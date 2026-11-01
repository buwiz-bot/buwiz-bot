// The /mcp routes Rox calls that no other test covers (Rust broker plan
// 4.3): resolve, detach, attachment rename, open and its one-time pass,
// apps, manifest, tools, and a server switched off and deleted. Written
// against Node; the same file runs through buwiz-broker, which answers them
// itself when it hosts MCP (BUWIZ_BROKER_MCP_HOST=1).
//
// `open` on the broker's PATH is a stub that records what it was handed, so
// no link reaches the system.
import assert from "node:assert/strict";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { oauthServer } from "./fixtures/oauth-server.mjs";
import { PORT, ROOT, env, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const dataDir = tempDir("mcp-routes");
const bin = tempDir("mcp-routes-bin");
const opened = join(bin, "opened");
const mark = join(dataDir, "apps-spawned");
const offMark = join(dataDir, "off-spawned");
const APPS = join(ROOT, "test", "fixtures", "mcp-apps-server.mjs");

process.env.BUWIZ_BROKER_DATA_DIR = dataDir;
process.env.MOCK = "1";
process.env.PORT = String(PORT);
writeFileSync(join(bin, "open"), `#!/bin/sh\necho "$@" >> '${opened}'\n`);
chmodSync(join(bin, "open"), 0o755);
process.env.PATH = `${bin}:${process.env.PATH}`;

const { db } = await import("../src/state.mjs");

// buwiz-broker hosting MCP answers these routes itself, and flags a remote
// server's sign-in from any call (Node did only in the model's calls)
const MCP_IN_RUST = !!process.env.BUWIZ_BROKER_CMD && env("BUWIZ_BROKER_MCP_HOST") === "1";

let server;
let fx;
let thread = "";

const pids = (file) => (existsSync(file) ? readFileSync(file, "utf8").split("\n").filter(Boolean).map(Number) : []);
const tabs = () => (existsSync(opened) ? readFileSync(opened, "utf8").split("\n").filter(Boolean) : []);
const listed = async (id) => (await request("GET", "/mcp/servers")).json.servers.find((s) => s.id === id);

function addRow(id, fields) {
  const r = { command: "", args: "[]", env: "{}", enabled: 1, url: null, auth: null, ...fields };

  db.prepare("INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at, url, auth) VALUES(?,?,?,?,?,?,?,?,?)").run(
    id, id, r.command, r.args, r.env, r.enabled, Date.now(), r.url, r.auth,
  );
}

function addAttachment(id, server, token, threadId) {
  db.prepare("INSERT INTO attachments(id,server,token,scope,title,open,key,details,thread,bot,turn,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)").run(
    id, server, token, "item", `Item ${id}`, "buwiz://app/apps/hello", "", null, threadId, null, `turn-${id}`, Date.now(),
  );
}

before(async () => {
  fx = await oauthServer();
  addRow("apps", { command: process.execPath, args: JSON.stringify([APPS]), env: JSON.stringify({ FIXTURE_MARK: mark }) });
  addRow("offapps", { command: process.execPath, args: JSON.stringify([APPS]), env: JSON.stringify({ FIXTURE_MARK: offMark }), enabled: 0 });
  server = await startServer(dataDir);
  thread = (await postJson("/thread/new", {})).json.id;
  assert.ok(thread, "a chat");
});

after(async () => {
  await server?.stop();
  await fx?.close();
});

test("/mcp/apps and /mcp/manifest list what enabled servers ship; a switched-off one is not started", async () => {
  const apps = (await request("GET", "/mcp/apps")).json.apps;

  assert.deepEqual(apps.filter((a) => a.server === "apps"), [{ server: "apps", app: "hello", uri: "nsh://apps/hello", title: "Hello", description: "A tiny app" }]);
  const m = (await request("GET", "/mcp/manifest")).json.servers.find((s) => s.id === "apps");

  assert.deepEqual(m, {
    id: "apps",
    title: "Apps Fixture",
    apps: [{ id: "hello", title: "Hello", description: "A tiny app" }],
    commands: [{ name: "go", description: "Open the hello app", resolve: "resolve" }],
    scopes: [{ name: "item", description: "An item", attach: true, resolve: "resolve" }],
  });
  assert.deepEqual(pids(offMark), [], "the switched-off server was started");
  assert.deepEqual((await request("GET", "/mcp/tools?id=offapps")).json, { tools: [], status: "off" });
  assert.equal((await listed("offapps")).status, "off");
});

test("/mcp/tools lists a server's tools with their switches; /mcp/tools/state flips one; an unknown server is 404", async () => {
  const t = await request("GET", "/mcp/tools?id=apps");

  assert.equal(t.json.status, "connected");
  assert.deepEqual(t.json.tools.map((x) => [x.name, x.enabled]), [["echo", true], ["resolve", true]]);
  assert.equal((await postJson("/mcp/tools/state", { id: "apps", tool: "echo", enabled: false })).json.ok, true);
  assert.deepEqual((await request("GET", "/mcp/tools?id=apps")).json.tools.map((x) => [x.name, x.enabled]), [["echo", false], ["resolve", true]]);
  const refused = await postJson("/mcp/call", { server: "apps", tool: "echo", args: {} });

  assert.deepEqual(refused.json, { ok: false, error: "echo is switched off for apps; turn it on in Plugins to let apps call it" });
  await postJson("/mcp/tools/state", { id: "apps", tool: "echo", enabled: true });
  assert.deepEqual((await postJson("/mcp/call", { server: "apps", tool: "echo", args: { a: 1 } })).json, { ok: true, result: { tool: "echo", args: { a: 1 } } });
  const none = await request("GET", "/mcp/tools?id=nobody");

  assert.equal(none.status, 404);
  assert.deepEqual(none.json, { error: "no such server" });
  assert.equal((await postJson("/mcp/tools/state", { id: "nobody", tool: "echo", enabled: false })).status, 400);
  assert.deepEqual((await postJson("/mcp/call", { server: "offapps", tool: "echo", args: {} })).json, { ok: false, error: "no such server" });
});

test("/mcp/resolve runs the tool the manifest names, with the chat's attached tokens of that server", async () => {
  const c = await postJson("/mcp/resolve", { server: "apps", name: "item", args: "", complete: true });

  assert.deepEqual(c.json, { choices: [{ label: "One", value: "one" }] });
  addAttachment("a1", "apps", "@apps:item:one", thread);
  addAttachment("a2", "other", "@other:item:x", thread);
  const r = await postJson("/mcp/resolve", { server: "apps", name: "go", args: "two", thread });

  assert.equal(r.json.ok, true);
  assert.deepEqual(r.json.details.got, { name: "go", args: "two", complete: false, context: { attached: ["@apps:item:one"] } });
  const plain = await postJson("/mcp/resolve", { server: "apps", name: "go", args: "two" });

  assert.deepEqual(plain.json.details.got, { name: "go", args: "two", complete: false });
  const lost = await postJson("/mcp/resolve", { server: "apps", name: "go", args: "two", thread: "no-such-chat" });

  assert.deepEqual(lost.json.details.got, { name: "go", args: "two", complete: false }, "a chat that doesn't exist has nothing attached");
  assert.deepEqual((await postJson("/mcp/resolve", { server: "apps", name: "nope", args: "" })).json, { ok: false, error: "apps has no /nope" });
  assert.deepEqual((await postJson("/mcp/resolve", { server: "nobody", name: "go", args: "" })).json, { ok: false, error: "no such server" });
});

test("/mcp/attachment/rename names a token everywhere, \"\" restores it; /mcp/detach takes either id form", async () => {
  addAttachment("a3", "apps", "@apps:item:three", thread);
  assert.equal((await postJson("/mcp/attachment/rename", { token: "@apps:item:three", title: "  Three  " })).json.ok, true);
  assert.equal(db.prepare("SELECT title FROM attachment_names WHERE token=?").get("@apps:item:three").title, "Three");
  assert.equal((await postJson("/mcp/attachment/rename", { token: "@apps:item:three", title: "" })).json.ok, true);
  assert.equal(db.prepare("SELECT title FROM attachment_names WHERE token=?").get("@apps:item:three"), undefined);
  const noToken = await postJson("/mcp/attachment/rename", { token: " ", title: "x" });

  assert.equal(noToken.status, 400);
  assert.deepEqual(noToken.json, { ok: false, error: "rename needs a token" });
  assert.equal((await postJson("/mcp/detach", { id: "attach:a3" })).json.ok, true);
  assert.equal(db.prepare("SELECT id FROM attachments WHERE id=?").get("a3"), undefined);
  assert.equal((await postJson("/mcp/detach", { id: "a2" })).json.ok, true);
  const gone = await postJson("/mcp/detach", { id: "a2" });

  assert.equal(gone.status, 400);
  assert.deepEqual(gone.json, { ok: false, error: "no attachment a2" });
});

test("/mcp/open hands only Rox links to the system; an app link carries a one-time pass that /mcp/open/pass redeems once", async () => {
  const bad = await postJson("/mcp/open", { url: "https://example.com/" });

  assert.equal(bad.status, 400);
  assert.equal(bad.json.ok, false);
  assert.equal((await postJson("/mcp/open", { url: "buwiz://view?path=relative/x" })).status, 400);
  assert.deepEqual((await postJson("/mcp/open", { url: "buwiz://chat/new?draft=hi" })).json, { ok: true });
  assert.deepEqual((await postJson("/mcp/open", { url: "buwiz://view?path=/tmp/x.pdf" })).json, { ok: true });
  assert.deepEqual((await postJson("/mcp/open", { url: "buwiz://app/apps/hello?x=1" })).json, { ok: true });
  const handed = tabs();

  assert.equal(handed.length, 3);
  assert.deepEqual(handed.slice(0, 2), ["buwiz://chat/new?draft=hi", "buwiz://view?path=/tmp/x.pdf"]);
  const pass = /^buwiz:\/\/app\/apps\/hello\?x=1&buwiz_pass=([A-Za-z0-9_-]{22})$/.exec(handed[2])?.[1];

  assert.ok(pass, handed[2]);
  assert.deepEqual((await postJson("/mcp/open/pass", { pass })).json, { ok: true });
  assert.deepEqual((await postJson("/mcp/open/pass", { pass })).json, { ok: false }, "a pass works once");
  assert.deepEqual((await postJson("/mcp/open/pass", { pass: "never-issued" })).json, { ok: false });
});

test("switching a server off stops it, on starts it; deleting drops the row and its process", async () => {
  await request("GET", "/mcp/tools?id=apps");
  const pid = pids(mark).at(-1);
  const alive = (p) => {
    try {
      process.kill(p, 0);

      return true;
    } catch {
      return false;
    }
  };

  assert.ok(alive(pid));
  assert.equal((await postJson("/mcp/server/state", { id: "apps", enabled: false })).json.ok, true);
  assert.ok(await waitFor(async () => !alive(pid)), "switched off, still running");
  assert.equal((await listed("apps")).status, "off");
  assert.equal((await postJson("/mcp/server/state", { id: "nobody", enabled: true })).status, 400);
  assert.equal((await postJson("/mcp/server/state", { id: "apps", enabled: true })).json.ok, true);
  assert.ok(await waitFor(async () => (await listed("apps")).status === "connected"), "switched on, never connected");
  const again = pids(mark).at(-1);

  assert.notEqual(again, pid);
  assert.equal((await postJson("/mcp/server/delete", { id: "apps" })).json.ok, true);
  assert.equal(db.prepare("SELECT id FROM mcp_servers WHERE id=?").get("apps"), undefined);
  assert.ok(await waitFor(async () => !alive(again)), "deleted, still running");
  assert.equal(await listed("apps"), undefined);
});

test("a remote server whose call fails as signed out shows needs-auth", { skip: !MCP_IN_RUST && "buwiz-broker hosting MCP (BUWIZ_BROKER_MCP_HOST=1) only" }, async () => {
  addRow("remote", { url: `${fx.base}/open` });
  assert.equal((await request("GET", "/mcp/tools?id=remote")).json.status, "connected");
  const c = await postJson("/mcp/call", { server: "remote", tool: "denied", args: {} });

  assert.equal(c.json.ok, false);
  const s = await listed("remote");

  assert.equal(s.status, "needs-auth");
  assert.equal(s.err, "Sign-in expired or was rejected");
  assert.deepEqual(tabs().filter((t) => !t.startsWith("buwiz://")), [], "no browser");
});
