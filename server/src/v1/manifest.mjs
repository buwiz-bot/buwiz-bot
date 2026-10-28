// v1/manifest.mjs — verifying a plugin's `rox://manifest` (spec 5).
//
// The broker trusts nothing a plugin says until its manifest passes, in
// the spec's order (5.2): I-JSON parse, protocol and version, trust store,
// publisher signature over JCS, then every field's grammar and limit
// (5.3-5.9, 6.6, Appendix A). A failure is a V1Error with the spec's code
// (manifest_invalid, protocol_unsupported, not_supported) and the field's
// path in the message, so the plugin author can find it. Checks that need
// more than the manifest (the plugin's tools/list, other installs) are
// separate functions the plugin manager calls. The same module answers
// the install review lines (13.1 step 3), the permissions digest and the
// props validation of 6.6.
import { createHash } from "node:crypto";
import { V1_RESERVED, isV1 } from "../grammar.mjs";
import { verify } from "./ed.mjs";
import { JcsError, canonicalize, canonicalSigned, isInt, isObject, isString, kindOf, parseIJson, signedBytes } from "./jcs.mjs";
import { isApproved } from "./trust.mjs";
import { V1Error } from "./errors.mjs";

export const HOST_PROTOCOL = "1.0.0";
export const APP_MIME = "text/x-nsh-tsx";
export const MANIFEST_URI = "rox://manifest";

// Appendix A, the limits a manifest is held to.
export const LIMITS = Object.freeze({
  manifestBytes: 256 * 1024,
  pluginId: 255,
  version: 32,
  title: 60,
  description: 300,
  commands: 32,
  scopes: 16,
  routines: 8,
  apps: 16,
  commandDescription: 200,
  commandArgs: 120,
  scopeDescription: 120,
  scopeHint: 1500,
  routineName: 80,
  routineDescription: 300,
  routinePrompt: 4000,
  publisherName: 80,
  permTools: 64,
  openApps: 16,
  openExternal: 8,
  attach: 16,
  pickExtensions: 16,
  pickMaxBytes: 50 * 1024 * 1024,
  storageQuota: 10 * 1024 * 1024,
  storageDefault: 1024 * 1024,
  previewMaxBytes: 8 * 1024 * 1024,
  propsBytes: 8 * 1024,
  propsDepth: 8,
  propsArray: 100,
  schemaProperties: 16,
  schemaMaxLength: 512,
  inlineWidth: [240, 960],
  inlineHeight: [96, 800],
  everyS: [60, 86400],
  reads: 4,
  readsTools: 8,
});

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;
const FINGERPRINT = /^sha256:[0-9a-f]{64}$/;
const SIG = /^[A-Za-z0-9_-]{86}$/;
const EXTENSION = /^\.[a-z0-9]{1,16}$/;

export const sha256hex = (s) => createHash("sha256").update(s).digest("hex");

/** sha256(JCS(permissions)), lowercase hex; absent permissions are `{}`. */
export const permsDigest = (perms) => sha256hex(canonicalSigned(perms ?? {}));

/** The manifest's identity hash: sha256 of its JCS bytes (5.2 step 7). */
export const manifestHash = (m) => sha256hex(canonicalSigned(m));

const chars = (s) => [...s].length;

/** -1, 0 or 1 comparing two semver strings by precedence (semver 2.0 11). */
export function compareSemver(a, b) {
  const pa = SEMVER.exec(a);
  const pb = SEMVER.exec(b);

  for (let i = 1; i <= 3; i++) if (Number(pa[i]) !== Number(pb[i])) return Number(pa[i]) < Number(pb[i]) ? -1 : 1;

  if (!pa[4] || !pb[4]) return pa[4] === pb[4] ? 0 : pa[4] ? -1 : 1;

  const xa = pa[4].split(".");
  const xb = pb[4].split(".");

  for (let i = 0; i < Math.max(xa.length, xb.length); i++) {
    if (xa[i] === undefined || xb[i] === undefined) return xa[i] === undefined ? -1 : 1;

    if (xa[i] === xb[i]) continue;
    const na = /^\d+$/.test(xa[i]);
    const nb = /^\d+$/.test(xb[i]);

    if (na && nb) return Number(xa[i]) < Number(xb[i]) ? -1 : 1;

    if (na !== nb) return na ? -1 : 1;

    return xa[i] < xb[i] ? -1 : 1;
  }

  return 0;
}

