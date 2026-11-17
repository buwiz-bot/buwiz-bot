// Plugins: an agent install stays off until the user enables it, apps
// can't call a switched-off tool, and manifest names follow the grammar.
//
// The row the agent's mcp_install leaves (switched off, waiting for the
// user) is written to the data directory before the broker starts;
// everything after that goes over HTTP.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { FIXTURE, postJson, request, seedDb, startServer, tempDir, waitFor } from "./helpers.mjs";

const dataDir = tempDir("mcp");
const mark = join(dataDir, "fixture-spawned");

let db;
const rowOf = (id) => db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(id);
const listed = async (id) => (await request("GET", "/mcp/servers")).json.servers.find((s) => s.id === id);

let server;
let agentId = "";

before(async () => {
  // the row the agent's mcp_install leaves: switched off, waiting for the user
  db = await seedDb(dataDir);
  db.prepare("INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at, agent_pending) VALUES(?,?,?,?,?,?,?,?)").run(
    "agent-fixture", "Agent Fixture", process.execPath, JSON.stringify([FIXTURE]), JSON.stringify({ FIXTURE_MARK: mark }), 0, Date.now(), 1,
  );
  agentId = "agent-fixture";
  server = await startServer(dataDir);
});

after(async () => {
  db?.close();
  await server?.stop();
});

test("a server the agent installed stays off and unspawned", async () => {
  const row = rowOf(agentId);
  assert.equal(row.enabled, 0);
  assert.equal(row.agent_pending, 1);
  assert.equal(existsSync(mark), false, "the command was spawned");
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

// a stdio server whose process died starts again on its next use

test("a stdio server whose process died starts again on its next use", async () => {
  const pids = () => readFileSync(mark, "utf8").split("\n").filter(Boolean).map(Number);
  const pid = pids().at(-1);
  const gone = () => {
    try {
      process.kill(pid, 0);

      return false;
    } catch {
      return true;
    }
  };

  process.kill(pid, "SIGKILL");
  assert.ok(await waitFor(async () => gone()), "the server process did not die");
  const call = await postJson("/mcp/call", { server: agentId, tool: "echo", args: { again: true } });

  assert.deepEqual(call.json, { ok: true, result: { tool: "echo", args: { again: true } } });
  assert.notEqual(pids().at(-1), pid, "a new process answered");
});
