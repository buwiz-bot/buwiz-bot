// v1/sessions.mjs — app opens and the calls they make (spec 4.4, 7.3, 8, 9.2).
//
// Rox opens an app by asking the broker for a session: the broker relays
// `rox.session.open` to the plugin, verifies the certificate with the
// plugin key from the install record, and keeps it. From then on every
// bridge call arrives as Rox's signed envelope and passes the nine checks
// of 8.2, in the spec's order, before anything reaches the plugin; the
// plugin then sees `_meta["rox/caller"]` (8.4). The broker is the gate for
// calls, so a check failing here means nothing is sent. Ticket titles,
// resolve tools and app sources go through the same module because they
// share its validation of plugin answers.
import { isV1 } from "../grammar.mjs";
import { fromB64u, verify } from "./ed.mjs";
import { refuse } from "./errors.mjs";
import { JcsError, canonicalize, isInt, isObject, isString, signedBytes } from "./jcs.mjs";
import { fingerprintOf } from "./signing.mjs";
import { LIMITS, permsDigest, validateProps } from "./manifest.mjs";
import { resultData } from "./plugins.mjs";

export const SKEW_MS = 60_000;
export const NONCE_WINDOW_MS = 5 * 60_000;
export const NONCE_MAX = 10_000;
export const CERT_MAX_MS = 8 * 3600_000;
export const MAX_INFLIGHT = 8;
export const MAX_PER_SECOND = 20;
export const ARGS_MAX = 256 * 1024;
export const RESULT_MAX = 1024 * 1024;
// a rox.preview call's result (the document); permissions.preview.max_bytes may lower it
export const PREVIEW_MAX = LIMITS.previewMaxBytes;
export const SOURCE_MAX = 512 * 1024;
const HEX64 = /^[0-9a-f]{64}$/;

const cap = (s, n) => {
  const cs = [...String(s ?? "")];

  return cs.length > n ? cs.slice(0, n).join("") : cs.join("");
};

const firstText = (r) => (r?.content || []).find((c) => c.type === "text")?.text || "";

/** A tool result's size as 7.3 counts it: structured plus text (and an
 *  embedded resource's text or blob), in bytes. */
export function resultBytes(r) {
  const piece = (c) => (c.type === "text" ? Buffer.byteLength(c.text || "") : c.type === "resource" ? Buffer.byteLength(String(c.resource?.text ?? c.resource?.blob ?? "")) : 0);
  const text = (r?.content || []).reduce((n, c) => n + piece(c || {}), 0);

  return text + (r?.structuredContent === undefined ? 0 : Buffer.byteLength(JSON.stringify(r.structuredContent)));
}

/** A tool result without `_meta` (8.4, 9.6: never shown past the broker). */
export function stripMeta(r) {
  const out = { ...r };

  delete out._meta;

  return out;
}

/** An open request a plugin made (resolve `open`, `_meta["rox/open"]`)
 *  checked against its own manifest `m`: the app is one of its apps (no
 *  other plugin's), props validate (6.6), the ticket follows the grammar.
 *  → the normalized open; throws V1Error. */
export function checkOpen(m, o) {
  if (!isObject(o)) refuse("invalid_args", "an open must be an object");

  if (o.plugin !== undefined && o.plugin !== m.id) refuse("not_declared", `an open of another plugin's app (${String(o.plugin).slice(0, 80)}) is refused`);
  const app = (m.apps || []).find((a) => a.id === o.app);

  if (!app) refuse("not_declared", `${String(o.app).slice(0, 60)} is not an app of ${m.id}`);

  if (o.ticket !== undefined && !isV1("ticket", o.ticket)) refuse("invalid_args", "ticket breaks the ticket grammar");

  if (o.placement !== undefined && !app.placement.includes(o.placement)) refuse("invalid_args", `${app.id} does not declare placement ${String(o.placement).slice(0, 20)}`);

  // 9.8: an answer's small card; anything else is its main card
  if (o.size !== undefined && o.size !== "small" && o.size !== "main") refuse("invalid_args", "size must be small or main");
  const props = o.props ?? {};

  validateProps(app.props, props);
  const out = { plugin: m.id, app: app.id, props, title: cap(isString(o.title) && o.title ? o.title : app.title, 60), placement: o.placement || app.placement[0] };

  if (o.ticket !== undefined) out.ticket = o.ticket;

  if (o.size === "small") out.size = "small";

  return out;
}

