// The brain's MCP servers (Rust broker plan 4.4): the tools the agent sees
// (toolList), its tool calls (execTool), a routine's check, and a remote
// server whose answer reads as signed out, in this process on a temp data
// dir. With BUWIZ_BROKER_CMD and BUWIZ_BROKER_MCP_HOST=1 the same modules reach
// the servers through `buwiz-broker helper` over the line, as the live
// broker hands Node its line (BUWIZ_V1_LINE, BUWIZ_LINE_HOSTS=mcp), and the
// servers run under buwiz-broker, not this process.
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { oauthServer } from "./fixtures/oauth-server.mjs";
import { HUB, PORT, ROOT, env, tempDir } from "./helpers.mjs";

// Node-only under the hub (BUWIZ_BROKER_HUB=1): tools.mjs and routines.mjs in this process; with the hub the Rust ports run them (MAGT-01…04)
const NODE_ONLY = { skip: HUB && "Node-only: tools.mjs and routines.mjs in this process; with the hub the Rust ports run them (MAGT-01…04)" };

const dataDir = tempDir("mcp-agent");
const mark = join(dataDir, "apps-spawned");
const offMark = join(dataDir, "off-spawned");
const APPS = join(ROOT, "test", "fixtures", "mcp-apps-server.mjs");
// (not under the hub, where every case here is skipped: no helper to start)
const RUST = !HUB && process.env.BUWIZ_BROKER_CMD && env("BUWIZ_BROKER_MCP_HOST") === "1" ? process.env.BUWIZ_BROKER_CMD.trim().split(/\s+/)[0] : "";

process.env.BUWIZ_BROKER_DATA_DIR = dataDir;
process.env.MOCK = "1";

// the line, before any module reads it (v1/line-env.mjs)
let helper = null;

if (RUST) {
  const port = PORT + 30;

  mkdirSync(join(dataDir, "generated"), { recursive: true });
  const token = `line-${process.pid}-${Date.now()}`;

  helper = spawn(RUST, ["helper", "--db", join(dataDir, "generated", "app.db"), "--port", String(port)], {
    cwd: ROOT,
    env: { ...process.env, BUWIZ_LINE_TOKEN: token, BUWIZ_PUBLIC_PORT: String(PORT) },
    stdio: ["ignore", "pipe", "inherit"],
  });
  await new Promise((resolve, reject) => {
    let out = "";

    helper.stdout.on("data", (d) => {
      out += d;
      if (/helper on 127\.0\.0\.1:\d+/.test(out)) resolve();
    });
    helper.on("exit", (code) => reject(new Error(`buwiz-broker helper exited (${code})`)));
  });
  Object.assign(process.env, { BUWIZ_V1_LINE: `http://127.0.0.1:${port}`, BUWIZ_V1_LINE_TOKEN: token, BUWIZ_LINE_HOSTS: "mcp" });
}

const { db, activeThread } = await import("../src/state.mjs");
const { toolList, execTool } = await import("../src/tools.mjs");
const { invalidate, statusOf } = await import("../src/mcp.mjs");
const { stopLineEvents } = await import("../src/mcp-line.mjs");
const { plugins } = await import("../src/v1/broker.mjs");
const { createRoutine, runRoutine } = await import("../src/routines.mjs");

let fx;

const pids = (file) => (existsSync(file) ? readFileSync(file, "utf8").split("\n").filter(Boolean).map(Number) : []);
const parentOf = (pid) => Number(execFileSync("ps", ["-o", "ppid=", "-p", String(pid)], { encoding: "utf8" }).trim());
const call = (name, input = {}) => execTool({ id: 1 }, activeThread(), { name, input });

function addRow(id, fields) {
  const r = { command: "", args: "[]", env: "{}", enabled: 1, url: null, headers: null, ...fields };

  db.prepare("INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at, url, headers) VALUES(?,?,?,?,?,?,?,?,?)").run(
    id, id, r.command, r.args, r.env, r.enabled, Date.now(), r.url, r.headers,
  );
}

before(async () => {
  fx = await oauthServer();
  addRow("apps", { command: process.execPath, args: JSON.stringify([APPS]), env: JSON.stringify({ FIXTURE_MARK: mark }) });
  addRow("offapps", { command: process.execPath, args: JSON.stringify([APPS]), env: JSON.stringify({ FIXTURE_MARK: offMark }), enabled: 0 });
});

after(async () => {
  for (const id of ["apps", "offapps", "remote"]) invalidate(id);
  await plugins.stopAll();
  stopLineEvents();
  helper?.kill("SIGTERM");
  await fx?.close();
});

test("the agent sees an enabled server's tools under mcp_<server>_<tool>; a switched-off server is never started", NODE_ONLY, async () => {
  const names = (await toolList(activeThread())).map((t) => t.name);

  assert.ok(names.includes("mcp_apps_echo") && names.includes("mcp_apps_resolve"), names.filter((n) => n.startsWith("mcp_")).join(" "));
  assert.ok(!names.some((n) => n.startsWith("mcp_offapps_")));
  assert.deepEqual(pids(offMark), []);
});

test("the agent's call reaches the server and comes back as text; under buwiz-broker, the server is buwiz-broker's child", NODE_ONLY, async () => {
  const r = await call("mcp_apps_echo", { a: 1 });

  assert.equal(r.is_error, false, r.content);
  assert.deepEqual(JSON.parse(r.content), { tool: "echo", args: { a: 1 } });
  const pid = pids(mark).at(-1);

  assert.ok(pid, "the server was started");
  assert.equal(parentOf(pid), RUST ? helper.pid : process.pid);
  const off = await call("mcp_offapps_echo", {});

  assert.equal(off.is_error, true);
  assert.match(off.content, /switched off/);
  const none = await call("mcp_nobody_echo", {});

  assert.equal(none.is_error, true);
  assert.match(none.content, /not configured/);
});

test("a routine's check calls the server; nothing new means no turn", NODE_ONLY, async () => {
  const r = await createRoutine({ name: "Echo check", thread: activeThread().id, every_s: 3600, prompt: "Report new items.", enabled: false, check: { server: "apps", tool: "echo", args: { n: 1 }, ids: "nothing[].id" } });

  await runRoutine(r.id);
  const row = db.prepare("SELECT last_error, last_check, last_run FROM routines WHERE id=?").get(r.id);

  assert.equal(row.last_error, null);
  assert.ok(row.last_check && row.last_check === row.last_run, "the check ran");
});

test("a remote server's answer that reads as signed out marks it; the agent's next call asks for sign-in", NODE_ONLY, async () => {
  addRow("remote", { url: `${fx.base}/open`, headers: JSON.stringify({ "x-api-key": "k" }) });
  const ok = await call("mcp_remote_whoami", {});

  assert.equal(ok.is_error, false, ok.content);
  const denied = await call("mcp_remote_denied", {});

  assert.equal(denied.is_error, true);
  assert.match(denied.content, /401 Unauthorized/);
  assert.equal(statusOf("remote").status, "needs-auth");
  const next = await call("mcp_remote_whoami", {});

  assert.equal(next.is_error, true);
  assert.match(next.content, /remote needs sign-in/);
});
