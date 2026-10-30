// GET /file/render for every kind but the office ones (those are
// office.test's OFF-21), and GET / (Rust broker plan phase 7: the last
// routes Node's office role answered). Pinned on Node; rox-broker's hub
// answers the same JSON.
//
// Code lines: Node colours them with Shiki, rox-broker with syntect and
// the same two themes (tools/tmtheme.mjs). The grammars differ, so these
// cases pin the shape exactly (lines, each token [text, dark, light], the
// tokens joining to the line, colours from the two palettes) and the
// colour of tokens both engines agree on; not every token's colour or
// every split.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { after, before, test } from "node:test";
import { cursorThemes } from "../src/files.mjs";
import { ROOT, offlineEnv, request, startServer, tempDir } from "./helpers.mjs";

let server;
const DATA = tempDir("render");
const GEN = join(DATA, "generated");
const FILES = join(GEN, "files");
const put = (rel, body) => {
  mkdirSync(join(FILES, rel, ".."), { recursive: true });
  writeFileSync(join(FILES, rel), body);
};

before(async () => {
  server = await startServer(DATA, { env: offlineEnv(join(DATA, "stub.log")) });
});

after(async () => {
  await server?.stop();
});

const render = async (path) => {
  const r = await request("GET", `/file/render?path=${encodeURIComponent(path)}`);

  assert.equal(r.status, 200, path);
  assert.equal(r.headers["content-type"], "application/json");

  return r;
};

/** Asserts the answer is `want`, key order included. */
const same = async (path, want) => assert.equal((await render(path)).text, JSON.stringify(want), path);

const meta = (name, ext, size, kind, variant) => ({ name, ext, size, kind, variant });

// ————— code lines —————

const colours = (i) => new Set(cursorThemes()[i].tokenColors.map((r) => r.settings.foreground).concat(cursorThemes()[i].colors["editor.foreground"]).map((c) => c.toUpperCase()));
const DARK = colours(0);
const LIGHT = colours(1);

/** Shiki's lines: split at \r\n and \n. */
const shikiLines = (code) => code.split(/\r\n|\n/);

/** The shape every coloured file's lines have. */
function assertLines(lines, code, label) {
  assert.ok(Array.isArray(lines), label);
  assert.deepEqual(lines.map((l) => l.map((t) => t[0]).join("")), shikiLines(code), `${label}: the text`);
  for (const line of lines) {
    for (const t of line) {
      assert.equal(t.length, 3, label);
      assert.ok(typeof t[0] === "string" && t[0].length > 0, `${label}: an empty token in ${JSON.stringify(line)}`);
      assert.ok(DARK.has(t[1]), `${label}: dark ${t[1]} is not cursor-dark's`);
      assert.ok(LIGHT.has(t[2]), `${label}: light ${t[2]} is not cursor-light's`);
    }
  }
}

const token = (lines, text) => lines.flat().find((t) => t[0] === text);
const KEYWORD = ["#82D2CE", "#B3003F"];
const STRING = ["#E394DC", "#9E94D5"];
const COMMENT = ["#E4E4E45E", "#141414AD"];

// one sample per extension files.mjs colours
const SAMPLES = {
  rs: 'use std::fmt;\n\n/// Docs.\nfn main() {\n    // hi\n    let x = "s";\r\n    println!("{x}");\n}\n',
  ts: 'const a: number = 1; // c\nexport function f(x: string) {\n  return `t${x}`;\n}\n',
  tsx: 'export default function A() {\n  return <View onPress={() => 1}><Text>{"hi"}</Text></View>;\n}\n',
  js: "const a = [1, 'b'];\nfunction f() { return a; }\n",
  jsx: "const el = <div className=\"a\">{x}</div>;\n",
  mjs: "import x from \"y\";\nexport default x;\n",
  cjs: "module.exports = { a: 1 };\n",
  py: 'def f(x):\n    # note\n    return "é😀" + str(x)\n',
  go: 'package main\n\nfunc main() {\n\tprintln("hi")\n}\n',
  swift: 'let x = "a"\nfunc f() -> Int { return 1 }\n',
  jsonc: '{\n  // c\n  "a": 1\n}\n',
  toml: '[package]\nname = "x"\nversion = 1\n',
  yaml: "a: 1\nb:\n  - \"c\"\n",
  yml: "k: v\n",
  sh: 'echo "hi" # c\nfor x in a b; do echo $x; done\n',
  bash: "x=1\n",
  zsh: "print -r -- $x\n",
  css: "a { color: red; }\n",
  scss: "$c: red;\n.a { .b { color: $c; } }\n",
  xml: '<?xml version="1.0"?>\n<a b="c">d</a>\n',
  sql: "SELECT a FROM b WHERE c = 'd';\n",
  c: '#include <stdio.h>\nint main(void) { return 0; }\n',
  h: "int f(void);\n",
  cc: "int main() { return 0; }\n",
  cpp: '#include <string>\nstd::string s = "a";\n',
  hpp: "class A {};\n",
  java: 'class A { String s = "a"; }\n',
  kt: 'fun main() { println("a") }\n',
  rb: "def f\n  'a'\nend\n",
  php: '<?php echo "a"; ?>\n',
  cs: 'class A { string s = "a"; }\n',
  ini: "[a]\nb = c\n",
  tex: "\\section{A} % c\n",
  txt: "one\r\ntwo\n",
  log: "a\n",
};

