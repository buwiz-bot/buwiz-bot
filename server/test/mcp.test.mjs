// Plugins: an agent install stays off until the user enables it, apps
// can't call a switched-off tool, and manifest names follow the grammar.
//
// The agent's mcp_install runs in this process (the tools module, on the
// test's data directory) before the server starts on the same directory;
// everything after that goes over HTTP.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { FIXTURE, PORT, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const dataDir = tempDir("mcp");
const mark = join(dataDir, "fixture-spawned");

process.env.GROK_DATA_DIR = dataDir;

process.env.MOCK = "1";

process.env.PORT = String(PORT);

const { toolMcpAdmin } = await import("../server/tools.mjs");
const { db } = await import("../server/state.mjs");

const rowOf = (id) => db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(id);
const listed = async (id) => (await request("GET", "/mcp/servers")).json.servers.find((s) => s.id === id);

let server;
let agentId = "";

before(async () => {
  const r = await toolMcpAdmin(null, "mcp_install", {
    name: "Agent Fixture",
    command: process.execPath,
    args: [FIXTURE],
    env: { FIXTURE_MARK: mark },
  });

  assert.ok(!r.is_error, r.content);
  agentId = /server ([a-z0-9-]+)/.exec(r.content)?.[1] || "";
  server = await startServer(dataDir);
});

after(async () => {
  await server?.stop();
});

test("mcp_install from the agent stores the server off and unspawned", async () => {
  assert.equal(agentId, "agent-fixture");
  const row = rowOf(agentId);
  assert.equal(row.enabled, 0);
  assert.equal(row.agent_pending, 1);
  assert.equal(existsSync(mark), false, "the command was spawned");
  const refused = await toolMcpAdmin(null, "mcp_enable", { id: agentId });
  assert.equal(refused.is_error, true);
  assert.match(refused.content, /Plugins/);
  assert.equal(rowOf(agentId).enabled, 0);
  const s = await listed(agentId);
  assert.equal(s.enabled, false);
  assert.equal(s.status, "off");
  const tools = await request("GET", `/mcp/tools?id=${agentId}`);
  assert.equal(tools.json.status, "off");
  const call = await postJson("/mcp/call", { server: agentId, tool: "echo", args: {} });
  assert.equal(call.json.ok, false);
  assert.equal(existsSync(mark), false, "the command was spawned");
});

test("the user's enable connects it", async () => {
  const r = await postJson("/mcp/server/state", { id: agentId, enabled: true });
  assert.equal(r.json.ok, true);

  const s = await waitFor(async () => {
    const x = await listed(agentId);

    return x?.status === "connected" && x;
  });

  assert.ok(s, "never connected");
  assert.equal(s.tools, 2);
  assert.ok(existsSync(mark));
  assert.equal(rowOf(agentId).agent_pending, null);
  const call = await postJson("/mcp/call", { server: agentId, tool: "echo", args: { a: 1 } });
  assert.deepEqual(call.json, { ok: true, result: { tool: "echo", args: { a: 1 } } });
});

test("/mcp/call refuses a tool the user switched off", async () => {
  const on = await postJson("/mcp/call", { server: agentId, tool: "secret", args: {} });
  assert.equal(on.json.ok, true);
  const off = await postJson("/mcp/tools/state", { id: agentId, tool: "secret", enabled: false });
  assert.equal(off.json.ok, true);
  const r = await postJson("/mcp/call", { server: agentId, tool: "secret", args: {} });
  assert.equal(r.json.ok, false);
  assert.match(r.json.error, /secret is switched off/);
  const echo = await postJson("/mcp/call", { server: agentId, tool: "echo", args: {} });
  assert.equal(echo.json.ok, true);
});

test("an install from the UI connects at once", async () => {
  const r = await postJson("/mcp/servers", { name: "UI Fixture", command: process.execPath, args: [FIXTURE] });
  assert.equal(r.json.ok, true);
  assert.equal(r.json.id, "ui-fixture");
  assert.equal(r.json.status, "connected");
  assert.equal(rowOf("ui-fixture").agent_pending, null);
});

test("manifest commands and scopes outside the grammar are dropped", async () => {
  const m = await request("GET", "/mcp/manifest");
  const s = m.json.servers.find((x) => x.id === "ui-fixture");
  assert.deepEqual(s.commands.map((c) => c.name), ["good"]);
  assert.deepEqual(s.scopes.map((c) => c.name), ["item"]);
  assert.match(server.log(), /mcp ui-fixture: manifest scopes entry "bad\.scope" dropped/);
});
