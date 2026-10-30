// The mermaid fallback (the Rust broker plan, item 6.2): Rox draws a
// ```mermaid fence natively and, when it can't, loads GET /mermaid?d=…
// (app/src/files.rs mermaid_url) in a web view, which loads /mermaid.js.
// The pages are pinned by their sha1, the same bytes whichever broker
// answers.
//
//   MER-01 the page: inline and full, light and dark, escaping, a lenient d
//   MER-02 /mermaid.js is the package's mermaid.min.js; near misses are 404
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { ROOT, offlineEnv, request, startServer, tempDir } from "./helpers.mjs";

const dir = tempDir("mermaid");
let server;

before(async () => {
  server = await startServer(dir, { env: offlineEnv(join(dir, "stubs.log")) });
});

after(async () => {
  await server?.stop();
});

const sha = (t) => createHash("sha1").update(t).digest("hex");
const d = (code) => Buffer.from(code, "utf8").toString("base64url");

test("MER-01 the page draws d: inline at the top left, full centred on a card; the code escaped", async () => {
  const code = "graph TD; a[\"x & y\"]-->b<c>; é→ü";
  const inline = await request("GET", `/mermaid?d=${d(code)}&theme=light&mode=inline`);

  assert.equal(inline.status, 200);
  assert.equal(inline.headers["content-type"], "text/html; charset=utf-8");
  assert.ok(inline.text.startsWith('<!doctype html><html><head><meta charset="utf-8"><style>\n'));
  assert.ok(inline.text.includes('#wrap{display:inline-block;transform-origin:0 0;}\n\n</style>'));
  assert.ok(inline.text.includes('<pre class="mermaid">graph TD; a["x &amp; y"]-->b&lt;c>; é→ü</pre>'));
  assert.ok(inline.text.includes('theme:"neutral",securityLevel:"strict"'));
  assert.equal(sha(inline.text), "16910f171a8a468140b25d55880b9388ae18855c");
  const full = await request("GET", `/mermaid?d=${d(code)}&theme=dark&mode=full`);

  assert.ok(full.text.includes("#wrap{display:inline-block;transform-origin:center;padding:28px;background:#262626;border-radius:12px}\nbody{display:flex;align-items:center;justify-content:center;height:100vh}\n</style>"));
  assert.ok(full.text.includes('theme:"dark",'));
  assert.equal(sha(full.text), "4374f79900fef046a8a8f75fae9c200977e1e4e1");
  const light = await request("GET", `/mermaid?d=${d(code)}&mode=full`);

  assert.ok(light.text.includes("background:#fff;"));
  // "+" reads as a space, then Buffer skips spaces and junk and takes "/"
  // from standard base64; bytes that aren't UTF-8 read as U+FFFD; no d at all
  const lax = await request("GET", "/mermaid?d=Z3J+/hcGg%20!&theme=dark");

  assert.ok(lax.text.includes('<pre class="mermaid">gr\u007f���</pre>'), lax.text);
  assert.ok((await request("GET", "/mermaid?x=1")).text.includes('<pre class="mermaid"></pre>'));
});

test("MER-02 /mermaid.js is the package's script; /mermaid without a query and /mermaid.js with one are 404", async () => {
  const js = await request("GET", "/mermaid.js");

  assert.equal(js.status, 200);
  assert.equal(js.headers["content-type"], "text/javascript");
  assert.equal(js.text, readFileSync(join(ROOT, "node_modules", "mermaid", "dist", "mermaid.min.js"), "utf8"));
  for (const path of ["/mermaid", "/mermaid.js?v=1"]) {
    const r = await request("GET", path);

    assert.deepEqual([r.status, r.text], [404, "not found"], path);
  }
});
