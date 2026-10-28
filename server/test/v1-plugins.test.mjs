// Spec 17.4 (reserved tools) and 17.7 (lifecycle), broker side, plus
// MAN-09: the fixture plugin under an in-process broker (in-memory db,
// file keystore, its own trust file). The restart policy runs on a fake
// clock so backoff and the 10-minute window take milliseconds.
import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, test } from "node:test";
import { generate, pubOf, verify } from "../server/v1/ed.mjs";
import { signedBytes } from "../server/v1/jcs.mjs";
import { manifestHash, permsDigest } from "../server/v1/manifest.mjs";
import { BACKOFF_S } from "../server/v1/plugins.mjs";
import { v1Tools } from "../server/v1/agent.mjs";
import { FIXTURE, FX, bed, broker, codeOf, fixtureManifest, fixtureSpec, installFixture, openApp, until } from "./v1-helpers.mjs";

const stops = [];

after(async () => {
  for (const s of stops) await s();
});

function setup(label, opts) {
  const b = bed(label);
  const k = broker(b, opts);

  stops.push(() => k.plugins.stopAll());

  return { b, ...k };
}

const alive = (pid) => {
  try {
    process.kill(pid, 0);

    return true;
  } catch {
    return false;
  }
};

// a raw MCP client on the fixture: what a plugin answers, without the broker
async function rawFixture(b, args = []) {
  const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
  const { StdioClientTransport } = await import("@modelcontextprotocol/sdk/client/stdio.js");
  const spec = fixtureSpec(b, args);
  const client = new Client({ name: "test", version: "1" });

  await client.connect(new StdioClientTransport({ ...spec, env: { PATH: process.env.PATH, ...spec.env }, stderr: "pipe" }));
  stops.push(() => client.close());

  return client;
}

const data = (r) => r.structuredContent ?? JSON.parse(r.content[0].text);

// ── 17.4 ─────────────────────────────────────────────────────────────────

test("RSV-01 rox.provision is the first request answered; other tools before it fail", async () => {
  const { b, plugins } = setup("rsv01");
  const c = await rawFixture(b);

  await assert.rejects(c.callTool({ name: "fx_echo", arguments: {} }), /rox.provision must come first/);
  const k = generate();
  const r = await c.callTool({ name: "rox.provision", arguments: { protocol: "1.0.0", plugin: FX, key: { alg: "ed25519", seed: k.seed }, pub: k.pub } });

  assert.equal(data(r).ok, true);
  assert.equal(data(r).manifest_sha256, manifestHash(JSON.parse(readFileSync(b.manifestFile, "utf8"))));
  // idempotent for the same key
  assert.equal(data(await c.callTool({ name: "rox.provision", arguments: { protocol: "1.0.0", plugin: FX, key: { alg: "ed25519", seed: k.seed }, pub: k.pub } })).ok, true);
  assert.equal(data(await c.callTool({ name: "fx_echo", arguments: { a: 1 } })).args.a, 1);
  // a plugin that does not answer rox.provision is plugin_invalid
  assert.equal(await codeOf(installFixture(plugins, b, ["--misbehave=skip-provision"])), "plugin_invalid");
});

test("RSV-02 rox.session.open with an unknown app or a wrong perms_sha256 fails invalid_args", async () => {
  const { b, plugins, sessions } = setup("rsv02");
  const c = await rawFixture(b);
  const k = generate();

  await c.callTool({ name: "rox.provision", arguments: { protocol: "1.0.0", plugin: FX, key: { alg: "ed25519", seed: k.seed }, pub: k.pub } });
  const base = { open_id: "AAAAAAAAAAAAAAAAAAAAAA", ks_pub: generate().pub, not_after: Date.now() + 60_000 };
  const unknown = await c.callTool({ name: "rox.session.open", arguments: { ...base, app: "nope", perms_sha256: permsDigest({}) } });

  assert.equal(unknown.isError, true);
  assert.equal(data(unknown).code, "invalid_args");
  const wrong = await c.callTool({ name: "rox.session.open", arguments: { ...base, app: "card", perms_sha256: permsDigest({ tools: [] }) } });

  assert.equal(data(wrong).code, "invalid_args");
  // the broker refuses both before relaying
  await installFixture(plugins, b);
  const m = plugins.manifestOf(plugins.rowOf(FX));

  assert.equal(await codeOf(openApp(sessions, m, "card", { perms: permsDigest({ tools: ["fx_echo", "fx_secret"] }) })), "invalid_args");
  assert.equal(await codeOf(sessions.open({ plugin: FX, app: "nope", open_id: "AAAAAAAAAAAAAAAAAAAAAA", ks_pub: generate().pub, perms_sha256: permsDigest({}), not_after: Date.now() + 1000 })), "not_found");
  assert.equal(await codeOf(openApp(sessions, m, "card", { notAfter: Date.now() + 9 * 3600_000 })), "invalid_args", "a certificate over 8 h");
});

