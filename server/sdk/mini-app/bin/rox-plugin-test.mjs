#!/usr/bin/env node
// sdk/mini-app/bin/rox-plugin-test.mjs — the plugin-side conformance checks
// of the Rox Mini App Protocol v1 (spec 17), run against one plugin.
//
//   npm run plugin-test -- --command node --args sdk/mini-app/template/server.mjs \
//       --sign-with ~/.config/rox/test.key
//   npm run plugin-test -- --command node --args path/server.mjs \
//       --manifest signed.json --key-file ~/.config/rox/test.key
//   npm run plugin-test -- --keygen ~/.config/rox/test.key
//
// The plugin is spawned the way Rox's broker spawns it, under the broker's
// real v1 manager (server/v1/plugins.mjs, sessions.mjs) running in this
// process on an in-memory database, a file keystore in a temp directory
// and a trust file written for this run. Nothing here opens a port, reads
// Rox's or the broker's data directory, or touches the login Keychain.
//
// --sign-with <key file>: reads the manifest the plugin serves, fills each
// app's fingerprint from the source it serves, signs it with the key (a
// 0600 file holding a base64url seed) and hands the signed copy to the
// plugin through the --manifest-env variable (default ROX_PLUGIN_MANIFEST).
// --manifest <file>: a manifest already signed; its key is trusted from
// --key-file, else from the shipped server/v1/trust.json.
//
// The checklist names the spec ID of each check and exits 1 when any
// fails. --json prints the results as JSON instead.
import { chmodSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve as resolvePath } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import { isV1 } from "../../../server/grammar.mjs";
import { generate, pubOf, random128, sign, verify } from "../../../server/v1/ed.mjs";
import { isObject, isString, parseIJson, signedBytes } from "../../../server/v1/jcs.mjs";
import { keystore } from "../../../server/v1/keystore.mjs";
import { MANIFEST_URI, checkTools, manifestHash, permsDigest, verifyManifest } from "../../../server/v1/manifest.mjs";
import { createPlugins, resultData } from "../../../server/v1/plugins.mjs";
import { checkOpen, createSessions, SOURCE_MAX } from "../../../server/v1/sessions.mjs";
import { fingerprintOf, signManifest } from "../../../server/v1/sign-manifest.mjs";
import { TRUST_FILE, loadTrust } from "../../../server/v1/trust.mjs";
import { newTicket } from "../lib/rox-plugin.mjs";

const USAGE = `usage: rox-plugin-test --command <cmd> [--args <arg>]... [--env KEY=VAL]...
                       (--sign-with <key file> [--key-id <id>] | --manifest <signed.json> [--key-file <key file>])
                       [--manifest-env NAME] [--json]
       rox-plugin-test --keygen <key file>`;

const ALLOWED_IMPORTS = ["nsh", "nsh/base", "rox"];
const NO_NETWORK = /\b(fetch|XMLHttpRequest|WebSocket|localStorage|sessionStorage)\b/;
const CALL_MS = 10_000;

// ── options ──────────────────────────────────────────────────────────────

export function parseArgs(argv) {
  const o = { command: "", args: [], env: {}, manifest: "", signWith: "", keyFile: "", keyId: "", manifestEnv: "ROX_PLUGIN_MANIFEST", json: false, keygen: "" };

  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];

    const next = () => {
      if (i + 1 >= argv.length) throw new Error(`${a} needs a value\n${USAGE}`);

      return argv[++i];
    };

    if (a === "--command") o.command = next();
    else if (a === "--args") {
      const v = next();

      o.args.push(...(v.startsWith("[") ? JSON.parse(v) : [v]));
    } else if (a === "--env") {
      const [k, ...rest] = next().split("=");

      if (!k || !rest.length) throw new Error("--env takes KEY=VAL");
      o.env[k] = rest.join("=");
    } else if (a === "--manifest") o.manifest = next();
    else if (a === "--sign-with") o.signWith = next();
    else if (a === "--key-file") o.keyFile = next();
    else if (a === "--key-id") o.keyId = next();
    else if (a === "--manifest-env") o.manifestEnv = next();
    else if (a === "--keygen") o.keygen = next();
    else if (a === "--json") o.json = true;
    else if (a === "--help" || a === "-h") throw new Error(USAGE);
    else throw new Error(`unknown option ${a}\n${USAGE}`);
  }

  if (o.keygen) return o;

  if (!o.command || !o.manifest === !o.signWith) throw new Error(USAGE);

  return o;
}