export const isSemver = (s) => isString(s) && SEMVER.test(s);

// ── field checks ─────────────────────────────────────────────────────────

const invalid = (path, why) => new V1Error("manifest_invalid", `${path}: ${why}`);

function text(v, path, { min = 0, max, required = true } = {}) {
  if (v === undefined && !required) return;

  if (!isString(v)) throw invalid(path, "must be a string");

  if (chars(v) < min) throw invalid(path, min === 1 ? "must not be empty" : `must be at least ${min} characters`);

  if (max !== undefined && chars(v) > max) throw invalid(path, `is over ${max} characters`);
}

function name(v, kind, path) {
  if (!isV1(kind, v)) throw invalid(path, `${JSON.stringify(v)?.slice(0, 80)} breaks the ${kind} grammar`);
}

function list(v, path, max, { required = false } = {}) {
  if (v === undefined && !required) return [];

  if (!Array.isArray(v)) throw invalid(path, "must be an array");

  if (v.length > max) throw invalid(path, `has ${v.length} entries; at most ${max}`);

  return v;
}

function obj(v, path) {
  if (!isObject(v)) throw invalid(path, "must be an object");

  return v;
}

// Objects that carry an access grant refuse unknown keys (2.2: fail closed).
function onlyKeys(o, allowed, path) {
  for (const k of Object.keys(o)) if (!allowed.includes(k)) throw invalid(`${path}.${k}`, "is not a known key here (refused: unknown keys in access grants fail closed)");
}

function int(v, path, lo, hi) {
  if (!isInt(v)) throw invalid(path, "must be an integer");

  if (v < lo || v > hi) throw invalid(path, `must be ${lo} to ${hi}`);
}

function unique(items, key, path) {
  const seen = new Set();

  for (const [i, x] of items.entries()) {
    if (seen.has(x[key])) throw invalid(`${path}[${i}].${key}`, `${JSON.stringify(x[key])} is not unique`);
    seen.add(x[key]);
  }
}

function ownTool(v, short, path) {
  name(v, "tool", path);

  if (!v.startsWith(`${short}_`)) throw invalid(path, `tool ${v} must start with ${short}_`);
}

// ── props schema subset (6.6) ────────────────────────────────────────────

const SCHEMA_KEYS = ["type", "properties", "required", "items", "enum", "maxLength", "minimum", "maximum"];
const SCHEMA_TYPES = ["object", "array", "string", "number", "integer", "boolean", "null"];

function schema(s, path, depth) {
  obj(s, path);

  if (depth > LIMITS.propsDepth) throw invalid(path, `nests deeper than ${LIMITS.propsDepth}`);

  for (const k of Object.keys(s)) if (!SCHEMA_KEYS.includes(k)) throw invalid(`${path}.${k}`, `is outside the props schema subset (${SCHEMA_KEYS.join(", ")})`);

  if (s.type !== undefined && !SCHEMA_TYPES.includes(s.type)) throw invalid(`${path}.type`, `must be one of ${SCHEMA_TYPES.join(", ")}`);

  if (s.properties !== undefined) {
    const props = Object.entries(obj(s.properties, `${path}.properties`));

    if (props.length > LIMITS.schemaProperties) throw invalid(`${path}.properties`, `has ${props.length} properties; at most ${LIMITS.schemaProperties}`);

    for (const [k, sub] of props) {
      name(k, "propKey", `${path}.properties`);
      schema(sub, `${path}.properties.${k}`, depth + 1);
    }
  }

  if (s.required !== undefined)
    for (const [i, r] of list(s.required, `${path}.required`, LIMITS.schemaProperties).entries())
      if (!isString(r) || !Object.hasOwn(s.properties || {}, r)) throw invalid(`${path}.required[${i}]`, "must name one of the properties");

  if (s.items !== undefined) schema(s.items, `${path}.items`, depth + 1);

  if (s.enum !== undefined)
    for (const [i, e] of list(s.enum, `${path}.enum`, LIMITS.propsArray, { required: true }).entries())
      if (["array", "object"].includes(kindOf(e))) throw invalid(`${path}.enum[${i}]`, "must be a scalar");

  if (s.maxLength !== undefined) int(s.maxLength, `${path}.maxLength`, 0, LIMITS.schemaMaxLength);

  for (const k of ["minimum", "maximum"]) if (s[k] !== undefined && !isInt(s[k])) throw invalid(`${path}.${k}`, "must be an integer");
}

