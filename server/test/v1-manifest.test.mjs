// Spec 17.1 (manifest), against server/v1/manifest.mjs: verification
// order, signature, protocol and version rules, every grammar of section 3
// and every manifest limit of Appendix A at the limit and one over. Also
// the JCS vectors shared with Rox and the ed25519 helpers.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { V1, isV1, v1Canonical, v1TypedTokens } from "../server/grammar.mjs";
import { b64u, fromB64u, generate, pubOf, sign, verify } from "../server/v1/ed.mjs";
import { canonicalSigned, canonicalize, parseIJson, signedBytes } from "../server/v1/jcs.mjs";
import { reviewOf, LIMITS, checkTools, compareSemver, validateProps, verifyManifest } from "../server/v1/manifest.mjs";
import { signManifest } from "../server/v1/sign-manifest.mjs";
import { loadTrust } from "../server/v1/trust.mjs";
import { ROOT, tempDir } from "./helpers.mjs";
import { FX, fixtureManifest, publisher, writeTrust } from "./v1-helpers.mjs";

const pub = publisher();
const dir = tempDir("v1-manifest");
const LONG_ID = `${"a".repeat(63)}.${"b".repeat(63)}.${"c".repeat(63)}.${"d".repeat(63)}`; // 255
const LONGER_ID = `${"a".repeat(63)}.${"b".repeat(63)}.${"c".repeat(63)}.${"d".repeat(62)}.e`; // 256
const trust = loadTrust(writeTrust(join(dir, "trust.json"), [pub], [[FX, pub.keyId], [LONG_ID, pub.keyId], [LONGER_ID, pub.keyId]]));

const signed = (edit) => fixtureManifest(pub, edit);
const check = (m) => verifyManifest(JSON.stringify(m), trust);
const ok = (edit) => assert.doesNotThrow(() => check(signed(edit)));

function refused(edit, code = "manifest_invalid", re) {
  assert.throws(() => check(signed(edit)), (e) => {
    assert.equal(e.code, code, e.message);

    if (re) assert.match(e.message, re);

    return true;
  });
}

// renames the fixture's short everywhere its tools are named
const reshort = (m, short) => {
  const text = JSON.stringify(m).replaceAll('"fx_', `"${short}_`);

  Object.assign(m, JSON.parse(text), { short });
};

const app = (id) => ({ id, title: id, description: "", entry: `rox-app://${id}`, mime: "text/x-nsh-tsx", sdk: "1", fingerprint: `sha256:${"0".repeat(64)}`, placement: ["full"] });

// ── JCS and keys ─────────────────────────────────────────────────────────

test("JCS: the shared vectors (RFC 8785) canonicalize as Rox expects", () => {
  const { vectors } = JSON.parse(readFileSync(join(ROOT, "test", "fixtures", "jcs-vectors.json"), "utf8"));

  assert.ok(vectors.length >= 12);

  for (const v of vectors) {
    if (v.error) {
      assert.throws(() => parseIJson(v.input), `${v.name} parses`);
      continue;
    }

    const value = parseIJson(v.input);

    assert.equal(canonicalize(value), v.jcs, v.name);
    assert.deepEqual(canonicalize(JSON.parse(v.jcs)), v.jcs, `${v.name} is a fixed point`);

    if (v.signable) assert.equal(canonicalSigned(value), v.jcs, v.name);
    else assert.throws(() => canonicalSigned(value), v.name);
  }
});

test("ed25519: raw base64url keys sign and verify; anything else is false", () => {
  const k = generate();

  assert.equal(pubOf(k.seed), k.pub);
  assert.equal(fromB64u(k.seed, 32).length, 32);
  const sig = sign(k.seed, Buffer.from("hello"));

  assert.equal(fromB64u(sig, 64).length, 64);
  assert.ok(verify(k.pub, Buffer.from("hello"), sig));
  assert.ok(!verify(k.pub, Buffer.from("hellO"), sig));
  assert.ok(!verify(generate().pub, Buffer.from("hello"), sig));
  assert.ok(!verify("not a key", Buffer.from("hello"), sig));
  assert.equal(fromB64u(`${b64u(Buffer.alloc(32))}=`, 32), null, "padding is refused");
});

// ── 17.1 ─────────────────────────────────────────────────────────────────

test("MAN-01 the manifest signature is over JCS and verifies with the publisher key", () => {
  const m = signed();
  const unsigned = { ...m };

  delete unsigned.signature;
  assert.ok(verify(pub.pub, signedBytes(unsigned), m.signature.value));
  // key order and whitespace are not part of what is signed
  const shuffled = Object.fromEntries(Object.entries(m).reverse());
  const r = verifyManifest(JSON.stringify(shuffled, null, 3), trust);

  assert.equal(r.manifest.id, FX);
  assert.match(r.sha256, /^[0-9a-f]{64}$/);
  assert.equal(r.sha256, check(m).sha256);
});

