// The leftover leaves no 5.x item names (track L): Reminders through JXA,
// Linear's three routes, and the file routes that write or hand a file to
// another app (/file/write, /file/open, /file/save). Run against Node they
// pin down what rox-broker's hub must match (BUWIZ_BROKER_HUB=1).
//
// osascript, open and secret are the PATH stubs: the Reminders answers and
// the save panel's chosen path come from a file the test writes. Linear's
// URL is fixed to api.linear.app in Node, so only what needs no network is
// covered: no LINEAR_API_KEY and `secret` failing. No test here has a key,
// so none can reach Linear.
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { offlineEnv, postJson, request, startServer, stubCalls, tempDir, waitFor } from "./helpers.mjs";

let server;
const DATA = tempDir("leftovers");
const GEN = join(DATA, "generated");
const FILES = join(GEN, "files");
const LOG = join(DATA, "stub-calls.jsonl");
const ANSWER = join(DATA, "osascript-answer.txt");
const OUTSIDE = tempDir("leftovers-outside");

const HINT = "grant permission in System Settings → Privacy & Security → Reminders / Automation";
const FAILED = { error: "Command failed: osascript -l JavaScript -e ", hint: HINT };

before(async () => {
  server = await startServer(DATA, {
    env: { ...offlineEnv(LOG), OSASCRIPT_STUB_FILE: ANSWER, LINEAR_API_KEY: undefined, SECRET_STUB_LINEAR_API_KEY: undefined },
  });
  mkdirSync(join(FILES, "t1"), { recursive: true });
  writeFileSync(join(FILES, "t1", "doc.pdf"), "pdf bytes");
  writeFileSync(join(FILES, "t1", "old.TXT"), "before");
  mkdirSync(join(GEN, "assets"), { recursive: true });
  writeFileSync(join(GEN, "assets", "img.png"), "png bytes");
  writeFileSync(join(GEN, "R1.tsx"), "export default () => null; // R1");
  mkdirSync(join(GEN, "R2"), { recursive: true });
  writeFileSync(join(GEN, "R2", "chart.v1.tsx"), "v1");
  writeFileSync(join(GEN, "R2", "chart.v2.tsx"), "v2");
  writeFileSync(join(DATA, "secret.txt"), "outside the files folder");
});

after(async () => {
  await server?.stop();
});

/** The stubs' calls made by `f`. */
async function callsOf(f) {
  const before = stubCalls(LOG).length;
  const out = await f();
  return [out, stubCalls(LOG).slice(before)];
}

const answer = (text) => writeFileSync(ANSWER, text);
/** A POST of `body` as is (bad JSON, say), typed as JSON for the guard. */
const raw = (path, body) => request("POST", path, { headers: { "Content-Type": "application/json" }, body });
const json200 = (r) => {
  assert.equal(r.status, 200, r.text);
  assert.equal(r.headers["content-type"], "application/json");
  return r.json;
};

test("LEFT-01 GET /data/reminders: the JXA list, wrapped; its errors as they are", async () => {
  const list = [{ name: "Pay rent", list: "Home", due: "2026-11-01T09:00:00.000Z", flagged: true }];
  answer(JSON.stringify(list) + "\n");
  const [r, calls] = await callsOf(() => request("GET", "/data/reminders"));
  assert.deepEqual(json200(r), { reminders: list });
  assert.equal(r.text, JSON.stringify({ reminders: list }));
  assert.equal(calls.length, 1);
  const { bin, argv, env } = calls[0];
  assert.equal(bin, "osascript");
  assert.deepEqual(argv.slice(0, 3), ["-l", "JavaScript", "-e"]);
  assert.equal(argv.length, 4);
  assert.ok(argv[3].startsWith('\n      const app = Application("Reminders");\n      const out = [];'), argv[3]);
  assert.ok(argv[3].includes("l.reminders.whose({completed: false})()"));
  assert.ok(argv[3].includes("if (out.length >= 50) break outer;"));
  assert.ok(argv[3].endsWith("\n      JSON.stringify(out);"));
  assert.equal(env, undefined);

  // nothing printed is JSON's null: no reminders
  answer("");
  assert.deepEqual(json200(await request("GET", "/data/reminders")), { reminders: [] });
  answer("  null \n");
  assert.deepEqual(json200(await request("GET", "/data/reminders")), { reminders: [] });
  // an object with `error` comes back as it is; any other value is wrapped
  answer('{"error":"no access","x":1}');
  assert.equal((await request("GET", "/data/reminders")).text, '{"error":"no access","x":1}');
  answer('{"a":1}');
  assert.equal((await request("GET", "/data/reminders")).text, '{"reminders":{"a":1}}');
  answer("0");
  assert.equal((await request("GET", "/data/reminders")).text, '{"reminders":[]}');
  // osascript failing (a denied permission): the first line of the error
  answer("!cancel");
  assert.deepEqual(json200(await request("GET", "/data/reminders")), FAILED);
});

