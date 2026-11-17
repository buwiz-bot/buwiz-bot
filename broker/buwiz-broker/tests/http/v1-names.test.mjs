// Protocol 1.0's names at the plugin boundary: the buwiz ones work and the
// older rox ones are refused. A plugin naming 2.0 or 2.1 (1.0's pre-release
// numbers) is told to install its 1.0 version, and an old record does not
// start; a `rox-session` certificate or a `rox-call` envelope does not verify;
// `rox.*` is no reserved tool name (the grammar's unit test is
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
    assert.ok(p.tools.every((t) => !t.name.startsWith("buwiz.") && !t.name.startsWith("rox.")), "no reserved tool is listed");
    const open = await openApp(api, p, "board");

    assert.equal(open.res.cert.typ, "buwiz-session");
    const r = await api.call(envelope(open, "fx_echo", { n: 1 }));

    assert.equal(r.ok, true);
    assert.equal(r.result.structuredContent.caller.origin, "app", '_meta["buwiz/caller"] reached the plugin');
    assert.deepEqual(await api.ticketTitle({ plugin: FX, ticket: "tkt_0000000000000000000000000A" }), { ok: true, valid: false });
    assert.equal((await api.appSource({ plugin: FX, app: "board" })).fingerprint.slice(0, 7), "sha256:", "read at buwiz-app://board");
  });
});

test("NAM-02 the rox names are refused: a rox-call envelope", async () => {
  await withHost("names-call", async ({ b, api }) => {
    await installFixture(api, b);
    const open = await openApp(api, await api.plugin(), "board");

    assert.equal(await codeOf(api.call(envelope(open, "fx_echo", {}, { typ: "rox-call" }))), "session_invalid");
    assert.equal((await api.call(envelope(open, "fx_echo", {}, { typ: "buwiz-call" }))).ok, true);
  });
});

test("NAM-03 a plugin naming protocol 2.0 or 2.1 (1.0's pre-release numbers) is told to install its 1.0 version", async () => {
  await withHost("names-old", async ({ b, api }) => {
    b.writeManifest((x) => (x.protocol = "2.1.0"));
    await assert.rejects(api.inspect(fixtureSpec(b)), (e) => e.code === "protocol_unsupported" && e.message === "Fixture uses the pre-release mini app protocol 2.1.0; install its 1.0 version");
  });
});

test("NAM-03c a plugin built for Rox (rox://manifest, asking for rox.provision) is told it was made for Rox", async () => {
  for (const mode of ["rox", "rox:quiet"]) {
    await withHost(`names-${mode.replace(":", "-")}`, async ({ b, api }) => {
      await assert.rejects(api.inspect(fixtureSpec(b, [`--misbehave=${mode}`])), (e) => e.code === "protocol_unsupported" && e.message === "Fixture was made for Rox, an older protocol; install its 1.0 version", mode);
    });
  }
});

test("NAM-03b an installed record from Rox (rox.* tools) or a pre-release protocol fails before it spawns", async () => {
  const b = bed("names-record");
  const dataDir = tempDir("v1-names-record-data");
  const env = { BUWIZ_KEYSTORE: `file:${b.keysDir}`, BUWIZ_TRUST_FILE: b.trustFile };
  const first = await startServer(dataDir, { env });

  try {
    await installFixture(hostApi(dataDir), b);
  } finally {
    await first.stop();
  }

  const cases = [
    [{ protocol: "1.1.0", tools: JSON.stringify([{ name: "rox.provision", description: "", readOnly: false }]) }, "Fixture was made for Rox, an older protocol; install its 1.0 version"],
    [{ protocol: "2.0.0", tools: "[]" }, "Fixture uses the pre-release mini app protocol 2.0.0; install its 1.0 version"],
  ];

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

test("NAM-04 a rox-session certificate does not verify", async () => {
  await withHost("names-cert", async ({ b, api }) => {
    await installFixture(api, b, ["--misbehave=old-cert"]);
    assert.equal(await codeOf(openApp(api, await api.plugin(), "board")), "plugin_bad_signature");
  });
});