before(() => {
  for (const [ext, code] of Object.entries(SAMPLES)) put(`code/sample.${ext}`, code);
});

test("RENDER-01 code: Shiki's shape for every extension it colours, and the tokens both engines colour alike", async () => {
  for (const [ext, code] of Object.entries(SAMPLES)) {
    const r = (await render(`code/sample.${ext}`)).json;

    assert.equal(r.kind, "text", ext);
    assert.equal(r.ext, ext);
    assert.deepEqual(Object.keys(r), ["name", "ext", "size", "kind", "variant", "lines"], ext);
    if (ext === "txt" || ext === "log") {
      // no grammar: one plain token per \n line (a \r stays)
      assert.deepEqual(r.lines, code.split("\n").map((l) => [[l, "", ""]]), ext);
      continue;
    }
    assertLines(r.lines, code, ext);
    assert.deepEqual(r.lines.at(-1), [], `${ext}: the line after the last newline`);
  }
  const rs = (await render("code/sample.rs")).json.lines;

  assert.deepEqual(token(rs, "fn"), ["fn", ...KEYWORD]);
  assert.deepEqual(token(rs, "let"), ["let", ...KEYWORD]);
  assert.deepEqual(token(rs, "// hi"), ["// hi", ...COMMENT]);
  assert.deepEqual(token(rs, '"s"'), ['"s"', ...STRING]);
  assert.deepEqual(rs[1], []);
  const ts = (await render("code/sample.ts")).json.lines;

  assert.deepEqual(token(ts, "const"), ["const", ...KEYWORD]);
  assert.deepEqual(token(ts, "// c"), ["// c", ...COMMENT]);
  assert.deepEqual(token((await render("code/sample.py")).json.lines, '"é😀"'), ['"é😀"', ...STRING]);
  assert.deepEqual(token((await render("code/sample.go")).json.lines, "func"), ["func", ...KEYWORD]);
});

test("RENDER-02 code edge cases: an empty file, a lone \\r, a long file cut at 400,000 UTF-16 units", async () => {
  put("code/empty.rs", "");
  await same("code/empty.rs", { ...meta("empty.rs", "rs", 0, "text", "generic"), lines: [[]] });
  put("code/empty.txt", "");
  await same("code/empty.txt", { ...meta("empty.txt", "txt", 0, "text", "generic"), lines: [[["", "", ""]]] });
  put("code/cr.rs", "a\rb\r\n\nc");
  const cr = (await render("code/cr.rs")).json.lines;

  assertLines(cr, "a\rb\r\n\nc", "cr.rs");
  assert.equal(cr.length, 3);
  put("code/long.txt", "é".repeat(400_005));
  const long = (await render("code/long.txt")).json;

  assert.equal(long.size, 800_010);
  assert.equal(long.lines[0][0][0].length, 400_000);
});

test("RENDER-03 markdown: the text, cut at 400,000 units; an upload's id prefix leaves the name", async () => {
  put("md/notes.md", "# Hi\n\nthere é\r\n");
  await same("md/notes.md", { ...meta("notes.md", "md", 16, "markdown", "generic"), text: "# Hi\n\nthere é\r\n" });
  put("md/ab12cd-x9z1-Report.MARKDOWN", "*x*");
  await same("md/ab12cd-x9z1-Report.MARKDOWN", { ...meta("Report.MARKDOWN", "markdown", 3, "markdown", "generic"), text: "*x*" });
  put("md/big.mdx", "a".repeat(400_001));
  const big = (await render("md/big.mdx")).json;

  assert.equal(big.text.length, 400_000);
  assert.equal(big.kind, "markdown");
  // bytes that aren't UTF-8 read as U+FFFD, as Node reads them
  put("md/bad.md", Buffer.from([0x61, 0xff, 0x62, 0xe2, 0x82]));
  await same("md/bad.md", { ...meta("bad.md", "md", 5, "markdown", "generic"), text: "a�b�" });
});