// ── the sections ─────────────────────────────────────────────────────────

function publisher(p) {
  obj(p, "publisher");

  if (!isString(p.id) || !/^[a-z][a-z0-9-]{1,62}(\.[a-z0-9-]{1,62})*$/.test(p.id)) throw invalid("publisher.id", "breaks the publisher id grammar");
  name(p.key_id, "keyId", "publisher.key_id");
  text(p.name, "publisher.name", { min: 1, max: LIMITS.publisherName });
}

function commands(m) {
  const cs = list(m.commands, "commands", LIMITS.commands);

  for (const [i, c] of cs.entries()) {
    const at = `commands[${i}]`;

    obj(c, at);
    name(c.name, "command", `${at}.name`);

    if (V1_RESERVED.commands.includes(c.name)) throw invalid(`${at}.name`, `/${c.name} is a built-in command`);
    text(c.description, `${at}.description`, { max: LIMITS.commandDescription });
    text(c.args, `${at}.args`, { max: LIMITS.commandArgs, required: false });
    ownTool(c.resolve, m.short, `${at}.resolve`);
  }

  unique(cs, "name", "commands");
}

function scopes(m) {
  const ss = list(m.scopes, "scopes", LIMITS.scopes);

  for (const [i, s] of ss.entries()) {
    const at = `scopes[${i}]`;

    obj(s, at);
    name(s.name, "scope", `${at}.name`);
    text(s.description, `${at}.description`, { max: LIMITS.scopeDescription });
    ownTool(s.resolve, m.short, `${at}.resolve`);

    if (s.attach !== undefined && kindOf(s.attach) !== "boolean") throw invalid(`${at}.attach`, "must be a boolean");
    text(s.hint, `${at}.hint`, { max: LIMITS.scopeHint, required: false });
  }

  unique(ss, "name", "scopes");
}

function routines(m) {
  const rs = list(m.routines, "routines", LIMITS.routines);

  for (const [i, r] of rs.entries()) {
    const at = `routines[${i}]`;

    obj(r, at);
    // a routine carries an access grant (`reads`): unknown keys, v0's
    // `needs` among them, are refused (2.2, 5.9)
    onlyKeys(r, ["id", "name", "description", "every_s", "query", "prompt", "reads"], at);
    name(r.id, "routineId", `${at}.id`);
    text(r.name, `${at}.name`, { min: 1, max: LIMITS.routineName });
    text(r.description, `${at}.description`, { max: LIMITS.routineDescription });
    int(r.every_s, `${at}.every_s`, ...LIMITS.everyS);
    obj(r.query, `${at}.query`);
    onlyKeys(r.query, ["tool", "field", "args"], `${at}.query`);
    ownTool(r.query.tool, m.short, `${at}.query.tool`);
    text(r.query.field, `${at}.query.field`, { min: 1, max: 64 });

    if (r.query.args !== undefined) obj(r.query.args, `${at}.query.args`);
    text(r.prompt, `${at}.prompt`, { min: 1, max: LIMITS.routinePrompt });
    let tools = 0;

    for (const [j, x] of list(r.reads, `${at}.reads`, LIMITS.reads).entries()) {
      obj(x, `${at}.reads[${j}]`);
      onlyKeys(x, ["plugin", "tools"], `${at}.reads[${j}]`);
      name(x.plugin, "pluginId", `${at}.reads[${j}].plugin`);

      for (const [k, t] of list(x.tools, `${at}.reads[${j}].tools`, LIMITS.readsTools, { required: true }).entries()) name(t, "tool", `${at}.reads[${j}].tools[${k}]`);
      tools += x.tools.length;
    }

    if (tools > LIMITS.readsTools) throw invalid(`${at}.reads`, `names ${tools} tools; at most ${LIMITS.readsTools}`);
  }

  unique(rs, "id", "routines");
}

