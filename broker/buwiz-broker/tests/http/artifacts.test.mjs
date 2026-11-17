// The Library and mini-app routes over HTTP on a seeded data dir (the Rust
// broker plan, phase 5 item 5.7b): /module, /app/*, /apps/meta,
// /artifacts, /artifact/*, /widgets, /publish, /unpublish, /reuse,
// /catalog, /widget-meta and /sources. Where a route writes, the rows it
// changed are pinned with tools/db-diff.mjs (a snapshot before and after,
// timestamps, uids and rowids left out). nsh, sips and secret are the PATH
// stubs; Drive is a stub MCP server named google-workspace (no Google
// call). Not here: /app/icon/gen (an image model; the route list of 5.7b
// given to this file leaves it out) and the generate tools (5.4/5.7a).
//
// The seed: chat "Builds" with R1 (two versions, a size hint), R2 (a
// declared popoutSize, reads /data/), R3 (an image card), a reply file
// (files/mabc123/report.md) and a Drive link; R4.tsx alone, from before
// versions.
//
//   ART-01 GET /module: head, a pinned version, unknown names, mcp:
//   ART-02 the start migrates R4 (v1, head 1); GET /apps/meta
//   ART-03 GET /app/info, and 404
//   ART-04 POST /app/meta: title, file (version files renamed), description,
//          w/h clamps, icon adopt and remove; GET /app/icon
//   ART-05 POST /app/version and /app/version/label
//   ART-06 POST /publish; GET /widgets; the w chunk
//   ART-07 POST /widget-meta; GET /catalog
//   ART-08 POST /reuse
//   ART-09 POST /unpublish (the file goes 700 ms later)
//   ART-10 GET /artifacts: the thread's apps first, then by title; files
//   ART-11 GET /artifact/info: an image, a file, a Drive file, an app; 404
//   ART-12 POST /artifact/meta, GET /artifact/icon
//   ART-13 POST /artifact/hide
//   ART-14 POST /artifact/rename-remote through the Drive stub
//   ART-15 POST /artifact/delete
//   ART-16 POST /app/delete: rows, tombstone, transcript, files
//   ART-17 GET /sources
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { after, before, test } from "node:test";
import { dbDiff } from "../tools/db-diff.mjs";
import { PORT, dbRows, nodeIn, offlineEnv, postJson, request, startServer, stubCalls, tempDir, waitFor } from "./helpers.mjs";

const dataDir = tempDir("artifacts");
const GEN = join(dataDir, "generated");
const stubs = tempDir("artifacts-stubs");
const LOG = join(stubs, "stubs.log");
const ORIGIN = `http://127.0.0.1:${PORT}`;
const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
const DRIVE_ID = "1AbCdEfGhIjK";
const CODE = {
  R1v1: 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text>count</Text>;\n}\n',
  R1v2: 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text color="blue">count</Text>;\n}\n',
  R2: 'import { Text } from "nsh";\n\nexport const popoutSize = { w: 500, h: 420 };\n// reads /data/timeclock\nexport default function R() {\n  return <Text>timer</Text>;\n}\n',
  R3: 'import { Image } from "nsh";\n\nexport default function R() {\n  return <Image src="asset:/generated/assets/img-cat.png" />;\n}\n',
  R4: 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text>legacy</Text>;\n}\n',
};
const LINK = { provider: "gdrive", id: DRIVE_ID, name: "Plan", mime: "application/vnd.google-apps.document", link: `https://docs.google.com/document/d/${DRIVE_ID}/edit`, action: "created", at: 1790000000000, account: "owner@example.com" };
const REPORT = { path: "mabc123/report.md", name: "report.md", ext: "md", kind: "markdown", size: 5 };
let server;
let chat = "";

// a stdio MCP server standing in for google-workspace: update_drive_file
// answers as workspace-mcp does; each call is logged
const DRIVE_STUB = join(stubs, "drive-stub.mjs");
const DRIVE_CALLS = join(stubs, "drive-calls.log");

writeFileSync(DRIVE_STUB, `
import { appendFileSync } from "node:fs";
import { createInterface } from "node:readline";
const send = (m) => process.stdout.write(JSON.stringify(m) + "\\n");
function answer(method, params) {
  if (method === "initialize") return { protocolVersion: params?.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: "drive-stub", version: "0.0.0" } };
  if (method === "ping") return {};
  if (method === "tools/list") return { tools: [{ name: "update_drive_file", description: "x", inputSchema: { type: "object" } }] };
  if (method === "resources/list") return { resources: [] };
  if (method === "tools/call") {
    appendFileSync(${JSON.stringify(DRIVE_CALLS)}, JSON.stringify({ tool: params.name, args: params.arguments }) + "\\n");
    const a = params.arguments || {};
    return { content: [{ type: "text", text: "Successfully updated file: " + a.name + "\\nFile ID: " + a.file_id + "\\n• Name: " + a.name + "\\nView file: https://docs.google.com/document/d/" + a.file_id + "/edit?renamed=1" }] };
  }
  return null;
}
createInterface({ input: process.stdin }).on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id === undefined) return;
  const result = answer(msg.method, msg.params);
  send(result ? { jsonrpc: "2.0", id: msg.id, result } : { jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "no method" } });
}).on("close", () => process.exit(0));
`);

const env = () => ({ ...offlineEnv(LOG), LINEAR_API_KEY: undefined, SECRET_STUB_LINEAR_API_KEY: undefined });

