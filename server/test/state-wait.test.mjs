// /state long-poll (`?wait=<ms>&v=<hash>`): a GET parks until a POST to the
// same name lands or the wait runs out; a stale `v` answers at once. Node's
// half lives in routes/apps.mjs, the Rust leaf's in
// buwiz-broker/src/leaf/state.rs — the same contract either way.
import assert from "node:assert/strict";
import { test } from "node:test";
import { join } from "node:path";
import { offlineEnv, postJson, request, startServer, tempDir } from "./helpers.mjs";

const start = (dir) => startServer(dir, { env: offlineEnv(join(dir, "stubs.log")) });

const get = (path) => request("GET", path);
const timed = async (path) => {
  const t = Date.now();
  const res = await Promise.race([
    get(path),
    new Promise((_, rej) => setTimeout(() => rej(new Error(`GET ${path} held too long`)), 12000)),
  ]);
  return { res, ms: Date.now() - t };
};

test("STATE-WAIT-01 a stale v answers at once with the newer state", async () => {
  const server = await start(tempDir("state-wait-1"));

  try {
    await postJson("/state/w", { n: 1 });
    const first = (await get("/state/w")).json;
    assert.ok(first.v, "GET /state answers a v");
    await postJson("/state/w", { n: 2 });

    const { res, ms } = await timed(`/state/w?wait=8000&v=${first.v}`);
    assert.equal(res.status, 200);
    assert.ok(ms < 4000, `stale v held ${ms}ms`);
    assert.equal(res.json.state.n, 2);
    assert.notEqual(res.json.v, first.v);
  } finally {
    await server.stop();
  }
});

test("STATE-WAIT-02 a current v holds until a POST to its name lands", async () => {
  const server = await start(tempDir("state-wait-2"));

  try {
    await postJson("/state/w", { n: 1 });
    const v = (await get("/state/w")).json.v;
    const pending = timed(`/state/w?wait=8000&v=${v}`);
    await new Promise((r) => setTimeout(r, 300));
    await postJson("/state/w", { n: 3 });

    const { res, ms } = await pending;
    assert.ok(ms < 6000, `the write did not wake the GET (${ms}ms)`);
    assert.equal(res.json.state.n, 3);
    assert.notEqual(res.json.v, v);
  } finally {
    await server.stop();
  }
});

test("STATE-WAIT-03 the wait runs out unchanged; other names do not wake it", async () => {
  const server = await start(tempDir("state-wait-3"));

  try {
    await postJson("/state/w", { n: 1 });
    const v = (await get("/state/w")).json.v;
    await postJson("/state/other", { x: 1 });

    const { res, ms } = await timed(`/state/w?wait=600&v=${v}`);
    assert.ok(ms >= 400, `returned after ${ms}ms, before the wait ran out`);
    assert.equal(res.json.state.n, 1);
    assert.equal(res.json.v, v);
  } finally {
    await server.stop();
  }
});
