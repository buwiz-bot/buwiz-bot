// The build pipeline (Rust broker plan, phase 5 item 5.7a) as Node runs it
// (pipeline.mjs), with nsh at the PATH stub (fixtures/bin/nsh, which records
// argv): runNsh's pass / failure text / missing binary, fixImports,
// checkCandidate in .stage, writeVersion's files, `versions` and `heads`
// rows and chunks (`R6v3.js`), setHead, the error card and the canned mock
// modules. PIPE-01..13 run pipeline.mjs in a Node of their own (nodeIn);
// Rust's hub/pipeline tests check the same files, rows, argv and chunk
// names against the same stub. PIPE-20..22 drive it end to end through a
// MOCK /chat (the legacy generate path), so through rox-broker they wait on
// /chat (5.2).
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { dbRows, nodeIn, offlineEnv, postJson, request, startServer, stubCalls, stubEnv, tempDir, waitFor } from "./helpers.mjs";

const sha = (s) => createHash("sha256").update(s).digest("hex");

/** Runs `code` with `P` (pipeline.mjs) and `S` (state.mjs) imported, on a
 *  data dir of its own with the nsh stub; `settle(files)` waits for files
 *  pipeline.mjs writes after it returns (emitChunk is fire-and-forget).
 *  Resolves to what the code printed last, parsed. */
function run(dir, code, env = {}) {
  const out = nodeIn(dir, `
    const { existsSync } = await import("node:fs");
    const { join } = await import("node:path");
    const P = await import("./src/pipeline.mjs");
    const S = await import("./src/state.mjs");
    const D = process.env.BUWIZ_BROKER_DATA_DIR;
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const settle = async (files) => {
      for (let i = 0; i < 400 && !files.every((f) => existsSync(join(D, f))); i++) await sleep(25);
      await sleep(150);
    };
    ${code}
  `, { ...stubEnv(join(dir, "stubs.log")), ...env });

  return JSON.parse(out);
}

const nshCalls = (dir) => stubCalls(join(dir, "stubs.log")).filter((c) => c.bin === "nsh").map((c) => c.argv);
const read = (...p) => readFileSync(join(...p), "utf8");
const FAIL = "error TS2304: Cannot find name 'nope'.\n";
const CODE = 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}\n';

test("PIPE-01 runNsh: a pass is null, a failure its stdout and stderr, a missing binary an empty string; argv as given", () => {
  const dir = tempDir("pipe-nsh");
  const pass = run(dir, `console.log(JSON.stringify(await P.runNsh(["check", "x.tsx"])));`);
  const fail = run(dir, `console.log(JSON.stringify(await P.runNsh(["build", "y.tsx", "--out", "o"])));`, { NSH_STUB: "fail" });
  const custom = run(dir, `console.log(JSON.stringify(await P.runNsh(["check", "z.tsx"])));`, { NSH_STUB: "fail", NSH_STUB_TEXT: "line one\nline two" });
  const missing = run(dir, `console.log(JSON.stringify(await P.runNsh(["check", "x.tsx"])));`, { NSH_STUB: "missing" });
  const absent = run(dir, `console.log(JSON.stringify(await P.runNsh(["check", "x.tsx"])));`, { NSH: join(dir, "no-such-nsh") });

  assert.equal(pass, null);
  assert.equal(fail, FAIL);
  assert.equal(custom, "line one\nline two\n");
  assert.equal(missing, "");
  assert.equal(absent, "");
  assert.deepEqual(nshCalls(dir), [["check", "x.tsx"], ["build", "y.tsx", "--out", "o"], ["check", "z.tsx"], ["check", "x.tsx"]]);
});