// ── db-diff: what a route changed ──────────────────────────────────────
let snaps = 0;

function snapshot() {
  const dir = join(stubs, `snap-${++snaps}`, "generated");

  mkdirSync(dir, { recursive: true });
  const file = join(dir, "app.db");
  const db = new DatabaseSync(join(GEN, "app.db"), { timeout: 5000 });

  try {
    db.exec(`VACUUM INTO '${file}'`);
  } finally {
    db.close();
  }

  return file;
}

/** The rows `run` changed: ["+ table {row}", "- table {row}"], sorted. */
async function changes(run) {
  const a = snapshot();
  const out = await run();
  const b = snapshot();

  return { out, rows: dbDiff(a, b).map((d) => `${d.side === "b" ? "+" : "-"} ${d.table} ${d.row}`).sort() };
}

function sql(q, ...args) {
  const db = new DatabaseSync(join(GEN, "app.db"), { timeout: 5000 });

  try {
    return db.prepare(q).run(...args);
  } finally {
    db.close();
  }
}

const turns = async (id) => (await request("GET", `/turns?thread=${id}`)).json.turns;
const mtime = (p) => Math.round(statSync(p).mtimeMs);
const versions = (name) => dbRows(dataDir, "SELECT v, parent, created_at, label FROM versions WHERE name=? ORDER BY v", name);

before(async () => {
  // Node never makes chunks/ (Rox's app does): without it every chunk copy
  // fails quietly and each start builds them all again
  mkdirSync(join(dataDir, "chunks"), { recursive: true });
  chat = nodeIn(dataDir, `
    const { store, nextThreadSeq, uuidv7, writeIndex } = await import("./src/state.mjs");
    const { writeVersion } = await import("./src/pipeline.mjs");
    const { GEN_DIR, OUT_DIR, ASSETS_DIR } = await import("./src/config.mjs");
    const { mkdirSync, writeFileSync } = await import("node:fs");
    const { join } = await import("node:path");
    const C = ${JSON.stringify(CODE)};
    writeVersion("R1", C.R1v1, null, "make a counter");
    writeVersion("R1", C.R1v2, 1, "make it blue");
    writeVersion("R2", C.R2, null, "make a timer");
    mkdirSync(ASSETS_DIR, { recursive: true });
    writeFileSync(join(ASSETS_DIR, "img-cat.png"), Buffer.from(${JSON.stringify(PNG.toString("base64"))}, "base64"));
    writeVersion("R3", C.R3, null, "draw a cat");
    writeFileSync(join(GEN_DIR, "R4.tsx"), C.R4);
    mkdirSync(join(OUT_DIR, "mabc123"), { recursive: true });
    writeFileSync(join(OUT_DIR, "mabc123", "report.md"), "# Q3\\n");
    const tid = "t" + nextThreadSeq();
    const u = (o) => ({ id: uuidv7(), ...o });
    store.list.push({ id: tid, uid: uuidv7(), title: "Builds", turns: [
      u({ role: "user", text: "make a counter" }),
      u({ role: "ai", name: "R1", label: "mock", caption: "Counter", art: { title: "Counter", desc: "Counts clicks", w: 400, h: 300 }, v: 1 }),
      u({ role: "user", text: "make it blue" }),
      u({ role: "ai", name: "R1", label: "mock", caption: "Counter", v: 2 }),
      u({ role: "user", text: "make a timer" }),
      u({ role: "ai", name: "R2", label: "mock", caption: "Timer" }),
      u({ role: "user", text: "draw a cat" }),
      u({ role: "ai", name: "R3", label: "image", art: { kind: "image", file: "img-cat.png", w: 64, h: 64, title: "Cat" } }),
      u({ role: "user", text: "write the report" }),
      u({ role: "ai", text: "", label: "mock", files: [${JSON.stringify(REPORT)}] }),
      u({ role: "user", text: "make a plan doc" }),
      u({ role: "ai", text: "Made it.", label: "mock", links: [${JSON.stringify(LINK)}] }),
    ] });
    store.active = tid;
    writeIndex();
    await new Promise((r) => setTimeout(r, 300));
    console.log(tid);
  `, env());
  server = await startServer(dataDir, { env: env() });
});

after(async () => {
  await server?.stop();
});

test("ART-01 GET /module: the head, a pinned version, unknown names, an mcp: name", async () => {
  const r = await request("GET", "/module?name=R1");

  assert.equal(r.status, 200);
  assert.match(r.headers["content-type"], /^application\/json/);
  assert.equal(r.text, JSON.stringify({ source: CODE.R1v2, published: false, v: 2, head: 2, versions: versions("R1"), title: null }));
  assert.equal((await request("GET", "/module?name=R1@v1")).text, JSON.stringify({ source: CODE.R1v1, published: false, v: 1, head: 2, versions: versions("R1"), title: null }));

  for (const name of ["R9", "", "R1@v9", "x", "R1@v", "../R1"]) assert.equal((await request("GET", `/module?name=${encodeURIComponent(name)}`)).text, '{"source":"","published":false}', name);
  assert.equal((await request("GET", "/module")).text, '{"source":"","published":false}');
  assert.equal((await request("GET", "/module?name=mcp:nope/app")).text, '{"source":"","error":"no such server"}');
});