test("LEFT-02 POST /data/reminders/add: the arguments travel as REM_* env vars", async () => {
  answer('{"ok":true,"name":"Milk","list":"Groceries"}');
  const [r, calls] = await callsOf(() => postJson("/data/reminders/add", { name: "Milk", list: "Groceries", due: "2026-11-02T10:00:00Z" }));
  assert.equal(json200(r) && r.text, '{"ok":true,"name":"Milk","list":"Groceries"}');
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].env, { REM_DUE: "2026-11-02T10:00:00Z", REM_LIST: "Groceries", REM_NAME: "Milk" });
  const script = calls[0].argv[3];
  assert.deepEqual(calls[0].argv.slice(0, 3), ["-l", "JavaScript", "-e"]);
  assert.ok(script.startsWith('\n            const app = Application("Reminders");\n            ObjC.import("Cocoa");'), script);
  assert.ok(script.includes("if (!list) list = app.defaultList();"));
  assert.ok(script.includes("list.reminders.push(app.Reminder(props));"));
  assert.ok(script.endsWith("\n            JSON.stringify({ ok: true, name, list: list.name() });"));

  // String() of each field; empty when missing
  answer("null");
  const [n, more] = await callsOf(() => postJson("/data/reminders/add", { name: 42, list: ["a", "b"] }));
  assert.equal(n.text, "null");
  assert.deepEqual(more[0].env, { REM_DUE: "", REM_LIST: "a,b", REM_NAME: "42" });

  // no name: refused before osascript runs
  for (const body of [{}, { name: "" }, { list: "Home" }]) {
    const [r2, none] = await callsOf(() => postJson("/data/reminders/add", body));
    assert.deepEqual(json200(r2), { error: "name?" });
    assert.deepEqual(none, []);
  }
  const [bad, none] = await callsOf(() => raw("/data/reminders/add", "not json"));
  assert.deepEqual(json200(bad), { error: "name?" });
  assert.deepEqual(none, []);
  const [empty] = await callsOf(() => request("POST", "/data/reminders/add"));
  assert.deepEqual(json200(empty), { error: "name?" });

  answer("!cancel");
  assert.deepEqual(json200(await postJson("/data/reminders/add", { name: "Milk" })), FAILED);
});

test("LEFT-03 POST /data/reminders/complete: the first open reminder of that name", async () => {
  answer('{"ok":false}');
  const [r, calls] = await callsOf(() => postJson("/data/reminders/complete", { name: "Pay rent" }));
  assert.equal(json200(r) && r.text, '{"ok":false}');
  assert.deepEqual(calls[0].env, { REM_DUE: "", REM_LIST: "", REM_NAME: "Pay rent" });
  const script = calls[0].argv[3];
  assert.ok(script.startsWith('\n            const app = Application("Reminders");\n            ObjC.import("Cocoa");'), script);
  assert.ok(script.includes('const name = ObjC.unwrap(env.objectForKey("REM_NAME"));'));
  assert.ok(script.includes("if (hits.length) { hits[0].completed = true; done = true; break outer; }"));
  assert.ok(script.endsWith("\n            JSON.stringify({ ok: done });"));

  const [no, none] = await callsOf(() => postJson("/data/reminders/complete", { list: "Home" }));
  assert.deepEqual(json200(no), { error: "name?" });
  assert.deepEqual(none, []);

  answer("!cancel");
  assert.deepEqual(json200(await postJson("/data/reminders/complete", { name: "x" })), FAILED);
});