test("RSV-03 the certificate verifies with the plugin public key; the seed is never logged", async () => {
  const { b, plugins, sessions } = setup("rsv03");
  const inst = await installFixture(plugins, b);
  const seed = readFileSync(join(b.keysDir, `${FX}.key`), "utf8").trim();

  assert.equal(pubOf(seed), inst.plugin_pub);
  assert.equal(statSync(join(b.keysDir, `${FX}.key`)).mode & 0o777, 0o600, "the file keystore keeps keys 0600");
  const o = await openApp(sessions, plugins.manifestOf(plugins.rowOf(FX)), "board");

  assert.ok(verify(inst.plugin_pub, signedBytes(o.res.cert), o.res.cert_sig));
  assert.deepEqual(Object.keys(o.res.cert).sort(), ["app", "ks_pub", "not_after", "open_id", "perms_sha256", "plugin", "typ", "v"]);
  assert.equal(o.res.cert.perms_sha256, permsDigest(o.res.permissions));
  // a misbehaving plugin that prints its key: stopped, and the log holds no seed
  const leaky = setup("rsv03-leak");

  const leakCode = await codeOf(installFixture(leaky.plugins, leaky.b, ["--misbehave=log-seed"]));

  // refused at the gate, or stopped as soon as the line arrives
  if (leakCode !== "plugin_invalid") {
    await until(() => leaky.plugins.runtime(FX).state === "failed", 5000, "the leak to fail the plugin");
    assert.equal(leaky.plugins.runtime(FX).code, "plugin_invalid");
  }

  const leakSeed = existsSync(join(leaky.b.keysDir, `${FX}.key`)) ? readFileSync(join(leaky.b.keysDir, `${FX}.key`), "utf8").trim() : "-none-";

  for (const dir of [join(b.dir, "logs"), join(leaky.b.dir, "logs")])
    for (const f of existsSync(dir) ? readdirSync(dir) : []) {
      const log = readFileSync(join(dir, f), "utf8");

      assert.ok(!log.includes(seed) && !log.includes(leakSeed), `${f} holds a seed`);
    }

  const logs = readdirSync(join(leaky.b.dir, "logs")).map((f) => readFileSync(join(leaky.b.dir, "logs", f), "utf8")).join("");

  assert.match(logs, /\[redacted key\]/);
  // a certificate signed by another key: plugin_bad_signature, plugin failed
  const bad = setup("rsv03-badcert");

  await installFixture(bad.plugins, bad.b, ["--misbehave=bad-cert"]);
  assert.equal(await codeOf(openApp(bad.sessions, bad.plugins.manifestOf(bad.plugins.rowOf(FX)), "board")), "plugin_bad_signature");
  assert.equal(bad.plugins.runtime(FX).state, "failed");
  assert.equal(bad.plugins.runtime(FX).code, "plugin_bad_signature");
});

test("RSV-04 rox.ticket.resolve returns a title and no subject field", async () => {
  const { b, plugins, sessions } = setup("rsv04");

  await installFixture(plugins, b);
  const tok = await sessions.resolve({ plugin: FX, kind: "scope", name: "item", value: "acme-123", complete: false });

  assert.equal(tok.result, "token");
  const t = await sessions.ticketTitle({ plugin: FX, ticket: tok.ticket });

  assert.deepEqual(t, { ok: true, valid: true, title: "Item acme-123" });
  assert.deepEqual(await sessions.ticketTitle({ plugin: FX, ticket: "tkt_0000000000000000000000000Z".slice(0, 30) }), { ok: true, valid: false });
  assert.equal(await codeOf(sessions.ticketTitle({ plugin: FX, ticket: "acme-123" })), "invalid_args");
});

