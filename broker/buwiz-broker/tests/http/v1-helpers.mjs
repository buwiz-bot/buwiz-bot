// Shared by the plugin-host HTTP tests: a publisher key and trust file made
// per run, the fixture plugin's manifest signed with it (by
// `buwiz-broker sign-manifest`), and the signed call envelopes the app
// sends. Nothing here touches the login Keychain.
import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { generate, random128, sign } from "../fixtures/v1/ed.mjs";
import { canonicalSigned, signedBytes } from "../fixtures/v1/jcs.mjs";
import { BROKER, request, startServer, tempDir } from "./helpers.mjs";

export const FIXTURE_DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "fixtures", "v1");
export const FIXTURE = join(FIXTURE_DIR, "server.mjs");
export const FX = "app.buwiz.fixture";

/** A publisher key of this run: {keyId, seed, pub}. */
export function publisher(keyId = `test-${random128().toLowerCase().replace(/[^a-z0-9]/g, "").slice(0, 10)}`) {
  return { keyId, ...generate() };
}

/** Writes a trust file trusting `pubs` and approving `[id, keyId]` pairs. */
export function writeTrust(file, pubs, approved) {
  const publishers = Object.fromEntries(pubs.map((p) => [p.keyId, { publisher: "app.buwiz", alg: "ed25519", public_key: p.pub, ...(p.firstParty ? { first_party: true } : {}) }]));

  writeFileSync(file, JSON.stringify({ v: 1, publishers, approved_plugins: approved.map(([id, key]) => ({ id, publisher_key_id: key })) }));

  return file;
}

/** `manifest` (an object) signed by `pub`, the apps' fingerprints filled
 *  from `sources` ({app id: file}). */
export function signManifest(manifest, pub, sources = {}, dir = tempDir("v1-sign")) {
  const input = join(dir, "unsigned.json");
  const key = join(dir, "publisher.key");
  const out = join(dir, "signed.json");

  writeFileSync(input, JSON.stringify(manifest));
  writeFileSync(key, `${pub.seed}\n`, { mode: 0o600 });
  execFileSync(BROKER, ["sign-manifest", input, "--key-file", key, ...Object.entries(sources).flatMap(([id, file]) => ["--app", `${id}=${file}`]), "--out", out], { stdio: "pipe" });

  return JSON.parse(readFileSync(out, "utf8"));
}

/** The fixture's manifest template, edited by `edit(m)` and signed by `pub`
 *  with the real app fingerprints. */
export function fixtureManifest(pub, edit = () => {}) {
  const m = JSON.parse(readFileSync(join(FIXTURE_DIR, "manifest.json"), "utf8"));

  m.publisher.key_id = pub.keyId;
  edit(m);

  const sources = {};

  for (const id of ["board", "card"]) if ((m.apps || []).some((a) => a.id === id)) sources[id] = join(FIXTURE_DIR, `${id}.tsx`);

  return signManifest(m, pub, sources);
}

/** One test bed: a temp dir with a publisher, a trust file and a signed
 *  fixture manifest at `manifestFile`. */
export function bed(label) {
  const dir = tempDir(`v1-${label}`);
  const pub = publisher();
  const trustFile = writeTrust(join(dir, "trust.json"), [pub], [[FX, pub.keyId], ["app.buwiz.other", pub.keyId]]);
  const manifestFile = join(dir, "manifest.json");

  const writeManifest = (edit) => {
    const m = fixtureManifest(pub, edit);

    writeFileSync(manifestFile, JSON.stringify(m, null, 2));

    return m;
  };

  writeManifest();

  return { dir, pub, trustFile, manifestFile, writeManifest, keysDir: join(dir, "keys"), statsFile: join(dir, "stats.json") };
}

/** The fixture's spawn spec, with extra args (e.g. --misbehave=…) and env. The
 *  fixture writes what it saw to the bed's `statsFile` (see its header). */
export const fixtureSpec = (b, args = [], env = {}) => ({ command: process.execPath, args: [FIXTURE, ...args], env: { FX_MANIFEST: b.manifestFile, ...(b.statsFile && { FX_STATS_FILE: b.statsFile }), ...env } });

/** Waits until `check()` is truthy; its value, or throws after `ms`. */
export async function until(check, ms = 10000, what = "condition") {
  const end = Date.now() + ms;

  while (Date.now() < end) {
    const v = await check();

    if (v) return v;
    await new Promise((r) => setTimeout(r, 20));
  }

  throw new Error(`timed out waiting for ${what}`);
}

/** The permissions digest of an app (spec 8.1): sha256 of the JCS bytes. */
export const permsDigest = (perms) => createHash("sha256").update(canonicalSigned(perms ?? {})).digest("hex");

/** A signed call envelope of `open` (8.1); `over` replaces fields,
 *  `signer` signs instead of the open's session key. */
export function envelope(open, tool, args = {}, over = {}, signer = open.ks.seed) {
  const env = { v: 1, typ: "buwiz-call", plugin: open.plugin, app: open.app, open_id: open.open_id, tool, args, actor: "person", ts: Date.now(), nonce: random128(), ...over };

  return { envelope: env, sig: sign(signer, signedBytes(env)) };
}

// ── the plugin host as the app reaches it: /v1/host/* over HTTP ─────────────

/** A refusal of a host route: `code`, `message`, `details`, `retryable`, `status`. */
export class HostError extends Error {
  constructor(r) {
    super(r.json?.message ?? r.text);
    this.code = r.json?.code ?? `http_${r.status}`;
    this.status = r.status;
    this.details = r.json?.details;
    this.retryable = r.json?.retryable;
  }
}