// A key file is the owner's alone, as sign-manifest.mjs insists.
function readKey(path) {
  if (statSync(path).mode & 0o077) throw new Error(`${path} is readable by others; chmod 600 it`);
  const seed = readFileSync(path, "utf8").trim();

  pubOf(seed);

  return seed;
}

function keygen(path) {
  const k = generate();

  writeFileSync(path, `${k.seed}\n`, { mode: 0o600, flag: "wx" });
  chmodSync(path, 0o600);

  return k.pub;
}

// ── the checklist ────────────────────────────────────────────────────────

function checklist() {
  const rows = [];

  const run = async (id, what, fn) => {
    try {
      const note = await fn();

      rows.push({ id, what, status: "pass", note: note || "" });

      return true;
    } catch (e) {
      rows.push({ id, what, status: "fail", note: String(e?.message || e).slice(0, 300) });

      return false;
    }
  };

  const skip = (id, what, why) => rows.push({ id, what, status: "skip", note: why });

  return { rows, run, skip };
}

const expect = (cond, msg) => {
  if (!cond) throw new Error(msg);
};

const codeOf = (p) => p.then(() => "resolved", (e) => e?.code ?? String(e?.message || e));
const errText = (e) => String(e?.message || e).slice(0, 200);

// a raw MCP client on the plugin, as the broker's own would be, with stderr kept
async function rawClient(spec) {
  const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
  const { StdioClientTransport } = await import("@modelcontextprotocol/sdk/client/stdio.js");
  const transport = new StdioClientTransport({ command: spec.command, args: spec.args, env: { PATH: process.env.PATH || "", ...spec.env }, stderr: "pipe" });
  const client = new Client({ name: "rox-plugin-test", version: "1.0.0" });
  const conn = { client, stderr: "" };

  transport.stderr?.on("data", (d) => (conn.stderr += String(d)));
  await client.connect(transport);
  conn.close = () => client.close().catch(() => {});

  return conn;
}

const callRaw = (conn, name, args) => conn.client.callTool({ name, arguments: args }, undefined, { timeout: CALL_MS });

// What the plugin serves before anything is signed: the manifest and the
// app sources, read without provisioning (resources are not gated, 7.8).
async function served(spec) {
  const conn = await rawClient(spec);

  try {
    const { contents = [] } = await conn.client.readResource({ uri: MANIFEST_URI });
    const m = parseIJson(contents.find((c) => isString(c.text))?.text ?? "");

    if (!isObject(m)) throw new Error("the plugin's rox://manifest is not a JSON object");
    const sources = {};

    for (const a of Array.isArray(m.apps) ? m.apps : []) {
      const r = await conn.client.readResource({ uri: a.entry });

      sources[a.id] = Buffer.from(r.contents?.[0]?.text ?? "", "utf8");
    }

    return { m, sources };
  } finally {
    await conn.close();
  }
}

// Which spec 17.1 ID a verifyManifest failure belongs to.
function manIdOf(e) {
  const msg = String(e?.message || "");

  if (e?.code === "protocol_unsupported" || /^(protocol|version):/.test(msg)) return "MAN-04";

  if (e?.code === "not_supported") return "MAN-05";

  if (/must be a JSON object|is not I-JSON/.test(msg)) return "MAN-03";

  if (/^(signature|publisher|id: .* approved|manifest: cannot)/.test(msg)) return "MAN-01";

  if (/permissions.*not a known key/.test(msg)) return "MAN-06";

  if (/^routines\[\d+\]\.\w+: is not a known key/.test(msg)) return "MAN-12";

  if (/built-in command/.test(msg)) return "MAN-11";

  if (/must start with .*_/.test(msg)) return "MAN-10";

  if (/is over|entries; at most|must be \d+ to \d+|properties; at most|nests deeper|bytes/.test(msg)) return "MAN-08";

  return "MAN-07";
}

