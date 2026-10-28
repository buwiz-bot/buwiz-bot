// The first-party Google Workspace plugin (server/v1/plugins/google-
// workspace): installed through the v1 manager with this run's publisher
// key, its read-only mail tools over a fake workspace-mcp upstream (test/
// fixtures/fake-gws-upstream.mjs, the real server's text format), and the
// reserved tools of spec 7.8. GWS-* are this suite's own IDs (not in spec
// 17); the reserved-tool tests reuse the RSV IDs they check.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { generate } from "../server/v1/ed.mjs";
import { GWS, bed, broker, codeOf, gwsSpec, installGws, upstreamCalls, writeMail } from "./v1-helpers.mjs";

const b = bed("gws");
const { plugins } = broker(b);
const meta = { "rox/caller": { origin: "agent", plugin: GWS, ts: Date.now() } };
const call = (name, args) => plugins.callTool(GWS, name, args, { meta });
const clients = [];

const MAIL = [
  { id: "18f1a", from: "BIR eFPS <efps@bir.gov.ph>", to: "owner@example.com", subject: "Tax Return Receipt Confirmation", date: "Mon, 26 Oct 2026 09:12:00 +0800", body: "Your 1701Q for TIN 000-000-000 was received.\n\nReference: 2026-ABC-123\nFrom: a forwarded line, not a header" },
  { id: "18f1b", from: "Ana <ana@example.com>", subject: "Lunch", date: "Tue, 27 Oct 2026 12:00:00 +0800", body: "  See you   at noon.\n" },
];

before(async () => {
  writeMail(b, MAIL);
  await installGws(plugins, b);
});

after(async () => {
  await plugins.stopAll();

  for (const c of clients) await c.close().catch(() => {});
});

// A raw MCP client on the plugin, without the broker (the P7 and
// misconfiguration tests); `provisioned` sends rox.provision first.
async function raw(env = {}, provisioned = true) {
  const spec = gwsSpec(b, env);
  const client = new Client({ name: "gws-test", version: "1.0.0" });

  await client.connect(new StdioClientTransport({ ...spec, stderr: "ignore" }));
  clients.push(client);

  if (provisioned) {
    const k = generate();

    await client.callTool({ name: "rox.provision", arguments: { protocol: "1.0.0", plugin: GWS, key: { alg: "ed25519", seed: k.seed }, pub: k.pub } });
  }

  return client;
}

const firstText = (r) => r.content.find((c) => c.type === "text")?.text || "";

test("GWS-01 installs through the v1 manager: no apps or commands, two read-only mail tools", () => {
  const p = plugins.list().find((x) => x.id === GWS);

  assert.equal(p.state, "running");
  assert.equal(p.short, "gws");
  assert.deepEqual([p.apps, p.commands, p.scopes, p.routines], [[], [], [], []]);
  assert.deepEqual(p.tools.map((t) => [t.name, t.readOnly]), [["gws_search_mail", true], ["gws_read_mail", true]]);

  for (const t of plugins.runtime(GWS).tools.filter((x) => x.name.startsWith("gws_"))) assert.equal(t.annotations?.readOnlyHint, true, `${t.name} has readOnlyHint`);
});

test("GWS-02 gws_search_mail maps the upstream's text to {items: [{id, from, subject, date, snippet}]}", async () => {
  const r = await call("gws_search_mail", { query: "from:bir.gov.ph", limit: 5 });

  assert.equal(r.isError, undefined);
  assert.deepEqual(r.structuredContent.items, [
    { id: "18f1a", from: MAIL[0].from, subject: MAIL[0].subject, date: MAIL[0].date, snippet: "Your 1701Q for TIN 000-000-000 was received. Reference: 2026-ABC-123 From: a forwarded line, not a header" },
    { id: "18f1b", from: MAIL[1].from, subject: "Lunch", date: MAIL[1].date, snippet: "See you at noon." },
  ]);
  const calls = upstreamCalls(b);
  const search = calls.find((c) => c.tool === "search_gmail_messages");

  assert.deepEqual(search.args, { query: "from:bir.gov.ph", page_size: 5, user_google_email: "owner@example.com" });
  assert.deepEqual(calls.find((c) => c.tool === "get_gmail_messages_content_batch").args.message_ids, ["18f1a", "18f1b"]);
  const none = await call("gws_search_mail", { query: "nothing at all" });

  assert.deepEqual(none.structuredContent, { items: [] });
});

