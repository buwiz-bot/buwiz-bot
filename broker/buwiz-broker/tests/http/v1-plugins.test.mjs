// The plugin host over the host routes (/v1/host/*): the reserved tools a
// plugin answers and what the host does with them (spec Appendix C.4), the
// install flow and the lifecycle (C.7), and the plugin events. The fixture
// plugin runs under a real broker with its own data dir, a file keystore and
// a trust file; the app's side is plain HTTP with the host token.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { join } from "node:path";
import { test } from "node:test";
import { generate, pubOf, verify } from "../fixtures/v1/ed.mjs";
import { canonicalSigned, signedBytes } from "../fixtures/v1/jcs.mjs";
import { PORT } from "./helpers.mjs";
import { FIXTURE, FX, alive, codeOf, dataOf, envelope, fixtureManifest, fixtureSpec, installFixture, openApp, permsDigest, rawFixture, statsOf, until, withHost } from "./v1-helpers.mjs";

const stateIs = (api, state) => async () => (await api.plugin())?.state === state;
const provision = (k) => ({ protocol: "1.0.0", plugin: FX, key: { alg: "ed25519", seed: k.seed }, pub: k.pub });

// ── C.4 ──────────────────────────────────────────────────────────────────

test("RSV-01 buwiz.provision is the first request answered; other tools before it fail", async () => {
  await withHost("rsv01", async ({ b, api }) => {
    const c = await rawFixture(b);

    try {
      await assert.rejects(c.callTool("fx_echo"), /buwiz.provision must come first/);
      const k = generate();
      const r = await c.callTool("buwiz.provision", provision(k));

      assert.equal(dataOf(r).ok, true);
      assert.equal(dataOf(r).manifest_sha256, createHash("sha256").update(canonicalSigned(JSON.parse(readFileSync(b.manifestFile, "utf8")))).digest("hex"));
      // idempotent for the same key
      assert.equal(dataOf(await c.callTool("buwiz.provision", provision(k))).ok, true);
      assert.equal(dataOf(await c.callTool("fx_echo", { a: 1 })).args.a, 1);
    } finally {
      c.close();
    }

    // a plugin that does not answer buwiz.provision is plugin_invalid
    assert.equal(await codeOf(installFixture(api, b, ["--misbehave=skip-provision"])), "plugin_invalid");
    assert.deepEqual(await api.list(), [], "nothing is installed");
  });
});

test("RSV-02 buwiz.session.open with an unknown app or a wrong perms_sha256 fails invalid_args", async () => {
  await withHost("rsv02", async ({ b, api }) => {
    const c = await rawFixture(b);

    try {
      await c.callTool("buwiz.provision", provision(generate()));
      const base = { open_id: "AAAAAAAAAAAAAAAAAAAAAA", ks_pub: generate().pub, not_after: Date.now() + 60_000 };
      const unknown = await c.callTool("buwiz.session.open", { ...base, app: "nope", perms_sha256: permsDigest({}) });

      assert.equal(unknown.isError, true);
      assert.equal(dataOf(unknown).code, "invalid_args");
      const wrong = await c.callTool("buwiz.session.open", { ...base, app: "card", perms_sha256: permsDigest({ tools: [] }) });

      assert.equal(dataOf(wrong).code, "invalid_args");
    } finally {
      c.close();
    }

    // the broker refuses all of it before relaying
    await installFixture(api, b);
    const p = await api.plugin();

    assert.equal(await codeOf(openApp(api, p, "card", { perms: permsDigest({ tools: ["fx_echo", "fx_secret"] }) })), "invalid_args");
    assert.equal(await codeOf(api.open({ plugin: FX, app: "nope", open_id: "AAAAAAAAAAAAAAAAAAAAAA", ks_pub: generate().pub, perms_sha256: permsDigest({}), not_after: Date.now() + 1000 })), "not_found");
    assert.equal(await codeOf(openApp(api, p, "card", { notAfter: Date.now() + 9 * 3600_000 })), "invalid_args", "a certificate over 8 h");
    assert.equal(statsOf(b.statsFile).sessions, 0, "none of them reached the plugin");
  });
});

const logsOf = (dataDir) => {
  const dir = join(dataDir, "v1", "logs");

  return (existsSync(dir) ? readdirSync(dir) : []).map((f) => [f, readFileSync(join(dir, f), "utf8")]);
};