const MAN_ROWS = [
  ["MAN-01", "the manifest is signed by a trusted publisher key; the signature verifies over JCS (4.2)"],
  ["MAN-03", "the manifest is a JSON object (I-JSON, no duplicate keys)"],
  ["MAN-04", "protocol 1.0.x is accepted by a 1.0 host; version is semver"],
  ["MAN-05", "needs is absent or []"],
  ["MAN-06", "permissions carry only known keys"],
  ["MAN-07", "every name follows its grammar (3)"],
  ["MAN-08", "every field is within its Appendix A limit"],
  ["MAN-11", "no command is named like a built-in"],
  ["MAN-12", "routines carry only known keys (no needs)"],
];

// The static import and default-export check of 6.3, with esbuild.
async function staticApp(source, file) {
  const { build } = await import("esbuild");
  const imports = [];

  const r = await build({
    stdin: { contents: source, loader: "tsx", sourcefile: file },
    bundle: true, write: false, format: "esm", jsx: "preserve", metafile: true, logLevel: "silent",
    plugins: [{ name: "imports", setup: (b) => b.onResolve({ filter: /.*/ }, (a) => (imports.push(a.path), { path: a.path, external: true })) }],
  });

  return { imports, exports: Object.values(r.metafile.outputs)[0]?.exports || [], code: r.outputFiles[0].text };
}

// The raw resolve answer (9.2) held to its shape and caps: what the host
// would otherwise cap or refuse silently. → the tickets it carries.
function checkResolve(m, data) {
  const r = data?.result;
  const found = [];

  expect(["choices", "open", "token", "error"].includes(r), `result ${JSON.stringify(r)?.slice(0, 40)} is not choices, open, token or error`);

  if (r === "choices") {
    expect(Array.isArray(data.choices), "choices must be an array");
    expect(data.choices.length <= 20, `${data.choices.length} choices; at most 20`);

    for (const c of data.choices) {
      expect(isObject(c) && isString(c.label) && isString(c.value), "each choice needs a label and a value");
      expect([...c.label].length <= 80, `label "${c.label.slice(0, 30)}…" is over 80 characters`);
      expect(c.detail === undefined || (isString(c.detail) && [...c.detail].length <= 160), "detail must be a string of at most 160 characters");

      if (isV1("ticket", c.value)) found.push(c.value);
    }
  }

  if (r === "token") {
    expect(isV1("ticket", data.ticket), "a token result needs a ticket (tkt_…)");
    expect(isString(data.title) && [...data.title].length <= 60, "a token's title is a string of at most 60 characters");
    found.push(data.ticket);
  }

  if (r === "error") expect(isString(data.message) && [...data.message].length <= 200, "an error's message is a string of at most 200 characters");

  if (r === "open") {
    expect(data.title === undefined || (isString(data.title) && [...data.title].length <= 60), "an open's title is at most 60 characters");
    checkOpen(m, data);

    if (data.ticket) found.push(data.ticket);

    if (isV1("ticket", data.props?.ticket)) found.push(data.props.ticket);
  }

  return found;
}

// ── the run ──────────────────────────────────────────────────────────────