test("LEFT-04 /data/linear/* with no key (no env var, `secret` failing) never leaves the machine", async () => {
  const [issues, calls] = await callsOf(() => request("GET", "/data/linear/issues"));
  assert.deepEqual(json200(issues), { error: "linear not configured — run `secret set LINEAR_API_KEY`" });
  assert.deepEqual(calls, [{ bin: "secret", argv: ["get", "LINEAR_API_KEY"] }]);

  const [states, again] = await callsOf(() => request("GET", "/data/linear/states"));
  assert.deepEqual(json200(states), { error: "linear not configured" });
  // a missing key is looked up again on every call
  assert.deepEqual(again, [{ bin: "secret", argv: ["get", "LINEAR_API_KEY"] }]);

  // assign: no uid is refused before any lookup
  for (const body of ["{}", '{"uid":""}', "nope", undefined]) {
    const [r, none] = await callsOf(() => raw("/data/linear/assign", body));
    assert.deepEqual(json200(r), { error: "uid?" }, String(body));
    assert.deepEqual(none, []);
  }
  // with a uid and no key, the viewer lookup comes back empty: 200, not 502
  const [assign, lookups] = await callsOf(() => postJson("/data/linear/assign", { uid: "abc" }));
  assert.deepEqual(json200(assign), { error: "no viewer" });
  assert.deepEqual(lookups, [{ bin: "secret", argv: ["get", "LINEAR_API_KEY"] }]);
});

test("LEFT-05 POST /file/write: the file and its versions under files/.versions", async () => {
  const b64 = (s) => Buffer.from(s).toString("base64");
  // a new file in an existing folder: v1 is the first save
  assert.deepEqual(json200(await postJson("/file/write", { path: "t1/new.docx", data: b64("one") })), { ok: true, v: 1 });
  assert.equal(readFileSync(join(FILES, "t1", "new.docx"), "utf8"), "one");
  assert.deepEqual(readdirSync(join(FILES, ".versions", "t1", "new.docx")), ["v1.docx"]);
  assert.equal((await postJson("/file/write", { path: "t1/new.docx", data: b64("two") })).text, '{"ok":true,"v":2}');
  assert.equal(readFileSync(join(FILES, "t1", "new.docx"), "utf8"), "two");
  assert.equal(readFileSync(join(FILES, ".versions", "t1", "new.docx", "v2.docx"), "utf8"), "two");

  // a file that was there before its first save keeps that content as v1;
  // the extension is lower-cased for the version files
  assert.deepEqual(json200(await postJson("/file/write", { path: "t1/old.TXT", data: b64("after") })), { ok: true, v: 2 });
  assert.equal(readFileSync(join(FILES, ".versions", "t1", "old.TXT", "v1.txt"), "utf8"), "before");
  assert.equal(readFileSync(join(FILES, ".versions", "t1", "old.TXT", "v2.txt"), "utf8"), "after");

  // an old version saved again becomes the file's next version
  assert.deepEqual(json200(await postJson("/file/write", { path: ".versions/t1/new.docx/v1.docx", data: b64("three") })), { ok: true, v: 3 });
  assert.equal(readFileSync(join(FILES, "t1", "new.docx"), "utf8"), "three");
  assert.equal(readFileSync(join(FILES, ".versions", "t1", "new.docx", "v1.docx"), "utf8"), "one");

  // no extension: .bin; no data: an empty file; base64url and stray characters
  assert.deepEqual(json200(await postJson("/file/write", { path: "t1/plain", data: "" })), { ok: true, v: 1 });
  assert.equal(readFileSync(join(FILES, ".versions", "t1", "plain", "v1.bin"), "utf8"), "");
  assert.deepEqual(json200(await postJson("/file/write", { path: "t1/plain", data: "_-8\n=Zm9v" })), { ok: true, v: 2 });
  assert.deepEqual([...readFileSync(join(FILES, "t1", "plain"))], [0xff, 0xef]);
  assert.deepEqual(json200(await postJson("/file/write", { path: "t1/plain", data: "aGk" })), { ok: true, v: 3 });
  assert.equal(readFileSync(join(FILES, "t1", "plain"), "utf8"), "hi");
});

