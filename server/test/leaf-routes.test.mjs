// The leaf routes the Rust broker answers itself (plan phase 2): /state,
// /prefs, /keymap, /data/timeclock, the skills list. Run against Node they
// pin down what the Rust side must match; with BUWIZ_BROKER_CMD naming
// buwiz-broker, they check that it does.
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { PORT, postJson, request, startServer, tempDir } from "./helpers.mjs";

let server;
const DATA = tempDir("leaf");

before(async () => {
  server = await startServer(DATA);
});

after(async () => {
  await server?.stop();
});

const post = (path, body) =>
  request("POST", path, { headers: { "Content-Type": "application/json" }, body });

test("STATE-01 a widget's state round-trips by path, query and body name", async () => {
  assert.deepEqual((await request("GET", "/state/leaf-a")).json, { state: null });
  assert.deepEqual((await postJson("/state/leaf-a", { n: 1, list: ["x"] })).json, { ok: true });
  const r = await request("GET", "/state/leaf-a");
  assert.equal(r.status, 200);
  assert.match(r.headers["content-type"], /application\/json/);
  assert.equal(r.text, '{"state":{"n":1,"list":["x"]}}');

  await postJson("/state?name=leaf-b", { state: { b: true } });
  assert.equal((await request("GET", "/state?name=leaf-b")).text, '{"state":{"b":true}}');

  await postJson("/state", { name: "leaf-c", state: [1, 2] });
  assert.equal((await request("GET", "/state/leaf-c")).text, '{"state":[1,2]}');

  // an encoded name is decoded; a state of null is kept as {}
  await postJson("/state/leaf%2Dd", { state: null });
  assert.equal((await request("GET", "/state/leaf-d")).text, '{"state":{}}');
});

test("STATE-02 numbers and key order come back as JavaScript writes them", async () => {
  await post("/state/leaf-num", '{"b":1.0,"a":1e21,"2":0.000001,"1":1e-7,"z":-0,"big":12345678901234567890}');
  assert.equal(
    (await request("GET", "/state/leaf-num")).text,
    '{"state":{"1":1e-7,"2":0.000001,"b":1,"a":1e+21,"z":0,"big":12345678901234567000}}',
  );
  await post("/state/leaf-dup", '{"a":1,"b":2,"a":3}');
  assert.equal((await request("GET", "/state/leaf-dup")).text, '{"state":{"a":3,"b":2}}');
  await post("/state/leaf-str", JSON.stringify({ s: 'q"\\\n\t\u0001\u2028é😀/' }));
  assert.equal((await request("GET", "/state/leaf-str")).json.state.s, 'q"\\\n\t\u0001\u2028é😀/');
});

test("STATE-03 a bad name or body is refused, and a bad name reads as empty", async () => {
  const bad = await postJson("/state/no%20spaces", { a: 1 });
  assert.equal(bad.status, 400);
  assert.deepEqual(bad.json, { ok: false, error: "bad module name" });
  assert.equal((await postJson("/state", { state: 1 })).status, 400);
  assert.deepEqual((await request("GET", "/state/no%20spaces")).json, { state: null });
  assert.deepEqual((await request("GET", "/state")).json, { state: null });

  const notJson = await post("/state/leaf-e", "{nope");
  assert.equal(notJson.status, 400);
  assert.equal(notJson.json.ok, false);

  const nul = await post("/state/leaf-e", "null");
  assert.equal(nul.status, 400);
  assert.match(nul.json.error, /reading 'state'/);
  assert.deepEqual((await request("GET", "/state/leaf-e")).json, { state: null });

  // a bare value is the state itself
  await post("/state/leaf-f", "5");
  assert.equal((await request("GET", "/state/leaf-f")).text, '{"state":5}');
});

test("STATE-04 an empty body stores {}", async () => {
  assert.equal((await request("POST", "/state/leaf-g")).status, 200);
  assert.equal((await request("GET", "/state/leaf-g")).text, '{"state":{}}');
});

test("PREFS-01 defaults, then what was set", async () => {
  const d = await request("GET", "/prefs");
  assert.equal(d.text, '{"theme":"dark","model":"","effort":"","fast":false,"unassigned_name":"Unassigned","chats_name":"Chats"}');

  assert.deepEqual((await postJson("/prefs", { theme: "light", model: 5, effort: "high", fast: "0" })).json, { ok: true });
  await postJson("/prefs", { theme: "neon", chats_name: "  My\n  chats  " });
  assert.deepEqual((await request("GET", "/prefs")).json, {
    theme: "light", model: "5", effort: "high", fast: true, unassigned_name: "Unassigned", chats_name: "My chats",
  });

  await postJson("/prefs", { fast: 0, model: null, unassigned_name: "x".repeat(70) });
  const p = (await request("GET", "/prefs")).json;
  assert.equal(p.fast, false);
  assert.equal(p.model, "null");
  assert.equal(p.unassigned_name, "x".repeat(60));
});

