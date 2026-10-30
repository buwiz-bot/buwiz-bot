// Node's office role (ROX_NODE_ROLE=office, Rust broker plan 5.0c): behind
// rox-broker's hub Node serves only the office routes and holds no hub
// state. On a seeded data dir that a normal start would rewrite (a thread
// without a uid, an unversioned module, stale .stage files, a host token,
// a database from before v1 routine instances with an enabled routine),
// an office-role start and stop leave app.db's rows, the host token, .stage
// and the index files as they were; an office route answers and a hub
// route gets 501. A normal start afterwards does rewrite them, so the seed
// is one the guards were needed for.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { postJson, request, startServer, tempDir } from "./helpers.mjs";

const dir = tempDir("office-role");
const gen = join(dir, "generated");
const stage = join(dir, ".stage");

/** Every table's rows, in rowid order. */
function rows() {
  const db = new DatabaseSync(join(gen, "app.db"), { readOnly: true });

  try {
    const tables = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").all();

    return Object.fromEntries(tables.map(({ name }) => [name, db.prepare(`SELECT * FROM "${name}" ORDER BY rowid`).all()]));
  } finally {
    db.close();
  }
}

/** A directory's files and their contents, recursively. */
function tree(root) {
  const out = {};

  if (!existsSync(root)) return out;
  for (const f of readdirSync(root, { recursive: true })) {
    const p = join(root, f);

    out[f] = statSync(p).isFile() ? readFileSync(p, "utf8") : "<dir>";
  }

  return out;
}

const read = (p) => (existsSync(p) ? readFileSync(p, "utf8") : null);

const snapshot = () => ({
  rows: rows(),
  hostToken: read(join(dir, "host-token")),
  stage: tree(stage),
  index: read(join(gen, "index.tsx")),
  shelf: read(join(dir, "widgets", "index.tsx")),
  generated: readdirSync(gen).sort(),
});

test("OFFICE-01 the office role writes no hub state, serves the office routes and refuses hub routes", async () => {
  // a data dir as a normal Node start leaves it, with a chat and a /state row
  let server = await startServer(dir);

  try {
    assert.equal((await postJson("/thread/new", {})).status, 200);
    assert.equal((await postJson("/state/office-role", { n: 1 })).status, 200);
  } finally {
    await server.stop();
  }

  // what a start would rewrite: a thread without its uid (backfillIds →
  // persist), a module from before versions (the version migration, the
  // index, the chunk backfill), stale stage files, a token of the hub's
  const db = new DatabaseSync(join(gen, "app.db"));

  db.exec("UPDATE threads SET uid=NULL WHERE rowid=(SELECT min(rowid) FROM threads)");
  // a v1 routine enabled before instances: v1/routines.mjs moves it to an
  // instance once, at import (rox-broker's job behind the hub)
  db.exec("DROP TABLE v1_instances");
  db.prepare("INSERT INTO v1_routines(plugin, routine, thread, enabled, seen, last_run) VALUES(?,?,?,1,?,?)").run("dev.example.office", "watch", "t1", "[\"a\"]", 1700000000000);
  db.close();
  writeFileSync(join(gen, "R41.tsx"), "export default function R41() { return null; }\n");
  writeFileSync(join(stage, "candidate.tsx"), "export const x = 1;\n");
  mkdirSync(join(stage, "out-7"), { recursive: true });
  writeFileSync(join(stage, "out-7", "bundle.js"), "// a check's output\n");
  writeFileSync(join(stage, "tsconfig.json"), "{\"the hub's\": true}\n");
  writeFileSync(join(dir, "host-token"), "the-hub-token", { mode: 0o600 });
  const before = snapshot();

  assert.ok(before.rows.threads.some((t) => t.uid === null), "the seed has a thread without a uid");
  assert.ok(!("v1_instances" in before.rows), "the seed is from before instances");

  server = await startServer(dir, { env: { ROX_NODE_ROLE: "office" } });

  try {
    const caps = await request("GET", "/office/caps");

    assert.equal(caps.status, 200, caps.text);
    assert.equal(typeof caps.json, "object");
    const page = await request("GET", `/mermaid?d=${Buffer.from("graph TD; a-->b").toString("base64url")}`);

    assert.equal(page.status, 200);
    assert.match(page.text, /mermaid/);
    const lit = await postJson("/highlight", { code: "const x = 1;", lang: "js" });

    assert.equal(lit.status, 200, lit.text);
    assert.ok("lines" in lit.json);
    // a pop-out stays Node's until 6.3 (D36): no such module, no window
    const pop = await postJson("/popout", { name: "R999" });

    assert.equal(pop.status, 200, pop.text);
    assert.deepEqual(pop.json, { ok: false, error: "no such module" });

    for (const [method, path] of [["GET", "/threads"], ["POST", "/thread/new"], ["GET", "/turns"], ["GET", "/v1/host/plugins"], ["POST", "/v1/hooks/ri_x/secret"], ["GET", "/state/office-role"], ["GET", "/routines"]]) {
      const r = method === "GET" ? await request(method, path) : await postJson(path, {});

      assert.equal(r.status, 501, `${method} ${path}: ${r.text}`);
      assert.deepEqual(r.json, { ok: false, code: "not_ported" }, `${method} ${path}`);
      assert.equal(r.headers["content-type"], "application/json");
    }
    // a moment for anything a start fires late (the chunk backfill, timers)
    await new Promise((r) => setTimeout(r, 500));
  } finally {
    await server.stop();
  }

  const office = snapshot();

  assert.ok(!("v1_instances" in office.rows), "no v1_instances: the move to instances did not run");
  assert.deepEqual(office.rows.v1_routines, before.rows.v1_routines);
  assert.deepEqual(office, before);

  // the control: a normal start on the same dir rewrites what the role kept
  server = await startServer(dir);
  await server.stop();
  const after = snapshot();

  assert.ok(after.rows.threads.every((t) => t.uid), "a normal start backfills the uid");
  assert.ok(!("candidate.tsx" in after.stage), "a normal start clears .stage");
  assert.ok(after.rows.heads.some((h) => h.name === "R41"), "a normal start migrates R41");
  assert.notEqual(after.hostToken, "the-hub-token", "a normal start writes its own host token");
  assert.deepEqual(
    after.rows.v1_instances.map((i) => [i.plugin, i.routine, i.thread, i.enabled, i.seen, i.last_run]),
    [["dev.example.office", "watch", "t1", 1, "[\"a\"]", 1700000000000]],
    "a normal start moves the routine to an instance",
  );
});