test("ART-02 the start migrates R4 to v1; GET /apps/meta lists every live app, numerically", async () => {
  assert.deepEqual(versions("R4").map(({ v, parent, label }) => ({ v, parent, label })), [{ v: 1, parent: null, label: null }]);
  assert.deepEqual(dbRows(dataDir, "SELECT prompt FROM versions WHERE name='R4'"), [{ prompt: null }]);
  assert.equal(readFileSync(join(GEN, "R4", "R4.v1.tsx"), "utf8"), CODE.R4);
  assert.deepEqual(dbRows(dataDir, "SELECT v FROM heads WHERE name='R4'"), [{ v: 1 }]);
  // the chunk backfill builds what has no chunk: R<n>.js and R<n>v<k>.js
  assert.ok(await waitFor(() => existsSync(join(dataDir, "chunks", "R4v1.js")), 10000), "R4v1.js");
  assert.deepEqual(readdirSync(join(dataDir, "chunks")).sort(), ["R1.js", "R1v1.js", "R1v2.js", "R2.js", "R2v1.js", "R3.js", "R3v1.js", "R4.js", "R4v1.js"]);
  assert.ok(stubCalls(LOG).some((c) => c.bin === "nsh" && c.argv.join(" ") === `build ${join(GEN, "R4", "R4.v1.tsx")} --out ${join(dataDir, ".stage", "chunk-R4v1")}`));
  const updated = (name) => Math.round(Math.max(...versions(name).map((x) => x.created_at)));
  const r = await request("GET", "/apps/meta");

  assert.equal(
    r.text,
    JSON.stringify({
      apps: [
        { name: "R1", title: "R1", file: "R1.tsx", description: "Counts clicks", icon: "", w: 400, h: 300, v: 2, updated_at: updated("R1") },
        { name: "R2", title: "R2", file: "R2.tsx", description: "", icon: "", w: 500, h: 420, v: 1, updated_at: updated("R2") },
        // an image card's size hint (turn.art) is a size like any other
        { name: "R3", title: "R3", file: "R3.tsx", description: "", icon: "", w: 64, h: 64, v: 1, updated_at: updated("R3") },
        { name: "R4", title: "R4", file: "R4.tsx", description: "", icon: "", w: 0, h: 0, v: 1, updated_at: updated("R4") },
      ],
    }),
  );
});

test("ART-03 GET /app/info: what the details modal shows; 404 for no app", async () => {
  const r = await request("GET", "/app/info?name=R1");
  const vs = versions("R1");
  const path = join(GEN, "R1", "R1.v2.tsx");

  assert.equal(r.status, 200);
  assert.equal(
    r.text,
    JSON.stringify({
      ok: true, generating: false, icon_error: "",
      name: "R1", title: "R1", file: "R1.tsx", description: "Counts clicks", icon: "", w: 400, h: 300, v: 2, updated_at: Math.round(vs[1].created_at),
      icon_prompt: "", versions: 2, history: vs, created_at: Math.round(vs[0].created_at), prompt: "make a counter", path, bytes: statSync(path).size,
      published: false, thread_id: chat, thread_title: "Builds", bot_name: "",
    }),
  );

  for (const name of ["R9", "x", ""]) {
    const no = await request("GET", `/app/info?name=${name}`);

    assert.deepEqual([no.status, no.json], [404, { ok: false, error: `no such app ${name}` }]);
  }
});

test("ART-04 POST /app/meta: title, file, description, sizes, icon; GET /app/icon", async () => {
  const { out, rows } = await changes(() => postJson("/app/meta", { name: "R1", title: "  Tally Counter  ", description: " Counts things ", w: 50, h: 5000 }));

  assert.equal(out.status, 200, out.text);
  assert.deepEqual(rows, [
    '+ apps {"name":"R1","title":"Tally Counter","file":null,"description":"Counts things","icon":null,"icon_prompt":null,"w":120,"h":2400}',
  ]);
  assert.deepEqual([out.json.ok, out.json.title, out.json.file, out.json.description, out.json.w, out.json.h], [true, "Tally Counter", "R1.tsx", "Counts things", 120, 2400]);
  // the title's slug names the version files now
  assert.deepEqual(readdirSync(join(GEN, "R1")).filter((f) => f.endsWith(".tsx")).sort(), ["tally-counter.v1.tsx", "tally-counter.v2.tsx"]);
  assert.equal(out.json.path, join(GEN, "R1", "tally-counter.v2.tsx"));
  // a file label wins over the title's slug; omitted fields keep their values
  const f = await postJson("/app/meta", { name: "R1", file: "My Counter!.tsx", w: 0 });

  assert.deepEqual([f.json.title, f.json.file, f.json.w, f.json.h], ["Tally Counter", "my-counter.tsx", 400, 2400], "w 0 clears it: the hint shows");
  assert.deepEqual(readdirSync(join(GEN, "R1")).filter((x) => x.endsWith(".tsx")).sort(), ["my-counter.v1.tsx", "my-counter.v2.tsx"]);
  assert.equal((await request("GET", "/module?name=R1@v1")).json.source, CODE.R1v1, "a pinned version still resolves");
  const bad = await postJson("/app/meta", { name: "R1", w: "wide" });

  assert.deepEqual([bad.status, bad.json], [400, { ok: false, error: "bad size wide" }]);
  assert.deepEqual((await postJson("/app/meta", { name: "R9", title: "x" })).json, { ok: false, error: "no such module R9" });
  // an icon: any image sips reads (the stub copies it) becomes icon.png
  assert.equal((await request("GET", "/app/icon?name=R1")).status, 404);
  const img = join(stubs, "icon-src.png");

  writeFileSync(img, PNG);
  const withIcon = await changes(() => postJson("/app/meta", { name: "R1", icon: `file://${img}` }));
  const icon = join(GEN, "R1", "icon.png");

  assert.equal(withIcon.out.status, 200, withIcon.out.text);
  assert.equal(withIcon.out.json.icon, `${ORIGIN}/app/icon?name=R1&t=${mtime(icon)}`);
  assert.deepEqual(withIcon.rows, [
    `+ apps {"name":"R1","title":"Tally Counter","file":"my-counter.tsx","description":"Counts things","icon":${JSON.stringify(icon)},"icon_prompt":null,"w":null,"h":2400}`,
    '- apps {"name":"R1","title":"Tally Counter","file":"my-counter.tsx","description":"Counts things","icon":null,"icon_prompt":null,"w":null,"h":2400}',
  ]);
  assert.ok(stubCalls(LOG).some((c) => c.bin === "sips" && c.argv.join(" ").startsWith(`-s format png -z 512 512 ${img} --out ${icon}.`)));
  const served = await request("GET", "/app/icon?name=R1&t=1");

  assert.equal(served.status, 200);
  assert.equal(served.headers["content-type"], "image/png");
  assert.equal(served.headers["cache-control"], "no-cache");
  assert.equal((await postJson("/app/meta", { name: "R1", icon: "/nope/missing.png" })).json.error, "no such image /nope/missing.png");
  assert.equal((await postJson("/app/meta", { name: "R1", icon: "relative.png" })).json.error, "no such image relative.png");
  // "" removes it
  const removed = await postJson("/app/meta", { name: "R1", icon: "" });

  assert.equal(removed.json.icon, "");
  assert.equal(existsSync(icon), false);
  const gone = await request("GET", "/app/icon?name=R1");

  assert.deepEqual([gone.status, gone.text], [404, "no icon"]);
  assert.equal((await request("GET", "/app/icon?name=../x")).status, 404);
});

