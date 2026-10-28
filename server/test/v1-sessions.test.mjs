// Spec 17.3 (bridge and signing) at the broker boundary, and the broker's
// half of 17.5 (resolve results): a fake Rox opens apps and signs
// envelopes as 8.1 says; the broker's nine checks (8.2) decide. Also the
// app source path (6.1, 6.2).
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { generate, random128 } from "../server/v1/ed.mjs";
import { FX, FIXTURE_DIR, bed, broker, codeOf, envelope, installFixture, openApp, until } from "./v1-helpers.mjs";
import { readFileSync } from "node:fs";

const b = bed("sessions");
const { plugins, sessions } = broker(b, { timeouts: { call: 800 } });
let m;
let board;

before(async () => {
  await installFixture(plugins, b);
  m = plugins.manifestOf(plugins.rowOf(FX));
  board = await openApp(sessions, m, "board");
});

after(() => plugins.stopAll());

const call = (open, tool, args, over, signer) => sessions.call(envelope(open, tool, args, over, signer));
const stats = async () => (await plugins.callTool(FX, "fx_stats", {})).structuredContent;

test("BRG-01 a tool outside permissions.tools is permission_denied and nothing is sent", async () => {
  const before = (await stats()).calls.fx_secret || 0;

  assert.equal(await codeOf(call(board, "fx_secret")), "permission_denied");
  assert.equal(await codeOf(call(board, "rox.provision")), "permission_denied");
  const card = await openApp(sessions, m, "card");

  assert.equal(await codeOf(call(card, "fx_slow", { ms: 1 })), "permission_denied", "board's tools are not card's");
  assert.equal((await stats()).calls.fx_secret || 0, before);
});

test("BRG-02 a tool the user disabled is refused on the app path (disabled)", async () => {
  plugins.setTool(FX, "fx_echo", true);
  assert.equal(await codeOf(call(board, "fx_echo")), "disabled");
  plugins.setTool(FX, "fx_echo", false);
  assert.equal((await call(board, "fx_echo")).ok, true);
});

test("BRG-03 a validly signed call with a fresh nonce succeeds; the plugin sees rox/caller", async () => {
  const r = await call(board, "fx_echo", { code: "1601C", n: 2 });

  assert.equal(r.ok, true);
  assert.deepEqual(r.result.structuredContent.args, { code: "1601C", n: 2 });
  const caller = r.result.structuredContent.caller;

  assert.deepEqual({ ...caller, ts: 0 }, { origin: "app", plugin: FX, app: "board", open_id: board.open_id, ts: 0 });
  // a result's _meta never leaves the broker
  const o = await call(board, "fx_open");

  assert.equal(o.result._meta, undefined);
});

test("BRG-04 the same envelope twice: the second is replayed", async () => {
  const e = envelope(board, "fx_echo", { once: true });

  assert.equal((await sessions.call(e)).ok, true);
  assert.equal(await codeOf(sessions.call(e)), "replayed");
});

test("BRG-05 an envelope 61 s old (or ahead) is stale; 59 s is fine", async () => {
  assert.equal(await codeOf(call(board, "fx_echo", {}, { ts: Date.now() - 61_000 })), "stale");
  assert.equal(await codeOf(call(board, "fx_echo", {}, { ts: Date.now() + 61_000 })), "stale");
  assert.equal((await call(board, "fx_echo", {}, { ts: Date.now() - 59_000 })).ok, true);
});

test("BRG-06 an envelope signed by another key, or changed after signing, is bad_signature", async () => {
  assert.equal(await codeOf(call(board, "fx_echo", {}, {}, generate().seed)), "bad_signature");
  const e = envelope(board, "fx_echo", { amount: 1 });

  e.envelope.args.amount = 1000;
  assert.equal(await codeOf(sessions.call(e)), "bad_signature");
  assert.equal(await codeOf(sessions.call({ envelope: e.envelope, sig: "x" })), "bad_signature");
});

test("BRG-07 a call without a registered session is session_invalid", async () => {
  const ghost = { ...board, open_id: random128() };

  assert.equal(await codeOf(call(ghost, "fx_echo")), "session_invalid");
  assert.equal(await codeOf(call(board, "fx_echo", {}, { app: "card" })), "session_invalid", "another app than the certificate's");
  assert.equal(await codeOf(call(board, "fx_echo", {}, { typ: "rox-session" })), "session_invalid");
  const short = await openApp(sessions, m, "board", { notAfter: Date.now() + 300 });

  await new Promise((r) => setTimeout(r, 350));
  assert.equal(await codeOf(call(short, "fx_echo")), "session_invalid", "an expired certificate");
  const closed = await openApp(sessions, m, "card");

  assert.deepEqual(await sessions.close({ open_id: closed.open_id }), { ok: true });
  assert.equal(await codeOf(call(closed, "fx_echo")), "session_invalid");
});

test("BRG-09 isError rejects tool_error with the first text", async () => {
  const e = await sessions.call(envelope(board, "fx_fail")).catch((x) => x);

  assert.equal(e.code, "tool_error");
  assert.equal(e.message, "fx_fail: it failed on purpose");
  assert.equal(e.retryable, false);
});