function permissions(p, m, at) {
  obj(p, at);
  onlyKeys(p, ["tools", "open", "attach", "pick_file", "storage", "copy", "preview"], at);

  for (const [i, t] of list(p.tools, `${at}.tools`, LIMITS.permTools).entries()) {
    if (isV1("reservedTool", t)) throw invalid(`${at}.tools[${i}]`, `${t} is reserved for Rox`);
    ownTool(t, m.short, `${at}.tools[${i}]`);
  }

  if (p.open !== undefined) {
    obj(p.open, `${at}.open`);
    onlyKeys(p.open, ["apps", "external"], `${at}.open`);
    const ids = (m.apps || []).map((a) => a?.id);

    for (const [i, a] of list(p.open.apps, `${at}.open.apps`, LIMITS.openApps).entries())
      if (!ids.includes(a)) throw invalid(`${at}.open.apps[${i}]`, `${JSON.stringify(a)?.slice(0, 60)} is not an app of this plugin`);

    for (const [i, o] of list(p.open.external, `${at}.open.external`, LIMITS.openExternal).entries()) {
      let ok = false;

      try {
        ok = isString(o) && new URL(o).protocol === "https:" && new URL(o).origin === o;
      } catch {}

      if (!ok) throw invalid(`${at}.open.external[${i}]`, "must be an https origin (https://host[:port], no path)");
    }
  }

  for (const [i, s] of list(p.attach, `${at}.attach`, LIMITS.attach).entries()) {
    const scope = (m.scopes || []).find((x) => x.name === s);

    if (!scope?.attach) throw invalid(`${at}.attach[${i}]`, `${JSON.stringify(s)?.slice(0, 60)} is not a scope with attach: true`);
  }

  if (p.pick_file !== undefined) {
    obj(p.pick_file, `${at}.pick_file`);
    onlyKeys(p.pick_file, ["extensions", "max_bytes", "multiple"], `${at}.pick_file`);

    for (const [i, e] of list(p.pick_file.extensions, `${at}.pick_file.extensions`, LIMITS.pickExtensions).entries())
      if (!isString(e) || !EXTENSION.test(e)) throw invalid(`${at}.pick_file.extensions[${i}]`, "must be like .xlsx");
    int(p.pick_file.max_bytes, `${at}.pick_file.max_bytes`, 1, LIMITS.pickMaxBytes);

    if (p.pick_file.multiple !== undefined && kindOf(p.pick_file.multiple) !== "boolean") throw invalid(`${at}.pick_file.multiple`, "must be a boolean");
  }

  if (p.storage !== undefined) {
    obj(p.storage, `${at}.storage`);
    onlyKeys(p.storage, ["quota_bytes"], `${at}.storage`);

    if (p.storage.quota_bytes !== undefined) int(p.storage.quota_bytes, `${at}.storage.quota_bytes`, 1, LIMITS.storageQuota);
  }

  if (p.copy !== undefined && kindOf(p.copy) !== "boolean") throw invalid(`${at}.copy`, "must be a boolean");

  if (p.preview !== undefined) {
    obj(p.preview, `${at}.preview`);
    onlyKeys(p.preview, ["max_bytes"], `${at}.preview`);

    if (p.preview.max_bytes !== undefined) int(p.preview.max_bytes, `${at}.preview.max_bytes`, 1, LIMITS.previewMaxBytes);
  }
}