test("MAN-02 a byte changed in the manifest fails verification (manifest_invalid)", () => {
  const m = signed();

  assert.throws(() => check({ ...m, title: "Fixturf" }), { code: "manifest_invalid" });
  const text = JSON.stringify(m).replace('"version":"1.0.0"', '"version":"1.0.1"');

  assert.throws(() => verifyManifest(text, trust), { code: "manifest_invalid" });
  // another publisher key, an unapproved id, a missing signature
  refused((x) => (x.id = "dev.goldcoders.unlisted"), "manifest_invalid", /approved/);
  assert.throws(() => check({ ...m, signature: undefined }), { code: "manifest_invalid" });
  const other = publisher(pub.keyId);

  assert.throws(() => check(signManifest(m, other.seed)), { code: "manifest_invalid", message: /does not verify/ });
});

test("MAN-03 non-object JSON is refused, not turned into {}", () => {
  for (const text of ["[]", "1", "null", '"x"', "", "{", '{"a":1,"a":2}'])
    assert.throws(() => verifyManifest(text, trust), { code: "manifest_invalid" }, text);
});

test("MAN-04 protocol 2.x and 1.1.0 are refused by a 1.0 host; 1.0.1 is accepted", () => {
  refused((m) => (m.protocol = "2.0.0"), "protocol_unsupported", /needs a Rox/);
  refused((m) => (m.protocol = "1.1.0"), "protocol_unsupported");
  refused((m) => (m.protocol = "0.9.0"), "protocol_unsupported");
  ok((m) => (m.protocol = "1.0.1"));
  refused((m) => (m.protocol = "1.0"), "manifest_invalid");
  refused((m) => (m.version = "v1"), "manifest_invalid");
  assert.equal(compareSemver("1.0.0-alpha", "1.0.0"), -1);
  assert.equal(compareSemver("1.10.0", "1.9.9"), 1);
});

test("MAN-05 a non-empty needs is refused (not_supported); [] is fine", () => {
  refused((m) => (m.needs = ["gmail"]), "not_supported");
  ok((m) => (m.needs = []));
});

test("MAN-06 an unknown key in permissions is refused; an unknown top-level key is ignored", () => {
  refused((m) => (m.apps[1].permissions.camera = true), "manifest_invalid", /camera/);
  refused((m) => (m.apps[0].permissions.open.intents = []), "manifest_invalid", /intents/);
  refused((m) => (m.apps[0].permissions.pick_file.paths = true), "manifest_invalid");
  refused((m) => (m.apps[0].permissions.preview.scripts = true), "manifest_invalid", /scripts/);
  refused((m) => (m.apps[0].permissions.preview = true), "manifest_invalid", /preview/);
  ok((m) => (m.x_future = { anything: 1 }));
});

const GRAMMAR = {
  pluginId: ["dev.goldcoders.bir", "dev.goldcoders"],
  short: ["bir", "b"],
  appId: ["tax-profile", "Tax"],
  command: ["taxform", "tax_form"],
  scope: ["branchcode", "-x"],
  routineId: ["bir-confirmations", "9lives"],
  tool: ["bir_resolve", "bir-resolve"],
  propKey: ["ticket", "_x"],
  ticket: ["tkt_0123456789ABCDEFGHJKMNPQRS", "tkt_0123456789ABCDEFGHIJKLMN"],
  canonicalToken: ["@bir:taxprofile:tkt_0123456789ABCDEFGHJKMNPQRS", "@bir:taxprofile:123456789"],
  reservedTool: ["rox.session.open", "rox_session"],
  publisherId: ["dev.goldcoders", "Dev"],
  keyId: ["gc-2026-01", "-gc"],
};

