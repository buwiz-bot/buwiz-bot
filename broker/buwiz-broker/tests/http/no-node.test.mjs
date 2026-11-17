// buwiz-broker with no Node at all: no Node child runs, and the broker
// answers /health, /meta, /, the CORS preflight and the 404 of a route
// nobody has.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { basename } from "node:path";
import { after, before, test } from "node:test";
import { PORT, request, startServer, tempDir } from "./helpers.mjs";

const DATA = tempDir("no-node");
let server;

before(async () => {
  server = await startServer(DATA, { env: { OPENCODEX_URL: "http://127.0.0.1:9", ANTHROPIC_BASE_URL: "http://127.0.0.1:9" } });
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

test("NONODE-01 no Node process runs, and the front door says it is up", async () => {
  const procs = group(server.pid);

  assert.deepEqual(procs.map((p) => p.pid), [server.pid], JSON.stringify(procs));
  assert.equal(basename(procs[0].command.split(" ")[0]), "buwiz-broker", procs[0].command);
  const health = await request("GET", "/health");

  assert.equal(health.status, 200);
  assert.deepEqual(health.json, { ok: true, broker: "buwiz-broker" });
  const meta = await request("GET", "/meta");

  assert.equal(meta.json.broker, "buwiz-broker");
  assert.equal(meta.json.node_version, undefined, "the broker has no Node to report");
  // a few routes, and one that no longer exists
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