test("ART-05 POST /app/version moves the head; /app/version/label names a version", async () => {
  const { out, rows } = await changes(() => postJson("/app/version", { name: "R1", v: 1 }));

  assert.equal(out.text, '{"ok":true,"v":1}');
  assert.deepEqual(rows, ['+ heads {"name":"R1","v":1}', '- heads {"name":"R1","v":2}']);
  assert.equal(readFileSync(join(GEN, "R1.tsx"), "utf8"), CODE.R1v1, "the head mirror");
  assert.deepEqual([(await request("GET", "/module?name=R1")).json.v, (await request("GET", "/module?name=R1")).json.head], [1, 1]);

  for (const body of [{ name: "R1", v: 9 }, { name: "R9", v: 1 }, { name: "R1" }]) assert.deepEqual(await postJson("/app/version", body).then((r) => [r.status, r.json]), [400, { ok: false, error: "no such version" }]);
  await postJson("/app/version", { name: "R1", v: 2 });
  const label = await changes(() => postJson("/app/version/label", { name: "R1", v: 1, label: "  first draft  " }));

  assert.equal(label.out.text, '{"ok":true}');
  assert.deepEqual(label.rows, ['+ versions {"name":"R1","v":1,"parent":null,"prompt":"make a counter","label":"first draft"}', '- versions {"name":"R1","v":1,"parent":null,"prompt":"make a counter","label":null}']);
  await postJson("/app/version/label", { name: "R1", v: 2, label: "x".repeat(50) });
  assert.equal(versions("R1")[1].label, "x".repeat(40));
  await postJson("/app/version/label", { name: "R1", v: 2, label: "   " });
  assert.equal(versions("R1")[1].label, null, "blank clears it");
  assert.deepEqual(await postJson("/app/version/label", { name: "R1", v: 7, label: "x" }).then((r) => [r.status, r.json]), [400, { ok: false, error: "no such version" }]);
});

