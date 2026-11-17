// A plugin's badge on its pins (`_meta["buwiz/badge"]`, spec 16.4) over HTTP, on a
// broker with its own data dir: the fixture plugin installed, a tool result
// that carries a badge from an app's signed call and from a routine's query,
// the badge read back from the plugin listing the app loads and from
// the events stream (`event: badge`), a clear, a restart and an uninstall.
// The app holds the pins themselves; the broker holds the badge of each
// (plugin, app, ticket).
//
//   BDG-01 a signed call whose result carries a badge: the app's answer has no
//          `_meta`; the listing has the badge; the stream says it
//   BDG-02 a routine's query result sets one too (Run now, no app open)
//   BDG-03 a malformed one and a failed result's change nothing and the
//          answer returns; a clear removes one, and the stream says so
//   BDG-04 a restart keeps the badge; the listing of a fresh app has it
//   BDG-05 uninstall removes it, and the stream says so
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { generate, random128 } from "../fixtures/v1/ed.mjs";
import { PORT, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";
import { FX, bed, envelope, fixtureSpec, permsDigest } from "./v1-helpers.mjs";

const b = bed("badge");
const dataDir = tempDir("v1-badge");
const stubLog = join(tempDir("v1-badge-stubs"), "stubs.log");
const TICKET = "tkt_0123456789ABCDEFGHJKMNPQRS";
const BADGER = {
  id: "fx-badger", name: "Fixture badger", description: "Sets a badge from its query.", every_s: 300,
  query: { tool: "fx_badge", field: "query", args: { badge: { app: "card", dot: "amber", text: "18h" } } }, prompt: "Note each new item.",
};
let server;
let token = "";
let open;
let manifest;
const brokerEnv = () => ({ ...offlineEnv(stubLog), BUWIZ_KEYSTORE: `file:${b.keysDir}`, BUWIZ_TRUST_FILE: b.trustFile });
const host = (method, path, obj) =>
  request(method, path, { headers: { "X-Buwiz-Host": token, ...(obj === undefined ? {} : { "Content-Type": "application/json" }) }, body: obj === undefined ? undefined : JSON.stringify(obj) });
const badges = async () => (await host("GET", "/v1/host/plugins")).json.plugins.find((p) => p.id === FX)?.badges;
const one = (app, ticket, dot, text) => ({ app, ticket, dot, text });

/** /v1/host/events: the badge events so far. */
function listen() {
  const out = { raw: "", events: [] };
  const req = httpRequest({ host: "127.0.0.1", port: PORT, path: "/v1/host/events", headers: { "X-Buwiz-Host": token } }, (res) => {
    res.setEncoding("utf8");
    res.on("data", (d) => {
      out.raw += d;
      out.events = [...out.raw.matchAll(/event: badge\ndata: (.+)\n\n/g)].map((m) => JSON.parse(m[1]));
    });
  });

  req.on("error", () => {});
  req.end();
  out.close = () => req.destroy();

  return out;
}

/** A signed call of fx_badge by the open app. */
const call = (args) => host("POST", "/v1/host/call", envelope(open, "fx_badge", args));

before(async () => {
  manifest = b.writeManifest((m) => m.routines.push(BADGER));
  server = await startServer(dataDir, { env: brokerEnv() });
  token = readFileSync(join(dataDir, "host-token"), "utf8");
});

after(async () => {
  await server?.stop();
});

test("BDG-01 a signed call whose result carries a badge: no _meta for the app; the listing has it; the stream says it", async () => {
  const spec = fixtureSpec(b);
  const i = await host("POST", "/v1/host/plugins/inspect", spec);
  const inst = await host("POST", "/v1/host/plugins/install", { ...spec, manifest_sha256: i.json.manifest_sha256 });

  assert.equal(inst.json.state, "running", inst.text);
  assert.deepEqual(await badges(), [], "none yet");
  const board = manifest.apps.find((a) => a.id === "board");
  const ks = generate();

  open = { plugin: FX, app: "board", open_id: random128(), ks_pub: ks.pub, perms_sha256: permsDigest(board.permissions), not_after: Date.now() + 600_000, ks };
  const { ks: _ks, ...ask } = open;

  assert.equal((await host("POST", "/v1/host/sessions/open", ask)).json.ok, true);
  const stream = listen();

  await waitFor(() => stream.raw, 5000);
  const r = await call({ badge: { app: "card", ticket: TICKET, dot: "red", text: "2 due" } });

  assert.equal(r.json.ok, true, r.text);
  assert.equal(r.json.result.structuredContent.badged, true);
  assert.ok(!("_meta" in r.json.result) && !r.text.includes("buwiz/badge"), `the app never sees the key: ${r.text}`);
  assert.deepEqual(await badges(), [one("card", TICKET, "red", "2 due")], "the plugin listing carries it");
  assert.ok(await waitFor(() => stream.events.length === 1, 5000), stream.raw);
  assert.deepEqual(stream.events[0], { plugin: FX, ...one("card", TICKET, "red", "2 due") });
  stream.close();
});

test("BDG-02 a routine's query result sets one with no app open", async () => {
  const chat = (await postJson("/thread/new", {})).json.id;
  const R = (path, body) => host("POST", `/v1/host/routines/${path}`, { plugin: FX, ...body });

  assert.equal((await R("enable", { routine: "fx-badger", thread: chat })).status, 200);
  const stream = listen();

  await waitFor(() => stream.raw, 5000);
  const run = await R("run", { routine: "fx-badger", wait: true });

  assert.equal(run.status, 200, run.text);
  assert.deepEqual(run.json.run.ran, true, run.text);
  assert.deepEqual(await badges(), [one("card", null, "amber", "18h"), one("card", TICKET, "red", "2 due")]);
  assert.ok(await waitFor(() => stream.events.length === 1, 5000), stream.raw);
  assert.deepEqual(stream.events[0], { plugin: FX, ...one("card", null, "amber", "18h") });
  stream.close();
});

test("BDG-03 a malformed badge or a failed result's changes nothing and the answer returns; a clear removes one", async () => {
  const was = await badges();
  // pace: the broker takes 20 signed calls a second per open
  for (const bad of [{ app: "forms", dot: "red" }, { app: "board", dot: "blue" }, { app: "board", text: "123456789" }, { app: "board", text: "a\nb" }, { app: "board" }, "red"]) {
    const r = await call({ badge: bad });

    assert.equal(r.json.ok, true, `${JSON.stringify(bad)}: ${r.text}`);
    assert.equal(r.json.result.structuredContent.badged, true);
  }

  const failed = await call({ fail: true, badge: { app: "board", dot: "red" } });

  assert.equal(failed.json.ok, false, "the tool failed: the call says so");
  assert.deepEqual(await badges(), was, "nothing changed");
  const stream = listen();

  await waitFor(() => stream.raw, 5000);
  const clear = await call({ badge: { app: "card", clear: true } });

  assert.equal(clear.json.ok, true, clear.text);
  assert.deepEqual(await badges(), [one("card", TICKET, "red", "2 due")], "the app-wide one is gone, the ticket's stays");
  assert.ok(await waitFor(() => stream.events.length === 1, 5000), stream.raw);
  assert.deepEqual(stream.events[0], { plugin: FX, ...one("card", null, "none", "") });
  stream.close();
});

test("BDG-04 a restart keeps the badge: the listing a fresh app loads has it", async () => {
  await server.stop();
  server = await startServer(dataDir, { env: brokerEnv() });
  token = readFileSync(join(dataDir, "host-token"), "utf8");
  assert.ok(await waitFor(async () => (await host("GET", "/v1/host/plugins")).json.plugins.find((p) => p.id === FX)?.state === "running", 20000), "the plugin started again");
  assert.deepEqual(await badges(), [one("card", TICKET, "red", "2 due")]);
});

test("BDG-05 uninstall removes the badge, and the stream says so", async () => {
  const stream = listen();

  await waitFor(() => stream.raw, 5000);
  assert.equal((await host("POST", `/v1/host/plugins/${FX}/uninstall`, {})).json.ok, true);
  assert.ok(await waitFor(() => stream.events.length === 1, 5000), stream.raw);
  assert.deepEqual(stream.events[0], { plugin: FX, ...one("card", TICKET, "none", "") });
  assert.deepEqual((await host("GET", "/v1/host/plugins")).json.plugins, []);
  stream.close();
});