test("RSV-03 the certificate verifies with the plugin public key; the seed is never logged", async () => {
  await withHost("rsv03", async ({ b, api, dataDir }) => {
    const inst = await installFixture(api, b);
    const keyFile = join(b.keysDir, `${FX}.key`);
    const seed = readFileSync(keyFile, "utf8").trim();

    assert.equal(pubOf(seed), inst.plugin_pub);
    assert.equal(statSync(keyFile).mode & 0o777, 0o600, "the file keystore keeps keys 0600");
    const o = await openApp(api, await api.plugin(), "board");

    assert.ok(verify(inst.plugin_pub, signedBytes(o.res.cert), o.res.cert_sig));
    assert.deepEqual(Object.keys(o.res.cert).sort(), ["app", "ks_pub", "not_after", "open_id", "perms_sha256", "plugin", "typ", "v"]);
    assert.equal(o.res.cert.perms_sha256, permsDigest(o.res.permissions));
    await api.disable();

    for (const [f, text] of logsOf(dataDir)) assert.ok(!text.includes(seed), `${f} holds a seed`);
  });

  // a misbehaving plugin that prints its key: stopped, and the log holds no seed
  await withHost("rsv03-leak", async ({ b, api, dataDir }) => {
    const leakCode = await codeOf(installFixture(api, b, ["--misbehave=log-seed"]));

    // refused at the gate, or stopped as soon as the line arrives
    if (leakCode !== "plugin_invalid") {
      await until(stateIs(api, "failed"), 5000, "the leak to fail the plugin");
      assert.equal((await api.plugin()).code, "plugin_invalid");
    }

    const keyFile = join(b.keysDir, `${FX}.key`);
    const leakSeed = existsSync(keyFile) ? readFileSync(keyFile, "utf8").trim() : "-none-";
    const logs = logsOf(dataDir);

    for (const [f, text] of logs) assert.ok(!text.includes(leakSeed), `${f} holds the leaked seed`);
    assert.match(logs.map(([, text]) => text).join(""), /\[redacted key\]/);
  });

  // a certificate signed by another key: plugin_bad_signature, plugin failed
  await withHost("rsv03-badcert", async ({ b, api }) => {
    await installFixture(api, b, ["--misbehave=bad-cert"]);
    assert.equal(await codeOf(openApp(api, await api.plugin(), "board")), "plugin_bad_signature");
    const p = await api.plugin();

    assert.deepEqual([p.state, p.code], ["failed", "plugin_bad_signature"]);
  });
});

test("RSV-04 buwiz.ticket.resolve returns a title and no subject field", async () => {
  await withHost("rsv04", async ({ b, api }) => {
    await installFixture(api, b);
    const tok = await api.resolve({ plugin: FX, kind: "scope", name: "item", value: "acme-123", complete: false });

    assert.equal(tok.result, "token");
    assert.deepEqual(await api.ticketTitle({ plugin: FX, ticket: tok.ticket }), { ok: true, valid: true, title: "Item acme-123" });
    assert.deepEqual(await api.ticketTitle({ plugin: FX, ticket: "tkt_0000000000000000000000000Z".slice(0, 30) }), { ok: true, valid: false });
    assert.equal(await codeOf(api.ticketTitle({ plugin: FX, ticket: "acme-123" })), "invalid_args");
  });
});

test("RSV-05 buwiz.session.close and buwiz.uninstall answer ok; an unknown buwiz.* answers -32601", async () => {
  await withHost("rsv05", async ({ b }) => {
    const c = await rawFixture(b);

    try {
      await c.callTool("buwiz.provision", provision(generate()));
      assert.deepEqual(dataOf(await c.callTool("buwiz.session.close", { open_id: "x" })), { ok: true });
      assert.deepEqual(dataOf(await c.callTool("buwiz.uninstall", {})), { ok: true });
      await assert.rejects(c.callTool("buwiz.revoke", {}), (e) => e.code === -32601);
    } finally {
      c.close();
    }
  });
});

// ── install, 5.2 step 6 ──────────────────────────────────────────────────

test("MAN-09 a short that collides with an enabled plugin is refused", async () => {
  await withHost("man09", async ({ b, api }) => {
    await installFixture(api, b);
    const other = join(b.dir, "other.json");

    writeFileSync(other, JSON.stringify(fixtureManifest(b.pub, (m) => (m.id = "app.buwiz.other"))));
    const spec = { command: process.execPath, args: [FIXTURE], env: { FX_MANIFEST: other } };

    await assert.rejects(api.inspect(spec), { code: "manifest_invalid", message: /short fx is already used/ });
    // stdio only (14.3)
    await assert.rejects(api.inspect({ url: "http://127.0.0.1:1/mcp", command: "x" }), { code: "not_supported" });
    await assert.rejects(api.inspect({ transport: "sse", command: "x" }), { code: "not_supported" });
  });
});