test("MAN-07 every grammar of section 3: one valid and one invalid value", () => {
  for (const [kind, [good, bad]] of Object.entries(GRAMMAR)) {
    assert.ok(isV1(kind, good), `${kind} ${good}`);
    assert.ok(!isV1(kind, bad), `${kind} ${bad}`);
  }

  assert.deepEqual(v1TypedTokens("see @bir:taxprofile:123-456 now"), [{ short: "bir", scope: "taxprofile", value: "123-456" }]);
  assert.deepEqual(v1TypedTokens("mail a@bir:taxprofile:1"), []);
  const slash = new RegExp(V1.slash);

  assert.deepEqual(slash.exec("/bir:taxform 1601C 2026-09").slice(1), ["bir", "taxform", "1601C 2026-09"]);
  assert.equal(slash.exec("/Tax"), null);
  // the same grammars refuse a manifest
  refused((m) => (m.id = "Dev.goldcoders.fixture"), "manifest_invalid");
  refused((m) => reshort(m, "f"), "manifest_invalid", /short/);
  refused((m) => (m.apps[1].id = "Card"), "manifest_invalid");
  refused((m) => (m.commands[0].name = "Board"), "manifest_invalid");
  refused((m) => (m.scopes[1].name = "no_te"), "manifest_invalid");
  refused((m) => (m.routines[0].id = "Digest"), "manifest_invalid");
  refused((m) => (m.apps[0].permissions.tools[0] = "fx-echo"), "manifest_invalid");
  refused((m) => (m.apps[1].props.properties["bad-key"] = { type: "string" }), "manifest_invalid");
  refused((m) => (m.publisher.id = "Goldcoders"), "manifest_invalid");
  refused((m) => (m.routines[0].reads[0].tools = ["search_mail!"]), "manifest_invalid");
  refused((m) => (m.short = "mcp"), "manifest_invalid", /reserved/);
  refused((m) => (m.id = "rox.goldcoders.fixture"), "manifest_invalid");
  refused((m) => (m.apps[0].permissions.tools.push("rox.provision")), "manifest_invalid", /reserved/);
  // a canonical token is what the typed token becomes
  assert.deepEqual(v1Canonical("@fx:item:tkt_0123456789ABCDEFGHJKMNPQRS"), { short: "fx", scope: "item", ticket: "tkt_0123456789ABCDEFGHJKMNPQRS" });
});

const at = (n, c = "x") => c.repeat(n);
const many = (n, f) => Array.from({ length: n }, (_, i) => f(i));

// [name, limit, edit(m, n)]: n = limit must pass, n = limit + 1 must fail
const LIMIT_CASES = [
  ["version chars", LIMITS.version, (m, n) => (m.version = `1.0.0-${at(n - 6, "a")}`)],
  ["title", LIMITS.title, (m, n) => (m.title = at(n))],
  ["description", LIMITS.description, (m, n) => (m.description = at(n))],
  ["publisher name", LIMITS.publisherName, (m, n) => (m.publisher.name = at(n))],
  ["short chars", 16, (m, n) => reshort(m, at(n, "f"))],
  ["tool name chars", 63, (m, n) => (m.commands[0].resolve = `fx_${at(n - 3, "r")}`)],
  ["commands", LIMITS.commands, (m, n) => (m.commands = many(n, (i) => ({ name: `c${i}`, description: "d", resolve: "fx_resolve" })))],
  ["command description", LIMITS.commandDescription, (m, n) => (m.commands[0].description = at(n))],
  ["command args", LIMITS.commandArgs, (m, n) => (m.commands[0].args = at(n))],
  ["scopes", LIMITS.scopes, (m, n) => (m.scopes = many(n, (i) => ({ name: i ? `s${i}` : "item", description: "d", resolve: "fx_resolve", attach: true })))],
  ["scope description", LIMITS.scopeDescription, (m, n) => (m.scopes[0].description = at(n))],
  ["scope hint", LIMITS.scopeHint, (m, n) => (m.scopes[0].hint = at(n))],
  ["routines", LIMITS.routines, (m, n) => (m.routines = many(n, (i) => ({ ...m.routines[0], id: `r${i}` })))],
  ["routine name", LIMITS.routineName, (m, n) => (m.routines[0].name = at(n))],
  ["routine description", LIMITS.routineDescription, (m, n) => (m.routines[0].description = at(n))],
  ["routine prompt", LIMITS.routinePrompt, (m, n) => (m.routines[0].prompt = at(n))],
  ["routine reads entries", LIMITS.reads, (m, n) => (m.routines[0].reads = many(n, () => ({ plugin: FX, tools: ["fx_search"] })))],
  ["routine reads tools", LIMITS.readsTools, (m, n) => (m.routines[0].reads = [{ plugin: FX, tools: many(Math.min(n, 4), (i) => `fx_a${i}`) }, { plugin: FX, tools: many(n - 4, (i) => `fx_b${i}`) }])],
  ["apps", LIMITS.apps, (m, n) => (m.apps = many(n, (i) => app(`a${i}`)))],
  ["app title", LIMITS.title, (m, n) => (m.apps[0].title = at(n))],
  ["app description", LIMITS.description, (m, n) => (m.apps[0].description = at(n))],
  ["permissions.tools", LIMITS.permTools, (m, n) => (m.apps[0].permissions.tools = many(n, (i) => `fx_t${i}`))],
  ["permissions.open.apps", LIMITS.openApps, (m, n) => (m.apps[0].permissions.open.apps = many(n, () => "card"))],
  ["permissions.open.external", LIMITS.openExternal, (m, n) => (m.apps[0].permissions.open.external = many(n, (i) => `https://e${i}.example.com`))],
  ["permissions.attach", LIMITS.attach, (m, n) => (m.apps[0].permissions.attach = many(n, () => "item"))],
  ["pick_file extensions", LIMITS.pickExtensions, (m, n) => (m.apps[0].permissions.pick_file.extensions = many(n, (i) => `.e${i}`))],
  ["pick_file max_bytes", LIMITS.pickMaxBytes, (m, n) => (m.apps[0].permissions.pick_file.max_bytes = n)],
  ["preview max_bytes", LIMITS.previewMaxBytes, (m, n) => (m.apps[0].permissions.preview.max_bytes = n)],
  ["storage quota_bytes", LIMITS.storageQuota, (m, n) => (m.apps[0].permissions.storage.quota_bytes = n)],
  ["props schema properties", LIMITS.schemaProperties, (m, n) => (m.apps[0].props.properties = Object.fromEntries(many(n, (i) => [`p${i}`, { type: "string" }])))],
  ["props schema maxLength", LIMITS.schemaMaxLength, (m, n) => (m.apps[0].props.properties.ticket.maxLength = n)],
  ["inline width", LIMITS.inlineWidth[1], (m, n) => (m.apps[1].inline.width = n)],
  ["inline height", LIMITS.inlineHeight[1], (m, n) => (m.apps[1].inline.height = n)],
  ["routine every_s", LIMITS.everyS[1], (m, n) => (m.routines[0].every_s = n)],
];