test("RENDER-04 JSON: parsed with JavaScript's key order, else coloured lines; .jsonc is code", async () => {
  put("j/data.json", '{"b": 1, "2": [1.50, true, null, "x"], "a": {"10": 0, "9": -0, "z": 1e21}, "b": 2}');
  await same("j/data.json", { ...meta("data.json", "json", 82, "json", "json"), json: { 2: [1.5, true, null, "x"], b: 2, a: { 9: 0, 10: 0, z: 1e21 } } });
  const bad = '{"a": 1,\n  "b": }\n';

  put("j/bad.json", bad);
  const r = (await render("j/bad.json")).json;

  assert.deepEqual(Object.keys(r), ["name", "ext", "size", "kind", "variant", "lines"]);
  assertLines(r.lines, bad, "bad.json");
  put("j/empty.json", "");
  assert.deepEqual((await render("j/empty.json")).json.lines, [[]]);
});

test("RENDER-05 HTML: its source as coloured lines", async () => {
  const html = '<!doctype html>\n<p class="a">Hi &amp; <b>there</b></p>\n';

  put("h/page.HTM", html);
  const r = (await render("h/page.HTM")).json;

  assert.deepEqual(Object.keys(r), ["name", "ext", "size", "kind", "variant", "lines"]);
  assert.equal(r.kind, "html");
  assert.equal(r.variant, "generic");
  assertLines(r.lines, html, "page.HTM");
  assert.deepEqual(token(r.lines, '"a"'), ['"a"', ...STRING]);
});

test("RENDER-06 CSV and TSV: quoted cells, CRLF, the file's own name, 2,000 rows at most", async () => {
  put("t/ab12cd-x9z1-budget.csv", 'a,"b,1","c ""q"""\r\n1,,3\n"multi\nline",x\n');
  await same("t/ab12cd-x9z1-budget.csv", {
    ...meta("budget.csv", "csv", 39, "table", "spreadsheet"),
    sheets: [{ name: "ab12cd-x9z1-budget.csv", rows: [["a", "b,1", 'c "q"'], ["1", "", "3"], ["multi\nline", "x"]] }],
  });
  put("t/s.tsv", "a\tb\nc");
  await same("t/s.tsv", { ...meta("s.tsv", "tsv", 5, "table", "spreadsheet"), sheets: [{ name: "s.tsv", rows: [["a", "b"], ["c"]] }] });
  put("t/many.csv", Array.from({ length: 2100 }, (_, i) => `r${i}`).join("\n"));
  const many = (await render("t/many.csv")).json.sheets[0].rows;

  assert.equal(many.length, 2000);
  assert.deepEqual(many.at(-1), ["r1999"]);
  put("t/empty.csv", "");
  await same("t/empty.csv", { ...meta("empty.csv", "csv", 0, "table", "spreadsheet"), sheets: [{ name: "empty.csv", rows: [] }] });
});

test("RENDER-07 .xls: what isn't a zip gets JSZip's refusal", async () => {
  put("x/a.xls", "abcde");
  await same("x/a.xls", { error: "Can't find end of central directory : is this a zip file ? If it is, see https://stuk.github.io/jszip/documentation/howto/read_zip.html" });
  put("x/e.xls", "");
  await same("x/e.xls", { error: "End of data reached (data length = 0, asked index = 4). Corrupted zip ?" });
  put("x/f.xls", "xy");
  await same("x/f.xls", { error: "End of data reached (data length = 2, asked index = 4). Corrupted zip ?" });
});

test("RENDER-08 media, PDFs and the rest: their meta only; sizes from sips or the SVG", async () => {
  put("m/shot.png", "png!");
  await same("m/shot.png", { ...meta("shot.png", "png", 4, "image", "media"), width: 64, height: 64 });
  put("m/logo.svg", '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="30.5"></svg>');
  await same("m/logo.svg", { ...meta("logo.svg", "svg", 72, "image", "media"), width: 120, height: 30.5 });
  put("m/vb.svg", '<svg viewBox="0 0 48 24"/>');
  await same("m/vb.svg", { ...meta("vb.svg", "svg", 26, "image", "media"), width: 48, height: 24 });
  put("m/empty.png", "");
  await same("m/empty.png", meta("empty.png", "png", 0, "image", "media"));
  put("m/doc.PDF", "%PDF-1.4");
  await same("m/doc.PDF", meta("doc.PDF", "pdf", 8, "pdf", "pdf"));
  put("m/card.vcf", "BEGIN:VCARD");
  await same("m/card.vcf", meta("card.vcf", "vcf", 11, "unknown", "unknown"));
  put("m/README", "x");
  await same("m/README", meta("README", "", 1, "unknown", "unknown"));
  put("m/deck.key", "k");
  await same("m/deck.key", meta("deck.key", "key", 1, "pptx", "keynote"));
  put("m/old.ppt", "p");
  await same("m/old.ppt", meta("old.ppt", "ppt", 1, "pptx", "ppt"));
  put("m/old.doc", "d");
  await same("m/old.doc", meta("old.doc", "doc", 1, "unknown", "docx"));
  put("m/book.xlsm", "x");
  await same("m/book.xlsm", meta("book.xlsm", "xlsm", 1, "unknown", "spreadsheet"));
  // no audio or video tool can read these bytes: the zeros Node falls back to
  put("m/voice.m4a", "not audio");
  await same("m/voice.m4a", { ...meta("voice.m4a", "m4a", 9, "audio", "audio"), duration: 0, peaks: [] });
  // the Quick Look stub's poster, sized by the sips stub
  put("m/clip.mov", "not video");
  await same("m/clip.mov", { ...meta("clip.mov", "mov", 9, "video", "video"), poster: "clip.mov.png", width: 64, height: 64 });
});

