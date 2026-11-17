// /state long-poll (`?wait=<ms>&v=<hash>`): a GET parks until a POST to the
// same name lands or the wait runs out; a stale `v` answers at once. Node's
// half lives in routes/apps.mjs, the Rust leaf's in
// buwiz-broker/src/leaf/state.rs — the same contract either way.
import assert from "node:assert/strict";
import { test } from "node:test";
import { join } from "node:path";
import { HUB, nodeIn, offlineEnv, postJson, request, startServer, tempDir } from "./helpers.mjs";

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

// Rust holding /state while Node runs /app/delete (no hub) cannot be woken
// by Node's delete: the two share only the database file.
const split = !!(process.env.BUWIZ_BROKER_CMD || "").trim() && !HUB;

test("STATE-WAIT-04 deleting a module wakes its waiters with the empty state", { skip: split && "Rust leaf with Node's delete: no wake across processes" }, async () => {
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

test("STATE-WAIT-05 a late close of a flushed waiter leaves a newer waiter registered", () => {
  // the handler driven in-process with stand-in req/res, so the order of
  // events (flush A, register B, then A's close) is fixed
  const out = nodeIn(tempDir("state-wait-5"), `
    import { EventEmitter } from "node:events";
    import handle from "./src/routes/apps.mjs";
    const res = () => Object.assign(new EventEmitter(), { writableEnded: false, setHeader() {}, end() { this.writableEnded = true; } });
    const get = async (url) => { const r = res(); const q = Object.assign(new EventEmitter(), { method: "GET", url, headers: {} }); await handle(q, r); return r; };
    const post = async (url, body) => { const r = res(); const q = Object.assign(new EventEmitter(), { method: "POST", url, headers: {} }); await handle(q, r); q.emit("data", body); q.emit("end"); return r; };
    const a = await get("/state/w?wait=20000");
    await post("/state/w", '{"n":1}');
    const b = await get("/state/w?wait=20000");
    a.emit("close");
    await post("/state/w", '{"n":2}');
    console.log(JSON.stringify({ aEnded: a.writableEnded, bEnded: b.writableEnded }));
  `);

  assert.equal(out, '{"aEnded":true,"bEnded":true}');
});

// The same vectors sit in buwiz-broker/src/leaf/state.rs: a change to either
// hash breaks a test on its own side.
test("STATE-WAIT-06 v is FNV-1a of the stored text, as the Rust broker computes it", async () => {
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