test("install: the review lists every permission (13.1 step 3) and a changed manifest is refused", async () => {
  await withHost("review", async ({ b, api }) => {
    const spec = fixtureSpec(b);
    const i = await api.inspect(spec);

    assert.equal((await api.list()).length, 0, "inspect installs nothing");
    assert.deepEqual(i.review.apps.find((a) => a.id === "board").lines, [
      "Can call 6 Fixture tools: fx_echo, fx_slow, fx_fail, fx_big, fx_search, fx_open",
      "Can open: Fixture Card",
      "Can open in your browser (asks each time): https://example.com",
      "Can attach: Item",
      "Can read files: .xlsx, .csv up to 25 MB",
      "Storage: 1 MB",
      "Copy to clipboard",
      "Show print previews (static pages, no scripts)",
    ]);
    assert.deepEqual(i.review.apps.find((a) => a.id === "card").inline, { width: 480, height: 320 });
    assert.deepEqual(i.review.routines, ["Routine 'Fixture digest' reads via app.buwiz.fixture: fx_search"]);
    assert.ok(i.review.lines.includes("Fixture 1.0.0 by Goldcoders"));
    // what is installed must be what was reviewed
    b.writeManifest((m) => (m.description = "changed after the review"));
    await assert.rejects(api.install({ ...spec, manifest_sha256: i.manifest_sha256 }), { code: "manifest_invalid", message: /reviewed/ });
    assert.equal((await api.list()).length, 0);
    assert.equal(existsSync(join(b.keysDir, `${FX}.key`)), false, "no key is kept for a refused install");
  });
});

// ── C.7 ──────────────────────────────────────────────────────────────────

async function update(api, b, edit, extra = {}) {
  b.writeManifest(edit);
  const spec = fixtureSpec(b);
  const i = await api.inspect(spec);

  return { i, r: await api.install({ ...spec, manifest_sha256: i.manifest_sha256, ...extra }).catch((e) => e) };
}

test("LCY-02 an update that adds a permission needs consent; one that removes a permission does not", async () => {
  await withHost("lcy02", async ({ b, api }) => {
    await installFixture(api, b);
    const gen = (await api.plugin()).generation;
    const add = await update(api, b, (m) => ((m.version = "1.1.0"), m.apps[1].permissions.tools.push("fx_search")));

    assert.deepEqual(add.i.update.added_permissions, ["Fixture Card: can call fx_search"]);
    assert.equal(add.r.code, "consent_declined");
    assert.equal((await api.plugin()).version, "1.0.0", "a declined update keeps the old version");
    const yes = await api.install({ ...fixtureSpec(b), manifest_sha256: add.i.manifest_sha256, approve_permissions: true });

    assert.equal(yes.updated, true);
    assert.equal((await api.plugin()).version, "1.1.0");
    assert.ok((await api.plugin()).generation > gen);
    const remove = await update(api, b, (m) => ((m.version = "1.2.0"), (m.apps[0].permissions.copy = false)));

    assert.deepEqual(remove.i.update.added_permissions, []);
    assert.equal(remove.r.ok, true);
    assert.equal((await api.plugin()).version, "1.2.0");
    // the same version with another manifest is manifest_changed
    const same = await update(api, b, (m) => ((m.version = "1.2.0"), (m.description = "same version, new text")));

    assert.equal(same.r.code, "manifest_changed");
  });
});

test("LCY-03 a downgrade is refused without the explicit choice", async () => {
  await withHost("lcy03", async ({ b, api }) => {
    b.writeManifest((m) => (m.version = "2.0.0"));
    await installFixture(api, b);
    const down = await update(api, b, (m) => (m.version = "1.9.0"));

    assert.equal(down.i.update.downgrade, true);
    assert.equal(down.r.code, "invalid_args");
    assert.equal(down.r.details.reason, "downgrade");
    assert.equal((await api.plugin()).version, "2.0.0");
    const yes = await api.install({ ...fixtureSpec(b), manifest_sha256: down.i.manifest_sha256, allow_downgrade: true });

    assert.equal(yes.ok, true);
    assert.equal((await api.plugin()).version, "1.9.0");
  });
});

test("LCY-05 disable stops the process and removes tools and commands; storage and keys are kept", async () => {
  await withHost("lcy05", async ({ b, api }) => {
    await installFixture(api, b);
    const { pid } = statsOf(b.statsFile);

    assert.ok(alive(pid), "the running plugin's process");
    const o = await openApp(api, await api.plugin(), "board");

    assert.equal(statsOf(b.statsFile).sessions, 1);
    assert.equal((await api.disable()).state, "stopped");
    await until(() => !alive(pid), 5000, "the process to exit");
    const p = await api.plugin();

    assert.deepEqual([p.state, p.enabled], ["stopped", false]);
    assert.equal(await codeOf(api.call(envelope(o, "fx_echo"))), "session_invalid", "its opens end");
    assert.equal(await codeOf(api.resolve({ plugin: FX, kind: "command", name: "board", value: "" })), "plugin_unavailable");
    assert.ok(existsSync(join(b.keysDir, `${FX}.key`)), "the key is kept");
    const on = await api.enable();

    assert.equal(on.state, "running");
    assert.equal(on.generation, 2);
    assert.equal(await codeOf(api.call(envelope(o, "fx_echo"))), "session_invalid", "the opens it had stay ended");
  });
});