test("RENDER-09 what isn't there: missing, a directory, outside the files folder, no path", async () => {
  await same("nope/gone.md", { ...meta("gone.md", "md", 0, "markdown", "generic"), error: "missing" });
  await same("/etc/passwd", { ...meta("passwd", "", 0, "unknown", "unknown"), error: "missing" });
  mkdirSync(join(FILES, "d", "folder.txt"), { recursive: true });
  await same("d/folder.txt", { error: "EISDIR: illegal operation on a directory, read" });
  for (const p of ["", "..", "../app.db", "a/../../app.db"]) await same(p, { error: "bad path" });
  // Node's route is /file/render?…: the bare path is its 404
  const bare = await request("GET", "/file/render");

  assert.equal(bare.status, 404);
  assert.equal(bare.text, "not found");
});

test("RENDER-10 a mini app's source: app:R<n> (head, module, widget) and app:R<n>@v<k>", async () => {
  const code = 'export default function A() {\n  return "v2";\n}\n';

  mkdirSync(join(GEN, "R5"), { recursive: true });
  writeFileSync(join(GEN, "R5", "demo.v2.tsx"), code);
  writeFileSync(join(GEN, "R5", "demo.v1.tsx"), "export default 1;\n");
  writeFileSync(join(GEN, "R3.tsx"), "export default 3;\n");
  mkdirSync(join(DATA, "widgets"), { recursive: true });
  writeFileSync(join(DATA, "widgets", "R4.tsx"), "export default 4;\n");

  const v2 = (await render("app:R5@v2")).json;

  assert.deepEqual(Object.keys(v2), ["name", "ext", "size", "kind", "variant", "lines"]);
  assert.deepEqual([v2.name, v2.ext, v2.size, v2.kind, v2.variant], ["demo.v2.tsx", "tsx", code.length, "text", "generic"]);
  assertLines(v2.lines, code, "app:R5@v2");
  assert.equal((await render("app:R5@v002")).json.name, "demo.v2.tsx");
  assert.equal((await render("app:R3")).json.name, "R3.tsx");
  assert.equal((await render("app:R4")).json.name, "R4.tsx");
  for (const p of ["app:R9", "app:R5@v7", "app:R5"]) await same(p, { error: "bad path" });
  // the head's version file comes first
  const db = new DatabaseSync(join(GEN, "app.db"), { timeout: 5000 });

  try {
    db.exec("CREATE TABLE IF NOT EXISTS heads(name TEXT PRIMARY KEY, v INTEGER NOT NULL)");
    db.prepare("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)").run("R5", 1);
    db.prepare("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)").run("R3", 4);
  } finally {
    db.close();
  }
  assert.equal((await render("app:R5")).json.name, "demo.v1.tsx");
  // a head with no version file: the module
  assert.equal((await render("app:R3")).json.name, "R3.tsx");
  // not an app path: a file of that name
  await same("app:R5@vx", { ...meta("app:R5@vx", "", 0, "unknown", "unknown"), error: "missing" });
});

test("RENDER-11 GET /: the broker's one-line status", async () => {
  const r = await request("GET", "/");
  const threads = (await request("GET", "/threads")).json.list.length;

  assert.equal(r.status, 200);
  assert.equal(r.headers["content-type"], "text/plain");
  assert.equal(r.text, `app-server ok — ${threads} threads, model=mock\n`);
  for (const p of ["/?x=1", "/nope"]) {
    const no = await request("GET", p);

    assert.equal(no.status, 404, p);
    assert.equal(no.text, "not found");
  }
});

test("RENDER-12 the tmTheme files rox-broker colours with are files.mjs's themes", () => {
  execFileSync(process.execPath, [join(ROOT, "tools", "tmtheme.mjs"), "--check"], { stdio: "pipe" });
});