export async function runPluginTest(o) {
  const dir = mkdtempSync(join(tmpdir(), "rox-plugin-test-"));
  const { rows, run, skip } = checklist();
  const spec = { command: o.command, args: o.args, env: { ...o.env } };
  const manifestFile = join(dir, "manifest.json");
  const trustFile = join(dir, "trust.json");
  const logDir = join(dir, "logs");
  let m = null;
  let raw = null;
  let plugins = null;
  const seeds = [];

  try {
    // the signed manifest and the trust file of this run
    if (o.signWith) {
      const seed = readKey(o.signWith);
      const got = await served(spec);
      const unsigned = got.m;

      delete unsigned.signature;

      if (!isObject(unsigned.publisher)) unsigned.publisher = {};

      if (o.keyId) unsigned.publisher.key_id = o.keyId;
      writeFileSync(manifestFile, JSON.stringify(signManifest(unsigned, seed, got.sources), null, 2));
      writeTrust(trustFile, { [unsigned.publisher.key_id]: pubOf(seed) }, unsigned);
    } else {
      const signed = parseIJson(readFileSync(o.manifest, "utf8"));
      const keyId = signed?.publisher?.key_id;
      const pubs = o.keyFile ? { [keyId]: pubOf(readKey(o.keyFile)) } : Object.fromEntries([...loadTrust(TRUST_FILE()).publishers].map(([k, p]) => [k, p.public_key]));

      writeFileSync(manifestFile, readFileSync(o.manifest));
      writeTrust(trustFile, pubs, signed);
    }

    spec.env[o.manifestEnv] = manifestFile;
    const trust = () => loadTrust(trustFile);

    // ── raw: what the plugin itself answers ─────────────────────────────
    raw = await rawClient(spec);
    const { contents = [] } = await raw.client.readResource({ uri: MANIFEST_URI });
    const text = contents.find((c) => isString(c.text))?.text;
    let failedId = "";

    try {
      m = verifyManifest(text, trust()).manifest;
    } catch (e) {
      failedId = manIdOf(e);
      rows.push({ id: failedId, what: "the manifest verifies (5.2)", status: "fail", note: errText(e) });
    }

    for (const [id, what] of MAN_ROWS) {
      if (id === failedId) continue;

      if (!m) skip(id, what, "the manifest did not verify");
      else rows.push({ id, what, status: "pass", note: "" });
    }

    if (m) {
      await run("MAN-02", "a byte changed in the manifest fails verification", () => {
        const changed = text.replace(`"${m.title}"`, `"${m.title.slice(0, -1)}${m.title.endsWith("x") ? "y" : "x"}"`);

        expect(changed !== text, "could not change the title");
        let code = "";

        try {
          verifyManifest(changed, trust());
        } catch (e) {
          code = e.code;
        }

        expect(code === "manifest_invalid", `a changed manifest was ${code || "accepted"}`);
      });
    }

    const { tools = [] } = await raw.client.listTools();
    const own = tools.filter((t) => !isV1("reservedTool", t.name));

    if (m)
      await run("MAN-10", `every tool in tools/list starts with ${m.short}_ and every tool the manifest names exists`, () => {
        checkTools(m, tools);

        return `${own.length} tools`;
      });
    else skip("MAN-10", "tool names", "the manifest did not verify");

    for (const a of m?.apps || []) await appChecks(raw, a, run);

    // RSV-01: nothing before rox.provision; provision answers the manifest hash
    const key = generate();

    seeds.push(key.seed);
    const provisionArgs = { protocol: "1.0.0", plugin: m?.id || "", key: { alg: "ed25519", seed: key.seed }, pub: key.pub };

    if (own.length)
      await run("RSV-01", `a tool before rox.provision fails (${own[0].name})`, async () => {
        const r = await callRaw(raw, own[0].name, {}).catch((e) => ({ thrown: e }));

        expect(r.thrown || r.isError, `${own[0].name} answered before rox.provision`);
      });

    await run("RSV-01", "rox.provision answers {ok: true, manifest_sha256} of the served manifest; again with the same key too", async () => {
      for (let i = 0; i < 2; i++) {
        const r = await callRaw(raw, "rox.provision", provisionArgs);
        const d = resultData(r);

        expect(!r.isError && d?.ok === true, `rox.provision answered ${JSON.stringify(d)?.slice(0, 120)}`);

        if (m) expect(d.manifest_sha256 === manifestHash(m), "manifest_sha256 is not sha256 of the served manifest's JCS bytes (5.2 step 7)");
      }
    });

    // RSV-02, RSV-03: session certificates
    const app = (m?.apps || [])[0];
    const reservedArgs = (over) => ({ app: app?.id, open_id: random128(), ks_pub: generate().pub, perms_sha256: permsDigest(app?.permissions), not_after: Date.now() + 3600_000, ...over });
    const isInvalid = (r) => r.isError && resultData(r)?.code === "invalid_args";

    await run("RSV-02", "rox.session.open with an unknown app fails invalid_args", async () => {
      expect(isInvalid(await callRaw(raw, "rox.session.open", reservedArgs({ app: "sdk-no-such-app" }))), "an unknown app was not refused with code invalid_args");
    });

    if (app) {
      await run("RSV-02", `rox.session.open of ${app.id} with a wrong perms_sha256, or not_after past 8 h, fails invalid_args`, async () => {
        expect(isInvalid(await callRaw(raw, "rox.session.open", reservedArgs({ perms_sha256: permsDigest({ tools: ["sdk_not_granted"] }) }))), "a wrong perms_sha256 was not refused");
        expect(isInvalid(await callRaw(raw, "rox.session.open", reservedArgs({ not_after: Date.now() + 9 * 3600_000 }))), "a certificate over 8 h was not refused");
      });

      await run("RSV-03", `the session certificate for ${app.id} is the request, signed by the plugin key over JCS (4.4)`, async () => {
        const req = reservedArgs();
        const r = await callRaw(raw, "rox.session.open", req);
        const d = resultData(r);

        expect(!r.isError && isObject(d?.cert), `rox.session.open answered ${JSON.stringify(d)?.slice(0, 120)}`);
        const want = { v: 1, typ: "rox-session", plugin: m.id, app: app.id, open_id: req.open_id, ks_pub: req.ks_pub, perms_sha256: req.perms_sha256, not_after: req.not_after };

        expect(JSON.stringify(Object.keys(d.cert).sort()) === JSON.stringify(Object.keys(want).sort()), `the certificate has fields ${Object.keys(d.cert).join(", ")}`);

        for (const [k, v] of Object.entries(want)) expect(d.cert[k] === v, `cert.${k} is ${JSON.stringify(d.cert[k])?.slice(0, 60)}, not ${JSON.stringify(v)?.slice(0, 60)}`);
        expect(verify(key.pub, signedBytes(d.cert), d.cert_sig), "cert_sig does not verify with the plugin public key");

        // RSV-05 below closes this open
        raw.openId = req.open_id;
      });
    } else skip("RSV-03", "session certificates", "the plugin has no apps");

    await run("RSV-05", "rox.session.close answers ok: true; an unknown rox.* answers -32601 (7.9)", async () => {
      const closed = resultData(await callRaw(raw, "rox.session.close", { open_id: raw.openId || random128() }));

      expect(closed?.ok === true, `rox.session.close answered ${JSON.stringify(closed)?.slice(0, 80)}`);
      const code = await callRaw(raw, "rox.sdk.probe", {}).then(() => "answered", (e) => e?.code);

      expect(code === -32601, `rox.sdk.probe answered ${code}, not -32601`);
    });

    await run("RSV-05", "rox.uninstall answers ok: true", async () => {
      const d = resultData(await callRaw(raw, "rox.uninstall", {}));

      expect(d?.ok === true, `rox.uninstall answered ${JSON.stringify(d)?.slice(0, 80)}`);
    });
    await raw.close();

    // ── under the broker's real v1 manager ──────────────────────────────
    const keys = keystore(`file:${join(dir, "keys")}`);

    plugins = createPlugins({ db: new DatabaseSync(":memory:"), keys, logDir, trust });
    const sessions = createSessions(plugins);
    let installed = false;

    await run("LCY-01", "install through the broker's gate: provision, verify, tools/list (5.2, 13.1)", async () => {
      const i = await plugins.inspect(spec);

      await plugins.install({ ...spec, manifest_sha256: i.manifest_sha256 });
      installed = true;
      seeds.push(await keys.get(i.manifest.id));

      return i.review.lines.length ? `review: ${i.review.lines.length} lines` : "";
    });

    if (installed) await brokerChecks(plugins, sessions, m, run, skip);
    else for (const [id, what] of [["BRG-03", "signed calls"], ["RSV-04", "ticket titles"], ["INV-04", "resolve shapes"]]) skip(id, what, "the plugin did not install");

    if (installed) await run("RSV-05", "uninstall through the broker (rox.uninstall, key removed)", () => plugins.uninstall(m.id).then(() => ""));
  } catch (e) {
    rows.push({ id: "RUN", what: "the run", status: "fail", note: errText(e) });
  } finally {
    await raw?.close();
    await plugins?.stopAll();
  }

  // RSV-03: no seed in anything the plugin wrote to stderr
  const logs = (raw?.stderr || "") + readLogs(logDir);
  const leaked = seeds.filter(Boolean).some((s) => logs.includes(s) || logs.includes(Buffer.from(s, "base64url").toString("base64").replace(/=+$/, "")));

  rows.push({ id: "RSV-03", what: "the plugin never writes its key to stderr (4.3, P4)", status: leaked || /\[redacted key\]/.test(logs) ? "fail" : "pass", note: leaked ? "a seed appeared in stderr" : "" });
  rmSync(dir, { recursive: true, force: true });

  return { plugin: m ? { id: m.id, short: m.short, version: m.version } : null, rows, ok: rows.every((r) => r.status !== "fail") };
}

