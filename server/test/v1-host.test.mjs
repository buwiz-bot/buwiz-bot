// The host-internal API (host design section 4) end to end: a server of
// this checkout on port 8799 with its own data dir, file keystore and trust
// file. Every /v1/host/* route needs the host token; the Phase 0 guard still
// runs first. Rox's whole path is driven over HTTP: inspect, install, the
// plugin list, events, a session, a signed call, resolve, ticket titles,
// app source, disable/enable and uninstall.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { generate, random128 } from "../server/v1/ed.mjs";
import { ROOT, tempDir } from "./helpers.mjs";
import { FX, bed, envelope, fixtureSpec, until } from "./v1-helpers.mjs";

const PORT = Number(process.env.V1_TEST_PORT || 8799);
const b = bed("host");
const dataDir = tempDir("v1-host-data");
let child;
let log = "";
let token = "";

function req(method, path, { headers = {}, body } = {}) {
  return new Promise((resolve, reject) => {
    const r = httpRequest({ host: "127.0.0.1", port: PORT, method, path, headers }, (res) => {
      let text = "";

      res.setEncoding("utf8");
      res.on("data", (c) => (text += c));
      res.on("end", () => {
        let json = null;

        try {
          json = JSON.parse(text);
        } catch {}

        resolve({ status: res.statusCode, json, text });
      });
    });

    r.on("error", reject);

    if (body !== undefined) r.write(body);
    r.end();
  });
}

function host(method, path, obj, extra = {}) {
  const headers = { "X-Rox-Host": token, ...extra };

  if (obj === undefined) return req(method, path, { headers });
  headers["Content-Type"] = "application/json";

  return req(method, path, { headers, body: JSON.stringify(obj) });
}

before(async () => {
  child = spawn(process.execPath, [join(ROOT, "server", "app-server.mjs")], {
    cwd: ROOT,
    env: { ...process.env, PORT: String(PORT), GROK_DATA_DIR: dataDir, MOCK: "1", GROK_ALLOWED_ORIGINS: "", ROX_KEYSTORE: `file:${b.keysDir}`, ROX_TRUST_FILE: b.trustFile },
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
  });
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  await until(() => req("GET", "/health").then((r) => r.status === 200, () => false), 20000, `the server on ${PORT}:\n${log}`);
  token = readFileSync(join(dataDir, "host-token"), "utf8");
});

after(async () => {
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {}

  await new Promise((r) => (child.exitCode !== null ? r() : child.once("exit", r)));
});

test("host token: 0600 file, required on every /v1/host route (401), guard still first", async () => {
  assert.equal(statSync(join(dataDir, "host-token")).mode & 0o777, 0o600);
  assert.match(token, /^[A-Za-z0-9_-]{43}$/);
  assert.equal((await req("GET", "/v1/host/plugins")).status, 401);
  assert.equal((await req("GET", "/v1/host/plugins", { headers: { "X-Rox-Host": "x".repeat(43) } })).status, 401);
  assert.equal((await req("POST", "/v1/host/call", { headers: { "Content-Type": "application/json" }, body: "{}" })).status, 401);
  assert.equal((await host("GET", "/v1/host/plugins", undefined, { Origin: "https://evil.example" })).status, 403, "web origins are refused before the token");
  assert.equal((await req("POST", "/v1/host/call", { headers: { "X-Rox-Host": token, "Content-Type": "text/plain" }, body: "{}" })).status, 415);
  const ok = await host("GET", "/v1/host/plugins");

  assert.equal(ok.status, 200);
  assert.deepEqual(ok.json, { ok: true, plugins: [] });
  assert.ok(!log.includes(token), "the token is not logged");
});

