// The bridge and signing (spec Appendix C.3) at the broker's door, and the
// broker's half of the resolve results (C.5): a stand-in for the app opens
// apps and signs call envelopes as 14.1 says over /v1/host/*, and the
// broker's nine checks (14.2) decide. Also the app source path (11.2).
//
// A call is signed with the session key of its open: the test makes a key,
// opens the app with the key's public half (/v1/host/sessions/open), and
// sends {envelope, sig} to /v1/host/call, where sig is the ed25519 signature
// over the envelope's canonical JSON (v1-helpers.mjs's `envelope`).
//
// The pure rules of a call (who is `origin: app`, which `_meta` keys reach the
// Bot, the press record, who may switch an account, the prompt text) are unit
// tests in src/helper/sessions.rs; the ones that need a running plugin are here.
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { generate, random128 } from "../fixtures/v1/ed.mjs";
import { startServer, tempDir } from "./helpers.mjs";
import { FIXTURE_DIR, FX, bed, codeOf, envelope, hostApi, installFixture, openApp, statsOf, until } from "./v1-helpers.mjs";

const b = bed("sessions");
const dataDir = tempDir("v1-sessions-data");
let server;
let api;
let plugin;

before(async () => {
  // the broker's limit on a call is 120 s; a test that waits for it sets it to 0.8 s
  server = await startServer(dataDir, { env: { BUWIZ_KEYSTORE: `file:${b.keysDir}`, BUWIZ_TRUST_FILE: b.trustFile, BUWIZ_V1_CALL_TIMEOUT_MS: "800" } });
  api = hostApi(dataDir);
  await installFixture(api, b);
  plugin = await api.plugin();
});

after(async () => {
  await server?.stop();
});

const call = (open, tool, args, over, signer) => api.call(envelope(open, tool, args, over, signer));
const fresh = (app = "board") => openApp(api, plugin, app);
const stats = () => statsOf(b.statsFile);

test("BRG-01 a tool outside permissions.tools is permission_denied and nothing is sent", async () => {
  const board = await fresh();
  const was = stats().calls.fx_secret || 0;

  assert.equal(await codeOf(call(board, "fx_secret")), "permission_denied");
  assert.equal(await codeOf(call(board, "buwiz.provision")), "permission_denied");
  const card = await fresh("card");

  assert.equal(await codeOf(call(card, "fx_slow", { ms: 1 })), "permission_denied", "board's tools are not card's");
  assert.equal(stats().calls.fx_secret || 0, was);
  assert.equal(stats().calls.fx_slow || 0, 0);
});

test("BRG-02 a tool the user disabled is refused on the app path (disabled)", async () => {
  const board = await fresh();

  await api.setTool(FX, "fx_echo", true);
  assert.equal(await codeOf(call(board, "fx_echo")), "disabled");
  await api.setTool(FX, "fx_echo", false);
  assert.equal((await call(board, "fx_echo")).ok, true);
});

test("BRG-03 a validly signed call with a fresh nonce succeeds; the plugin sees buwiz/caller", async () => {
  const board = await fresh();
  const r = await call(board, "fx_echo", { code: "1601C", n: 2 });

  assert.equal(r.ok, true);
  assert.deepEqual(r.result.structuredContent.args, { code: "1601C", n: 2 });
  const caller = r.result.structuredContent.caller;

  assert.deepEqual({ ...caller, ts: 0 }, { origin: "app", plugin: FX, app: "board", open_id: board.open_id, ts: 0 });
  // a result's _meta never leaves the broker, but for the keys the Bot itself reads
  const o = await call(board, "fx_open");

  assert.equal(o.result._meta, undefined);
});

test("a plugin hears what the Bot does with files at initialize", async () => {
  assert.deepEqual(stats().client, { experimental: { "buwiz/documents": ["word", "sheet", "image", "pdf", "open_with", "save"] } });
});

test("BRG-04 the same envelope twice: the second is replayed", async () => {
  const board = await fresh();
  const e = envelope(board, "fx_echo", { once: true });

  assert.equal((await api.call(e)).ok, true);
  assert.equal(await codeOf(api.call(e)), "replayed");
});

test("BRG-05 an envelope 61 s old (or ahead) is stale; 59 s is fine", async () => {
  const board = await fresh();

  assert.equal(await codeOf(call(board, "fx_echo", {}, { ts: Date.now() - 61_000 })), "stale");
  assert.equal(await codeOf(call(board, "fx_echo", {}, { ts: Date.now() + 61_000 })), "stale");
  assert.equal((await call(board, "fx_echo", {}, { ts: Date.now() - 59_000 })).ok, true);
});

