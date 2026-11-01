// tools/db-diff.mjs (the Rust broker plan, phase 5 §6): two app.db files
// compare row by row, leaving out rowids, uids, timestamps and names built
// from Date.now(). A database Node's own store wrote is the input.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { ROOT, tempDir } from "./helpers.mjs";

const TOOL = join(ROOT, "tools", "db-diff.mjs");

// a data dir whose app.db Node's store wrote: chats, turns of several
// kinds, a bot with memories, a group
function seeded(label) {
  const dir = tempDir(label);

  mkdirSync(join(dir, "widgets"), { recursive: true });
  execFileSync(process.execPath, ["--input-type=module", "-e", `
    const { store, nextThreadSeq, uuidv7, writeIndex } = await import("./src/state.mjs");
    const t = "t" + nextThreadSeq();
    store.list.push({ id: t, uid: uuidv7(), title: "Seeded", turns: [
      { id: uuidv7(), role: "user", text: "hello" },
      { id: uuidv7(), role: "ai", text: "files", label: "mock", files: [{ path: "m" + Date.now().toString(36) + "/a.txt", name: "a.txt" }] },
    ] });
    const bid = "b" + Date.now().toString(36) + "abc";
    store.bots.push({ id: bid, name: "Ada", shape: "blob", color: "#1084FE", threadId: t, memories: [{ id: uuidv7(), text: "likes tea", at: Date.now() }], integrations: [] });
    writeIndex();
    const { newGroup } = await import("./src/groups.mjs");
    newGroup({ name: "Team", item: { kind: "bot", ref: bid } });
    process.exit(0);
  `], { cwd: ROOT, env: { ...process.env, BUWIZ_BROKER_DATA_DIR: dir, MOCK: "1" }, stdio: "pipe" });

  return join(dir, "generated", "app.db");
}

const run = (a, b) => spawnSync(process.execPath, [TOOL, a, b], { encoding: "utf8" });

test("DBDIFF-01 a copy is equal: exit 0", () => {
  const a = seeded("dbdiff-a");
  const b = join(tempDir("dbdiff-copy"), "app.db");

  copyFileSync(a, b);
  const r = run(a, b);

  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /same rows/);
});

test("DBDIFF-02 the same writes in two data dirs are equal: ids, uids, times and Date.now() names left out", () => {
  const a = seeded("dbdiff-run1");
  const b = seeded("dbdiff-run2");
  const r = run(a, b);

  assert.equal(r.status, 0, r.stdout + r.stderr);
});

test("DBDIFF-03 a planted change: exit 1, the table and both rows named", () => {
  const a = seeded("dbdiff-plant");
  const b = join(tempDir("dbdiff-planted"), "app.db");

  copyFileSync(a, b);
  const db = new DatabaseSync(b);

  db.prepare("UPDATE threads SET title='Planted' WHERE title='Seeded'").run();
  // what db-diff leaves out changes too, and does not count
  db.prepare("UPDATE turns SET id=id+1000, uid='01a00000-0000-7000-8000-000000000000'").run();
  db.close();
  const r = run(a, b);

  assert.equal(r.status, 1, r.stdout);
  const lines = r.stdout.split("\n").filter((l) => l.startsWith("threads: "));

  assert.equal(lines.length, 2, r.stdout);
  assert.match(lines.find((l) => l.includes("only in A")), /"title":"Seeded"/);
  assert.match(lines.find((l) => l.includes("only in B")), /"title":"Planted"/);
  assert.match(r.stdout, /2 rows differ/);
  assert.ok(!/^turns: /m.test(r.stdout), "turns.id and uids are not compared");
});

test("DBDIFF-04 a missing file is a usage error: exit 2", () => {
  const r = run(join(tempDir("dbdiff-none"), "app.db"), "");

  assert.equal(r.status, 2);
  assert.match(r.stderr, /usage: db-diff\.mjs <a\.db> <b\.db>/);
});
