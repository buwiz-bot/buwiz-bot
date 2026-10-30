// rox-broker with no Node at all (Rust broker plan phase 7):
// ROX_BROKER_HUB=1 with ROX_BROKER_NO_NODE=1 starts no Node child, and
// answers what Node would have: /health, /, the CORS preflight, the 404 of a
// route nobody has. The preflight case is pinned on Node too (it ignores
// both variables); the rest needs rox-broker (ROX_BROKER_CMD).
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { after, before, test } from "node:test";
import { PORT, brokerCommand, request, serverEnv, startServer, tempDir } from "./helpers.mjs";

const RUST = !!(process.env.ROX_BROKER_CMD || "").trim();
const RUST_ONLY = { skip: !RUST && "rox-broker only (ROX_BROKER_CMD)" };
const DATA = tempDir("no-node");
let server;

before(async () => {
  server = await startServer(DATA, { env: { ROX_BROKER_HUB: "1", ROX_BROKER_NO_NODE: "1", OPENCODEX_URL: "http://127.0.0.1:9", ANTHROPIC_BASE_URL: "http://127.0.0.1:9" } });
});

after(async () => {
  await server?.stop();
});

/** The commands running in the broker's process group. */
const group = (pgid) =>
  execFileSync("ps", ["-A", "-o", "pid=,pgid=,command="], { encoding: "utf8" })
    .split("\n")
    .map((l) => l.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/))
    .filter((m) => m && Number(m[2]) === pgid)
    .map((m) => ({ pid: Number(m[1]), command: m[3] }));

test("NONODE-01 no Node process runs, and the front door says it is up", RUST_ONLY, async () => {
  const procs = group(server.pid);

  assert.deepEqual(procs.map((p) => p.pid), [server.pid], JSON.stringify(procs));
  assert.ok(!/\bnode\b|app-server/.test(procs[0].command), procs[0].command);
  const health = await request("GET", "/health");

  assert.equal(health.status, 200);
  assert.deepEqual(health.json, { ok: true, broker: "rox-broker" });
  const meta = await request("GET", "/meta");

  assert.equal(meta.json.broker, "rox-broker");
  assert.equal(meta.json.node_version, "");
  // a few hub routes, Node's last resort, and a route deleted in 6.2
  assert.match((await request("GET", "/")).text, /^app-server ok — \d+ threads, model=mock\n$/);
  assert.equal((await request("GET", "/threads")).status, 200);
  for (const [m, p] of [["GET", "/nope"], ["POST", "/highlight"]]) {
    const r = await request(m, p);

    assert.equal(r.status, 404, p);
    assert.equal(r.text, "not found");
  }
  // still none after the requests
  assert.deepEqual(group(server.pid).map((p) => p.pid), [server.pid]);
});

test("NONODE-02 a CORS preflight: 204 with the origin echoed, a foreign origin refused", async () => {
  const origin = `http://127.0.0.1:${PORT}`;
  const ok = await request("OPTIONS", "/threads", { headers: { Origin: origin, "Access-Control-Request-Method": "POST" } });

  assert.equal(ok.status, 204);
  assert.equal(ok.text, "");
  assert.equal(ok.headers["access-control-allow-origin"], origin);
  assert.equal(ok.headers["access-control-allow-headers"], "content-type");
  assert.equal(ok.headers.vary, "Origin");
  const plain = await request("OPTIONS", "/nope");

  assert.equal(plain.status, 204);
  assert.equal(plain.headers["access-control-allow-origin"], undefined);
  const evil = await request("OPTIONS", "/threads", { headers: { Origin: "https://evil.example" } });

  assert.equal(evil.status, 403);
  assert.equal(evil.json.ok, false);
});

test("NONODE-03 ROX_BROKER_NO_NODE=1 without the hub is refused at start", RUST_ONLY, () => {
  const [bin, args] = brokerCommand();
  const r = spawnSync(bin, args, { env: serverEnv(DATA, { ROX_BROKER_HUB: undefined, ROX_BROKER_NO_NODE: "1" }), encoding: "utf8", timeout: 10000 });

  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /ROX_BROKER_NO_NODE=1\) needs ROX_BROKER_HUB=1/);
});