function apps(m) {
  const as = list(m.apps, "apps", LIMITS.apps);

  for (const [i, a] of as.entries()) {
    const at = `apps[${i}]`;

    obj(a, at);
    name(a.id, "appId", `${at}.id`);
    text(a.title, `${at}.title`, { min: 1, max: LIMITS.title });
    text(a.description, `${at}.description`, { max: LIMITS.description });

    if (a.entry !== `rox-app://${a.id}`) throw invalid(`${at}.entry`, `must be rox-app://${a.id}`);

    if (a.mime !== APP_MIME) throw invalid(`${at}.mime`, `must be ${APP_MIME}`);

    if (a.sdk !== "1") throw invalid(`${at}.sdk`, 'must be "1"');

    if (!isString(a.fingerprint) || !FINGERPRINT.test(a.fingerprint)) throw invalid(`${at}.fingerprint`, "must be sha256:<64 lowercase hex>");

    if (a.props !== undefined) {
      schema(a.props, `${at}.props`, 1);

      if (a.props.type !== "object") throw invalid(`${at}.props.type`, 'must be "object"');
    }

    const place = list(a.placement, `${at}.placement`, 2, { required: true });

    if (!place.length || place.some((p) => !["full", "inline"].includes(p)) || new Set(place).size !== place.length)
      throw invalid(`${at}.placement`, 'must be ["full"], ["inline"] or ["full","inline"]');

    if (place.includes("inline")) {
      obj(a.inline, `${at}.inline`);
      int(a.inline.width, `${at}.inline.width`, ...LIMITS.inlineWidth);
      int(a.inline.height, `${at}.inline.height`, ...LIMITS.inlineHeight);

      if (a.inline.min_width !== undefined) int(a.inline.min_width, `${at}.inline.min_width`, LIMITS.inlineWidth[0], a.inline.width);

      // rox.fit's range (6.5): min_height <= height <= max_height
      if (a.inline.min_height !== undefined) int(a.inline.min_height, `${at}.inline.min_height`, LIMITS.inlineHeight[0], a.inline.height);

      if (a.inline.max_height !== undefined) int(a.inline.max_height, `${at}.inline.max_height`, a.inline.height, LIMITS.inlineHeight[1]);

      if (a.inline.resizable !== undefined && kindOf(a.inline.resizable) !== "boolean") throw invalid(`${at}.inline.resizable`, "must be a boolean");
    }

    if (a.permissions !== undefined) permissions(a.permissions, m, `${at}.permissions`);
  }

  unique(as, "id", "apps");
}

function fields(m) {
  name(m.id, "pluginId", "id");

  if (V1_RESERVED.idPrefixes.some((p) => m.id.startsWith(p))) throw invalid("id", `${m.id} uses a prefix reserved for Rox`);
  name(m.short, "short", "short");

  if (V1_RESERVED.shorts.includes(m.short)) throw invalid("short", `${m.short} is reserved`);
  text(m.title, "title", { min: 1, max: LIMITS.title });
  text(m.description, "description", { max: LIMITS.description, required: false });
  text(m.icon, "icon", { min: 1, max: 2048, required: false });
  commands(m);
  scopes(m);
  routines(m);
  apps(m);
}

// ── verification (5.2) ───────────────────────────────────────────────────

function protocolOf(m, host) {
  if (!isSemver(m.protocol)) throw invalid("protocol", "must be a semver string");
  const [maj, min] = m.protocol.split(".").map(Number);
  const [hmaj, hmin] = host.split(".").map(Number);

  if (maj !== hmaj || min > hmin)
    throw new V1Error("protocol_unsupported", `the plugin implements protocol ${m.protocol}; this Rox supports ${hmaj}.${hmin}, so it needs a Rox that supports ${maj}.${min}`);

  if (!isSemver(m.version) || m.version.length > LIMITS.version) throw invalid("version", `must be semver, at most ${LIMITS.version} characters`);
}