test("ART-06 POST /publish copies the app to widgets/ with its caption; GET /widgets", async () => {
  const { out, rows } = await changes(() => postJson("/publish", { name: "R2" }));

  assert.equal(out.text, '{"ok":true,"title":"Timer"}');
  assert.deepEqual(rows, ['+ widgets {"name":"R2","title":"Timer","prompt":"make a timer"}']);
  assert.equal(readFileSync(join(dataDir, "widgets", "R2.tsx"), "utf8"), CODE.R2);
  assert.ok(await waitFor(() => stubCalls(LOG).some((c) => c.bin === "nsh" && c.argv[0] === "build" && c.argv[1] === join(dataDir, "widgets", "R2.tsx")), 5000), "the w chunk");
  assert.ok(await waitFor(() => existsSync(join(dataDir, "chunks", "R2w.js")), 5000));
  const w = await request("GET", "/widgets");

  assert.equal(w.text, JSON.stringify([{ name: "R2", title: "Timer", prompt: "make a timer", published_at: dbRows(dataDir, "SELECT published_at FROM widgets")[0].published_at }]));
  assert.match(w.json[0].published_at, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  assert.equal((await request("GET", "/module?name=R2")).json.published, true);
  const img = await postJson("/publish", { name: "R4" });

  assert.equal(img.text, '{"ok":true,"title":"R4"}', "no card anywhere: its name");
  await postJson("/unpublish", { name: "R4" });

  for (const name of ["R99", "x", ""]) assert.deepEqual(await postJson("/publish", { name }).then((r) => [r.status, r.json]), [400, { ok: false, error: `no such generated module ${name}` }]);
});

test("ART-07 POST /widget-meta and GET /catalog", async () => {
  const cat = async () => (await request("GET", "/catalog")).json.list;
  const base = { name: "R2", title: "Timer", publishedAt: (await request("GET", "/widgets")).json[0].published_at };

  assert.deepEqual(await cat(), [{ ...base, kinds: ["api"], archived: false, tags: [] }]);
  const tags = await changes(() => postJson("/widget-meta", { name: "R2", tags: [" Work ", "", "TIME", 5, "a", "b", "c", "d", "e", "f"] }));

  assert.equal(tags.out.text, '{"ok":true,"tags":["work","time","5","a","b","c","d","e"]}');
  assert.deepEqual(tags.rows, ['+ meta {"k":"tags:R2","v":"work,time,5,a,b,c,d,e"}']);
  const renamed = await changes(() => postJson("/widget-meta", { name: "R2", title: "  My **Timer**: v2  ", archived: true }));

  assert.equal(renamed.out.text, '{"ok":true}');
  assert.deepEqual(renamed.rows, ['+ meta {"k":"arch:R2","v":"1"}', '+ meta {"k":"title:R2","v":"My **Timer**: v2"}']);
  // state saved for it: "db" comes first
  assert.equal((await postJson("/state/R2", { n: 1 })).status, 200);
  assert.deepEqual(await cat(), [{ ...base, title: "My **Timer**: v2", kinds: ["db", "api"], archived: true, tags: ["work", "time", "5", "a", "b", "c", "d", "e"] }]);
  // clearing
  assert.equal((await postJson("/widget-meta", { name: "R2", tags: [], title: "", archived: false })).text, '{"ok":true,"tags":[]}');
  assert.deepEqual(await cat(), [{ ...base, kinds: ["db", "api"], archived: false, tags: [] }], "the caption, cleaned: up to the first : . or dash");
  assert.equal((await postJson("/widget-meta", { name: "R2", tags: "work" })).text, '{"ok":false}');
  assert.equal((await postJson("/widget-meta", { tags: [] })).text, '{"ok":false}');
  const bad = await request("POST", "/widget-meta", { headers: { "Content-Type": "application/json" }, body: "{" });

  assert.equal(bad.status, 200, "a parse error is still a 200");
  assert.equal(bad.json.ok, false);
  assert.match(bad.json.error, /^SyntaxError: /);
  // BUG: archived without a name writes the row `arch:undefined` before the
  // name is checked (routes/apps.mjs /widget-meta); the answer is {ok:false}
  const noName = await changes(() => postJson("/widget-meta", { archived: true }));

  assert.equal(noName.out.text, '{"ok":false}');
  assert.deepEqual(noName.rows, ['+ meta {"k":"arch:undefined","v":"1"}']);
});

test("ART-08 POST /reuse posts the published app as a widget card in the active chat", async () => {
  const { out } = await changes(() => postJson("/reuse", { name: "R2" }));

  assert.equal(out.text, '{"ok":true}');
  const last = (await turns(chat)).at(-1);

  assert.deepEqual([last.role, last.name, last.label, last.caption, last.reaction, last.retries, last.widget], ["ai", "R2", "widget", "", "", 0, true]);
  assert.deepEqual(await postJson("/reuse", { name: "R1" }).then((r) => [r.status, r.json]), [400, { ok: false, error: "no published widget R1" }]);
  // BUG: no name reaches SQLite unchecked (widgetRow(undefined)); the
  // refusal carries node:sqlite's own message
  assert.deepEqual(await postJson("/reuse", {}).then((r) => [r.status, r.json]), [400, { ok: false, error: "Provided value cannot be bound to SQLite parameter 1." }]);
});

test("ART-09 POST /unpublish: the row at once, the file 700 ms later", async () => {
  const { out, rows } = await changes(() => postJson("/unpublish", { name: "R2" }));

  assert.equal(out.text, '{"ok":true}');
  assert.deepEqual(rows, ['- widgets {"name":"R2","title":"Timer","prompt":"make a timer"}']);
  assert.ok(existsSync(join(dataDir, "widgets", "R2.tsx")), "not yet");
  assert.ok(await waitFor(() => !existsSync(join(dataDir, "widgets", "R2.tsx")), 3000), "gone after the delay");
  assert.deepEqual((await request("GET", "/catalog")).json, { list: [] });
  assert.equal((await postJson("/unpublish", { name: "R2" })).text, '{"ok":true}', "twice is fine");
});

test("ART-10 GET /artifacts: the thread's apps first, then by title; reply files; q filters", async () => {
  const all = (await request("GET", `/artifacts?thread=${chat}`)).json.items;

  assert.deepEqual(all.map((x) => [x.kind, x.name, x.scope]), [
    ["app", "R2", "thread"],
    ["app", "R3", "thread"],
    ["app", "R1", "thread"],
    ["app", "R4", "global"],
    ["file", "report.md", "global"],
  ]);
  assert.equal(
    JSON.stringify(all.find((x) => x.name === "R1")),
    JSON.stringify({ kind: "app", name: "R1", title: "Tally Counter", file: "my-counter.tsx", description: "Counts things", icon: "", v: 2, published: false, uri: "module://R1", scope: "thread" }),
  );
  assert.equal(JSON.stringify(all.at(-1)), JSON.stringify({ kind: "file", name: "report.md", title: "report.md", file: "mabc123/report.md", uri: `file://${join(GEN, "files", "mabc123", "report.md")}`, scope: "global" }));
  const none = (await request("GET", "/artifacts")).json.items;

  assert.deepEqual(none.map((x) => [x.name, x.scope]), [["R2", "global"], ["R3", "global"], ["R4", "global"], ["report.md", "global"], ["R1", "global"]], "by title alone");
  assert.deepEqual((await request("GET", "/artifacts?q=TALLY")).json.items.map((x) => x.name), ["R1"]);
  assert.deepEqual((await request("GET", "/artifacts?q=report")).json.items.map((x) => x.name), ["report.md"]);
});

test("ART-11 GET /artifact/info: an image, a reply file, a Drive file, an app; 404", async () => {
  const img = (await request("GET", "/artifact/info?id=R3")).json;
  const asset = join(GEN, "assets", "img-cat.png");

  assert.equal(
    JSON.stringify(img),
    JSON.stringify({
      ok: true, id: "R3", name: "R3", kind: "image", title: "Cat", filename: "img-cat.png", file: "img-cat.png", description: "", icon: "",
      meta: { w: 64, h: 64 }, w: 64, h: 64, i: 7, origin: img.origin, storage: { where: "mac", uri: `file://${asset}` }, versioning: "rox", published: false,
      mtime: statSync(asset).mtimeMs, v: 1, versions: 1, history: versions("R3"), thread_id: chat, thread_title: "Builds", bot_name: "",
      created_at: Math.round(versions("R3")[0].created_at), updated_at: null, prompt: "draw a cat", path: asset, bytes: PNG.length,
    }),
  );
  const ts = await turns(chat);
  const uid = (await request("GET", "/threads")).json.list.find((t) => t.id === chat).uid;

  assert.equal(JSON.stringify(img.origin), JSON.stringify({ thread: chat, uid, i: 7, turn: ts[7].id }));
  const file = (await request("GET", `/artifact/info?id=${encodeURIComponent("file:mabc123/report.md")}`)).json;
  const p = join(GEN, "files", "mabc123", "report.md");

  assert.deepEqual(Object.keys(file), ["ok", "id", "name", "kind", "title", "filename", "file", "description", "icon", "meta", "attach", "i", "origin", "storage", "versioning", "published", "mtime", "v", "versions", "history", "thread_id", "thread_title", "bot_name", "created_at", "updated_at", "prompt", "path", "bytes"]);
  assert.deepEqual([file.kind, file.title, file.filename, file.file, file.i, file.versioning, file.v, file.versions, file.prompt, file.path, file.bytes], ["document", "report.md", "report.md", "mabc123/report.md", 9, "rox", 0, 0, "", p, 5]);
  assert.deepEqual(file.attach, REPORT);
  assert.equal(file.meta.size, 5);
  assert.equal(file.meta.ext, "md");
  const drive = (await request("GET", `/artifact/info?id=gdrive:${DRIVE_ID}`)).json;

  assert.equal(
    JSON.stringify(drive),
    JSON.stringify({
      ok: true, id: `gdrive:${DRIVE_ID}`, name: `gdrive:${DRIVE_ID}`, kind: "document", title: "Plan", filename: "Plan", file: "", description: "", icon: "",
      meta: { provider: "Google Drive", type: "Google Doc", mime: LINK.mime, action: "created" }, i: 11, origin: drive.origin,
      storage: { where: "connector", provider: "gdrive", link: LINK.link, uri: LINK.link }, versioning: "provider", published: false, mtime: LINK.at, created_at: LINK.at, v: 0, versions: 0,
      history: [], thread_id: chat, thread_title: "Builds", bot_name: "", updated_at: LINK.at, prompt: "", path: "", bytes: 0,
    }),
  );
  assert.equal(JSON.stringify(drive.origin), JSON.stringify({ thread: chat, uid, i: 11, turn: ts[11].id }));
  // an app's info is /app/info's
  assert.equal((await request("GET", "/artifact/info?id=R1")).text, (await request("GET", "/app/info?name=R1")).text);

  for (const id of ["R9", "file:nope.md", "gdrive:nope", "attach:nope", ""]) {
    const r = await request("GET", `/artifact/info?id=${encodeURIComponent(id)}`);

    assert.equal(r.status, 404, id);
    assert.equal(r.json.ok, false);
  }
  assert.equal((await request("GET", "/artifact/info?id=R9")).json.error, "no such app R9", "an R-name without an image is an app");
  assert.equal((await request("GET", "/artifact/info?id=gdrive:nope")).json.error, "no such artifact gdrive:nope");
});

test("ART-12 POST /artifact/meta: a file's and an image's details in artifact_meta, an app's in apps; GET /artifact/icon", async () => {
  const id = "file:mabc123/report.md";
  const f = await changes(() => postJson("/artifact/meta", { id, title: "  Q3 report  ", filename: "Q3: final/report.MD", description: "The numbers" }));

  assert.equal(f.out.status, 200, f.out.text);
  assert.deepEqual(f.rows, ['+ artifact_meta {"id":"file:{rel}/report.md","title":"Q3 report","file":"Q3- final-report.md","description":"The numbers","icon":null}']);
  assert.deepEqual([f.out.json.title, f.out.json.filename, f.out.json.description, f.out.json.attach.name], ["Q3 report", "Q3- final-report.md", "The numbers", "Q3- final-report.md"]);
  assert.equal(typeof f.out.json.updated_at, "number");
  // a file card shows its edited display name
  assert.equal((await turns(chat)).find((u) => u.files.length).files[0].name, "Q3- final-report.md");
  assert.equal((await postJson("/artifact/meta", { id, file: "" })).json.filename, "report.md", "a blank name clears it");
  const img = await postJson("/artifact/meta", { id: "R3", title: "Kitty" });

  assert.equal(img.json.title, "Kitty");
  assert.deepEqual(dbRows(dataDir, "SELECT id, title FROM artifact_meta WHERE id='R3'"), [{ id: "R3", title: "Kitty" }]);
  const app = await changes(() => postJson("/artifact/meta", { id: "R4", title: "Legacy", filename: "old one" }));

  assert.deepEqual(app.rows, ['+ apps {"name":"R4","title":"Legacy","file":"old-one.tsx","description":null,"icon":null,"icon_prompt":null,"w":null,"h":null}']);
  assert.equal(app.out.json.file, "old-one.tsx");
  assert.deepEqual(await postJson("/artifact/meta", { id: "file:nope.md", title: "x" }).then((r) => [r.status, r.json]), [400, { ok: false, error: "no such artifact file:nope.md" }]);
  // an icon of its own, under generated/icons
  const src = join(stubs, "art-icon.png");

  writeFileSync(src, PNG);
  const withIcon = await postJson("/artifact/meta", { id, icon: src });

  assert.match(withIcon.json.icon, new RegExp(`^${ORIGIN}/artifact/icon\\?id=file%3Amabc123%2Freport\\.md&t=\\d+$`));
  const served = await request("GET", `/artifact/icon?id=${encodeURIComponent(id)}`);

  assert.deepEqual([served.status, served.headers["content-type"], served.headers["cache-control"]], [200, "image/png", "no-cache"]);
  assert.equal((await postJson("/artifact/meta", { id, icon: "" })).json.icon, "");
  assert.deepEqual(await request("GET", `/artifact/icon?id=${encodeURIComponent(id)}`).then((r) => [r.status, r.text]), [404, "no icon"]);
  assert.equal((await request("GET", "/artifact/icon")).status, 404);
});

test("ART-13 POST /artifact/hide: out of the Library and back", async () => {
  const hide = await changes(() => postJson("/artifact/hide", { id: "R3" }));

  assert.equal(hide.out.text, '{"ok":true}');
  assert.deepEqual(hide.rows, ['+ library_hidden {"id":"R3"}']);
  const lib = async () => (await request("GET", `/thread/apps?thread=${chat}`)).json.apps.find((x) => x.id === "R3");

  assert.equal((await lib()).archived, true, "still listed, flagged");

  for (const id of [`gdrive:${DRIVE_ID}`, "file:mabc123/report.md", "attach:a1"]) assert.equal((await postJson("/artifact/hide", { id })).status, 200, id);
  const back = await changes(() => postJson("/artifact/hide", { id: "R3", hidden: false }));

  assert.deepEqual(back.rows, ['- library_hidden {"id":"R3"}']);
  assert.equal((await lib()).archived, undefined);
  assert.deepEqual(await postJson("/artifact/hide", { id: "x" }).then((r) => [r.status, r.json]), [400, { ok: false, error: "bad artifact id x" }]);
  assert.deepEqual(await postJson("/artifact/hide", {}).then((r) => [r.status, r.json]), [400, { ok: false, error: "bad artifact id " }]);
});

test("ART-14 POST /artifact/rename-remote renames a Drive file through the connected server (a stub)", async () => {
  const is = async (body, error) => assert.deepEqual(await postJson("/artifact/rename-remote", body).then((r) => [r.status, r.json]), [400, { ok: false, error }], JSON.stringify(body));

  await is({ id: "R1", name: "x" }, "R1 is not stored in a connected app");
  await is({ id: `gdrive:${DRIVE_ID}`, name: "   " }, "a file needs a name");
  await is({ id: "gdrive:nope", name: "x" }, "no such artifact gdrive:nope");
  await is({ id: `gdrive:${DRIVE_ID}`, name: "Plan v2" }, "Google Drive isn't connected");
  sql(
    "INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at, url, auth) VALUES(?,?,?,?,?,?,?,?,?)",
    "google-workspace", "Google Workspace", process.execPath, JSON.stringify([DRIVE_STUB]), "{}", 1, Date.now(), null, null,
  );
  const r = await postJson("/artifact/rename-remote", { id: `gdrive:${DRIVE_ID}`, name: "  Plan v2  " });

  assert.equal(r.status, 200, r.text);
  assert.deepEqual(readFileSync(DRIVE_CALLS, "utf8").trim().split("\n").map((l) => JSON.parse(l)), [
    { tool: "update_drive_file", args: { file_id: DRIVE_ID, name: "Plan v2", user_google_email: "owner@example.com" } },
  ]);
  assert.deepEqual([r.json.title, r.json.meta.action, r.json.storage.link], ["Plan v2", "renamed", `https://docs.google.com/document/d/${DRIVE_ID}/edit?renamed=1`]);
  // the rename is recorded on the turn that holds the link
  const chip = (await turns(chat)).find((u) => u.links.length).links[0];

  assert.deepEqual([chip.name, chip.action, chip.link], ["Plan v2", "renamed", `https://docs.google.com/document/d/${DRIVE_ID}/edit?renamed=1`]);
  assert.equal(chip.account, undefined, "the account never leaves the broker");
});

test("ART-15 POST /artifact/delete: a reply file leaves the chat, the disk, its groups and the Library", async () => {
  const id = "file:mabc123/report.md";

  sql("INSERT INTO group_items(group_id, kind, ref, thread, ord) VALUES(?,?,?,?,?)", "g1", "artifact", id, chat, 0);
  sql("INSERT INTO group_items(group_id, kind, ref, thread, ord) VALUES(?,?,?,?,?)", "g1", "artifact", "R2", chat, 1);
  await postJson("/artifact/meta", { id, title: "Q3" });
  const before = (await turns(chat)).length;
  const { out, rows } = await changes(() => postJson("/artifact/delete", { id }));

  assert.equal(out.text, '{"ok":true}');
  // db-diff reads "mabc123/" as {rel}; persist() renumbers the later turns' seq
  assert.deepEqual(rows.filter((x) => !x.includes(" turns ")), [
    '- artifact_meta {"id":"file:{rel}/report.md","title":"Q3","file":null,"description":"The numbers","icon":null}',
    `- group_items {"group_id":"g1","kind":"artifact","ref":"file:{rel}/report.md","thread":"${chat}","ord":0,"title":null}`,
    '- library_hidden {"id":"file:{rel}/report.md"}',
  ]);
  const gone = rows.filter((x) => x.startsWith("- turns") && x.includes("report.md"));

  assert.equal(gone.length, 1);
  assert.match(gone[0], /"role":"ai","text":"","name":null,"label":"mock".*"meta":"\{\\"files\\":\[\{\\"path\\":\\"\{rel\}\/report\.md\\"/);
  assert.equal(rows.filter((x) => x.startsWith("- turns")).length - rows.filter((x) => x.startsWith("+ turns")).length, 1);
  assert.equal((await turns(chat)).length, before - 1, "the reply that held only the file is gone");
  assert.equal(existsSync(join(GEN, "files", "mabc123", "report.md")), false);
  assert.deepEqual(await postJson("/artifact/delete", { id: "R1" }).then((r) => [r.status, r.json]), [400, { ok: false, error: "R1 isn't a file — use /app/delete" }]);
});

test("ART-16 POST /app/delete: rows, tombstone, the transcript, and the files 700 ms later", async () => {
  sql("INSERT INTO kv(name, data) VALUES(?, ?)", "R1", "{}");
  sql("INSERT INTO group_items(group_id, kind, ref, thread, ord) VALUES(?,?,?,?,?)", "g1", "artifact", "R1@v2", chat, 2);
  await postJson("/artifact/hide", { id: "R1" });
  const before = await turns(chat);
  const { out, rows } = await changes(() => postJson("/app/delete", { name: "R1" }));

  assert.equal(out.text, '{"ok":true}');
  const dropped = rows.filter((x) => !x.includes(" turns ") && !x.includes(" threads "));

  assert.deepEqual(dropped, [
    '+ tombstones {"name":"R1"}',
    '- apps {"name":"R1","title":"Tally Counter","file":"my-counter.tsx","description":"Counts things","icon":null,"icon_prompt":null,"w":null,"h":2400}',
    `- group_items {"group_id":"g1","kind":"artifact","ref":"R1@v2","thread":"${chat}","ord":2,"title":null}`,
    '- heads {"name":"R1","v":2}',
    '- kv {"name":"R1","data":"{}"}',
    '- library_hidden {"id":"R1"}',
    '- versions {"name":"R1","v":1,"parent":null,"prompt":"make a counter","label":"first draft"}',
    '- versions {"name":"R1","v":2,"parent":1,"prompt":"make it blue","label":null}',
  ]);
  // every ai turn naming it goes, and the user prompt right above it
  const after = await turns(chat);

  assert.deepEqual(after.map((u) => u.text || u.name), before.filter((u) => u.name !== "R1" && !["make a counter", "make it blue"].includes(u.text)).map((u) => u.text || u.name));
  assert.equal(rows.filter((x) => x.startsWith("- turns")).length - rows.filter((x) => x.startsWith("+ turns")).length, 4);
  assert.ok(existsSync(join(GEN, "R1.tsx")), "not yet");
  assert.ok(await waitFor(() => !existsSync(join(GEN, "R1.tsx")) && !existsSync(join(GEN, "R1")), 3000), "the files go after the delay");
  assert.ok(await waitFor(() => !readdirSync(join(dataDir, "chunks")).some((f) => f === "R1.js" || /^R1v\d+\.js$/.test(f)), 3000), "and its chunks");
  assert.deepEqual((await request("GET", "/apps/meta")).json.apps.map((a) => a.name), ["R2", "R3", "R4"]);
  assert.equal((await request("GET", "/artifacts?q=R1")).json.items.length, 0);
  // everywhere: the published copy goes too
  await postJson("/publish", { name: "R4" });
  const all = await changes(() => postJson("/app/delete", { name: "R4", everywhere: true }));

  assert.ok(all.rows.includes('- widgets {"name":"R4","title":"R4","prompt":""}'), all.rows.join("\n"));
  assert.ok(await waitFor(() => !existsSync(join(dataDir, "widgets", "R4.tsx")), 3000));
  assert.deepEqual(await postJson("/app/delete", { name: "x" }).then((r) => [r.status, r.json]), [400, { ok: false, error: "no such module x" }]);
});

test("ART-17 GET /sources: timeclock and reminders (linear only with a key)", async () => {
  const r = await request("GET", "/sources");

  assert.equal(r.text, '{"sources":[{"name":"timeclock"},{"name":"reminders"}]}');
  assert.match(r.headers["content-type"], /^application\/json/);
});