test("PREFS-02 a blank section name is refused, after the fields before it are kept", async () => {
  const r = await postJson("/prefs", { effort: "low", unassigned_name: "Inbox", chats_name: "   " });
  assert.equal(r.status, 400);
  assert.deepEqual(r.json, { ok: false, error: "a section needs a name" });
  const p = (await request("GET", "/prefs")).json;
  assert.equal(p.effort, "low");
  assert.equal(p.unassigned_name, "Inbox");
});

test("KEYMAP-01 the user's shortcuts, read fresh and written cleaned", async () => {
  assert.equal((await request("GET", "/keymap")).text, '{"keymap":{}}');
  const ok = await postJson("/keymap", { keymap: { "chat.new": [" Mod+N ", "", "x", "y", "z", "w"], "2": [], "a.b": [5] } });
  assert.deepEqual(ok.json, { ok: true });
  assert.equal(
    readFileSync(join(DATA, "keymap.json"), "utf8"),
    '{\n  "2": [],\n  "chat.new": [\n    "mod+n",\n    "x",\n    "y",\n    "z"\n  ],\n  "a.b": [\n    "5"\n  ]\n}\n',
  );
  assert.equal((await request("GET", "/keymap")).text, '{"keymap":{"2":[],"chat.new":["mod+n","x","y","z"],"a.b":["5"]}}');

  writeFileSync(join(DATA, "keymap.json"), "[1,2]");
  assert.equal((await request("GET", "/keymap")).text, '{"keymap":{}}');
  writeFileSync(join(DATA, "keymap.json"), "not json");
  assert.equal((await request("GET", "/keymap")).text, '{"keymap":{}}');
});

test("KEYMAP-02 a bad keymap is refused and the file is left alone", async () => {
  writeFileSync(join(DATA, "keymap.json"), '{"keep":["k"]}');
  for (const [body, error] of [
    [{}, "keymap must be an object"],
    [{ keymap: [] }, "keymap must be an object"],
    [{ keymap: { "has space": [] } }, "bad entry has space"],
    [{ keymap: { ok: "mod+k" } }, "bad entry ok"],
  ]) {
    const r = await postJson("/keymap", body);
    assert.equal(r.status, 400);
    assert.match(r.headers["content-type"], /application\/json/);
    assert.deepEqual(r.json, { ok: false, error });
  }
  assert.equal(readFileSync(join(DATA, "keymap.json"), "utf8"), '{"keep":["k"]}');
});

test("TIME-01 clocking in and out", async () => {
  const start = (await request("GET", "/data/timeclock")).json;
  assert.deepEqual(start, { status: "out", since: null, todayMin: 0, sessions: [] });

  const inn = (await request("POST", "/data/timeclock/in")).json;
  assert.equal(inn.ok, true);
  assert.equal(inn.already, false);
  assert.equal(inn.status, "in");
  assert.match(inn.since, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  assert.deepEqual(Object.keys(inn), ["ok", "already", "status", "since", "todayMin", "sessions"]);
  assert.deepEqual(inn.sessions, [{ in: inn.since, out: null, min: 0 }]);

  const again = (await request("POST", "/data/timeclock/in")).json;
  assert.equal(again.already, true);
  assert.equal(again.since, inn.since);

  const out = (await request("POST", "/data/timeclock/out")).json;
  assert.deepEqual(Object.keys(out), ["ok", "status", "since", "todayMin", "sessions"]);
  assert.equal(out.status, "out");
  assert.equal(out.sessions.length, 1);
  assert.equal(out.sessions[0].in, inn.since);
  assert.match(out.sessions[0].out, /Z$/);
});

test("SKILLS-01 the skills list", async () => {
  assert.equal((await request("GET", "/skills")).text, '{"skills":[]}');
  const made = await postJson("/skills", { name: "tidy", description: "Tidy up", instructions: "Do it." });
  assert.equal(made.status, 200);
  const list = (await request("GET", "/skills")).json.skills;
  assert.equal(list.length, 1);
  assert.deepEqual(Object.keys(list[0]), ["id", "name", "description", "body", "enabled"]);
  assert.equal(list[0].name, "tidy");
  assert.equal(list[0].enabled, true);
});

test("LEAF-01 the guard and CORS hold on these routes", async () => {
  const foreign = await request("POST", "/state/leaf-h", { headers: { Origin: "https://evil.example", "Content-Type": "application/json" }, body: "{}" });
  assert.equal(foreign.status, 403);
  assert.equal(foreign.json.ok, false);
  const text = await request("POST", "/prefs", { headers: { "Content-Type": "text/plain" }, body: "{}" });
  assert.equal(text.status, 415);
  const own = await request("GET", "/prefs", { headers: { Origin: `http://127.0.0.1:${PORT}` } });
  assert.equal(own.status, 200);
  assert.equal(own.headers["access-control-allow-origin"], `http://127.0.0.1:${PORT}`);
  assert.equal(own.headers.vary, "Origin");
  const pre = await request("OPTIONS", "/state/leaf-h", { headers: { Origin: `http://localhost:${PORT}` } });
  assert.equal(pre.status, 204);
});