test("BRG-10 a 2 MiB result rejects result_too_large; just under 1 MiB passes", async () => {
  assert.equal(await codeOf(call(board, "fx_big", { bytes: 2 * 1024 * 1024 })), "result_too_large");
  assert.equal((await call(board, "fx_big", { bytes: 1024 * 1024 - 64 })).ok, true);
  assert.equal(await codeOf(call(board, "fx_echo", { x: "y".repeat(256 * 1024) })), "invalid_args", "args over 256 KiB");
});

test("BRG-11 a call past the broker limit rejects timeout and sends cancellation", async () => {
  const was = (await stats()).cancelled;
  const e = await sessions.call(envelope(board, "fx_slow", { ms: 3000 })).catch((x) => x);

  assert.equal(e.code, "timeout");
  assert.equal(e.retryable, true);
  await until(async () => (await stats()).cancelled > was, 3000, "the plugin to see the cancellation");
  assert.equal(plugins.runtime(FX).state, "running", "a plugin that answers after the cancel is not restarted");
});

test("BRG-12 the 9th concurrent call waits; the 21st call in one second is rate_limited", async () => {
  const o = await openApp(sessions, m, "board");
  const nine = Array.from({ length: 9 }, () => call(o, "fx_slow", { ms: 300 }));

  await new Promise((r) => setImmediate(r));
  const s = sessions.opens.get(o.open_id);

  assert.equal(s.active, 8);
  assert.equal(s.queue.length, 1, "the ninth waits in order");
  assert.ok((await Promise.all(nine)).every((r) => r.ok));
  const o2 = await openApp(sessions, m, "board");
  const codes = await Promise.all(Array.from({ length: 21 }, () => codeOf(call(o2, "fx_echo"))));

  assert.equal(codes.filter((c) => c === "rate_limited").length, 1);
  assert.equal(codes.at(-1), "rate_limited");
});

test("BRG-08 a call on a stale plugin generation is plugin_restarted", async () => {
  const o = await openApp(sessions, m, "board");
  const rt = plugins.runtime(FX);
  const gen = rt.generation;
  const inflight = codeOf(call(o, "fx_slow", { ms: 600 }));

  await new Promise((r) => setTimeout(r, 100));
  process.kill(rt.conn.transport.pid, "SIGKILL");
  assert.equal(await inflight, "plugin_restarted", "an in-flight call");
  await until(() => rt.state === "restarting", 3000, "restarting");
  assert.equal(await codeOf(call(o, "fx_echo")), "plugin_unavailable", "while it restarts");
  await until(() => rt.state === "running", 5000, "the restart (1 s backoff)");
  assert.equal(rt.generation, gen + 1);
  assert.equal(await codeOf(call(o, "fx_echo")), "plugin_restarted");
  const fresh = await openApp(sessions, m, "board");

  assert.equal((await call(fresh, "fx_echo")).ok, true, "a new open works");
});

// ── 17.5, broker side ────────────────────────────────────────────────────

test("INV-04 resolve choices are capped at 20, labels at 80, details at 160", async () => {
  const r = await sessions.resolve({ plugin: FX, kind: "scope", name: "item", value: "many", complete: true });

  assert.equal(r.result, "choices");
  assert.equal(r.choices.length, 20);
  assert.ok(r.choices.every((c) => [...c.label].length <= 80 && [...c.detail].length <= 160 && c.value.startsWith("tkt_")));
  const err = await sessions.resolve({ plugin: FX, kind: "scope", name: "item", value: "error" });

  assert.equal(err.result, "error");
  assert.ok([...err.message].length <= 200);
});

test("INV-05 a resolve open naming another plugin's app is refused", async () => {
  assert.equal(await codeOf(sessions.resolve({ plugin: FX, kind: "command", name: "board", value: "foreign" })), "not_declared");
  assert.equal(await codeOf(sessions.resolve({ plugin: FX, kind: "command", name: "board", value: "bad-props" })), "invalid_args", "props that fail the schema");
  const open = await sessions.resolve({ plugin: FX, kind: "command", name: "board", value: "card", context: { attached: [] } });

  assert.equal(open.result, "open");
  assert.equal(open.app, "card");
  assert.equal(open.plugin, FX);
  assert.equal(open.placement, "inline");
  assert.match(open.ticket, /^tkt_[0-9A-HJKMNP-TV-Z]{26}$/);
  assert.equal(await codeOf(sessions.resolve({ plugin: FX, kind: "command", name: "taxform", value: "" })), "not_found");
});

test("app source: the fingerprint is checked; a one-byte change is fingerprint_mismatch", async () => {
  const src = await sessions.appSource({ plugin: FX, app: "card" });

  assert.equal(src.fingerprint, m.apps.find((a) => a.id === "card").fingerprint);
  assert.equal(src.source, readFileSync(join(FIXTURE_DIR, "card.tsx"), "utf8"));
  const b2 = bed("source");
  const k2 = broker(b2);
  const changed = join(b2.dir, "card.tsx");

  writeFileSync(changed, `${readFileSync(join(FIXTURE_DIR, "card.tsx"), "utf8")} `);
  await installFixture(k2.plugins, b2, [], { FX_APP_CARD: changed });
  const e = await k2.sessions.appSource({ plugin: FX, app: "card" }).catch((x) => x);

  assert.equal(e.code, "fingerprint_mismatch");
  assert.match(e.message, /does not match its signed manifest/);
  await k2.plugins.stopAll();
});