test("LEFT-06 POST /file/write stays inside the files folder", async () => {
  const bad = { ok: false, error: "bad path" };
  for (const path of ["../escape.txt", "../../escape.txt", "t1/../../escape.txt", "", "nodir/x.txt", "/etc/x.txt", `${OUTSIDE}/x.txt`, "..", "."]) {
    assert.deepEqual(json200(await postJson("/file/write", { path, data: "eA==" })), bad, path);
  }
  assert.deepEqual(json200(await postJson("/file/write", {})), bad);
  assert.deepEqual(json200(await request("POST", "/file/write")), bad);
  assert.equal(existsSync(join(GEN, "escape.txt")), false);
  assert.equal(existsSync(join(DATA, "escape.txt")), false);
  assert.deepEqual(readdirSync(OUTSIDE), []);
  // an absolute path is joined under the files folder, not taken as is
  mkdirSync(join(FILES, "abs"), { recursive: true });
  assert.deepEqual(json200(await postJson("/file/write", { path: "/abs/in.txt", data: "aW4=" })), { ok: true, v: 1 });
  assert.equal(readFileSync(join(FILES, "abs", "in.txt"), "utf8"), "in");

  // what throws comes back as {ok: false, error} (its first 200 characters):
  // a body that is no JSON or null, a folder where the file should be
  assert.deepEqual(json200(await raw("/file/write", "nope")), { ok: false, error: `Unexpected token 'o', "nope" is not valid JSON` });
  assert.deepEqual(json200(await raw("/file/write", "null")), { ok: false, error: "Cannot destructure property 'path' of 'JSON.parse(...)' as it is null." });
  assert.deepEqual(json200(await postJson("/file/write", { path: "t1", data: "eA==" })), {
    ok: false,
    error: `ENOTSUP: operation not supported on socket, copyfile '${join(FILES, "t1")}' -> '${join(FILES, ".versions", "t1", "v1.bin")}'`.slice(0, 200),
  });
});

test("LEFT-07 POST /file/open: a reply's file or an app's source, in its own app", async () => {
  const opened = async (body) => {
    const opens = stubCalls(LOG).filter((c) => c.bin === "open").length;
    const [r, calls] = await callsOf(async () => {
      const r = await (typeof body === "string" ? raw("/file/open", body) : postJson("/file/open", body));
      // open runs detached: wait for its log line
      if (r.json?.ok) await waitFor(() => stubCalls(LOG).filter((c) => c.bin === "open").length > opens, 5000);
      return r;
    });
    return [json200(r), calls.filter((c) => c.bin === "open").map((c) => c.argv)];
  };
  assert.deepEqual(await opened({ path: "t1/doc.pdf" }), [{ ok: true }, [[join(FILES, "t1", "doc.pdf")]]]);
  // an app's source: the current version, else generated/R<n>.tsx; a pinned one
  assert.deepEqual(await opened({ path: "app:R1" }), [{ ok: true }, [[join(GEN, "R1.tsx")]]]);
  assert.deepEqual(await opened({ path: "app:R2@v1" }), [{ ok: true }, [[join(GEN, "R2", "chart.v1.tsx")]]]);
  const db = new DatabaseSync(join(GEN, "app.db"), { timeout: 5000 });
  db.prepare("INSERT OR REPLACE INTO heads(name, v) VALUES ('R2', 2)").run();
  db.close();
  assert.deepEqual(await opened({ path: "app:R2" }), [{ ok: true }, [[join(GEN, "R2", "chart.v2.tsx")]]]);

  for (const body of [{ path: "t1/missing.pdf" }, { path: "../secret.txt" }, { path: "../../secret.txt" }, { path: "app:R9" }, { path: "app:R2@v7" }, { path: "app:R1@v1" }, {}, "nope", ""]) {
    assert.deepEqual(await opened(body), [{ ok: false }, []], JSON.stringify(body));
  }
});