export function createSessions(plugins) {
  const clock = plugins.clock;
  const opens = new Map(); // open_id → session

  // ── open / close (4.4) ───────────────────────────────────────────────

  function checkOpenRequest(b) {
    if (!isObject(b)) refuse("invalid_args", "send {plugin, app, open_id, ks_pub, perms_sha256, not_after}");

    if (!isV1("pluginId", b.plugin) || !isV1("appId", b.app)) refuse("invalid_args", "plugin and app must be ids");

    if (!fromB64u(b.open_id, 16)) refuse("invalid_args", "open_id must be 128 bits of base64url");

    if (!fromB64u(b.ks_pub, 32)) refuse("invalid_args", "ks_pub must be a 32-byte ed25519 key in base64url");

    if (!HEX64.test(String(b.perms_sha256))) refuse("invalid_args", "perms_sha256 must be 64 lowercase hex");
    const now = clock.now();

    if (!isInt(b.not_after) || b.not_after <= now || b.not_after > now + CERT_MAX_MS) refuse("invalid_args", "not_after must be in the next 8 hours (unix ms)");

    if (opens.has(b.open_id)) refuse("invalid_args", "open_id is already open");
  }

  function certMatches(cert, b) {
    return isObject(cert) && cert.v === 1 && cert.typ === "rox-session" && cert.plugin === b.plugin && cert.app === b.app && cert.open_id === b.open_id
      && cert.ks_pub === b.ks_pub && cert.perms_sha256 === b.perms_sha256 && cert.not_after === b.not_after;
  }

  /** POST /v1/host/sessions/open → {cert, cert_sig, plugin_pub, permissions}. */
  async function open(b) {
    checkOpenRequest(b);
    const row = plugins.rowOf(b.plugin);

    if (!row) refuse("not_found", `${b.plugin} is not installed`);
    const rt = plugins.runtime(b.plugin);

    if (!row.enabled || rt.state !== "running") refuse("plugin_unavailable", `${b.plugin} is not running`);
    const app = (plugins.manifestOf(row).apps || []).find((a) => a.id === b.app);

    if (!app) refuse("not_found", `${b.plugin} has no app ${b.app}`);

    if (permsDigest(app.permissions) !== b.perms_sha256) refuse("invalid_args", "perms_sha256 is not the digest of the installed app's permissions");
    const generation = rt.generation;
    const res = await plugins.callReserved(b.plugin, "rox.session.open", { app: b.app, open_id: b.open_id, ks_pub: b.ks_pub, perms_sha256: b.perms_sha256, not_after: b.not_after });

    if (res === null) {
      await plugins.fail(b.plugin, "plugin_invalid", "the plugin does not implement rox.session.open");
      refuse("plugin_invalid", "the plugin does not implement rox.session.open");
    }

    let good = false;

    try {
      good = certMatches(res?.cert, b) && verify(row.plugin_pub, signedBytes(res.cert), res.cert_sig);
    } catch {}

    if (!good) {
      await plugins.fail(b.plugin, "plugin_bad_signature", "a session certificate failed verification");
      refuse("plugin_bad_signature", "the plugin's session certificate does not verify");
    }

    opens.set(b.open_id, {
      plugin: b.plugin, app: b.app, open_id: b.open_id, cert: res.cert, cert_sig: res.cert_sig, generation,
      nonces: new Map(), active: 0, queue: [], recent: [],
    });

    return { ok: true, cert: res.cert, cert_sig: res.cert_sig, plugin_pub: row.plugin_pub, permissions: app.permissions || {} };
  }

  /** POST /v1/host/sessions/close: forget the open; tell the plugin (best effort). */
  async function close(b) {
    const s = opens.get(b?.open_id);

    if (!s) return { ok: true };
    opens.delete(s.open_id);

    if (plugins.runtime(s.plugin).state === "running") await plugins.callReserved(s.plugin, "rox.session.close", { open_id: s.open_id }).catch(() => {});

    return { ok: true };
  }

  // a plugin's opens end with it (disable, uninstall, update: 13.3-13.6)
  plugins.onClose(async (pluginId) => {
    for (const s of opens.values()) if (s.plugin === pluginId) await close({ open_id: s.open_id });
  });

  // ── the call (8.2) ───────────────────────────────────────────────────

  function checkNonce(s, nonce, now) {
    for (const [n, at] of s.nonces) {
      if (now - at <= NONCE_WINDOW_MS) break;
      s.nonces.delete(n);
    }

    if (s.nonces.has(nonce)) refuse("replayed", "this envelope was already sent");
    s.nonces.set(nonce, now);

    while (s.nonces.size > NONCE_MAX) s.nonces.delete(s.nonces.keys().next().value);
  }

  // checks 1 to 9, in order; → {s, row, rt, app}
  function check(envelope, sig) {
    const now = clock.now();
    const s = opens.get(envelope.open_id);
    const row = s && plugins.rowOf(s.plugin);

    // 1: a registered open whose certificate verifies and has not expired
    if (!s || !row || !verify(row.plugin_pub, signedBytes(s.cert), s.cert_sig)) refuse("session_invalid", "no open with a valid certificate has this open_id");

    if (s.cert.not_after <= now) {
      opens.delete(s.open_id);
      refuse("session_invalid", "the session certificate expired");
    }

    // 2
    if (envelope.v !== 1 || envelope.typ !== "rox-call" || envelope.plugin !== s.cert.plugin || envelope.app !== s.cert.app || envelope.open_id !== s.cert.open_id)
      refuse("session_invalid", "the envelope names another plugin, app or open");

    // 3
    if (!verify(s.cert.ks_pub, signedBytes(envelope), sig)) refuse("bad_signature", "the envelope's signature does not verify with the session key");

    // 4
    if (!isInt(envelope.ts) || Math.abs(now - envelope.ts) > SKEW_MS) refuse("stale", "the envelope's time is more than 60 s from the broker's");

    // 5
    if (!fromB64u(envelope.nonce, 16)) refuse("invalid_args", "nonce must be 128 bits of base64url");
    checkNonce(s, envelope.nonce, now);
    // 6
    const rt = plugins.runtime(s.plugin);

    if (s.generation !== rt.generation) refuse("plugin_restarted", "the plugin restarted since this open began");
    // 7: the installed permissions, and the certificate covers exactly them
    const app = (plugins.manifestOf(row).apps || []).find((a) => a.id === s.app);
    const perms = app?.permissions || {};

    if (!app || permsDigest(perms) !== s.cert.perms_sha256 || !(perms.tools || []).includes(envelope.tool)) refuse("permission_denied", `${String(envelope.tool).slice(0, 80)} is not in this app's permissions`);

    // 8
    if (plugins.disabledOf(row).has(envelope.tool)) refuse("disabled", `${envelope.tool} is switched off`);

    // 9
    if (!row.enabled || rt.state !== "running") refuse("plugin_unavailable", `${s.plugin} is not running`);

    return { s, rt, perms };
  }

  function rate(s) {
    const now = clock.now();

    while (s.recent.length && now - s.recent[0] >= 1000) s.recent.shift();

    if (s.recent.length >= MAX_PER_SECOND) refuse("rate_limited", `more than ${MAX_PER_SECOND} calls in a second`);
    s.recent.push(now);
  }

  async function slot(s) {
    if (s.active < MAX_INFLIGHT) {
      s.active++;

      return;
    }

    await new Promise((resolve) => s.queue.push(resolve));
  }

  function release(s) {
    const next = s.queue.shift();

    if (next) next();
    else s.active--;
  }

  /** POST /v1/host/call {envelope, sig} → {ok, result} with `_meta` stripped. */
  async function call(b) {
    if (!isObject(b) || !isObject(b.envelope) || !isString(b.sig)) refuse("invalid_args", "send {envelope, sig}");
    const { envelope, sig } = b;

    try {
      signedBytes(envelope);
    } catch (e) {
      refuse("invalid_args", e instanceof JcsError ? e.message : "the envelope cannot be canonicalized");
    }

    const { s, rt, perms } = check(envelope, sig);
    const args = envelope.args ?? {};
    // rox.preview: Rox calls the app's own tool for a document; only an app
    // with permissions.preview gets the larger result limit
    const preview = envelope.purpose !== undefined;

    if (preview && envelope.purpose !== "preview") refuse("invalid_args", "purpose must be \"preview\" when present");

    if (preview && !isObject(perms.preview)) refuse("permission_denied", "this app has no preview permission");
    const max = preview ? Math.min(perms.preview.max_bytes ?? PREVIEW_MAX, PREVIEW_MAX) : RESULT_MAX;

    if (!isObject(args)) refuse("invalid_args", "args must be a JSON object");

    if (Buffer.byteLength(canonicalize(args)) > ARGS_MAX) refuse("invalid_args", `args are over ${ARGS_MAX} bytes`);

    if (!rt.tools.some((t) => t.name === envelope.tool)) refuse("not_found", `${s.plugin} has no tool ${envelope.tool}`);
    rate(s);
    await slot(s);
    let r;

    try {
      const caller = { origin: "app", plugin: s.plugin, app: s.app, open_id: s.open_id, ts: envelope.ts };

      r = await plugins.callTool(s.plugin, envelope.tool, args, { meta: { "rox/caller": caller } });
    } finally {
      release(s);
    }

    if (r.isError) refuse("tool_error", cap(firstText(r) || `${envelope.tool} failed`, 500));

    if (resultBytes(r) > max) refuse("result_too_large", `${envelope.tool}'s result is over ${preview ? `${Math.round((max / 1048576) * 10) / 10} MiB` : "1 MiB"}`);

    return { ok: true, result: stripMeta(r) };
  }

  // ── tickets, resolve, app source ─────────────────────────────────────

  function running(pluginId) {
    const row = plugins.rowOf(pluginId);

    if (!row) refuse("not_found", `${String(pluginId).slice(0, 80)} is not installed`);

    if (!row.enabled || plugins.runtime(pluginId).state !== "running") refuse("plugin_unavailable", `${pluginId} is not running`);

    return row;
  }

  /** POST /v1/host/tickets/title {plugin, ticket} → {valid, title?} (7.4). */
  async function ticketTitle(b) {
    if (!isV1("ticket", b?.ticket)) refuse("invalid_args", "ticket breaks the ticket grammar");
    running(b.plugin);
    const res = await plugins.callReserved(b.plugin, "rox.ticket.resolve", { ticket: b.ticket });

    if (res === null) refuse("not_supported", `${b.plugin} does not resolve tickets`);

    // a title and nothing else crosses: never a subject (P3)
    if (res?.valid !== true || !isString(res.title)) return { ok: true, valid: false };

    return { ok: true, valid: true, title: cap(res.title, 120) };
  }

  function choices(list) {
    if (!Array.isArray(list)) refuse("plugin_invalid", "choices must be an array");

    return list.slice(0, 20).map((c) => {
      if (!isObject(c) || !isString(c.label) || !isString(c.value)) refuse("plugin_invalid", "each choice needs a label and a value");
      const out = { label: cap(c.label, 80), value: cap(c.value, 512) };

      if (isString(c.detail)) out.detail = cap(c.detail, 160);

      return out;
    });
  }

  // a resolve tool's answer as 9.2 allows it
  function resolved(m, kind, name, data) {
    const r = data?.result;

    if (r === "choices") return { result: "choices", choices: choices(data.choices) };

    if (r === "open") return { result: "open", ...checkOpen(m, data) };

    if (r === "error") return { result: "error", message: cap(isString(data.message) ? data.message : "Could not resolve that", 200) };

    if (r === "token") {
      if (!isV1("ticket", data.ticket)) refuse("plugin_invalid", "a token result needs a ticket");
      const out = { result: "token", ticket: data.ticket, title: cap(isString(data.title) ? data.title : "", 60) };

      if (kind === "scope") out.token = `@${m.short}:${name}:${data.ticket}`;

      return out;
    }

    refuse("plugin_invalid", `a resolve tool answered result ${JSON.stringify(r)?.slice(0, 40)}; 9.2 allows choices, open, token, error`);
  }

  /** POST /v1/host/resolve {plugin, kind, name, value, complete, context}
   *  → the 9.2 result (10 s). */
  async function resolve(b) {
    if (!isObject(b) || !["command", "scope"].includes(b.kind)) refuse("invalid_args", "kind must be command or scope");
    const row = running(b.plugin);
    const m = plugins.manifestOf(row);
    const entry = (b.kind === "command" ? m.commands : m.scopes)?.find((x) => x.name === b.name);

    if (!entry) refuse("not_found", `${b.plugin} has no ${b.kind} ${String(b.name).slice(0, 40)}`);

    if (plugins.disabledOf(row).has(entry.resolve)) refuse("disabled", `${entry.resolve} is switched off`);

    const attached = (isObject(b.context) && Array.isArray(b.context.attached) ? b.context.attached : [])
      .flatMap((a) => (isObject(a) && isString(a.scope) && isV1("canonicalToken", a.token) ? [{ scope: a.scope, token: a.token }] : []));

    const input = { kind: b.kind, name: b.name, value: String(b.value ?? ""), complete: b.complete === true, context: { attached } };

    const r = await plugins.callTool(b.plugin, entry.resolve, input, {
      meta: { "rox/caller": { origin: "resolve", plugin: b.plugin, ts: plugins.clock.now() } },
      timeout: plugins.timeouts.resolve,
    });

    if (r.isError) return { ok: true, result: "error", message: cap(firstText(r) || "Could not resolve that", 200) };

    return { ok: true, ...resolved(m, b.kind, b.name, resultData(r)) };
  }

  /** POST /v1/host/apps/source {plugin, app} → {source, fingerprint} (6.1, 6.2). */
  async function appSource(b) {
    const row = running(b?.plugin);
    const app = (plugins.manifestOf(row).apps || []).find((a) => a.id === b.app);

    if (!app) refuse("not_found", `${b.plugin} has no app ${String(b.app).slice(0, 60)}`);
    const { contents = [] } = await plugins.readResource(b.plugin, app.entry);

    if (contents.length !== 1 || !isString(contents[0].text)) refuse("plugin_invalid", `${app.entry} must answer exactly one text content`);

    if (contents[0].mimeType !== undefined && contents[0].mimeType !== app.mime) refuse("plugin_invalid", `${app.entry} is ${contents[0].mimeType}, not ${app.mime}`);
    const source = contents[0].text;

    if (Buffer.byteLength(source, "utf8") > SOURCE_MAX) refuse("result_too_large", `${app.entry} is over 512 KiB`);
    const fingerprint = fingerprintOf(Buffer.from(source, "utf8"));

    if (fingerprint !== app.fingerprint) refuse("fingerprint_mismatch", "This app's code does not match its signed manifest. Update the plugin.");

    return { ok: true, source, fingerprint };
  }

  return { open, close, call, ticketTitle, resolve, appSource, opens };
}
