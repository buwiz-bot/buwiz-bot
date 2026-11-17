// /state long-poll (`?wait=<ms>&v=<hash>`): a GET parks until a POST to the
// same name lands or the wait runs out; a stale `v` answers at once. The
// broker's side is src/leaf/state.rs.
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

test("STATE-WAIT-04 deleting a module wakes its waiters with the empty state", async () => {
  const server = await start(tempDir("state-wait-4"));

  try {
    await postJson("/state/R5", { n: 1 });
    const v = (await get("/state/R5")).json.v;
    const pending = timed(`/state/R5?wait=8000&v=${v}`);
    await new Promise((r) => setTimeout(r, 300));
    await postJson("/app/delete", { name: "R5" });

    const { res, ms } = await pending;
    assert.ok(ms < 6000, `the delete did not wake the GET (${ms}ms)`);
    assert.equal(res.json.state, null);
    assert.equal(res.json.v, "cbf29ce484222325");
  } finally {
    await server.stop();
  }
});

// The same vectors sit in buwiz-broker/src/leaf/state.rs.
test("STATE-WAIT-06 v is FNV-1a of the stored text", async () => {
  const server = await start(tempDir("state-wait-6"));

  try {
    assert.equal((await get("/state/none")).json.v, "cbf29ce484222325");
    await postJson("/state/ascii", { n: 1 });
    assert.equal((await get("/state/ascii")).json.v, "0c85d1734a1666a0");
    await postJson("/state/utf", { s: "caf\u00e9 \u{1f600}" });
    assert.equal((await get("/state/utf")).json.v, "91d0c4d813eafc7b");
  } finally {
    await server.stop();
  }
});