test("LCY-06 uninstall removes the key, the record and the opens; a later open is not_found", async () => {
  await withHost("lcy06", async ({ b, api }) => {
    await installFixture(api, b);
    const p = await api.plugin();
    const o = await openApp(api, p, "card");
    const { pid } = statsOf(b.statsFile);

    assert.deepEqual(await api.uninstall(), { ok: true });
    await until(() => !alive(pid), 5000, "the process to exit");
    assert.equal(existsSync(join(b.keysDir, `${FX}.key`)), false);
    assert.deepEqual(await api.list(), []);
    assert.equal(await codeOf(api.call(envelope(o, "fx_echo"))), "session_invalid");
    assert.equal(await codeOf(openApp(api, p, "card")), "not_found");
    assert.equal(await codeOf(api.appSource({ plugin: FX, app: "card" })), "not_found");
    // installs again from scratch, with a new key
    const again = await installFixture(api, b);

    assert.equal(again.ok, true);
    assert.notEqual(again.plugin_pub, p.plugin_pub);
  });
});

test("LCY-08 a restart provisions and verifies again; a changed manifest fails manifest_changed", async () => {
  await withHost("lcy08", async ({ b, api }) => {
    await installFixture(api, b);
    b.writeManifest((m) => (m.description = "edited in place, same version"));
    process.kill(statsOf(b.statsFile).pid, "SIGKILL");
    await until(async () => ["restarting", "failed"].includes((await api.plugin()).state), 5000, "the restart to begin");
    await until(stateIs(api, "failed"), 8000, "failed");
    assert.equal((await api.plugin()).code, "manifest_changed");
    await new Promise((r) => setTimeout(r, 1500));
    assert.equal((await api.plugin()).state, "failed", "no restart loop");
    // a new version found on restart waits for the update flow
    b.writeManifest((m) => (m.version = "1.0.1"));
    assert.equal((await api.enable()).state, "update_available");
    // resources/list_changed: read and verified again while running
    b.writeManifest();
    assert.equal((await api.enable()).state, "running");
    const o = await openApp(api, await api.plugin(), "board");

    b.writeManifest((m) => (m.title = "Fixture, edited"));
    assert.equal((await api.call(envelope(o, "fx_echo", { reload: true }))).ok, true);
    await until(stateIs(api, "failed"), 5000, "failed after list_changed");
    assert.equal((await api.plugin()).code, "manifest_changed");
    // a manifest that no longer verifies is manifest_invalid
    writeFileSync(b.manifestFile, "[]");
    assert.equal((await api.enable()).code, "manifest_invalid");
  });
});

test("LCY-07 a plugin that crashes mid-call fails the call plugin_restarted and comes back", async () => {
  await withHost("crash", async ({ b, api }) => {
    await installFixture(api, b, ["--misbehave=crash-after:2"]);
    const o = await openApp(api, await api.plugin(), "board");

    assert.equal((await api.call(envelope(o, "fx_echo"))).ok, true);
    assert.equal((await api.call(envelope(o, "fx_echo"))).ok, true);
    assert.equal(await codeOf(api.call(envelope(o, "fx_echo"))), "plugin_restarted");
    await until(async () => (await api.plugin()).state === "running" && (await api.plugin()).generation === 2, 8000, "the restart");
  });
});

// ── events ───────────────────────────────────────────────────────────────

test("EVT-01 sessions opening, calling and closing announce no plugin event; a state change does", async () => {
  await withHost("events", async ({ b, api }) => {
    await installFixture(api, b);
    const p = await api.plugin();
    const seen = [];

    const stream = httpRequest({ host: "127.0.0.1", port: PORT, path: "/v1/host/events", headers: { "X-Buwiz-Host": api.token } }, (res) => {
      res.setEncoding("utf8");
      res.on("data", (d) => {
        for (const m of d.matchAll(/event: plugin\ndata: (.+)\n/g)) seen.push(JSON.parse(m[1]));
      });
    });

    stream.end();

    try {
      await new Promise((r) => setTimeout(r, 200));

      for (let i = 0; i < 3; i++) {
        const open = await openApp(api, p, "board");

        assert.equal(open.res.ok, true, JSON.stringify(open.res));
        assert.equal((await api.call(envelope(open, "fx_slow", { ms: 1 }))).ok, true);
        assert.deepEqual(await api.close({ open_id: open.open_id }), { ok: true });
      }

      await new Promise((r) => setTimeout(r, 300));
      assert.deepEqual(seen, [], "no event while the plugin runs on");
      await api.disable();
      await api.enable();
      await until(() => seen.some((e) => e.id === FX && e.state === "running" && e.generation === 2), 5000, "the events");
    } finally {
      stream.destroy();
    }
  });
});