// [name, lowest allowed, edit]: the low end, and one under it
const LOWER_CASES = [
  ["short chars", 2, (m, n) => reshort(m, at(n, "f"))],
  ["inline width", LIMITS.inlineWidth[0], (m, n) => (m.apps[1].inline.width = n, m.apps[1].inline.min_width = Math.min(m.apps[1].inline.min_width, n))],
  ["inline height", LIMITS.inlineHeight[0], (m, n) => (m.apps[1].inline.height = n)],
  ["routine every_s", LIMITS.everyS[0], (m, n) => (m.routines[0].every_s = n)],
  ["title", 1, (m, n) => (m.title = at(n))],
];

test("MAN-08 every manifest limit of Appendix A, at the limit and one over", () => {
  for (const [what, limit, edit] of LIMIT_CASES) {
    assert.doesNotThrow(() => check(signed((m) => edit(m, limit))), `${what} at ${limit}`);
    assert.throws(() => check(signed((m) => edit(m, limit + 1))), { code: "manifest_invalid" }, `${what} at ${limit + 1}`);
  }

  for (const [what, low, edit] of LOWER_CASES) {
    assert.doesNotThrow(() => check(signed((m) => edit(m, low))), `${what} at ${low}`);
    assert.throws(() => check(signed((m) => edit(m, low - 1))), { code: "manifest_invalid" }, `${what} at ${low - 1}`);
  }

  // plugin id: 255 characters, then 256 (both on the approved list)
  ok((m) => (m.id = LONG_ID));
  refused((m) => (m.id = LONGER_ID), "manifest_invalid", /pluginId/);

  // manifest bytes: pad an ignored field to exactly 256 KiB, then one more
  const sized = (bytes) => {
    let m = signed((x) => (x.x_pad = ""));
    const base = Buffer.byteLength(JSON.stringify(m));

    m = signed((x) => (x.x_pad = at(bytes - base)));

    return JSON.stringify(m);
  };

  assert.equal(Buffer.byteLength(sized(LIMITS.manifestBytes)), LIMITS.manifestBytes);
  assert.doesNotThrow(() => verifyManifest(sized(LIMITS.manifestBytes), trust));
  assert.throws(() => verifyManifest(sized(LIMITS.manifestBytes + 1), trust), { code: "manifest_invalid", message: /bytes/ });
  // props at run time: 8 KiB, depth 8, 100 array items
  const schema = { type: "object" };
  const sizedProps = (n) => ({ s: at(n - Buffer.byteLength(JSON.stringify({ s: "" }))) });

  assert.doesNotThrow(() => validateProps(schema, sizedProps(LIMITS.propsBytes)));
  assert.throws(() => validateProps(schema, sizedProps(LIMITS.propsBytes + 1)), { code: "invalid_args" });
  const deep = (d) => (d === 1 ? {} : { a: deep(d - 1) });

  assert.doesNotThrow(() => validateProps(schema, deep(8)));
  assert.throws(() => validateProps(schema, deep(9)), { code: "invalid_args" });
  assert.doesNotThrow(() => validateProps(schema, { a: many(100, () => 1) }));
  assert.throws(() => validateProps(schema, { a: many(101, () => 1) }), { code: "invalid_args" });
});