// [input, what fixImports makes of it]
const FIX = [
  // nothing missing: unchanged
  [CODE, CODE],
  // react imports dropped; core names added to a new nsh import
  [
    'import React from "react";\nimport { useState } from \'react-native\';\nexport default function R() {\n  const n = signal(0);\n  return <View><Text>{n()}</Text></View>;\n}\n',
    'import { signal, Text, View } from "nsh";\nexport default function R() {\n  const n = signal(0);\n  return <View><Text>{n()}</Text></View>;\n}\n',
  ],
  // merged into the nsh import there is; aliases count as imported
  [
    'import { View as V } from "nsh";\nconst a = signal(1); effect(() => a());\nexport default () => <V><Text>x</Text></V>;\n',
    'import { View as V, effect, signal, Text } from "nsh";\nconst a = signal(1); effect(() => a());\nexport default () => <V><Text>x</Text></V>;\n',
  ],
  // module names grouped by module, above everything
  [
    'import { View } from "nsh";\nexport default () => <View><Button /><Card /><List /><Image src="a" /></View>;\n',
    'import { Image } from "nsh/image";\nimport { List } from "nsh/list";\nimport { Button, Card } from "nsh/ui/mobile";\nimport { View } from "nsh";\nexport default () => <View><Button /><Card /><List /><Image src="a" /></View>;\n',
  ],
  // a word inside another word is not a use; one inside a string is
  [
    'export default () => "TextView mounted";\n',
    'export default () => "TextView mounted";\n',
  ],
  [
    'export default () => "a View here";\n',
    'import { View } from "nsh";\nexport default () => "a View here";\n',
  ],
];

test("PIPE-02 fixImports drops react-family imports and adds the nsh names a module uses without importing", () => {
  const dir = tempDir("pipe-fix");
  const got = run(dir, `console.log(JSON.stringify(${JSON.stringify(FIX.map(([i]) => i))}.map(P.fixImports)));`);

  assert.deepEqual(got, FIX.map(([, o]) => o));
});

test("PIPE-03 checkCandidate on a pass: the entry, nsh build then check, null; the stage pair removed, the build output kept", () => {
  const dir = tempDir("pipe-check");
  const got = run(dir, `
    const { writeFileSync, readFileSync } = await import("node:fs");
    writeFileSync(join(D, ".stage", "R5.tsx"), ${JSON.stringify(CODE)});
    const err = await P.checkCandidate("R5");
    console.log(JSON.stringify({ err, entry: existsSync(join(D, ".stage", "check-R5.tsx")), mod: existsSync(join(D, ".stage", "R5.tsx")), out: readFileSync(join(D, ".stage", "out-R5", "bundle.js"), "utf8") }));
  `);
  const stage = join(dir, ".stage");

  assert.deepEqual(got, { err: null, entry: false, mod: false, out: "// nsh stub bundle of check-R5.tsx\nexport default null;\n" });
  assert.deepEqual(nshCalls(dir), [["build", join(stage, "check-R5.tsx"), "--out", join(stage, "out-R5")], ["check", join(stage, "check-R5.tsx")]]);
  // what config.mjs writes at start, for nsh check
  assert.equal(read(stage, "tsconfig.json"), '{"extends":"./.nsh/tsconfig.json","include":["*.ts","*.tsx"]}\n');
});