test("host token: a second server that cannot bind the port leaves the running server's token", async () => {
  const second = spawn(process.execPath, [join(ROOT, "server", "app-server.mjs")], {
    cwd: ROOT,
    env: { ...process.env, PORT: String(PORT), GROK_DATA_DIR: dataDir, MOCK: "1", ROX_KEYSTORE: `file:${b.keysDir}`, ROX_TRUST_FILE: b.trustFile },
    stdio: "ignore",
  });

  await new Promise((r) => second.once("exit", r));
  assert.equal(readFileSync(join(dataDir, "host-token"), "utf8"), token, "the token file is the running server's");
  assert.equal((await host("GET", "/v1/host/plugins")).status, 200, "Rox can still talk to the running server");
});

test("Rox's path over HTTP: inspect, install, list, open, call, resolve, title, source", async () => {
  const spec = fixtureSpec(b);
  const i = await host("POST", "/v1/host/plugins/inspect", spec);

  assert.equal(i.json.ok, true, i.text);
  assert.ok(i.json.review.lines.length > 5);
  const inst = await host("POST", "/v1/host/plugins/install", { ...spec, manifest_sha256: i.json.manifest_sha256 });

  assert.equal(inst.json.state, "running", inst.text);
  const list = (await host("GET", "/v1/host/plugins")).json.plugins;

  assert.equal(list.length, 1);
  assert.equal(list[0].short, "fx");
  assert.ok(list[0].tools.every((t) => !t.name.startsWith("rox.")));
  assert.ok(list[0].tools.find((t) => t.name === "fx_search").readOnly);
  const board = list[0].apps.find((a) => a.id === "board");
  const ks = generate();
  const open = { plugin: FX, app: "board", open_id: random128(), ks_pub: ks.pub, perms_sha256: board.perms_sha256, not_after: Date.now() + 600_000 };
  const o = await host("POST", "/v1/host/sessions/open", open);

  assert.equal(o.json.ok, true, o.text);
  assert.equal(o.json.plugin_pub, list[0].plugin_pub);
  assert.deepEqual(o.json.permissions, board.permissions);
  const e = envelope({ ...open, ks }, "fx_echo", { via: "http" });
  const c = await host("POST", "/v1/host/call", e);

  assert.equal(c.json.ok, true, c.text);
  assert.deepEqual(c.json.result.structuredContent.args, { via: "http" });
  const again = await host("POST", "/v1/host/call", e);

  assert.equal(again.status, 400);
  assert.deepEqual({ ...again.json, message: "" }, { ok: false, code: "replayed", message: "", retryable: false });
  const forged = await host("POST", "/v1/host/call", envelope({ ...open, ks: generate() }, "fx_echo"));

  assert.equal(forged.json.code, "bad_signature", "the token alone cannot call");
  const tok = await host("POST", "/v1/host/resolve", { plugin: FX, kind: "scope", name: "item", value: "acme", complete: false, context: { attached: [] } });

  assert.equal(tok.json.result, "token", tok.text);
  const title = await host("POST", "/v1/host/tickets/title", { plugin: FX, ticket: tok.json.ticket });

  assert.deepEqual(title.json, { ok: true, valid: true, title: "Item acme" });
  const src = await host("POST", "/v1/host/apps/source", { plugin: FX, app: "card" });

  assert.equal(src.json.fingerprint, list[0].apps.find((a) => a.id === "card").fingerprint);
  assert.deepEqual((await host("POST", "/v1/host/sessions/close", { open_id: open.open_id })).json, { ok: true });
  // v0 routes never reach a v1 plugin
  const v0 = await req("POST", "/mcp/call", { headers: { "Content-Type": "application/json" }, body: JSON.stringify({ server: FX, tool: "fx_echo", args: {} }) });

  assert.notEqual(v0.json?.ok, true);
});