test("BRG-06 an envelope signed by another key, or changed after signing, is bad_signature", async () => {
  const board = await fresh();

  assert.equal(await codeOf(call(board, "fx_echo", {}, {}, generate().seed)), "bad_signature");
  const e = envelope(board, "fx_echo", { amount: 1 });

  e.envelope.args.amount = 1000;
  assert.equal(await codeOf(api.call(e)), "bad_signature");
  assert.equal(await codeOf(api.call({ envelope: e.envelope, sig: "x" })), "bad_signature");
});

test("BRG-07 a call without a registered session is session_invalid", async () => {
  const board = await fresh();
  const ghost = { ...board, open_id: random128() };

  assert.equal(await codeOf(call(ghost, "fx_echo")), "session_invalid");
  assert.equal(await codeOf(call(board, "fx_echo", {}, { app: "card" })), "session_invalid", "another app than the certificate's");
  assert.equal(await codeOf(call(board, "fx_echo", {}, { typ: "buwiz-session" })), "session_invalid");
  const short = await openApp(api, plugin, "board", { notAfter: Date.now() + 300 });

  await new Promise((r) => setTimeout(r, 350));
  assert.equal(await codeOf(call(short, "fx_echo")), "session_invalid", "an expired certificate");
  const closed = await fresh("card");

  assert.deepEqual(await api.close({ open_id: closed.open_id }), { ok: true });
  assert.equal(await codeOf(call(closed, "fx_echo")), "session_invalid");
});

test("BRG-09 isError rejects tool_error with the first text", async () => {
  const board = await fresh();
  const e = await api.call(envelope(board, "fx_fail")).catch((x) => x);

  assert.equal(e.code, "tool_error");
  assert.equal(e.message, "fx_fail: it failed on purpose");
  assert.equal(e.retryable, false);
});

test("BRG-10 a 2 MiB result rejects result_too_large; just under 1 MiB passes", async () => {
  const board = await fresh();

  assert.equal(await codeOf(call(board, "fx_big", { bytes: 2 * 1024 * 1024 })), "result_too_large");
  assert.equal((await call(board, "fx_big", { bytes: 1024 * 1024 - 64 })).ok, true);
  assert.equal(await codeOf(call(board, "fx_echo", { x: "y".repeat(256 * 1024) })), "invalid_args", "args over 256 KiB");
});

test("buwiz.preview: only a preview call of an app with permissions.preview gets its larger limit (max_bytes)", async () => {
  const board = await fresh();
  const doc = (bytes) => ({ bytes, as: "resource" });
  const pv = { purpose: "preview" };

  // an embedded resource counts toward the ordinary 1 MiB
  assert.equal(await codeOf(call(board, "fx_big", doc(2 * 1024 * 1024))), "result_too_large");
  const r = await call(board, "fx_big", doc(2 * 1024 * 1024), pv);

  assert.equal(r.ok, true);
  assert.equal(r.result.content[0].resource.text.length, 2 * 1024 * 1024);
  assert.equal(await codeOf(call(board, "fx_big", doc(3 * 1024 * 1024 + 1), pv)), "result_too_large", "over the fixture's max_bytes");
  assert.equal(await codeOf(call(board, "fx_big", doc(1), { purpose: "print" })), "invalid_args");
  const card = await fresh("card");

  assert.equal(await codeOf(call(card, "fx_echo", {}, pv)), "permission_denied", "card has no preview permission");
  assert.equal(await codeOf(call(board, "fx_secret", {}, pv)), "permission_denied", "the tool must still be the app's own");
});

test("BRG-11 a call past the broker limit rejects timeout and sends cancellation", async () => {
  const board = await fresh();
  const was = stats().cancelled;
  const e = await api.call(envelope(board, "fx_slow", { ms: 3000 })).catch((x) => x);

  assert.equal(e.code, "timeout");
  assert.equal(e.retryable, true);
  await until(() => stats().cancelled > was, 3000, "the plugin to see the cancellation");
  assert.equal((await api.plugin()).state, "running", "a plugin that answers after the cancel is not restarted");
});

test("BRG-12 the 9th concurrent call waits; the 21st call in one second is rate_limited", async () => {
  const o = await fresh();
  const was = stats().max_inflight;
  const nine = await Promise.all(Array.from({ length: 9 }, () => call(o, "fx_slow", { ms: 300 })));

  assert.ok(nine.every((r) => r.ok));
  assert.ok(stats().max_inflight >= 8, "eight run at once");
  assert.equal(stats().max_inflight, Math.max(8, was), "and never a ninth: it waits in order");
  const o2 = await fresh();
  // all at once, so they land inside one second however slow the machine;
  // which one is refused depends on arrival order, which HTTP does not keep
  const codes = await Promise.all(Array.from({ length: 21 }, () => codeOf(call(o2, "fx_echo"))));

  assert.equal(codes.filter((c) => c === "rate_limited").length, 1, codes.join(" "));
  assert.equal(codes.filter((c) => c === "resolved").length, 20, codes.join(" "));
});