function writeTrust(file, pubs, m) {
  const publishers = Object.fromEntries(Object.entries(pubs).map(([k, pub]) => [k, { publisher: String(m?.publisher?.id || ""), alg: "ed25519", public_key: pub }]));

  writeFileSync(file, JSON.stringify({ v: 1, publishers, approved_plugins: [{ id: String(m?.id || ""), publisher_key_id: String(m?.publisher?.key_id || "") }] }));
}

function readLogs(logDir) {
  try {
    return readdirSync(logDir).map((f) => readFileSync(join(logDir, f), "utf8")).join("\n");
  } catch {
    return "";
  }
}

// APP-01..04 for one app, on the raw client: what the plugin serves.
async function appChecks(raw, a, run) {
  let source = null;

  await run("APP-01", `${a.id}: ${a.entry} is one text/x-nsh-tsx content whose fingerprint matches the manifest (6.1, 6.2)`, async () => {
    const { contents = [] } = await raw.client.readResource({ uri: a.entry });

    expect(contents.length === 1 && isString(contents[0].text), `${a.entry} answered ${contents.length} contents; exactly one text content is allowed`);
    expect(contents[0].mimeType === undefined || contents[0].mimeType === a.mime, `${a.entry} is ${contents[0].mimeType}`);
    source = contents[0].text;
    expect(Buffer.byteLength(source) <= SOURCE_MAX, `${a.entry} is over ${SOURCE_MAX} bytes`);
    expect(fingerprintOf(Buffer.from(source, "utf8")) === a.fingerprint, `the served source's fingerprint is ${fingerprintOf(Buffer.from(source, "utf8"))}, the manifest says ${a.fingerprint}`);
  });

  if (source === null) return;
  let s = null;

  await run("APP-02", `${a.id}: compiles as TSX and has a default export (6.3)`, async () => {
    s = await staticApp(source, `${a.id}.tsx`).catch((e) => Promise.reject(new Error(`does not compile: ${e.errors?.[0]?.text || errText(e)}`)));
    expect(s.exports.includes("default"), `no default export (exports: ${s.exports.join(", ") || "none"})`);
  });

  if (!s) return;

  await run("APP-03", `${a.id}: imports only ${ALLOWED_IMPORTS.join(", ")} (6.3)`, () => {
    const bad = s.imports.filter((i) => !ALLOWED_IMPORTS.includes(i));

    expect(!bad.length, `unsupported_import: ${bad.join(", ")}`);
  });

  await run("APP-04", `${a.id}: uses no fetch, XMLHttpRequest, WebSocket or web storage (6.3)`, () => {
    const hit = NO_NETWORK.exec(s.code);

    expect(!hit, `uses ${hit?.[1]}; apps get data from rox.call only`);
  });
}