test("RSV-05 rox.session.close and rox.uninstall answer ok; an unknown rox.* answers -32601", async () => {
  const { b } = setup("rsv05");
  const c = await rawFixture(b);
  const k = generate();

  await c.callTool({ name: "rox.provision", arguments: { protocol: "1.0.0", plugin: FX, key: { alg: "ed25519", seed: k.seed }, pub: k.pub } });
  assert.deepEqual(data(await c.callTool({ name: "rox.session.close", arguments: { open_id: "x" } })), { ok: true });
  assert.deepEqual(data(await c.callTool({ name: "rox.uninstall", arguments: {} })), { ok: true });
  await assert.rejects(c.callTool({ name: "rox.revoke", arguments: {} }), (e) => e.code === -32601);
});

// ── install, 5.2 step 6 ──────────────────────────────────────────────────

test("MAN-09 a short that collides with an enabled plugin is refused", async () => {
  const { b, plugins } = setup("man09");

  await installFixture(plugins, b);
  const other = join(b.dir, "other.json");

  writeFileSync(other, JSON.stringify(fixtureManifest(b.pub, (m) => (m.id = "dev.goldcoders.other"))));
  const spec = { command: process.execPath, args: [FIXTURE], env: { FX_MANIFEST: other } };

  await assert.rejects(plugins.inspect(spec), { code: "manifest_invalid", message: /short fx is already used/ });
  // stdio only (14.3)
  await assert.rejects(plugins.inspect({ url: "http://127.0.0.1:1/mcp", command: "x" }), { code: "not_supported" });
  await assert.rejects(plugins.inspect({ transport: "sse", command: "x" }), { code: "not_supported" });
});

test("install: the review lists every permission (13.1 step 3) and a changed manifest is refused", async () => {
  const { b, plugins } = setup("review");
  const spec = fixtureSpec(b);
  const i = await plugins.inspect(spec);

  assert.equal(plugins.rows().length, 0, "inspect installs nothing");
  assert.deepEqual(i.review.apps.find((a) => a.id === "board").lines, [
    "Can call 6 Fixture tools: fx_echo, fx_slow, fx_fail, fx_big, fx_search, fx_open",
    "Can open: Fixture Card",
    "Can open in your browser (asks each time): https://example.com",
    "Can attach: Item",
    "Can read files: .xlsx, .csv up to 25 MB",
    "Storage: 1 MB",
    "Copy to clipboard",
  ]);
  assert.deepEqual(i.review.apps.find((a) => a.id === "card").inline, { width: 480, height: 320 });
  assert.deepEqual(i.review.routines, ["Routine 'Fixture digest' reads via dev.goldcoders.fixture: fx_search"]);
  assert.ok(i.review.lines.includes("Fixture 1.0.0 by Goldcoders"));
  // what is installed must be what was reviewed
  b.writeManifest((m) => (m.description = "changed after the review"));
  await assert.rejects(plugins.install({ ...spec, manifest_sha256: i.manifest_sha256 }), { code: "manifest_invalid", message: /reviewed/ });
  assert.equal(plugins.rows().length, 0);
  assert.equal(existsSync(join(b.keysDir, `${FX}.key`)), false, "no key is kept for a refused install");
});

// ── 17.7 ─────────────────────────────────────────────────────────────────

async function update(plugins, b, edit, extra = {}) {
  b.writeManifest(edit);
  const spec = fixtureSpec(b);
  const i = await plugins.inspect(spec);

  return { i, r: await plugins.install({ ...spec, manifest_sha256: i.manifest_sha256, ...extra }).catch((e) => e) };
}