function signatureOf(m, trust) {
  publisher(m.publisher);
  const key = trust.publishers.get(m.publisher.key_id);

  if (!key) throw invalid("publisher.key_id", `${m.publisher.key_id} is not a trusted publisher key`);

  if (key.publisher && key.publisher !== m.publisher.id) throw invalid("publisher.id", `key ${m.publisher.key_id} belongs to ${key.publisher}`);

  if (!isString(m.id) || !isApproved(trust, m.id, m.publisher.key_id)) throw invalid("id", `${String(m.id).slice(0, 80)} is not on the approved list for key ${m.publisher.key_id}`);

  const s = obj(m.signature, "signature");

  if (s.alg !== "ed25519") throw invalid("signature.alg", 'must be "ed25519"');

  if (s.key_id !== m.publisher.key_id) throw invalid("signature.key_id", "must equal publisher.key_id");

  if (!isString(s.value) || !SIG.test(s.value)) throw invalid("signature.value", "must be 64 bytes of base64url");

  const unsigned = { ...m };

  delete unsigned.signature;
  let bytes;

  try {
    bytes = signedBytes(unsigned);
  } catch (e) {
    throw invalid("manifest", e instanceof JcsError ? e.message : "cannot be canonicalized");
  }

  if (!verify(key.public_key, bytes, s.value)) throw invalid("signature", "does not verify with the publisher key");
}

/** Verifies manifest text against `trust` (spec 5.2 steps 1-5).
 *  → {manifest, sha256}; throws V1Error otherwise. */
export function verifyManifest(textIn, trust, host = HOST_PROTOCOL) {
  const raw = String(textIn ?? "");

  if (Buffer.byteLength(raw, "utf8") > LIMITS.manifestBytes) throw invalid("manifest", `is over ${LIMITS.manifestBytes} bytes`);
  let m;

  try {
    m = parseIJson(raw);
  } catch (e) {
    throw invalid("manifest", `is not I-JSON: ${e.message}`);
  }

  if (!isObject(m)) throw invalid("manifest", `must be a JSON object, not ${kindOf(m)}`);
  protocolOf(m, host);
  signatureOf(m, trust);

  if (m.needs !== undefined && !(Array.isArray(m.needs) && !m.needs.length))
    throw new V1Error("not_supported", "needs: shared services are not supported in v1 (5.3); it must be absent or []");
  fields(m);

  return { manifest: m, sha256: manifestHash(m) };
}

// ── checks against the running plugin ────────────────────────────────────

/** The plugin's tools/list against its manifest: every tool but the
 *  reserved `rox.*` follows the grammar and starts with `<short>_`
 *  (MAN-10), and every tool the manifest names exists. */
export function checkTools(m, tools) {
  const names = new Set();

  for (const t of tools) {
    if (isV1("reservedTool", t.name)) continue;

    if (!isV1("tool", t.name) || !t.name.startsWith(`${m.short}_`)) throw invalid("tools/list", `tool ${String(t.name).slice(0, 80)} must follow the tool grammar and start with ${m.short}_`);
    names.add(t.name);
  }

  const named = [
    ...(m.commands || []).map((c) => [c.resolve, "commands"]),
    ...(m.scopes || []).map((s) => [s.resolve, "scopes"]),
    ...(m.routines || []).map((r) => [r.query.tool, "routines"]),
    ...(m.apps || []).flatMap((a) => (a.permissions?.tools || []).map((t) => [t, `apps.${a.id}.permissions`])),
  ];

  for (const [t, where] of named) if (!names.has(t)) throw invalid(where, `names ${t}, which the plugin's tools/list does not have`);
}

// ── props (6.6) ──────────────────────────────────────────────────────────

const bad = (why) => new V1Error("invalid_args", `props: ${why}`);

