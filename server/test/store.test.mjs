// The store (Rust broker plan, phase 5 item 5.1): what Node's state.mjs
// loads and writes, read over HTTP. Data is seeded through Node's own
// modules (as agents.test.mjs A2A-06 does) or as an older schema wrote it.
// STORE-03 is, for now, Node reloading what Node wrote; with the Rust hub
// it becomes Node loading what Rust wrote (R8).
import assert from "node:assert/strict";
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { dbDiff } from "../tools/db-diff.mjs";
import { dbRows, nodeIn, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const start = (dir, env = {}) => startServer(dir, { env: { ...offlineEnv(join(dir, "stubs.log")), ...env } });

// every read Rox makes of the store, as text: /threads and each /turns
async function snapshot() {
  const threads = await request("GET", "/threads");
  const out = { "/threads": threads.text };

  for (const t of threads.json.list) out[`/turns?thread=${t.id}`] = (await request("GET", `/turns?thread=${t.id}`)).text;

  return out;
}

const v7ms = (id) => parseInt(id.replace(/-/g, "").slice(0, 12), 16);

test("STORE-01 a Node-seeded store with every turn kind, meta field and bot field loads the same twice; turns.meta keeps its key order", async () => {
  const dir = tempDir("store-load");
  const seeded = JSON.parse(nodeIn(dir, `
    const { store, nextThreadSeq, uuidv7, writeIndex } = await import("./src/state.mjs");
    const t1 = store.list[0];
    const t2 = { id: "t" + nextThreadSeq(), uid: uuidv7(), title: "Routine thread", hidden: true, archived: true, routine: { plugin: "p.one", id: "daily", name: "Daily", thread: t1.id }, turns: [] };
    const t3 = { id: "t" + nextThreadSeq(), uid: uuidv7(), title: "Q3", agents: { subject: "Q3", participants: ["ann", "bob"] }, turns: [] };
    store.list.push(t2, t3);
    t1.turns.push(
      { id: uuidv7(), role: "user", text: "hello", reaction: "🎉", reply: { i: 0, text: "quoted", files: ["m1/a.txt"] }, attachments: [{ uri: "file:///x/a.png", name: "a.png", kind: "image" }], v1Attachments: [{ plugin: "p.one", token: "@p:x:1" }] },
      { id: uuidv7(), role: "ai", text: "answer", name: "", label: "claude-haiku-5-5", caption: "cap", reaction: "👍", retries: 2, files: [{ path: "m1/a.txt", name: "a.txt", kind: "text" }], links: [{ provider: "gdrive", id: "f1", name: "Doc", link: "https://example.com/d", mime: "application/vnd.google-apps.document", account: "a@example.com" }], suggest: ["Next step"], art: { title: "T", desc: "D", w: 100, h: 80 }, undef: undefined },
      { id: uuidv7(), role: "ai", text: null, name: "R1", label: "mock", v: 1, widget: true, art: { title: "Counter", desc: "", w: 0, h: 0 } },
      { id: uuidv7(), role: "notice", text: "Daily ran", notice: { routine: "daily", thread: t2.id } },
    );
    t2.turns.push({ id: uuidv7(), role: "user", text: "Routine · Daily", routine: { plugin: "p.one", id: "daily", run: 1 } });
    t3.turns.push(
      { id: uuidv7(), role: "agent_message", text: "Check Q3.", message: { id: "m1", from: { id: "ann", name: "Ann" }, to: ["bob"], subject: "Q3" }, pending: ["bob"], read_by: ["ann"] },
      { id: uuidv7(), role: "ai", text: "woken", agent: { id: "bob", name: "Bob" } },
    );
    store.bots.push(
      { id: "ann", name: "Ann", shape: "star", color: "#FF309B", threadId: t1.id, pinned: true, section: "Work", hidden: false, image: "file:///x/ann.png", crop: { x: 0.1, y: 0.2, w: 0.5, h: 0.5 }, model: "gpt-5.5", effort: "high", fast: true, label: "tagline", description: "", instructions: "Be kind.", memories: [{ id: uuidv7(), text: "likes tea", at: 1700000000000 }], integrations: [{ id: "s1", kind: "skill", at: 1700000000000, via: "user" }], made_by: "bob", purpose: "help Bob", accepts: ["bob"] },
      { id: "bob", name: "Bob", shape: "blob", color: "#1084FE", threadId: null, helper: { parent: "ann", task: "sort" } },
    );
    store.sections = ["Work"];
    writeIndex();
    console.log(JSON.stringify({ t1: t1.id, t2: t2.id, t3: t3.id }));
  `));
  assert.equal(dbRows(dir, "SELECT reaction FROM turns WHERE thread_id=? AND seq=0", seeded.t1)[0].reaction, "🎉");
  const metaOf = (seq) => dbRows(dir, "SELECT meta FROM turns WHERE thread_id=? AND seq=?", seeded.t1, seq)[0].meta;

  // the order persist() writes, undefined dropped
  assert.equal(metaOf(0), '{"reply":{"i":0,"text":"quoted","files":["m1/a.txt"]},"attachments":[{"uri":"file:///x/a.png","name":"a.png","kind":"image"}],"v1Attachments":[{"plugin":"p.one","token":"@p:x:1"}]}');
  assert.equal(metaOf(1), '{"files":[{"path":"m1/a.txt","name":"a.txt","kind":"text"}],"art":{"title":"T","desc":"D","w":100,"h":80},"links":[{"provider":"gdrive","id":"f1","name":"Doc","link":"https://example.com/d","mime":"application/vnd.google-apps.document","account":"a@example.com"}],"suggest":["Next step"]}');
  assert.equal(metaOf(2), '{"art":{"title":"Counter","desc":"","w":0,"h":0}}');
  assert.equal(dbRows(dir, "SELECT meta FROM turns WHERE thread_id=? AND seq=0", seeded.t3)[0].meta, '{"message":{"id":"m1","from":{"id":"ann","name":"Ann"},"to":["bob"],"subject":"Q3"},"pending":["bob"],"read_by":["ann"]}');
  assert.deepEqual(dbRows(dir, "SELECT id, description, instructions, extra, crop FROM bots ORDER BY ord"), [
    { id: "ann", description: "", instructions: "Be kind.", extra: '{"made_by":"bob","purpose":"help Bob","accepts":["bob"]}', crop: '{"x":0.1,"y":0.2,"w":0.5,"h":0.5}' },
    { id: "bob", description: null, instructions: null, extra: '{"helper":{"parent":"ann","task":"sort"}}', crop: null },
  ]);
  let server = await start(dir);
  let first;

  try {
    first = await snapshot();
  } finally {
    await server.stop();
  }

  server = await start(dir);

  try {
    assert.deepEqual(await snapshot(), first, "the second load reads the same bytes");
  } finally {
    await server.stop();
  }

  const threads = JSON.parse(first["/threads"]);

  assert.deepEqual(threads.list.map((t) => [t.id, t.routine || null, t.agents || null, t.hidden, t.archived]), [
    [seeded.t1, null, null, false, false],
    [seeded.t2, { plugin: "p.one", id: "daily", name: "Daily", thread: seeded.t1 }, null, true, true],
    [seeded.t3, null, { subject: "Q3", participants: ["ann", "bob"] }, false, false],
  ]);
  // a routine's thread answers as the bot of the chat it reports to
  assert.deepEqual(threads.list[1].bot, { id: "ann", name: "Ann", shape: "star", color: "#FF309B" });
  assert.deepEqual(threads.sections, ["Work"]);
  const [ann, bob] = threads.bots;

  assert.deepEqual(ann, {
    id: "ann", name: "Ann", shape: "star", color: "#FF309B", threadId: seeded.t1, pinned: true, section: "Work", hidden: false, image: "file:///x/ann.png",
    crop: { x: 0.1, y: 0.2, w: 0.5, h: 0.5 }, model: "gpt-5.5", effort: "high", fast: true, label: "tagline", description: "", instructions: "Be kind.",
    memories: ann.memories, integrations: [{ id: "s1", name: "s1", kind: "skill", missing: true }], updated_at: 0, made_by: "bob", purpose: "help Bob", accepts: ["bob"], preview: "Daily ran",
  });
  assert.deepEqual(ann.memories.map((m) => [m.text, m.at]), [["likes tea", 1700000000000]]);
  // a NULL description reads as the defaults
  assert.equal(bob.description, "Bob is a personal assistant bot in Grok Shell.");
  assert.deepEqual(bob.helper, { parent: "ann", task: "sort" });
  // the start's persist() wrote the defaults it loaded
  assert.equal(dbRows(dir, "SELECT description FROM bots WHERE id='bob'")[0].description, "Bob is a personal assistant bot in Grok Shell.");
  const t1 = JSON.parse(first[`/turns?thread=${seeded.t1}`]).turns;

  assert.deepEqual(t1[0].reply, { i: 0, text: "quoted", files: ["m1/a.txt"] });
  // BUG: a user message's reaction is written (turns.reaction) but never
  // loaded back: loadStore reads only role, uid, text and meta for user,
  // notice and agent_message rows, so the badge is gone after a restart
  // (and the start's persist() writes the column back as NULL)
  assert.equal(dbRows(dir, "SELECT reaction FROM turns WHERE thread_id=? AND seq=0", seeded.t1)[0].reaction, null);
  assert.equal(t1[0].reaction, "");
  assert.deepEqual(t1[0].v1Attachments, [{ plugin: "p.one", token: "@p:x:1" }]);
  assert.deepEqual(t1[1].links, [{ provider: "gdrive", id: "f1", name: "Doc", link: "https://example.com/d", mime: "application/vnd.google-apps.document", kind: "document", type: "Google Doc" }]);
  assert.deepEqual([t1[1].reaction, t1[1].retries, t1[1].caption, t1[1].suggest], ["👍", 2, "cap", ["Next step"]]);
  assert.deepEqual(t1[1].art, { title: "T", desc: "D", w: 100, h: 80 });
  assert.deepEqual([t1[2].name, t1[2].v, t1[2].widget, t1[2].text], ["R1", 1, true, ""]);
  assert.deepEqual(t1[3].notice, { routine: "daily", thread: seeded.t2 });
  assert.deepEqual(JSON.parse(first[`/turns?thread=${seeded.t2}`]).turns[0].routine, { plugin: "p.one", id: "daily", run: 1 });
  const t3 = JSON.parse(first[`/turns?thread=${seeded.t3}`]).turns;

  assert.deepEqual(t3[0].message, { id: "m1", from: { id: "ann", name: "Ann" }, to: ["bob"], subject: "Q3" });
  // pending and read_by are kept, but /turns doesn't show them
  assert.equal(t3[0].pending, undefined);
  assert.deepEqual(t3[1].agent, { id: "bob", name: "Bob" });
});

// a scripted run of store writes over HTTP, the same for any broker
async function writeSequence() {
  const a = (await postJson("/thread/new", {})).json.id;

  await postJson("/thread/update", { id: a, title: "Plans", hidden: true });
  const bot = (await postJson("/bot/new", { name: "Ada" })).json.bot;

  await postJson("/bot/update", { id: bot.id, label: "helper", section: "Team", model: "m1", crop: { x: 1, y: 1, w: 2, h: 2 } });
  await postJson("/bot/memory", { id: bot.id, op: "add", text: "likes tea" });
  await postJson("/group/new", { name: "Team", item: { kind: "bot", ref: bot.id } });

  for (const [prompt, thread] of [["make a counter", a], ["/mock md", a], ["/upload", bot.threadId], ["weekly chart", bot.threadId]]) {
    const id = (await postJson("/chat", { prompt, thread })).json.id;

    await waitFor(async () => (await request("GET", `/status?id=${id}`)).json.done, 30000);
  }

  await postJson("/thread/select", { id: a });
  await postJson("/react", { key: "idx:0", r: "❤️" });
  await postJson("/turn/delete", { key: "idx:3" });
  await postJson("/state/R1", { n: 3 });
  await postJson("/thread/select", { id: bot.threadId });
}

test("STORE-02 the same HTTP writes in two data dirs give the same rows (db-diff)", async () => {
  const dirs = [tempDir("store-write-a"), tempDir("store-write-b")];

  for (const dir of dirs) {
    const server = await start(dir);

    try {
      await writeSequence();
    } finally {
      await server.stop();
    }
  }

  const [a, b] = dirs.map((d) => join(d, "generated", "app.db"));

  assert.deepEqual(dbDiff(a, b), []);
  // and the rows are there
  assert.equal(dbRows(dirs[0], "SELECT COUNT(*) AS n FROM turns")[0].n, 7);
});

test("STORE-03 a restart loads what was written: every read the same", async () => {
  const dir = tempDir("store-reload");
  let server = await start(dir);
  let before;

  try {
    await writeSequence();
    before = { ...(await snapshot()), "/groups": (await request("GET", "/groups")).text };
  } finally {
    await server.stop();
  }

  // what a restart changes, and nothing else: bots' updated_at (never
  // persisted) and the user message's reaction (BUG: never loaded back,
  // see STORE-01)
  const threads = JSON.parse(before["/threads"]);
  const a = threads.list.find((t) => t.title === "make a counter").id;
  const turns = JSON.parse(before[`/turns?thread=${a}`]);

  assert.equal(turns.turns[0].reaction, "❤️");
  turns.turns[0].reaction = "";
  for (const b of threads.bots) b.updated_at = 0;
  const expected = { ...before, "/threads": JSON.stringify(threads), [`/turns?thread=${a}`]: JSON.stringify(turns) };

  server = await start(dir);

  try {
    assert.deepEqual({ ...(await snapshot()), "/groups": (await request("GET", "/groups")).text }, expected);
  } finally {
    await server.stop();
  }
});

test("STORE-04 a building turn is never persisted; a seq gap loads in order and the next persist closes it", async () => {
  const dir = tempDir("store-building");
  const t = nodeIn(dir, `
    const { store, uuidv7, writeIndex } = await import("./src/state.mjs");
    const t = store.list[0];
    t.turns.push(
      { id: uuidv7(), role: "user", text: "build it" },
      { id: uuidv7(), role: "ai", building: true, status: "checking types…", label: "x", art: { title: "Soon", desc: "", w: 0, h: 0 } },
      { id: uuidv7(), role: "ai", text: "done" },
    );
    writeIndex();
    console.log(t.id);
  `);

  assert.deepEqual(dbRows(dir, "SELECT seq, role, text FROM turns WHERE thread_id=? ORDER BY seq", t), [
    { seq: 0, role: "user", text: "build it" },
    { seq: 2, role: "ai", text: "done" },
  ]);
  const server = await start(dir);

  try {
    const turns = (await request("GET", `/turns?thread=${t}`)).json.turns;

    assert.deepEqual(turns.map((x) => [x.i, x.role, x.text, x.building]), [[0, "user", "build it", undefined], [1, "ai", "done", undefined]]);
    // the start's persist() renumbers
    assert.deepEqual(dbRows(dir, "SELECT seq FROM turns WHERE thread_id=? ORDER BY seq", t).map((r) => r.seq), [0, 1]);
  } finally {
    await server.stop();
  }
});

test("STORE-05 a database from before uids gets them backfilled by Node's time rule, once", async () => {
  const dir = tempDir("store-uids");
  const K = 1700000000000;

  mkdirSync(join(dir, "generated"), { recursive: true });
  mkdirSync(join(dir, "widgets"), { recursive: true });
  const db = new DatabaseSync(join(dir, "generated", "app.db"));

  db.exec(`
    CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
    CREATE TABLE threads(id TEXT PRIMARY KEY, title TEXT NOT NULL, ord INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE turns(id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL, seq INTEGER NOT NULL, role TEXT NOT NULL,
      text TEXT, name TEXT, label TEXT, caption TEXT, reaction TEXT, retries INTEGER DEFAULT 0, widget INTEGER DEFAULT 0);
    CREATE TABLE versions(name TEXT NOT NULL, v INTEGER NOT NULL, parent INTEGER, prompt TEXT, created_at INTEGER, PRIMARY KEY(name, v));
    INSERT INTO meta VALUES('active','t2');
    INSERT INTO threads VALUES('t2','Second',0),('t1','First',1);
    INSERT INTO turns(thread_id,seq,role,text,name,label) VALUES
      ('t1',0,'user','a',NULL,NULL),('t1',1,'ai',NULL,'R1','mock'),
      ('t2',0,'user','b',NULL,NULL),('t2',1,'ai','c',NULL,'claude');
    INSERT INTO versions VALUES('R1',1,NULL,'a',${K});
  `);
  db.close();
  const read = async () => {
    const t = (await request("GET", "/threads")).json.list;
    const out = {};

    for (const x of t) out[x.id] = { uid: x.uid, turns: (await request("GET", `/turns?thread=${x.id}`)).json.turns.map((u) => u.id) };

    return out;
  };
  let server = await start(dir);
  let ids;

  try {
    ids = await read();
  } finally {
    await server.stop();
  }

  // in thread-number order, each thread then its turns; the artifact turn
  // takes its version's time and the rest count up around it
  assert.deepEqual(
    [ids.t1.uid, ...ids.t1.turns, ids.t2.uid, ...ids.t2.turns].map(v7ms),
    [K - 2, K - 1, K, K + 1, K + 2, K + 3],
  );
  assert.deepEqual(dbRows(dir, "SELECT id, uid FROM threads ORDER BY ord"), [{ id: "t2", uid: ids.t2.uid }, { id: "t1", uid: ids.t1.uid }]);
  server = await start(dir);

  try {
    assert.deepEqual(await read(), ids, "idempotent");
  } finally {
    await server.stop();
  }
});

test("STORE-06 generated/index.tsx: every module, version and published widget, in directory order; rewritten only when that set changes", async () => {
  const dir = tempDir("store-index");
  const gen = join(dir, "generated");

  mkdirSync(join(gen, "R2"), { recursive: true });
  mkdirSync(join(dir, "widgets"), { recursive: true });
  const mod = (s) => `import { Text } from "nsh";\nexport default function R() { return <Text>${s}</Text>; }\n`;

  writeFileSync(join(gen, "R1.tsx"), mod("one"));
  writeFileSync(join(gen, "R2.tsx"), mod("two"));
  writeFileSync(join(gen, "R2", "two.v1.tsx"), mod("two v1"));
  writeFileSync(join(gen, "R2", "two.v2.tsx"), mod("two v2"));
  writeFileSync(join(dir, "widgets", "R1.tsx"), mod("one"));
  const server = await start(dir);

  try {
    const file = join(gen, "index.tsx");
    // the start's migration gave R1 its v1 file (R2 had one)
    const names = (d) => readdirSync(d).filter((f) => /^R\d+\.tsx$/.test(f)).map((f) => f.slice(0, -4));
    const g = names(gen);
    const vers = g.flatMap((n) => readdirSync(join(gen, n)).flatMap((f) => (/\.v(\d+)\.tsx$/.test(f) ? [{ n, v: Number(f.match(/\.v(\d+)\.tsx$/)[1]), file: f.slice(0, -4) }] : [])));
    const pubs = names(join(dir, "widgets"));

    assert.deepEqual(vers.map((x) => `${x.n}/${x.file}`).sort(), ["R1/R1.v1", "R2/two.v1", "R2/two.v2"]);
    const expected =
      [...g.map((n) => `import ${n} from "./${n}";`), ...vers.map((x) => `import ${x.n}v${x.v} from "./${x.n}/${x.file}";`), ...pubs.map((n) => `import ${n}w from "../widgets/${n}";`)].join("\n") +
      "\n\n// Regenerated by server/app-server.mjs — module registry only.\n\nexport const MODULES: Record<string, any> = {\n" +
      [...g.map((n) => `  "${n}": ${n},`), ...vers.map((x) => `  "${x.n}@v${x.v}": ${x.n}v${x.v},`), ...pubs.map((n) => `  "${n}w": ${n}w,`)].join("\n") +
      "\n};\n";

    assert.equal(readFileSync(file, "utf8"), expected);
    const at = statSync(file).mtimeMs;

    await postJson("/thread/new", {});
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(statSync(file).mtimeMs, at, "a store write leaves the index alone");
  } finally {
    await server.stop();
  }
});

test("STORE-07 widget state and timeclock writes during store writes: none fails, no SQLITE_BUSY", async () => {
  const dir = tempDir("store-busy");
  const server = await start(dir);

  try {
    const bot = (await postJson("/bot/new", { name: "Busy" })).json.bot;
    const calls = [];

    for (let k = 0; k < 300; k++) calls.push(postJson(`/state/w${k % 30}`, { k, pad: "x".repeat(200) }));

    for (let k = 0; k < 40; k++) {
      calls.push(postJson("/thread/update", { id: bot.threadId, title: `t${k}` }));
      calls.push(postJson("/bot/memory", { id: bot.id, op: "add", text: `m${k}` }));
    }

    for (let k = 0; k < 5; k++) calls.push(postJson(`/data/timeclock/${k % 2 ? "out" : "in"}`, {}));
    const rs = await Promise.all(calls);

    assert.deepEqual(rs.filter((r) => r.status !== 200).map((r) => `${r.status} ${r.text}`), []);
    assert.equal((await request("GET", "/threads")).json.bots[0].memories.length, 40);
    assert.doesNotMatch(server.log(), /SQLITE_BUSY|database is locked/);
  } finally {
    await server.stop();
  }
});