test("LEFT-08 POST /file/save: the save panel, then a copy where it says", async () => {
  const saved = async (body) => {
    const [r, calls] = await callsOf(() => postJson("/file/save", body));
    return [json200(r), calls.filter((c) => c.bin === "osascript").map((c) => c.argv)];
  };
  const panel = (name) => ["-e", `POSIX path of (choose file name with prompt "Download" default name "${name}")`];

  const dest = join(OUTSIDE, "copy.pdf");
  answer(`${dest}\n`);
  assert.deepEqual(await saved({ path: "t1/doc.pdf" }), [{ ok: true, dest }, [panel("doc.pdf")]]);
  assert.equal(readFileSync(dest, "utf8"), "pdf bytes");
  rmSync(dest);

  // `name` is the panel's default, its folders, quotes and backslashes dropped
  const named = join(OUTSIDE, "named.pdf");
  answer(named);
  assert.deepEqual(await saved({ path: "t1/doc.pdf", name: 'a/b/My "big" \\doc.pdf' }), [{ ok: true, dest: named }, [panel("My big doc.pdf")]]);
  rmSync(named);

  // a generated image by its bare file name
  const img = join(OUTSIDE, "img.png");
  answer(img);
  assert.deepEqual(await saved({ asset: "../../x/img.png" }), [{ ok: true, dest: img }, [panel("img.png")]]);
  assert.equal(readFileSync(img, "utf8"), "png bytes");
  rmSync(img);

  // an app's source
  const src = join(OUTSIDE, "R1.tsx");
  answer(src);
  assert.deepEqual(await saved({ path: "app:R1" }), [{ ok: true, dest: src }, [panel("R1.tsx")]]);
  rmSync(src);

  // the panel cancelled, or a path the copy can't take
  answer("!cancel");
  assert.deepEqual(await saved({ path: "t1/doc.pdf" }), [{ ok: false, cancelled: true }, [panel("doc.pdf")]]);
  answer("");
  assert.deepEqual(await saved({ path: "t1/doc.pdf" }), [{ ok: false, cancelled: true }, [panel("doc.pdf")]]);
  answer(join(OUTSIDE, "no-such-dir", "x.pdf"));
  assert.deepEqual(await saved({ path: "t1/doc.pdf" }), [{ ok: false, cancelled: true }, [panel("doc.pdf")]]);

  // a file not there, or outside the files folder: no panel at all
  for (const body of [{ path: "t1/missing.pdf" }, { path: "../secret.txt" }, { asset: "nope.png" }, { asset: "x/" }, {}]) {
    assert.deepEqual(await saved(body), [{ ok: false }, []], JSON.stringify(body));
  }
  assert.deepEqual(readdirSync(OUTSIDE), []);
});

test("LEFT-09 these routes match the URL exactly, as Node does: a query string is not found", async () => {
  for (const [method, url] of [["GET", "/data/reminders?x=1"], ["GET", "/data/linear/issues?x"], ["POST", "/file/write?x=1"], ["POST", "/file/open?"]]) {
    const r = method === "POST" ? await raw(url, "{}") : await request(method, url);
    assert.equal(r.status, 404, url);
    assert.equal(r.text, "not found");
  }
});