test("GWS-03 gws_read_mail returns {id, from, to, subject, date, body}; the body is capped at 64 KiB", async () => {
  const r = await call("gws_read_mail", { id: "18f1a" });

  assert.deepEqual(r.structuredContent, { id: "18f1a", from: MAIL[0].from, to: "owner@example.com", subject: MAIL[0].subject, date: MAIL[0].date, body: MAIL[0].body });
  assert.equal((await call("gws_read_mail", { id: "18f1b" })).structuredContent.to, "", "a missing To is empty");
  writeMail(b, [...MAIL, { id: "big", from: "x@example.com", subject: "Big", date: "today", body: "é".repeat(70_000) }]);

  try {
    const big = (await call("gws_read_mail", { id: "big" })).structuredContent.body;

    assert.ok(Buffer.byteLength(big) <= 64 * 1024);
    assert.ok(big.endsWith("…[truncated]"));
    assert.ok(!big.includes("�"), "no half character");
  } finally {
    writeMail(b, MAIL);
  }

  const gone = await call("gws_read_mail", { id: "nope" });

  assert.equal(gone.isError, true);
  assert.match(firstText(gone), /no message nope/);
});

test("GWS-04 a missing or failing upstream is an isError result with a plain message", async () => {
  const missing = await raw({ GWS_UPSTREAM: "" });
  const r1 = await missing.callTool({ name: "gws_search_mail", arguments: { query: "x" } });

  assert.equal(r1.isError, true);
  assert.match(firstText(r1), /GWS_UPSTREAM/);
  const upstream = JSON.parse(gwsSpec(b).env.GWS_UPSTREAM);

  upstream.env.FAKE_GWS_FAIL = "1";
  const failing = await raw({ GWS_UPSTREAM: JSON.stringify(upstream) });
  const r2 = await failing.callTool({ name: "gws_read_mail", arguments: { id: "18f1a" } });

  assert.equal(r2.isError, true);
  assert.match(firstText(r2), /refused get_gmail_messages_content_batch: ACTION REQUIRED/);
  const dead = await raw({ GWS_UPSTREAM: JSON.stringify({ command: "/nonexistent/workspace-mcp" }) });
  const r3 = await dead.callTool({ name: "gws_search_mail", arguments: { query: "x" } });

  assert.equal(r3.isError, true);
  assert.match(firstText(r3), /could not start/);
  const noEmail = await raw({ GWS_USER_EMAIL: "" });

  assert.match(firstText(await noEmail.callTool({ name: "gws_search_mail", arguments: { query: "x" } })), /GWS_USER_EMAIL/);
});

test("RSV-01 gws: rox.provision comes first; other tools before it fail", async () => {
  const c = await raw({}, false);

  await assert.rejects(c.callTool({ name: "gws_search_mail", arguments: { query: "x" } }), /rox\.provision must come first/);
});

test("RSV-02 gws: rox.session.open fails invalid_args (the plugin has no apps)", async () => {
  assert.equal(await codeOf(plugins.callReserved(GWS, "rox.session.open", { app: "inbox", open_id: "x", ks_pub: "x", perms_sha256: "0".repeat(64), not_after: Date.now() + 1000 })), "invalid_args");
});

test("RSV-04 gws: rox.ticket.resolve answers valid:false and no subject", async () => {
  assert.deepEqual(await plugins.callReserved(GWS, "rox.ticket.resolve", { ticket: "tkt_00000000000000000000000000" }), { valid: false });
});

test("RSV-05 gws: rox.session.close and rox.uninstall answer ok; unknown rox.* is -32601", async () => {
  assert.deepEqual(await plugins.callReserved(GWS, "rox.session.close", { open_id: "x" }), { ok: true });
  assert.deepEqual(await plugins.callReserved(GWS, "rox.uninstall", {}), { ok: true });
  assert.equal(await plugins.callReserved(GWS, "rox.frobnicate", {}), null);
});