/** The host routes of the broker running on `dataDir` (its host token is read
 *  from there): each call answers a success's JSON body and throws a HostError
 *  for a refusal. */
export function hostApi(dataDir) {
  const token = readFileSync(join(dataDir, "host-token"), "utf8").trim();
  const headers = { "X-Buwiz-Host": token, "Content-Type": "application/json" };
  const raw = (method, path, obj) => request(method, path, { headers, ...(obj === undefined ? {} : { body: JSON.stringify(obj) }) });

  const post = async (path, obj = {}) => {
    const r = await raw("POST", path, obj);

    if (r.status >= 400 || r.json?.ok === false || !r.json) throw new HostError(r);

    return r.json;
  };

  const plugin = (id) => `/v1/host/plugins/${id}`;

  return {
    token, raw, post,
    list: async () => (await raw("GET", "/v1/host/plugins")).json.plugins,
    plugin: async (id = FX) => (await raw("GET", "/v1/host/plugins")).json.plugins.find((p) => p.id === id),
    inspect: (spec) => post("/v1/host/plugins/inspect", spec),
    install: (body) => post("/v1/host/plugins/install", body),
    // an enable that ends in a failed or waiting plugin is still an answer: {state, code, message, generation}
    enable: async (id = FX) => (await raw("POST", `${plugin(id)}/enable`, {})).json,
    disable: async (id = FX) => (await raw("POST", `${plugin(id)}/disable`, {})).json,
    uninstall: (id = FX) => post(`${plugin(id)}/uninstall`),
    setTool: (id, tool, disabled) => post(`${plugin(id)}/tools`, { tool, disabled }),
    open: (req) => post("/v1/host/sessions/open", req),
    close: (req) => post("/v1/host/sessions/close", req),
    call: (env) => post("/v1/host/call", env),
    resolve: (req) => post("/v1/host/resolve", req),
    ticketTitle: (req) => post("/v1/host/tickets/title", req),
    appSource: (req) => post("/v1/host/apps/source", req),
  };
}

/** inspect + install of the fixture (with `args` and `env` added to its
 *  spawn spec) through the host routes, as the app does after the review. */
export async function installFixture(api, b, args = [], env = {}) {
  const spec = fixtureSpec(b, args, env);
  const i = await api.inspect(spec);

  return api.install({ ...spec, manifest_sha256: i.manifest_sha256 });
}

/** The code a promise rejects with (a HostError's), or "resolved". */
export const codeOf = (p) => p.then(() => "resolved", (e) => e.code || String(e));

/** Opens an app of `plugin` (an entry of the plugin list) the way the app
 *  does: a session key, an open_id, the app's permissions digest. → the open
 *  with its key and the broker's answer (`res`). */
export async function openApp(api, plugin, app, { notAfter = Date.now() + 3600_000, perms } = {}) {
  const ks = generate();
  const a = plugin.apps.find((x) => x.id === app);
  const req = { plugin: plugin.id, app, open_id: random128(), ks_pub: ks.pub, perms_sha256: perms ?? permsDigest(a?.permissions), not_after: notAfter };
  const res = await api.open(req);

  return { ...req, ks, res };
}

/** What the fixture last wrote to its FX_STATS_FILE (fx_stats' answer and its pid). */
export const statsOf = (file) => JSON.parse(readFileSync(file, "utf8"));

/** Whether process `pid` is alive. */
export const alive = (pid) => {
  try {
    process.kill(pid, 0);

    return true;
  } catch {
    return false;
  }
};

/** The fixture on a pipe, with no broker: initialize done, `callTool(name, args)`
 *  answers the result and rejects with {code, message} for a JSON-RPC error. */
export async function rawFixture(b, args = []) {
  const spec = fixtureSpec(b, args);
  const child = spawn(spec.command, spec.args, { env: { PATH: process.env.PATH, ...spec.env }, stdio: ["pipe", "pipe", "ignore"] });
  const waiting = new Map();
  let id = 0;
  const send = (m) => child.stdin.write(`${JSON.stringify(m)}\n`);

  createInterface({ input: child.stdout }).on("line", (line) => {
    const m = JSON.parse(line);
    const w = waiting.get(m.id);

    if (!w) return;
    waiting.delete(m.id);

    if (m.error) w.reject(Object.assign(new Error(m.error.message), { code: m.error.code }));
    else w.resolve(m.result);
  });

  const rpc = (method, params) => new Promise((resolve, reject) => (waiting.set(++id, { resolve, reject }), send({ jsonrpc: "2.0", id, method, params })));

  await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } });
  send({ jsonrpc: "2.0", method: "notifications/initialized" });

  return { callTool: (name, a = {}) => rpc("tools/call", { name, arguments: a }), close: () => child.kill() };
}

/** A tool result's data: its structuredContent, else its first text parsed. */
export const dataOf = (r) => r.structuredContent ?? JSON.parse(r.content[0].text);

/** A broker of its own for `fn`: a bed (publisher, trust file, signed fixture
 *  manifest), a data dir, the bed's file keystore and trust file, `env` over
 *  them. `fn` gets {b, api, dataDir, server}; the broker is stopped after. */
export async function withHost(label, fn, env = {}) {
  const b = bed(label);
  const dataDir = tempDir(`v1-${label}-data`);
  const server = await startServer(dataDir, { env: { BUWIZ_KEYSTORE: `file:${b.keysDir}`, BUWIZ_TRUST_FILE: b.trustFile, ...env } });

  try {
    return await fn({ b, api: hostApi(dataDir), dataDir, server });
  } finally {
    await server.stop();
  }
}