test("PIPE-04 checkCandidate's entry imports the candidate; a failing build is the answer and check never runs", () => {
  const dir = tempDir("pipe-check-fail");
  const got = run(dir, `
    const { writeFileSync, readFileSync } = await import("node:fs");
    writeFileSync(join(D, ".stage", "R7.tsx"), ${JSON.stringify(`${CODE}// nsh-stub:fail\n`)});
    // the entry as nsh sees it, read back before the verdict removes it
    const entry = new Promise((ok) => { const t = setInterval(() => { try { ok(readFileSync(join(D, ".stage", "check-R7.tsx"), "utf8")); clearInterval(t); } catch {} }, 1); });
    const err = await P.checkCandidate("R7");
    console.log(JSON.stringify({ err, entry: await entry, left: existsSync(join(D, ".stage", "check-R7.tsx")) || existsSync(join(D, ".stage", "R7.tsx")) }));
  `);

  assert.deepEqual(got, { err: FAIL, entry: 'import { mount } from "nsh";\nimport R from "./R7";\nmount(R);\n', left: false });
  assert.deepEqual(nshCalls(dir).map((a) => a[0]), ["build"]);
});

test("PIPE-05 checkCandidate with nsh missing answers an empty string, still a failure, and check never runs", () => {
  const dir = tempDir("pipe-check-missing");
  const missing = run(dir, `
    (await import("node:fs")).writeFileSync(join(D, ".stage", "R2.tsx"), "x");
    console.log(JSON.stringify(await P.checkCandidate("R2")));
  `, { NSH_STUB: "missing" });

  assert.equal(missing, "");
  assert.deepEqual(nshCalls(dir).map((a) => a[0]), ["build"]);
});

test("PIPE-06 writeVersion: the version file, the head mirror, versions and heads rows, two chunks (R5v1.js and R5.js)", () => {
  const dir = tempDir("pipe-write");
  mkdirSync(join(dir, "chunks"), { recursive: true });
  const got = run(dir, `
    const v = P.writeVersion("R5", ${JSON.stringify(CODE)}, null, "make a counter", "");
    await settle(["chunks/R5v1.js", "chunks/R5.js"]);
    console.log(JSON.stringify(v));
  `);
  const gen = join(dir, "generated");

  assert.equal(got, 1);
  assert.deepEqual(readdirSync(join(gen, "R5")), ["R5.v1.tsx"]);
  assert.equal(read(gen, "R5", "R5.v1.tsx"), CODE);
  assert.equal(read(gen, "R5.tsx"), CODE);
  const rows = dbRows(dir, "SELECT * FROM versions");

  assert.ok(Math.abs(rows[0].created_at - Date.now()) < 60000);
  assert.deepEqual(rows.map((r) => ({ ...r, created_at: 0 })), [{ name: "R5", v: 1, parent: null, prompt: "make a counter", created_at: 0, label: null }]);
  assert.deepEqual(dbRows(dir, "SELECT * FROM heads"), [{ name: "R5", v: 1 }]);
  // one build per chunk, each into its own stage dir
  assert.deepEqual(nshCalls(dir).sort(), [
    ["build", join(gen, "R5", "R5.v1.tsx"), "--out", join(dir, ".stage", "chunk-R5v1")],
    ["build", join(gen, "R5.tsx"), "--out", join(dir, ".stage", "chunk-R5")],
  ].sort());
  assert.deepEqual(readdirSync(join(dir, "chunks")).sort(), ["R5.js", "R5v1.js"]);
  assert.equal(read(dir, "chunks", "R5v1.js"), "// nsh stub bundle of R5.v1.tsx\nexport default null;\n");
  assert.equal(read(dir, "chunks", "R5.js"), "// nsh stub bundle of R5.tsx\nexport default null;\n");
  // no published copy, no widgets file
  assert.ok(!existsSync(join(dir, "widgets", "R5.tsx")));
});

test("PIPE-07 writeVersion again: the next v, its parent and label, the file named by the app's slug, a published copy mirrored", () => {
  const dir = tempDir("pipe-write-2");
  mkdirSync(join(dir, "chunks"), { recursive: true });
  const long = "p".repeat(600);
  const got = run(dir, `
    const a = P.writeVersion("R6", "one", null, "", null);
    S.db.prepare("INSERT INTO apps(name,title,file) VALUES(?,?,?)").run("R6", "My Counter!", null);
    S.db.prepare("INSERT INTO widgets(name,title) VALUES(?,?)").run("R6", "My Counter!");
    const b = P.writeVersion("R6", "two", 1, ${JSON.stringify(long)}, "  slow-ball and a very long branch name here  ");
    S.db.prepare("UPDATE apps SET file=? WHERE name=?").run("ticker.tsx", "R6");
    const c = P.writeVersion("R6", "three", 1, "again", "x");
    await settle(["chunks/R6v1.js", "chunks/R6v2.js", "chunks/R6v3.js", "chunks/R6.js"]);
    console.log(JSON.stringify([a, b, c, P.headOf("R6"), P.versionsOf("R6").map((r) => ({ ...r, created_at: typeof r.created_at })), P.verSlug("R6"), P.verFile("R6", 2), P.verFile("R6", 9), P.chunkKey("R6@v3"), P.chunkKey("R6"), P.chunkKey("x@v3")]));
  `);
  const gen = join(dir, "generated");

  assert.deepEqual(got, [
    1, 2, 3, 3,
    [
      { v: 1, parent: null, created_at: "number", label: null },
      { v: 2, parent: 1, created_at: "number", label: "slow-ball and a very long branch name he" },
      { v: 3, parent: 1, created_at: "number", label: "x" },
    ],
    "ticker", join(gen, "R6", "my-counter.v2.tsx"), null, "R6v3", "R6", "x@v3",
  ]);
  assert.deepEqual(readdirSync(join(gen, "R6")).sort(), ["R6.v1.tsx", "my-counter.v2.tsx", "ticker.v3.tsx"]);
  assert.equal(read(gen, "R6.tsx"), "three");
  assert.equal(read(dir, "widgets", "R6.tsx"), "three");
  assert.deepEqual(dbRows(dir, "SELECT name, v, length(prompt) AS n FROM versions ORDER BY v"), [{ name: "R6", v: 1, n: null }, { name: "R6", v: 2, n: 500 }, { name: "R6", v: 3, n: 5 }]);
  assert.deepEqual(dbRows(dir, "SELECT * FROM heads"), [{ name: "R6", v: 3 }]);
  assert.deepEqual(readdirSync(join(dir, "chunks")).sort(), ["R6.js", "R6v1.js", "R6v2.js", "R6v3.js"]);
});

test("PIPE-08 writeVersion with no chunks dir, or a chunk build that fails: the version lands, no chunk does", () => {
  const dir = tempDir("pipe-nochunks");
  // BUG: nothing in Node makes <data>/chunks (the app launcher did), so on
  // a data dir without it every chunk copy fails silently
  run(dir, `
    P.writeVersion("R3", "a", null, "p", null);
    await settle([".stage/chunk-R3v1/bundle.js", ".stage/chunk-R3/bundle.js"]);
    console.log("null");
  `);
  assert.ok(!existsSync(join(dir, "chunks")));
  assert.equal(read(dir, "generated", "R3", "R3.v1.tsx"), "a");
  assert.deepEqual(dbRows(dir, "SELECT name, v FROM heads"), [{ name: "R3", v: 1 }]);

  const failing = tempDir("pipe-chunkfail");
  mkdirSync(join(failing, "chunks"));
  const logged = run(failing, `
    const lines = [];
    console.log = (l) => lines.push(l);
    P.writeVersion("R4", "b", null, "p", null);
    for (let i = 0; i < 400 && lines.length < 2; i++) await sleep(25);
    process.stdout.write(JSON.stringify(lines.sort()) + "\\n");
  `, { NSH_STUB: "fail", NSH_STUB_TEXT: "first\nlast line" });

  assert.deepEqual(logged, ["chunk R4: last line", "chunk R4v1: last line"]);
  assert.deepEqual(readdirSync(join(failing, "chunks")), []);
  assert.equal(read(failing, "generated", "R4.tsx"), "b");
});

test("PIPE-09 setHead: false for a version with no file; else heads moves, the mirrors take that version, one chunk build, the index rewritten", () => {
  const dir = tempDir("pipe-head");
  mkdirSync(join(dir, "chunks"), { recursive: true });
  const got = run(dir, `
    P.writeVersion("R8", "one", null, "p", null);
    P.writeVersion("R8", "two", 1, "p", null);
    S.db.prepare("INSERT INTO widgets(name,title) VALUES(?,?)").run("R8", "t");
    await settle(["chunks/R8v1.js", "chunks/R8v2.js", "chunks/R8.js"]);
    const before = (await import("node:fs")).readdirSync(join(D, "chunks")).length;
    const n = (await import("node:fs")).readFileSync(process.env.BUWIZ_STUB_LOG, "utf8").split("\\n").filter(Boolean).length;
    const r = [P.setHead("R8", 5), P.setHead("R8", 1), P.headOf("R8"), P.headOf("R9")];
    for (let i = 0; i < 400 && (await import("node:fs")).readFileSync(process.env.BUWIZ_STUB_LOG, "utf8").split("\\n").filter(Boolean).length < n + 1; i++) await sleep(25);
    await sleep(300);
    console.log(JSON.stringify({ r, before, n }));
  `);
  const gen = join(dir, "generated");

  assert.deepEqual(got.r, [false, true, 1, null]);
  assert.equal(read(gen, "R8.tsx"), "one");
  assert.equal(read(dir, "widgets", "R8.tsx"), "one");
  assert.deepEqual(dbRows(dir, "SELECT * FROM heads"), [{ name: "R8", v: 1 }]);
  // the only build after the writes: the bare module's chunk
  assert.deepEqual(nshCalls(dir).slice(got.n), [["build", join(gen, "R8.tsx"), "--out", join(dir, ".stage", "chunk-R8")]]);
  assert.equal(read(dir, "chunks", "R8.js"), "// nsh stub bundle of R8.tsx\nexport default null;\n");
  // the store's index (writeIndex), with every version addressable
  assert.equal(read(gen, "index.tsx"), [
    'import R8 from "./R8";',
    'import R8v1 from "./R8/R8.v1";',
    'import R8v2 from "./R8/R8.v2";',
    'import R8w from "../widgets/R8";',
    "",
    "// Regenerated by server/app-server.mjs — module registry only.",
    "",
    "export const MODULES: Record<string, any> = {",
    '  "R8": R8,',
    '  "R8@v1": R8v1,',
    '  "R8@v2": R8v2,',
    '  "R8w": R8w,',
    "};",
    "",
  ].join("\n"));
  assert.ok(existsSync(join(dir, "widgets", "index.tsx")), "writeShelf ran");
});

test("PIPE-10 the error card: the note in a red card, JSON-quoted, cut at 600 characters", () => {
  const dir = tempDir("pipe-card");
  const got = run(dir, `console.log(JSON.stringify([P.errorCard("unknown"), P.errorCard(${JSON.stringify(FAIL)}), P.errorCard("x".repeat(700) + "y")]));`);
  const card = (note) => `import { Text, View } from "nsh";

export default function R() {
  return (
    <View class="gap-2 rounded-xl border border-red-200 bg-red-50 p-3">
      <Text class="text-sm font-semibold text-red-700">generation failed</Text>
      <Text class="text-xs text-red-600">{${note}}</Text>
    </View>
  );
}
`;

  assert.equal(got[0], card('"unknown"'));
  assert.equal(got[1], card(`"error TS2304: Cannot find name 'nope'.\\n"`));
  assert.equal(got[2], card(`"${"x".repeat(600)}"`));
});

// the canned module each prompt gets (MOCK), by sha256 of its text: Rust's
// mock_module tests pin the same hashes
const MOCKS = {
  "a counter please": "counter",
  "weekly bar chart": "chart",
  "a booking form": "form",
  "my todo list": "todo",
  "NVDA stock price": "stock",
  "a quote: \"hi\" \\ there": "default",
};

const MOCK_SHA = {
  counter: "32cfaa8d3fa97ce47f96dd138b94271df0aa3e90fd10a831958caa1960889aca",
  chart: "fcd7e40baffef91d8048b1cc81105a15c46a31d5e891a2f03f4694c8cf5123ca",
  form: "181ebbb3fb053ddb896af1f3c88d4f172515bfe47365187cff8834c7baa1cad7",
  todo: "265528ef17ed5b5d13f02d04225c2c0b3d96aca746a4e07887ee12c9db62df68",
  stock: "38c1b59447738dfd11a76c7ae0f26bfce317dae18579df1ceda2a1eaf7cdaf5c",
  default: "2a7c5fef082d0816ce152a024a7fae6be1f9e8a9cc3b3c25de6663cb0b50717b",
};

test("PIPE-11 mockModule picks a canned module by keyword; the default one quotes the prompt", () => {
  const dir = tempDir("pipe-mock");
  const got = run(dir, `console.log(JSON.stringify(${JSON.stringify(Object.keys(MOCKS))}.map(P.mockModule)));`);
  const hashes = Object.fromEntries(Object.values(MOCKS).map((k, i) => [k, sha(got[i])]));

  assert.match(got[0], /^import \{ signal, Text, View \} from "nsh";\n\nexport default function R\(\) \{\n  const n = signal\(0\);/);
  assert.ok(got[5].includes(`<Text class="text-sm text-zinc-700">{"You asked for: a quote: \\"hi\\" \\\\ there"}</Text>`));
  assert.deepEqual(hashes, MOCK_SHA);
});

test("PIPE-12 walkFiles lists files under a dir, recursively, as {name, rel}; a missing dir is empty", () => {
  const dir = tempDir("pipe-walk");
  const root = join(dir, "w");

  mkdirSync(join(root, "a", "b"), { recursive: true });
  writeFileSync(join(root, "top.txt"), "");
  writeFileSync(join(root, "a", "b", "deep.md"), "");
  const got = run(dir, `console.log(JSON.stringify([P.walkFiles(${JSON.stringify(root)}).sort((x, y) => x.rel.localeCompare(y.rel)), P.walkFiles(join(D, "nope"))]));`);

  assert.deepEqual(got, [[{ name: "deep.md", rel: "a/b/deep.md" }, { name: "top.txt", rel: "top.txt" }], []]);
});

test("PIPE-13 config.mjs clears stale stage files at start, and keeps the rest", () => {
  const dir = tempDir("pipe-stage");

  mkdirSync(join(dir, ".stage", "out-R1"), { recursive: true });
  mkdirSync(join(dir, ".stage", "chunk-R1"), { recursive: true });
  writeFileSync(join(dir, ".stage", "check-R1.tsx"), "");
  writeFileSync(join(dir, ".stage", "R1.tsx"), "");
  writeFileSync(join(dir, ".stage", "keep.json"), "");
  run(dir, `console.log("null");`);
  assert.deepEqual(readdirSync(join(dir, ".stage")).sort(), ["chunk-R1", "keep.json", "tsconfig.json"]);
});

// ————— end to end: a MOCK /chat through the legacy generate path —————

const startAt = (dir, env = {}) => startServer(dir, { env: { ...offlineEnv(join(dir, "stubs.log")), ...env } });
const chat = async (prompt) => {
  const r = await postJson("/chat", { prompt });

  assert.equal(r.status, 200, r.text);

  return waitFor(async () => {
    const j = (await request("GET", `/status?id=${r.json.id}`)).json;

    return j?.done && j;
  }, 30000);
};
const lastTurn = async () => {
  const threads = (await request("GET", "/threads")).json;

  return (await request("GET", `/turns?thread=${threads.active}`)).json.turns.at(-1);
};

test("PIPE-20 a MOCK /chat that passes: the checked module lands as v1 with its chunks; index.tsx imports it", async () => {
  const dir = tempDir("pipe-chat");
  mkdirSync(join(dir, "chunks"), { recursive: true });
  const server = await startAt(dir);

  try {
    const st = await chat("a counter please");

    assert.ok(st, "the job finished");
    const turn = await lastTurn();
    const name = turn.name;

    assert.match(name, /^R\d+$/);
    assert.equal(turn.v, 1);
    assert.equal(st.result, `${name}.tsx via mock`);
    const gen = join(dir, "generated");
    const code = read(gen, `${name}.tsx`);

    assert.equal(sha(code), MOCK_SHA.counter);
    assert.equal(read(gen, name, `${name}.v1.tsx`), code);
    assert.deepEqual(dbRows(dir, "SELECT name, v, parent, prompt, label FROM versions"), [{ name, v: 1, parent: null, prompt: "a counter please", label: null }]);
    assert.deepEqual(dbRows(dir, "SELECT * FROM heads"), [{ name, v: 1 }]);
    assert.ok(await waitFor(() => existsSync(join(dir, "chunks", `${name}v1.js`)) && existsSync(join(dir, "chunks", `${name}.js`))), "both chunks");
    const stage = join(dir, ".stage");
    const calls = nshCalls(dir);

    assert.deepEqual(calls.slice(0, 2), [["build", join(stage, `check-${name}.tsx`), "--out", join(stage, `out-${name}`)], ["check", join(stage, `check-${name}.tsx`)]]);
    assert.deepEqual(calls.slice(2).sort(), [
      ["build", join(gen, name, `${name}.v1.tsx`), "--out", join(stage, `chunk-${name}v1`)],
      ["build", join(gen, `${name}.tsx`), "--out", join(stage, `chunk-${name}`)],
    ].sort());
    assert.equal(read(gen, "index.tsx"), [
      `import ${name} from "./${name}";`,
      `import ${name}v1 from "./${name}/${name}.v1";`,
      "",
      "// Regenerated by server/app-server.mjs — module registry only.",
      "",
      "export const MODULES: Record<string, any> = {",
      `  "${name}": ${name},`,
      `  "${name}@v1": ${name}v1,`,
      "};",
      "",
    ].join("\n"));

    // a module nsh rejects: the error card with nsh's text lands instead
    const bad = await chat("make it nsh-stub:fail");
    const card = await lastTurn();

    assert.equal(bad.result, `${card.name}.tsx via mock`);
    assert.match(bad.detail, /^failed — /);
    assert.equal(read(gen, `${card.name}.tsx`), ERROR_CARD(`"${FAIL.replace("\n", "\\n")}"`));
    assert.deepEqual(dbRows(dir, "SELECT name, v FROM heads ORDER BY name"), [{ name, v: 1 }, { name: card.name, v: 1 }].sort((a, b) => a.name.localeCompare(b.name)));
  } finally {
    await server.stop();
  }
});

test("PIPE-21 a MOCK /chat with nsh missing: the card says \"unknown\"", async () => {
  const dir = tempDir("pipe-chat-missing");
  const server = await startAt(dir, { NSH_STUB: "missing" });

  try {
    const st = await chat("a counter please");
    const turn = await lastTurn();

    assert.equal(st.result, `${turn.name}.tsx via mock`);
    assert.equal(read(dir, "generated", `${turn.name}.tsx`), ERROR_CARD('"unknown"'));
    assert.deepEqual(dbRows(dir, "SELECT name, v FROM versions"), [{ name: turn.name, v: 1 }]);
    // check never ran: the build already failed; then the two chunk builds
    assert.deepEqual(nshCalls(dir).map((a) => a[0]), ["build", "build", "build"]);
  } finally {
    await server.stop();
  }
});

const ERROR_CARD = (note) => `import { Text, View } from "nsh";

export default function R() {
  return (
    <View class="gap-2 rounded-xl border border-red-200 bg-red-50 p-3">
      <Text class="text-sm font-semibold text-red-700">generation failed</Text>
      <Text class="text-xs text-red-600">{${note}}</Text>
    </View>
  );
}
`;