test("BRG-08 a call on a stale plugin generation is plugin_restarted", async () => {
  const o = await fresh();
  const gen = (await api.plugin()).generation;
  const inflight = codeOf(call(o, "fx_slow", { ms: 600 }));

  await new Promise((r) => setTimeout(r, 100));
  process.kill(stats().pid, "SIGKILL");
  assert.equal(await inflight, "plugin_restarted", "an in-flight call");
  await until(async () => (await api.plugin()).state === "restarting", 3000, "restarting");
  assert.equal(await codeOf(call(o, "fx_echo")), "plugin_unavailable", "while it restarts");
  await until(async () => (await api.plugin()).state === "running", 5000, "the restart (1 s backoff)");
  assert.equal((await api.plugin()).generation, gen + 1);
  assert.equal(await codeOf(call(o, "fx_echo")), "plugin_restarted");
  const again = await fresh();

  assert.equal((await call(again, "fx_echo")).ok, true, "a new open works");
});

// ── C.5, broker side ─────────────────────────────────────────────────────

test("INV-04 resolve choices are capped at 20, labels at 80, details at 160", async () => {
  const r = await api.resolve({ plugin: FX, kind: "scope", name: "item", value: "many", complete: true });

  assert.equal(r.result, "choices");
  assert.equal(r.choices.length, 20);
  assert.ok(r.choices.every((c) => [...c.label].length <= 80 && [...c.detail].length <= 160 && c.value.startsWith("tkt_")));
  const err = await api.resolve({ plugin: FX, kind: "scope", name: "item", value: "error" });

  assert.equal(err.result, "error");
  assert.ok([...err.message].length <= 200);
});

test("INV-05 a resolve open naming another plugin's app is refused", async () => {
  assert.equal(await codeOf(api.resolve({ plugin: FX, kind: "command", name: "board", value: "foreign" })), "not_declared");
  assert.equal(await codeOf(api.resolve({ plugin: FX, kind: "command", name: "board", value: "bad-props" })), "invalid_args", "props that fail the schema");
  const open = await api.resolve({ plugin: FX, kind: "command", name: "board", value: "card", context: { attached: [] } });

  assert.equal(open.result, "open");
  assert.equal(open.app, "card");
  assert.equal(open.plugin, FX);
  assert.equal(open.placement, "inline");
  assert.match(open.ticket, /^tkt_[0-9A-HJKMNP-TV-Z]{26}$/);
  assert.equal(await codeOf(api.resolve({ plugin: FX, kind: "command", name: "taxform", value: "" })), "not_found");
});

test("INV-10 a resolve prompt answers a command with text for the composer; a scope's, a blank one or one over 4000 characters is refused", async () => {
  const p = await api.resolve({ plugin: FX, kind: "command", name: "board", value: "prompt" });

  assert.deepEqual(p, { ok: true, result: "prompt", text: "Draft this month's board update:\n- what shipped\n- what is next" });

  for (const value of ["prompt-long", "prompt-blank"]) assert.equal(await codeOf(api.resolve({ plugin: FX, kind: "command", name: "board", value })), "plugin_invalid", value);
  const scope = await api.resolve({ plugin: FX, kind: "scope", name: "item", value: "prompt" }).catch((e) => e);

  assert.equal(scope.code, "plugin_invalid");
  assert.equal(scope.message, "a prompt result answers a command, not a scope");
});

test("app source: the fingerprint is checked; a one-byte change is fingerprint_mismatch", async () => {
  const card = readFileSync(join(FIXTURE_DIR, "card.tsx"), "utf8");
  const src = await api.appSource({ plugin: FX, app: "card" });

  assert.equal(src.fingerprint, plugin.apps.find((a) => a.id === "card").fingerprint);
  assert.equal(src.source, card);
  // the same plugin again, its card one byte longer than the manifest signed
  const changed = join(b.dir, "card.tsx");

  writeFileSync(changed, `${card} `);
  await api.uninstall();
  await installFixture(api, b, [], { FX_APP_CARD: changed });
  const e = await api.appSource({ plugin: FX, app: "card" }).catch((x) => x);

  assert.equal(e.code, "fingerprint_mismatch");
  assert.match(e.message, /does not match its signed manifest/);
});