test("LCY-02 an update that adds a permission needs consent; one that removes a permission does not", async () => {
  const { b, plugins } = setup("lcy02");

  await installFixture(plugins, b);
  const gen = plugins.runtime(FX).generation;
  const add = await update(plugins, b, (m) => ((m.version = "1.1.0"), m.apps[1].permissions.tools.push("fx_search")));

  assert.deepEqual(add.i.update.added_permissions, ["Fixture Card: can call fx_search"]);
  assert.equal(add.r.code, "consent_declined");
  assert.equal(plugins.rowOf(FX).version, "1.0.0", "a declined update keeps the old version");
  const yes = await plugins.install({ ...fixtureSpec(b), manifest_sha256: add.i.manifest_sha256, approve_permissions: true });

  assert.equal(yes.updated, true);
  assert.equal(plugins.rowOf(FX).version, "1.1.0");
  assert.ok(plugins.runtime(FX).generation > gen);
  const remove = await update(plugins, b, (m) => ((m.version = "1.2.0"), (m.apps[0].permissions.copy = false)));

  assert.deepEqual(remove.i.update.added_permissions, []);
  assert.equal(remove.r.ok, true);
  assert.equal(plugins.rowOf(FX).version, "1.2.0");
  // the same version with another manifest is manifest_changed
  const same = await update(plugins, b, (m) => ((m.version = "1.2.0"), (m.description = "same version, new text")));

  assert.equal(same.r.code, "manifest_changed");
});

test("LCY-03 a downgrade is refused without the explicit choice", async () => {
  const { b, plugins } = setup("lcy03");

  b.writeManifest((m) => (m.version = "2.0.0"));
  await installFixture(plugins, b);
  const down = await update(plugins, b, (m) => (m.version = "1.9.0"));

  assert.equal(down.i.update.downgrade, true);
  assert.equal(down.r.code, "invalid_args");
  assert.equal(down.r.details.reason, "downgrade");
  assert.equal(plugins.rowOf(FX).version, "2.0.0");
  const yes = await plugins.install({ ...fixtureSpec(b), manifest_sha256: down.i.manifest_sha256, allow_downgrade: true });

  assert.equal(yes.ok, true);
  assert.equal(plugins.rowOf(FX).version, "1.9.0");
});

test("LCY-05 disable stops the process and removes tools and commands; storage and keys are kept", async () => {
  const { b, plugins, sessions } = setup("lcy05");

  await installFixture(plugins, b);
  const pid = plugins.runtime(FX).conn.transport.pid;
  const o = await openApp(sessions, plugins.manifestOf(plugins.rowOf(FX)), "board");

  assert.ok(v1Tools(plugins).length > 0);
  await plugins.disable(FX);
  await until(() => !alive(pid), 5000, "the process to exit");
  assert.equal(plugins.runtime(FX).state, "stopped");
  assert.deepEqual(v1Tools(plugins), []);
  assert.equal(plugins.list()[0].enabled, false);
  assert.equal(sessions.opens.has(o.open_id), false, "its opens end");
  assert.equal(await codeOf(sessions.resolve({ plugin: FX, kind: "command", name: "board", value: "" })), "plugin_unavailable");
  assert.ok(existsSync(join(b.keysDir, `${FX}.key`)), "the key is kept");
  const on = await plugins.enable(FX);

  assert.equal(on.state, "running");
  assert.equal(on.generation, 2);
  assert.ok(v1Tools(plugins).length > 0);
});

test("LCY-06 uninstall removes the key, the record and the opens; a later open is not_found", async () => {
  const { b, plugins, sessions } = setup("lcy06");

  await installFixture(plugins, b);
  const m = plugins.manifestOf(plugins.rowOf(FX));
  const o = await openApp(sessions, m, "card");
  const pid = plugins.runtime(FX).conn.transport.pid;

  assert.deepEqual(await plugins.uninstall(FX), { ok: true });
  await until(() => !alive(pid), 5000, "the process to exit");
  assert.equal(existsSync(join(b.keysDir, `${FX}.key`)), false);
  assert.deepEqual(plugins.list(), []);
  assert.equal(sessions.opens.has(o.open_id), false);
  assert.equal(await codeOf(openApp(sessions, m, "card")), "not_found");
  assert.equal(await codeOf(sessions.appSource({ plugin: FX, app: "card" })), "not_found");
  // installs again from scratch, with a new key
  const again = await installFixture(plugins, b);

  assert.equal(again.ok, true);
});