function propsLimits(v, depth, path) {
  if (depth > LIMITS.propsDepth) throw bad(`${path} nests deeper than ${LIMITS.propsDepth}`);
  const k = kindOf(v);

  if (k === "array") {
    if (v.length > LIMITS.propsArray) throw bad(`${path} has over ${LIMITS.propsArray} items`);

    for (const [i, x] of v.entries()) propsLimits(x, depth + 1, `${path}[${i}]`);
  } else if (k === "object") {
    for (const [key, x] of Object.entries(v)) {
      if (!isV1("propKey", key)) throw bad(`key ${JSON.stringify(key).slice(0, 40)} breaks the prop key grammar`);
      propsLimits(x, depth + 1, `${path}.${key}`);
    }
  } else if (k === "other") throw bad(`${path} is not JSON`);
}

function typeOk(type, v) {
  const k = kindOf(v);

  if (type === "integer") return k === "number" && Number.isInteger(v);

  return type === k;
}

function propsMatch(s, v, path) {
  if (s.type !== undefined && !typeOk(s.type, v)) throw bad(`${path} must be ${s.type}`);

  if (s.enum && !s.enum.some((e) => canonicalize(e) === canonicalize(v))) throw bad(`${path} is not one of the allowed values`);

  if (s.maxLength !== undefined && isString(v) && chars(v) > s.maxLength) throw bad(`${path} is over ${s.maxLength} characters`);

  if (s.minimum !== undefined && kindOf(v) === "number" && v < s.minimum) throw bad(`${path} is under ${s.minimum}`);

  if (s.maximum !== undefined && kindOf(v) === "number" && v > s.maximum) throw bad(`${path} is over ${s.maximum}`);

  if (isObject(v)) {
    for (const r of s.required || []) if (!Object.hasOwn(v, r)) throw bad(`${path}.${r} is required`);

    for (const [key, sub] of Object.entries(s.properties || {})) if (Object.hasOwn(v, key)) propsMatch(sub, v[key], `${path}.${key}`);
  }

  if (Array.isArray(v) && s.items) for (const [i, x] of v.entries()) propsMatch(s.items, x, `${path}[${i}]`);
}

/** Props for an open of an app with props schema `s` (6.6): an object,
 *  at most 8 KiB, depth 8, 100 items per array, prop-key grammar, and
 *  valid against the schema subset. Throws V1Error invalid_args. */
export function validateProps(s, props) {
  if (!isObject(props)) throw bad("must be a JSON object");
  let size;

  try {
    size = Buffer.byteLength(canonicalize(props), "utf8");
  } catch (e) {
    throw bad(e.message);
  }

  if (size > LIMITS.propsBytes) throw bad(`is ${size} bytes; at most ${LIMITS.propsBytes}`);
  propsLimits(props, 1, "props");

  if (s) propsMatch(s, props, "props");
}

// ── install review (13.1 step 3) and update consent (13.2) ───────────────