test("manifest: a props schema outside the subset and non-integer numbers are refused", () => {
  refused((m) => (m.apps[0].props.properties.ticket.pattern = "^tkt_"), "manifest_invalid", /subset/);
  // signing refuses non-integers, and so does verification
  assert.throws(() => signed((m) => (m.apps[1].inline.width = 480.5)), /integers only/);
  const text = JSON.stringify(signed()).replace('"maximum":99', '"maximum":9.5');

  assert.throws(() => verifyManifest(text, trust), { code: "manifest_invalid", message: /integers only/ });
  refused((m) => (m.apps[1].entry = "nsh://fx/card"), "manifest_invalid", /rox-app/);
  refused((m) => (m.apps[0].permissions.attach = ["note"]), "manifest_invalid", /attach: true/);
  refused((m) => (m.apps[0].permissions.open.apps = ["elsewhere"]), "manifest_invalid");
  refused((m) => (m.apps[0].permissions.open.external = ["http://example.com"]), "manifest_invalid");
});

test("MAN-10 a tool name that does not start with <short>_ is refused", () => {
  refused((m) => (m.commands[0].resolve = "bir_resolve"), "manifest_invalid", /start with fx_/);
  refused((m) => (m.apps[0].permissions.tools[0] = "echo"), "manifest_invalid", /start with fx_/);
  refused((m) => (m.routines[0].query.tool = "gws_search"), "manifest_invalid");
  const m = check(signed()).manifest;
  const tools = ["fx_echo", "fx_slow", "fx_fail", "fx_big", "fx_resolve", "fx_open", "fx_search"].map((name) => ({ name }));

  assert.doesNotThrow(() => checkTools(m, [...tools, { name: "rox.provision" }]));
  assert.throws(() => checkTools(m, [...tools, { name: "search" }]), { code: "manifest_invalid", message: /start with fx_/ });
  assert.throws(() => checkTools(m, tools.slice(1)), { code: "manifest_invalid", message: /fx_echo/ });
});

test("MAN-11 a command named like a built-in (/status) is refused", () => {
  refused((m) => (m.commands[0].name = "status"), "manifest_invalid", /built-in/);
});

test("MAN-12 routines[].needs is refused as an unknown key", () => {
  refused((m) => (m.routines[0].needs = "gmail"), "manifest_invalid", /needs/);
  refused((m) => (m.routines[0].reads[0].scope = "all"), "manifest_invalid");
});

test("sign-manifest CLI: --key-file (0600 only) and --app fingerprints give a manifest that verifies", () => {
  const keyFile = join(dir, "publisher.key");
  const template = join(dir, "template.json");
  const out = join(dir, "signed.json");
  const m = JSON.parse(readFileSync(join(ROOT, "server", "v1", "fixture", "manifest.json"), "utf8"));
  const fx = join(ROOT, "server", "v1", "fixture");

  m.publisher.key_id = pub.keyId;
  writeFileSync(template, JSON.stringify(m));
  writeFileSync(keyFile, pub.seed, { mode: 0o644 });
  chmodSync(keyFile, 0o644);
  const run = () => execFileSync(process.execPath, [join(ROOT, "server", "v1", "sign-manifest.mjs"), template, "--key-file", keyFile, "--app", `board=${join(fx, "board.tsx")}`, "--app", `card=${join(fx, "card.tsx")}`, "--out", out], { stdio: "pipe" });

  assert.throws(run, /readable by others/);
  chmodSync(keyFile, 0o600);
  run();
  const r = verifyManifest(readFileSync(out, "utf8"), trust);

  assert.equal(r.manifest.apps[1].fingerprint, `sha256:${createHash("sha256").update(readFileSync(join(fx, "card.tsx"))).digest("hex")}`);
  assert.ok(!readFileSync(out, "utf8").includes(pub.seed), "the seed is not in the output");
});

test("the install review names a scope by its description's first word, without punctuation", () => {
  const m = {
    title: "T", version: "1.0.0", publisher: { name: "P" }, scopes: [{ name: "taxform", description: "Return: form and period", attach: true }],
    apps: [{ id: "a", title: "A", placement: ["full"], permissions: { attach: ["taxform"] } }],
  };

  assert.ok(reviewOf(m).lines.includes("  Can attach: Return"));
});