function fakeClock() {
  const c = { t: 1_800_000_000_000, timers: [] };

  c.now = () => c.t;
  c.setTimeout = (f, ms) => (c.timers.push({ f, ms }), c.timers.length);
  c.clearTimeout = () => {};

  c.fire = () => c.timers.shift().f();

  return c;
}

test("LCY-07 a killed plugin restarts with backoff; the seventh restart in 10 minutes is refused (failed)", async () => {
  const clock = fakeClock();
  const { b, plugins } = setup("lcy07", { clock });
  const seen = [];

  plugins.events.on("plugin", (e) => seen.push(e.state));
  await installFixture(plugins, b);

  for (let n = 0; n < 6; n++) {
    const rt = plugins.runtime(FX);
    const gen = rt.generation;

    process.kill(rt.conn.transport.pid, "SIGKILL");
    await until(() => rt.state === "restarting", 5000, "restarting");
    assert.equal(clock.timers.at(-1).ms, BACKOFF_S[n] * 1000, `restart ${n + 1} waits ${BACKOFF_S[n]} s`);
    clock.t += BACKOFF_S[n] * 1000;
    clock.fire();
    await until(() => rt.state === "running", 5000, "running again");
    assert.equal(rt.generation, gen + 1, "each restart is a new generation");
  }

  process.kill(plugins.runtime(FX).conn.transport.pid, "SIGKILL");
  await until(() => plugins.runtime(FX).state === "failed", 5000, "failed");
  assert.equal(plugins.runtime(FX).code, "failed");
  assert.equal(clock.timers.length, 0, "no seventh restart is scheduled");
  assert.ok(seen.includes("restarting") && seen.at(-1) === "failed");
  // past the 10-minute window the count starts over (after a Retry)
  clock.t += 11 * 60_000;
  assert.equal((await plugins.enable(FX)).state, "running");
});

test("LCY-08 a restart provisions and verifies again; a changed manifest fails manifest_changed", async () => {
  const clock = fakeClock();
  const { b, plugins } = setup("lcy08", { clock });

  await installFixture(plugins, b);
  const rt = plugins.runtime(FX);

  b.writeManifest((m) => (m.description = "edited in place, same version"));
  process.kill(rt.conn.transport.pid, "SIGKILL");
  await until(() => rt.state === "restarting", 5000, "restarting");
  clock.fire();
  await until(() => rt.state === "failed", 5000, "failed");
  assert.equal(rt.code, "manifest_changed");
  assert.equal(clock.timers.length, 0, "no restart loop");
  // a new version found on restart waits for the update flow
  b.writeManifest((m) => (m.version = "1.0.1"));
  assert.equal((await plugins.enable(FX)).state, "update_available");
  // resources/list_changed: read and verified again while running
  b.writeManifest();
  assert.equal((await plugins.enable(FX)).state, "running");
  b.writeManifest((m) => (m.title = "Fixture, edited"));
  await plugins.callTool(FX, "fx_reload", {});
  await until(() => rt.state === "failed", 5000, "failed after list_changed");
  assert.equal(rt.code, "manifest_changed");
  // a manifest that no longer verifies is manifest_invalid
  writeFileSync(b.manifestFile, "[]");
  assert.equal((await plugins.enable(FX)).code, "manifest_invalid");
});

test("LCY-07 a plugin that crashes mid-call fails the call plugin_restarted and comes back", async () => {
  const { b, plugins } = setup("crash");

  await installFixture(plugins, b, ["--misbehave=crash-after:2"]);
  const rt = plugins.runtime(FX);

  await plugins.callTool(FX, "fx_echo", {});
  await plugins.callTool(FX, "fx_echo", {});
  assert.equal(await codeOf(plugins.callTool(FX, "fx_echo", {})), "plugin_restarted");
  await until(() => rt.state === "running" && rt.generation === 2, 5000, "the restart");
  assert.equal(rt.nextDelay, 1000);
});