test("events: disable and enable stream plugin state and generation", async () => {
  const seen = [];

  const stream = httpRequest({ host: "127.0.0.1", port: PORT, path: "/v1/host/events", headers: { "X-Rox-Host": token } }, (res) => {
    res.setEncoding("utf8");
    res.on("data", (d) => {
      for (const m of d.matchAll(/event: plugin\ndata: (.+)\n/g)) seen.push(JSON.parse(m[1]));
    });
  });

  stream.end();
  await new Promise((r) => setTimeout(r, 200));
  assert.equal((await host("POST", `/v1/host/plugins/${FX}/disable`, {})).json.state, "stopped");
  assert.equal((await host("POST", `/v1/host/plugins/${FX}/enable`, {})).json.state, "running");
  await until(() => seen.some((e) => e.state === "running" && e.generation === 2), 5000, "the events");
  assert.ok(seen.some((e) => e.id === FX && e.state === "stopped"));
  stream.destroy();
  const off = await host("POST", `/v1/host/plugins/${FX}/tools`, { tool: "fx_echo", disabled: true });

  assert.deepEqual(off.json, { ok: true, tool: "fx_echo", disabled: true });
  assert.equal((await host("GET", "/v1/host/plugins")).json.plugins[0].tools.find((t) => t.name === "fx_echo").disabled, true);
  assert.equal((await host("POST", `/v1/host/plugins/${FX}/tools`, { tool: "nope", disabled: true })).json.code, "not_found");
  assert.equal((await host("POST", "/v1/host/plugins/inspect", { url: "http://127.0.0.1:1/mcp", command: "x" })).json.code, "not_supported");
});

test("RTN-09 POST /v1/host/routines/approve stores the approval; the plugin list shows it", async () => {
  const body = JSON.stringify({ plugin: FX, routine: "fx-digest" });

  assert.equal((await req("POST", "/v1/host/routines/approve", { headers: { "Content-Type": "application/json" }, body })).status, 401, "host token");
  const digest = async () => (await host("GET", "/v1/host/plugins")).json.plugins.find((p) => p.id === FX).routines.find((r) => r.id === "fx-digest");
  const was = await digest();

  assert.deepEqual([was.approved, was.enabled, was.available], [false, false, false]);
  const r = await host("POST", "/v1/host/routines/approve", { plugin: FX, routine: "fx-digest" });

  assert.equal(r.status, 200);
  assert.equal(r.json.routine.approved, true);
  const now = await digest();

  assert.equal(now.approved, true);
  assert.equal(now.available, true);
  assert.deepEqual(now.reads_status.map((x) => x.allowed), [true]);
  const bad = await host("POST", "/v1/host/routines/enable", { plugin: FX, routine: "fx-digest", thread: "no-such-chat" });

  assert.deepEqual([bad.status, bad.json.code], [400, "invalid_args"]);
  const unknown = await host("POST", "/v1/host/routines/approve", { plugin: FX, routine: "nope" });

  assert.deepEqual([unknown.status, unknown.json.code], [404, "not_found"]);
  // Run now: a paused routine is refused with the reason
  const paused = await host("POST", "/v1/host/routines/run", { plugin: FX, routine: "fx-digest" });

  assert.deepEqual([paused.status, paused.json.code], [409, "disabled"]);
  assert.match(paused.json.message, /not enabled in any chat/);
  assert.equal((await digest()).running, false);
  assert.deepEqual((await digest()).instances, [], "no instance yet");
  // a webhook takes no host token, and a wrong one is just not found
  const hook = await req("POST", "/v1/hooks/ri_nosuchinstance/secret", { headers: { "Content-Type": "application/json" }, body: "{}" });

  assert.deepEqual([hook.status, hook.json.code], [404, "not_found"]);
});

test("uninstall over HTTP leaves nothing behind", async () => {
  const seed = readFileSync(join(b.keysDir, `${FX}.key`), "utf8").trim();

  assert.deepEqual((await host("POST", `/v1/host/plugins/${FX}/uninstall`, {})).json, { ok: true });
  assert.deepEqual((await host("GET", "/v1/host/plugins")).json.plugins, []);
  assert.equal((await host("POST", "/v1/host/apps/source", { plugin: FX, app: "card" })).status, 404);
  assert.ok(!log.includes(seed), "the plugin key is not in the server log");
  assert.throws(() => statSync(join(b.keysDir, `${FX}.key`)), "the key is deleted");
});
