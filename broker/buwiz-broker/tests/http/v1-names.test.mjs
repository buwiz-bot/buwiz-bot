// Protocol 1.0's names at the plugin boundary: the buwiz ones work. A plugin
// naming any other protocol gets the one refusal, and a record of an older
// protocol does not start; a certificate or call envelope of another type does
// not verify (the grammar's unit test is
// `every_grammar_takes_its_good_value_and_refuses_its_bad_one`, src/v1/mod.rs);
// the plugin hears `buwiz/caller`.
import assert from "node:assert/strict";
import { existsSync, rmSync } from "node:fs";
import { test } from "node:test";
import { startServer, seedDb, tempDir } from "./helpers.mjs";
import { FX, bed, codeOf, envelope, fixtureSpec, hostApi, installFixture, openApp, until, withHost } from "./v1-helpers.mjs";

test("NAM-01 the buwiz names: buwiz.provision ran, buwiz-session verified, buwiz-call goes through, the plugin sees buwiz/caller", async () => {
  await withHost("names", async ({ b, api }) => {
    await installFixture(api, b);
    const p = await api.plugin();

    assert.equal(p.state, "running", "the gate sent buwiz.provision and it answered");
    assert.ok(p.tools.every((t) => !t.name.startsWith("buwiz.")), "no reserved tool is listed");
    const open = await openApp(api, p, "board");

    assert.equal(open.res.cert.typ, "buwiz-session");
    const r = await api.call(envelope(open, "fx_echo", { n: 1 }));

    assert.equal(r.ok, true);
    assert.equal(r.result.structuredContent.caller.origin, "app", '_meta["buwiz/caller"] reached the plugin');
    assert.deepEqual(await api.ticketTitle({ plugin: FX, ticket: "tkt_0000000000000000000000000A" }), { ok: true, valid: false });
    assert.equal((await api.appSource({ plugin: FX, app: "board" })).fingerprint.slice(0, 7), "sha256:", "read at buwiz-app://board");
  });
});

test("NAM-02 a call envelope of another type is refused", async () => {
  await withHost("names-call", async ({ b, api }) => {
    await installFixture(api, b);
    const open = await openApp(api, await api.plugin(), "board");

    assert.equal(await codeOf(api.call(envelope(open, "fx_echo", {}, { typ: "other-call" }))), "session_invalid");
    assert.equal((await api.call(envelope(open, "fx_echo", {}, { typ: "buwiz-call" }))).ok, true);
  });
});

test("NAM-03 a plugin naming a protocol other than 1.0 gets the one refusal", async () => {
  for (const protocol of ["0.9.0", "1.1.0", "2.0.0", "2.1.0", "3.0.0"]) {
    await withHost(`names-old-${protocol}`, async ({ b, api }) => {
      b.writeManifest((x) => (x.protocol = protocol));
      await assert.rejects(api.inspect(fixtureSpec(b)), (e) => e.code === "protocol_unsupported" && e.message === `the plugin implements protocol ${protocol}; this Buwiz Bot supports 1.0`, protocol);
    });
  }
});

test("NAM-03c a plugin built for the old name gets an ordinary refusal and none of its tools is listed", async () => {
  await withHost("names-old-name", async ({ b, api }) => {
    const spec = fixtureSpec(b, ["--misbehave=old-name"]);
    await assert.rejects(api.inspect(spec), (e) => e.code === "plugin_invalid" && e.message.startsWith("buwiz.provision failed:") && !/older|Rox|rox/.test(e.message));
    assert.equal(await api.plugin(), undefined, "nothing was installed");
  });
});

test("NAM-03d a stored record whose tools carry the old prefix fails to start and lists no tool", async () => {
  const b = bed("names-old-record");
  const dataDir = tempDir("v1-names-old-record-data");
  const env = { BUWIZ_KEYSTORE: `file:${b.keysDir}`, BUWIZ_TRUST_FILE: b.trustFile };
  const first = await startServer(dataDir, { env });

  try {
    await installFixture(hostApi(dataDir), b);
  } finally {
    await first.stop();
  }

  const db = await seedDb(dataDir);

  try {
    db.prepare("UPDATE v1_plugins SET tools=?, args=? WHERE id=?").run(JSON.stringify([{ name: "rox.provision", description: "", readOnly: false }, { name: "fx_echo", description: "", readOnly: true }]), JSON.stringify(fixtureSpec(b, ["--misbehave=old-name"]).args), FX);
  } finally {
    db.close();
  }

  rmSync(b.statsFile, { force: true });
  const server = await startServer(dataDir, { env });

  try {
    const api = hostApi(dataDir);
    const p = await until(async () => (await api.plugin()).state === "failed" && (await api.plugin()), 10000, "the record to fail");
    assert.deepEqual(p.tools.filter((t) => t.name.startsWith("rox.")), [], "no old-prefix tool is listed");
    assert.equal(await codeOf(api.setTool(FX, "rox.provision", true)), "not_found", "and none is switchable");
  } finally {
    await server.stop();
  }
});

test("NAM-03b an installed record of an older protocol fails before it spawns", async () => {
  const b = bed("names-record");
  const dataDir = tempDir("v1-names-record-data");
  const env = { BUWIZ_KEYSTORE: `file:${b.keysDir}`, BUWIZ_TRUST_FILE: b.trustFile };
  const first = await startServer(dataDir, { env });

  try {
    await installFixture(hostApi(dataDir), b);
  } finally {
    await first.stop();
  }

  const cases = [[{ protocol: "0.9.0", tools: "[]" }, "Fixture 1.0.0 (app.buwiz.fixture) was made for an older Buwiz Bot (mini app protocol 0.9.0; this one runs 1.0.0), so it can't start. Install its current version, or uninstall it."]];

  for (const [row, message] of cases) {
    const db = await seedDb(dataDir);

    try {
      db.prepare("UPDATE v1_plugins SET protocol=?, tools=? WHERE id=?").run(row.protocol, row.tools, FX);
    } finally {
      db.close();
    }

    rmSync(b.statsFile, { force: true });
    const server = await startServer(dataDir, { env });

    try {
      const api = hostApi(dataDir);

      await until(async () => (await api.plugin()).state === "failed", 10000, "the old record to fail");
      const p = await api.plugin();

      assert.deepEqual([p.code, p.message], ["protocol_unsupported", message]);
      assert.equal(existsSync(b.statsFile), false, "never spawned");
    } finally {
      await server.stop();
    }
  }
});

test("NAM-04 a certificate of another type does not verify", async () => {
  await withHost("names-cert", async ({ b, api }) => {
    await installFixture(api, b, ["--misbehave=old-cert"]);
    assert.equal(await codeOf(openApp(api, await api.plugin(), "board")), "plugin_bad_signature");
  });
});
