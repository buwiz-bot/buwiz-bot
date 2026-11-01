// The read side of files (plan item 2.5): a reply's files with Range,
// their saved versions, attachments, avatars and generated assets. Run
// against Node they pin down what buwiz-broker must match.
import assert from "node:assert/strict";
import { mkdirSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { request, startServer, tempDir } from "./helpers.mjs";

let server;
const DATA = tempDir("files");
const GEN = join(DATA, "generated");
const BYTES = "0123456789abcdef";

before(async () => {
  server = await startServer(DATA);
  mkdirSync(join(GEN, "files", "t1"), { recursive: true });
  writeFileSync(join(GEN, "files", "t1", "doc.PDF"), BYTES);
  writeFileSync(join(GEN, "files", "t1", "notes.txt"), "plain");
  writeFileSync(join(GEN, "files", "t1", "empty.png"), "");
  mkdirSync(join(GEN, "files", "my dir"), { recursive: true });
  writeFileSync(join(GEN, "files", "my dir", "é.mp4"), "video");
  const vdir = join(GEN, "files", ".versions", "t1", "a.docx");
  mkdirSync(vdir, { recursive: true });
  for (const [f, body, at] of [["v2.docx", "two", 1700000000.5], ["v10.docx", "ten!", 1700000100.123], ["v1.docx", "1", 1600000000], ["x.txt", "no", 1]]) {
    writeFileSync(join(vdir, f), body);
    utimesSync(join(vdir, f), at, at);
  }
  mkdirSync(join(GEN, "attachments"), { recursive: true });
  writeFileSync(join(GEN, "attachments", "a1.JPG"), "jpeg");
  writeFileSync(join(GEN, "avatars", "b1.gif"), "gif!");
  mkdirSync(join(GEN, "assets"), { recursive: true });
  writeFileSync(join(GEN, "assets", "img.webp"), "webp");
  writeFileSync(join(GEN, "assets", "noext"), "raw");
});

after(async () => {
  await server?.stop();
});

const range = (path, r) => request("GET", path, { headers: { Range: r } });

test("FILE-01 a reply's file, whole and by range", async () => {
  const r = await request("GET", "/file/raw/t1/doc.PDF");
  assert.equal(r.status, 200);
  assert.equal(r.text, BYTES);
  assert.equal(r.headers["content-type"], "application/pdf");
  assert.equal(r.headers["accept-ranges"], "bytes");
  assert.equal(r.headers["content-length"], "16");
  assert.equal(r.headers["cache-control"], undefined);

  const mid = await range("/file/raw/t1/doc.PDF?x=1", "bytes=2-5");
  assert.equal(mid.status, 206);
  assert.equal(mid.text, "2345");
  assert.equal(mid.headers["content-range"], "bytes 2-5/16");
  assert.equal(mid.headers["content-length"], "4");
  assert.equal((await range("/file/raw/t1/doc.PDF", "bytes=-3")).text, "def");
  assert.equal((await range("/file/raw/t1/doc.PDF", "bytes=10-")).text, "abcdef");
  assert.equal((await range("/file/raw/t1/doc.PDF", "bytes=14-99")).text, "ef");
  for (const r of ["bytes=16-", "bytes=5-2", "bytes=-"]) {
    const no = await range("/file/raw/t1/doc.PDF", r);
    assert.equal(no.status, 416, r);
    assert.equal(no.headers["content-range"], "bytes */16");
    assert.equal(no.text, "");
  }
  // not a byte range at all: the whole file
  assert.equal((await range("/file/raw/t1/doc.PDF", "items=1-2")).status, 200);

  assert.equal((await request("GET", "/file/raw/t1/notes.txt")).headers["content-type"], "application/octet-stream");
  const v = await request("GET", "/file/raw/my%20dir/%C3%A9.mp4");
  assert.equal(v.headers["content-type"], "video/mp4");
  assert.equal(v.text, "video");
  const empty = await request("GET", "/file/raw/t1/empty.png");
  assert.equal(empty.status, 200);
  assert.equal(empty.headers["content-length"], "0");
});

test("FILE-02 nothing outside the files folder, and nothing missing", async () => {
  for (const p of ["/file/raw/t1/nope.pdf", "/file/raw/..%2F..%2Fgenerated%2Fapp.db", "/file/raw/../app.db", "/file/raw/"]) {
    const r = await request("GET", p);
    assert.equal(r.status, 404, p);
    assert.equal(r.text, "");
  }
  assert.equal((await request("GET", "/file/raw/%E0%A4%A")).status, 500);
});

test("FILE-03 a file's saved versions", async () => {
  const r = await request("GET", "/file/versions?path=t1/a.docx");
  assert.equal(r.status, 200);
  assert.match(r.headers["content-type"], /application\/json/);
  const at = (f) => Math.round(statSync(join(GEN, "files", ".versions", "t1", "a.docx", f)).mtimeMs);
  assert.deepEqual(r.json, {
    v: 10,
    versions: [
      { v: 1, at: at("v1.docx"), bytes: 1, path: ".versions/t1/a.docx/v1.docx" },
      { v: 2, at: at("v2.docx"), bytes: 3, path: ".versions/t1/a.docx/v2.docx" },
      { v: 10, at: at("v10.docx"), bytes: 4, path: ".versions/t1/a.docx/v10.docx" },
    ],
  });
  // a version's own path names the same file
  assert.equal((await request("GET", "/file/versions?path=.versions%2Ft1%2Fa.docx%2Fv2.docx")).text, r.text);
  assert.equal((await request("GET", "/file/versions?path=t1/none.docx")).text, '{"v":0,"versions":[]}');
  assert.equal((await request("GET", "/file/versions?x=1")).text, '{"v":0,"versions":[]}');
});

test("FILE-04 attachments and avatars, by bare file name", async () => {
  const a = await request("GET", "/files/attachments/a1.JPG");
  assert.equal(a.status, 200);
  assert.equal(a.text, "jpeg");
  assert.equal(a.headers["content-type"], "image/jpeg");
  assert.equal(a.headers["cache-control"], "max-age=86400");
  assert.equal((await request("GET", "/files/avatars/x%2Fb1.gif?v=2")).text, "gif!");
  assert.equal((await request("GET", "/files/avatars/x/b1.gif")).headers["content-type"], "image/gif");
  assert.equal((await range("/files/attachments/a1.JPG", "bytes=1-2")).text, "pe");
  assert.equal((await request("GET", "/files/attachments/none.png")).status, 404);
  assert.equal((await request("GET", "/files/other/a1.JPG")).status, 404);
});

test("FILE-05 generated assets", async () => {
  const a = await request("GET", "/asset/generated/assets/img.webp");
  assert.equal(a.status, 200);
  assert.equal(a.headers["content-type"], "image/webp");
  assert.equal(a.headers["cache-control"], "max-age=86400");
  assert.equal((await request("GET", "/asset/generated/assets/sub/img.webp?x")).text, "webp");
  assert.equal((await request("GET", "/asset/generated/assets/noext")).headers["content-type"], "application/octet-stream");
  assert.equal((await request("GET", "/asset/generated/assets/")).status, 404);
  assert.equal((await request("GET", "/asset/generated/assets/none.png")).status, 404);
});