// A fake Rox: a session key per open, envelopes signed as spec 8.1 says.
async function openAs(sessions, m, app) {
  const ks = generate();
  const req = { plugin: m.id, app: app.id, open_id: random128(), ks_pub: ks.pub, perms_sha256: permsDigest(app.permissions), not_after: Date.now() + 3600_000 };

  await sessions.open(req);

  return { ...req, ks };
}

function envelope(open, tool, args = {}, signer = open.ks.seed) {
  const env = { v: 1, typ: "rox-call", plugin: open.plugin, app: open.app, open_id: open.open_id, tool, args, ts: Date.now(), nonce: random128() };

  return { envelope: env, sig: sign(signer, signedBytes(env)) };
}

async function brokerChecks(plugins, sessions, m, runAny, skip) {
  // once the broker has stopped the plugin (a bad certificate fails it,
  // 4.4 step 4), later checks say so instead of failing one by one
  const run = (id, what, fn) => {
    const rt = plugins.runtime(m.id);

    return rt.state === "running" ? runAny(id, what, fn) : skip(id, what, `the broker stopped the plugin (${rt.code || rt.state})`);
  };

  // fingerprints of what the running plugin serves (the broker's check)
  for (const a of m.apps || [])
    await run("APP-01", `${a.id}: the broker's source check passes (fingerprint_mismatch otherwise)`, () => sessions.appSource({ plugin: m.id, app: a.id }).then(() => ""));

  // BRG-03, BRG-04, BRG-06: a session per app with tools, one signed call per tool
  const withTools = (m.apps || []).filter((a) => a.permissions?.tools?.length);

  if (!withTools.length) skip("BRG-03", "signed calls", "no app has permissions.tools");

  for (const a of withTools) {
    let open = null;

    await run("BRG-03", `${a.id}: a session opens; the broker verifies the certificate (4.4 step 5)`, async () => {
      open = await openAs(sessions, m, a);
    });

    if (!open) continue;
    let first = null;

    for (const tool of a.permissions.tools)
      await run("BRG-03", `${a.id}: a signed call of ${tool} reaches the plugin and is answered`, async () => {
        const call = envelope(open, tool);

        first ||= call;
        const code = await codeOf(sessions.call(call));

        // an isError answer still went through the signed path
        expect(code === "resolved" || code === "tool_error", `${tool}: ${code}`);

        return code === "tool_error" ? "answered isError for {} args" : "";
      });

    if (!first) continue;
    await run("BRG-04", `${a.id}: the same envelope sent again is replayed`, async () => expect((await codeOf(sessions.call(first))) === "replayed", "a replayed envelope was not refused"));
    await run("BRG-06", `${a.id}: an envelope signed by another key is bad_signature`, async () => {
      const code = await codeOf(sessions.call(envelope(open, a.permissions.tools[0], {}, generate().seed)));

      expect(code === "bad_signature", `a forged envelope was ${code}`);
    });
    await sessions.close({ open_id: open.open_id });
  }

  // INV-04, INV-05: every command and scope's resolve, raw and through the broker
  const tickets = [];
  const entries = [...(m.commands || []).map((c) => ["command", c]), ...(m.scopes || []).map((s) => ["scope", s])];

  const resolveRaw = (kind, e, value, complete) =>
    plugins.callTool(m.id, e.resolve, { kind, name: e.name, value, complete, context: { attached: [] } }, { meta: { "rox/caller": { origin: "resolve", plugin: m.id, ts: Date.now() } }, timeout: 10_000 });

  if (!entries.length) skip("INV-04", "resolve shapes", "the manifest has no commands or scopes");

  for (const [kind, e] of entries) {
    const modes = kind === "scope" ? [["", true], ["sdk-probe", false]] : [["sdk-probe", false]];

    for (const [value, complete] of modes)
      await run(kind === "command" ? "INV-05" : "INV-04", `${kind} ${e.name}: ${e.resolve} with complete: ${complete} answers a 9.2 shape within its caps${kind === "command" ? "; an open names an app of this plugin" : ""}`, async () => {
        const r = await resolveRaw(kind, e, value, complete);

        if (r.isError) return "answered isError (shown as an error)";
        const data = resultData(r);

        tickets.push(...checkResolve(m, data));
        await sessions.resolve({ plugin: m.id, kind, name: e.name, value, complete });

        return `result ${data.result}`;
      });

    // an attachable scope: a chosen completion resolves to a token (7.6)
    if (kind === "scope" && e.attach) {
      const choice = tickets.find((t) => isV1("ticket", t));

      if (!choice) skip("INV-04", `scope ${e.name}: a chosen completion resolves to a token`, "the completion offered no ticket");
      else
        await run("INV-04", `scope ${e.name}: a chosen completion resolves to a token (7.6)`, async () => {
          const res = await sessions.resolve({ plugin: m.id, kind, name: e.name, value: choice, complete: false });

          expect(res.result === "token" && isV1("canonicalToken", res.token), `answered ${res.result}`);
        });
    }
  }

  // RSV-04: titles only, never a subject
  const titleOnly = (d) => Object.keys(d || {}).every((k) => k === "valid" || k === "title");

  await run("RSV-04", "rox.ticket.resolve of an unknown ticket is {valid: false}", async () => {
    const d = await plugins.callReserved(m.id, "rox.ticket.resolve", { ticket: newTicket() });

    expect(d === null || (d.valid === false && titleOnly(d)), `answered ${JSON.stringify(d)?.slice(0, 120)}`);

    return d === null ? "not implemented (-32601)" : "";
  });

  if (!tickets.length) skip("RSV-04", "a ticket the plugin issued resolves to a title", "no resolve answer carried a ticket");
  else
    await run("RSV-04", "rox.ticket.resolve of an issued ticket returns a title and no subject field", async () => {
      const d = await plugins.callReserved(m.id, "rox.ticket.resolve", { ticket: tickets[0] });

      expect(d?.valid === true && isString(d.title), `answered ${JSON.stringify(d)?.slice(0, 120)}`);
      expect(titleOnly(d), `fields other than valid and title: ${Object.keys(d).join(", ")}`);
      expect([...d.title].length <= 120, "the title is over 120 characters");
    });
}