const mb = (n) => (n >= 1024 * 1024 ? `${Math.round((n / 1048576) * 10) / 10} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);

// the description's first word (5.8) without trailing punctuation ("Return:")
const noun = (s) => String(s?.description || s?.name || "").trim().split(/\s+/)[0].replace(/[^\p{L}\p{N}]+$/u, "") || s?.name;

function permissionLines(m, p) {
  if (!p) return [];
  const lines = [];
  const title = (id) => m.apps.find((a) => a.id === id)?.title || id;

  if (p.tools?.length) lines.push(`Can call ${p.tools.length} ${m.title} tool${p.tools.length === 1 ? "" : "s"}: ${p.tools.join(", ")}`);

  if (p.open?.apps?.length) lines.push(`Can open: ${p.open.apps.map(title).join(", ")}`);

  if (p.open?.external?.length) lines.push(`Can open in your browser (asks each time): ${p.open.external.join(", ")}`);

  if (p.attach?.length) lines.push(`Can attach: ${p.attach.map((s) => noun(m.scopes.find((x) => x.name === s))).join(", ")}`);

  if (p.pick_file) {
    const f = p.pick_file;

    lines.push(`Can read files: ${(f.extensions || []).join(", ") || "any type"} up to ${mb(f.max_bytes)}${f.multiple ? ", several at once" : ""}`);
  }

  if (p.storage) lines.push(`Storage: ${mb(p.storage.quota_bytes ?? LIMITS.storageDefault)}`);

  if (p.copy) lines.push("Copy to clipboard");

  if (p.preview) lines.push("Show print previews (static pages, no scripts)");

  return lines;
}

/** What the install screen shows (13.1 step 3). `titleOf(pluginId)` names
 *  another plugin a routine reads from. */
export function reviewOf(m, titleOf = (id) => id) {
  const apps = (m.apps || []).map((a) => ({
    id: a.id,
    title: a.title,
    placement: a.placement,
    inline: a.placement.includes("inline") ? { width: a.inline.width, height: a.inline.height } : null,
    lines: permissionLines(m, a.permissions),
  }));

  const routines = (m.routines || []).flatMap((r) => (r.reads || []).map((x) => `Routine '${r.name}' reads via ${titleOf(x.plugin)}: ${x.tools.join(", ")}`));
  const head = [`${m.title} ${m.version} by ${m.publisher.name}`, ...(m.description ? [m.description] : [])];

  const appLines = apps.flatMap((a) => [
    `${a.title} (${a.placement.map((p) => (p === "inline" ? `inline ${a.inline.width}×${a.inline.height}` : "full screen")).join(", ")})`,
    ...a.lines.map((l) => `  ${l}`),
  ]);

  return {
    title: m.title,
    publisher: m.publisher.name,
    version: m.version,
    description: m.description || "",
    apps,
    routines,
    lines: [...head, ...appLines, ...routines],
  };
}

const added = (a = [], b = []) => b.filter((x) => !a.includes(x));

/** The permissions `next` grants that `prev` did not (13.2: any added key
 *  or entry needs consent; removals do not). Plain lines; [] = none. */
export function addedPermissions(prev, next) {
  const out = [];

  for (const app of next.apps || []) {
    const was = (prev.apps || []).find((a) => a.id === app.id)?.permissions || {};
    const now = app.permissions || {};
    const at = (s) => out.push(`${app.title}: ${s}`);

    for (const t of added(was.tools, now.tools)) at(`can call ${t}`);

    for (const x of added(was.open?.apps, now.open?.apps)) at(`can open ${x}`);

    for (const x of added(was.open?.external, now.open?.external)) at(`can open ${x} in the browser`);

    for (const x of added(was.attach, now.attach)) at(`can attach ${x}`);

    if (now.pick_file) {
      const w = was.pick_file;

      if (!w) at("can read files");
      else {
        for (const x of added(w.extensions, now.pick_file.extensions)) at(`can read ${x} files`);

        if (now.pick_file.max_bytes > w.max_bytes) at(`can read files up to ${mb(now.pick_file.max_bytes)}`);

        if (now.pick_file.multiple && !w.multiple) at("can read several files at once");
      }
    }

    if (now.storage && (!was.storage || (now.storage.quota_bytes ?? LIMITS.storageDefault) > (was.storage.quota_bytes ?? LIMITS.storageDefault)))
      at(`storage ${mb(now.storage.quota_bytes ?? LIMITS.storageDefault)}`);

    if (now.copy && !was.copy) at("copy to clipboard");

    if (now.preview && !was.preview) at("show print previews");
    else if (now.preview && (now.preview.max_bytes ?? LIMITS.previewMaxBytes) > (was.preview.max_bytes ?? LIMITS.previewMaxBytes))
      at(`print previews up to ${mb(now.preview.max_bytes ?? LIMITS.previewMaxBytes)}`);
  }

  for (const r of next.routines || []) {
    const was = (prev.routines || []).find((x) => x.id === r.id);

    for (const x of r.reads || []) {
      const had = was?.reads?.find((y) => y.plugin === x.plugin)?.tools;

      for (const t of added(had, x.tools)) out.push(`Routine '${r.name}' reads ${x.plugin}: ${t}`);
    }
  }

  return out;
}
