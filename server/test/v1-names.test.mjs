// Protocol 2.0's names at the plugin boundary: the buwiz ones work and the
// 1.x rox ones are refused. A plugin built for protocol 1.x is told to
// update; a `rox-session` certificate or a `rox-call` envelope does not
// verify; `rox.*` tools are no longer reserved, so a plugin cannot list
// them; the plugin hears `buwiz/caller`.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { isV1 } from "../src/grammar.mjs";
import { FX, bed, broker, codeOf, envelope, fixtureSpec, installFixture, openApp } from "./v1-helpers.mjs";

const b = bed("names");
const { plugins, sessions } = broker(b, { timeouts: { call: 2000 } });
let m;

before(async () => {
  await installFixture(plugins, b);
  m = plugins.manifestOf(plugins.rowOf(FX));
});

after(() => plugins.stopAll());

test("NAM-01 the buwiz names: buwiz.provision ran, buwiz-session verified, buwiz-call goes through, the plugin sees buwiz/caller", async () => {
  assert.equal(plugins.runtime(FX).state, "running");
  assert.ok(plugins.runtime(FX).tools.some((t) => t.name === "buwiz.provision"));
  const open = await openApp(sessions, m, "board");

  assert.equal(open.res.cert.typ, "buwiz-session");
  const r = await sessions.call(envelope(open, "fx_echo", { n: 1 }));

  assert.equal(r.ok, true);
  assert.equal(r.result.structuredContent.caller.origin, "app", "_meta[\"buwiz/caller\"] reached the plugin");
  assert.deepEqual(await plugins.callReserved(FX, "buwiz.ticket.resolve", { ticket: "tkt_0000000000000000000000000A" }), { valid: false });
  assert.equal((await sessions.appSource({ plugin: FX, app: "board" })).fingerprint.slice(0, 7), "sha256:", "read at buwiz-app://board");
});

test("NAM-02 the rox names are refused: a rox-call envelope, rox.* as reserved names", async () => {
  const open = await openApp(sessions, m, "board");

  assert.equal(await codeOf(sessions.call(envelope(open, "fx_echo", {}, { typ: "rox-call" }))), "session_invalid");
  assert.ok(isV1("reservedTool", "buwiz.session.open"));
  assert.ok(!isV1("reservedTool", "rox.session.open"), "rox.* is no reserved name in 2.0");
});

test("NAM-03 a protocol 1.x plugin is told to update", async () => {
  const old = bed("names-old");

  old.writeManifest((x) => (x.protocol = "1.1.0"));
  const o = broker(old);

  await assert.rejects(o.plugins.inspect(fixtureSpec(old)), (e) => e.code === "protocol_unsupported" && /this plugin is built for an older protocol; update it/.test(e.message));
  await o.plugins.stopAll();
});

test("NAM-04 a rox-session certificate does not verify", async () => {
  const old = bed("names-cert");
  const o = broker(old);

  await installFixture(o.plugins, old, ["--misbehave=old-cert"]);
  const mm = o.plugins.manifestOf(o.plugins.rowOf(FX));

  assert.equal(await codeOf(openApp(o.sessions, mm, "board")), "plugin_bad_signature");
  await o.plugins.stopAll();
});