// ── output ───────────────────────────────────────────────────────────────

export function format(res, o) {
  const mark = { pass: "[x]", fail: "[ ]", skip: "[-]" };
  const word = { pass: "pass", fail: "FAIL", skip: "skip" };
  const head = res.plugin ? `${res.plugin.id} ${res.plugin.version}` : "(no verified manifest)";
  const n = (s) => res.rows.filter((r) => r.status === s).length;

  return [
    `Rox Mini App Protocol v1, plugin checks: ${head}`,
    `  ${[o.command, ...o.args].join(" ")}`,
    ...res.rows.toSorted((a, b) => (a.id === b.id ? 0 : a.id < b.id ? -1 : 1)).map((r) => `  ${mark[r.status]} ${r.id} ${word[r.status]}  ${r.what}${r.note ? `\n        ${r.note}` : ""}`),
    `${n("pass")} pass, ${n("fail")} fail, ${n("skip")} skipped: ${res.ok ? "conforms" : "does NOT conform"}`,
  ].join("\n");
}

async function main(argv) {
  const o = parseArgs(argv);

  if (o.keygen) {
    const pub = keygen(resolvePath(o.keygen));

    console.log(`wrote ${o.keygen} (0600). Public key: ${pub}\nKeep it for local tests only; release manifests are signed with --keychain.`);

    return 0;
  }

  const res = await runPluginTest(o);

  console.log(o.json ? JSON.stringify(res, null, 2) : format(res, o));

  return res.ok ? 0 : 1;
}

if (process.argv[1] === fileURLToPath(import.meta.url))
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (e) => {
      console.error(`rox-plugin-test: ${String(e?.message || e)}`);
      process.exit(2);
    },
  );
