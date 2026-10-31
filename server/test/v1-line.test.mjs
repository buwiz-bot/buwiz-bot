// rox-broker hosting the v1 plugins (ROX_BROKER_V1_HOST=1, the Rust broker
// plan's D10): the plugin processes are rox-broker's, so a Node restart
// leaves them running, and Node, back, sees them as they are and calls
// them again. Runs only with BUWIZ_BROKER_CMD (a rox-broker); port
// V1_TEST_PORT + 1.
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { join } from "node:path";
import { after, test } from "node:test";
import { HUB, ROOT, brokerCommand, tempDir } from "./helpers.mjs";
import { FX, bed, fixtureSpec, until } from "./v1-helpers.mjs";

const RUST = !!process.env.BUWIZ_BROKER_CMD;
const PORT = Number(process.env.V1_TEST_PORT || 8799) + 1;
let broker;

after(async () => {
  if (!broker || broker.exitCode !== null) return;
  broker.kill("SIGTERM");
  await new Promise((r) => broker.once("exit", r));
});

function req(method, path, token, obj) {
  return new Promise((resolve, reject) => {
    const headers = { "X-Buwiz-Host": token, ...(obj ? { "Content-Type": "application/json" } : {}) };
    const r = httpRequest({ host: "127.0.0.1", port: PORT, method, path, headers }, (res) => {
      let text = "";

      res.setEncoding("utf8");
      res.on("data", (c) => (text += c));
      res.on("end", () => resolve({ status: res.statusCode, json: text ? JSON.parse(text) : null }));
    });

    r.on("error", reject);

    if (obj) r.write(JSON.stringify(obj));
    r.end();
  });
}

const children = (pid) => {
  try {
    return execFileSync("pgrep", ["-P", String(pid)], { encoding: "utf8" }).split("\n").filter(Boolean).map(Number);
  } catch {
    return [];
  }
};
const commandOf = (pid) => execFileSync("ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" }).trim();
const alive = (pid) => {
  try {
    process.kill(pid, 0);

    return true;
  } catch {
    return false;
  }
};

test("a Node restart leaves rox-broker's plugin running; Node, back, lists it and calls it", { skip: (!RUST && "needs BUWIZ_BROKER_CMD") || (process.env.ROX_BROKER_NO_NODE === "1" && "Node-only: restarts the Node behind rox-broker; with ROX_BROKER_NO_NODE=1 there is none") }, async () => {
  const b = bed("line");
  const data = tempDir("v1-line-data");
  const [bin, args] = brokerCommand();
  const env = { ...process.env, ROX_BROKER_V1_HOST: "1", MOCK: "1", GROK_ALLOWED_ORIGINS: "", BUWIZ_KEYSTORE: `file:${b.keysDir}`, BUWIZ_TRUST_FILE: b.trustFile };

  let log = "";

  broker = spawn(bin, [...args, "--port", String(PORT), "--data", data], { cwd: ROOT, env, stdio: ["ignore", "pipe", "pipe"] });
  broker.stdout.on("data", (d) => (log += d));
  broker.stderr.on("data", (d) => (log += d));
  const healthy = () => req("GET", "/health", "").then((r) => r.status === 200, () => false);
  const tokenFile = join(data, "host-token");

  await until(healthy, 20000, `the broker on ${PORT}:\n${log.slice(-2000)}`);
  let token = readFileSync(tokenFile, "utf8");
  const i = await req("POST", "/v1/host/plugins/inspect", token, fixtureSpec(b));
  const inst = await req("POST", "/v1/host/plugins/install", token, { ...fixtureSpec(b), manifest_sha256: i.json.manifest_sha256 });

  assert.equal(inst.json.state, "running");
  const plugin = children(broker.pid).find((p) => commandOf(p).includes("fixture/server.mjs"));
  const node = children(broker.pid).find((p) => commandOf(p).includes("app-server.mjs"));

  assert.ok(plugin && node, "the plugin and Node are both rox-broker's children");
  process.kill(node, "SIGKILL");
  // Node writes a new host token at each start; with the hub on (D30) the
  // token is rox-broker's, written once, and Node comes back in its office role
  const isNode = (p) => {
    try {
      return commandOf(p).includes("app-server.mjs");
    } catch {
      return false;
    }
  };
  const back = HUB
    ? () => children(broker.pid).some((p) => p !== node && isNode(p)) && readFileSync(tokenFile, "utf8") === token
    : () => readFileSync(tokenFile, "utf8") !== token;

  await until(async () => !alive(node) && (await healthy()) && back(), 20000, "Node to come back");
  token = readFileSync(tokenFile, "utf8");
  assert.ok(alive(plugin), "the plugin process outlived Node");
  const listed = (await req("GET", "/v1/host/plugins", token)).json.plugins.find((p) => p.id === FX);

  assert.equal(listed.state, "running");
  assert.equal(listed.generation, inst.json.generation, "not restarted: the same generation");
  const r = await req("POST", "/v1/host/resolve", token, { plugin: FX, kind: "scope", name: "item", value: "acme-1" });

  assert.equal(r.json.result, "token");
  // stopping rox-broker stops its plugins
  broker.kill("SIGTERM");
  await new Promise((done) => broker.once("exit", done));
  await until(() => !alive(plugin), 10000, "the plugin to stop");
});
