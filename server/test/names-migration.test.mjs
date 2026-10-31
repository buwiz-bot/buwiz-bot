// Protocol 2.0's one-time rewrite of app.db (names-migration.mjs) on a
// store shaped like one written before it: chats with rox:// links, a
// reply's rox-app card link, stored rox/<key> _meta, a pinned link, an
// attachment's open. It runs once (the marker), changes nothing a second
// time, leaves keys and the v1 install records alone.
import assert from "node:assert/strict";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { MARKER, migrateNames } from "../src/names-migration.mjs";
import { tempDir } from "./helpers.mjs";

function oldStore() {
  const db = new DatabaseSync(join(tempDir("names-migration"), "app.db"));

  db.exec(`
    CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
    CREATE TABLE turns(thread TEXT, i INTEGER, role TEXT, content TEXT, links TEXT, meta TEXT, PRIMARY KEY(thread, i));
    CREATE TABLE group_items(group_id TEXT, kind TEXT, ref TEXT, title TEXT, ord INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(group_id, kind, ref));
    CREATE TABLE attachments(id INTEGER PRIMARY KEY, token TEXT, open TEXT, n INTEGER);
    CREATE TABLE kv(k TEXT PRIMARY KEY, v TEXT);
    CREATE TABLE v1_plugins(id TEXT PRIMARY KEY, manifest TEXT);
  `);
  const link = { provider: "rox-app", id: "rox://app/dev.goldcoders.bir/forms?ticket=tkt_1", link: "rox://app/dev.goldcoders.bir/forms?ticket=tkt_1", card: { plugin: "dev.goldcoders.bir", app: "forms" } };

  db.prepare("INSERT INTO turns VALUES(?,?,?,?,?,?)").run("t1", 0, "assistant", "Open [the form](rox://app/dev.goldcoders.bir/forms) or [this chat](rox://chat/t2?m=4).", JSON.stringify([link]), JSON.stringify({ "rox/caller": { origin: "agent" }, "rox/open": { app: "forms" } }));
  db.prepare("INSERT INTO turns VALUES(?,?,?,?,?,?)").run("t1", 1, "user", "nothing old here", null, null);
  db.prepare("INSERT INTO group_items VALUES(?,?,?,?,?)").run("g1", "link", "rox://view?path=/tmp/x.pdf", "rox://view?path=/tmp/x.pdf", 0);
  db.prepare("INSERT INTO attachments VALUES(?,?,?,?)").run(1, "@bir:taxprofile:tkt_1", "rox://app/bir/forms?props=%7B%7D", 3);
  db.prepare("INSERT INTO kv VALUES(?,?)").run("/state/rox-theme", "rox://app/x/y");
  db.prepare("INSERT INTO v1_plugins VALUES(?,?)").run("dev.goldcoders.bir", '{"apps":[{"entry":"rox-app://forms"}]}');

  return db;
}

test("NAMES-01 the rox names in a store become the buwiz ones, once; keys and install records stay", () => {
  const db = oldStore();
  const changed = migrateNames(db);

  assert.equal(changed, 6, "turn 0: content, links and meta; the attachment's open; the kv value; the pin's title (ref is a key)");
  const t = db.prepare("SELECT * FROM turns WHERE i=0").get();

  assert.equal(t.content, "Open [the form](buwiz://app/dev.goldcoders.bir/forms) or [this chat](buwiz://chat/t2?m=4).");
  const [l] = JSON.parse(t.links);

  assert.deepEqual([l.provider, l.id, l.link], ["buwiz-app", "buwiz://app/dev.goldcoders.bir/forms?ticket=tkt_1", "buwiz://app/dev.goldcoders.bir/forms?ticket=tkt_1"]);
  assert.deepEqual(Object.keys(JSON.parse(t.meta)), ["buwiz/caller", "buwiz/open"]);
  assert.equal(db.prepare("SELECT content FROM turns WHERE i=1").get().content, "nothing old here");
  assert.equal(db.prepare("SELECT open FROM attachments").get().open, "buwiz://app/bir/forms?props=%7B%7D");
  const pin = db.prepare("SELECT * FROM group_items").get();

  assert.deepEqual([pin.ref, pin.title], ["rox://view?path=/tmp/x.pdf", "buwiz://view?path=/tmp/x.pdf"], "a key column is never rewritten");
  assert.deepEqual({ ...db.prepare("SELECT * FROM kv").get() }, { k: "/state/rox-theme", v: "buwiz://app/x/y" }, "/state/rox-* keys keep their names");
  assert.equal(db.prepare("SELECT manifest FROM v1_plugins").get().manifest, '{"apps":[{"entry":"rox-app://forms"}]}');
  assert.ok(db.prepare("SELECT v FROM meta WHERE k=?").get(MARKER));
  // marked: nothing runs again, even on text that looks old
  db.prepare("UPDATE turns SET content='rox://as typed later' WHERE i=1").run();
  assert.equal(migrateNames(db), 0);
  assert.equal(db.prepare("SELECT content FROM turns WHERE i=1").get().content, "rox://as typed later");
});

test("NAMES-02 the rewrite is idempotent without its marker too", () => {
  const db = oldStore();

  migrateNames(db);
  const before = db.prepare("SELECT content, links, meta FROM turns ORDER BY i").all();

  db.prepare("DELETE FROM meta WHERE k=?").run(MARKER);
  assert.equal(migrateNames(db), 0);
  assert.deepEqual(db.prepare("SELECT content, links, meta FROM turns ORDER BY i").all(), before);
});
