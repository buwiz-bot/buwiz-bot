// v1/plugins.mjs — the broker's v1 plugins: install records, processes,
// restarts (spec 4.3, 5.2, 12, 13, 14).
//
// A v1 plugin is a stdio MCP child the broker owns, apart from the v0
// mcp_servers (mcp.mjs), which keep working as they do. Every spawn runs
// the same gate: connect, `rox.provision` with the plugin key as the first
// request (7.8), read `rox://manifest`, verify it (5.2), check tools/list
// against it, and compare it with the install record (12.1). Only then is
// the plugin `running`, under a new generation. A crash restarts it with
// backoff 1-60 s; the seventh restart within 10 minutes leaves it
// `failed` (14.4). A verification failure never loops: it is `failed`
// with the spec's code. Install records live in `v1_plugins`; keys live
// in the keystore and reach the plugin only on its stdin.
//
// createPlugins() takes its clock, keystore and timeouts as options so the
// tests can run the restart policy in milliseconds; broker.mjs builds the
// one the server uses.
import { EventEmitter } from "node:events";
import { appendFileSync, mkdirSync, renameSync, statSync } from "node:fs";
import { join } from "node:path";
import { isV1 } from "../grammar.mjs";
import { generate, pubOf } from "./ed.mjs";
import { V1Error, refuse } from "./errors.mjs";
import { isObject, isString, kindOf } from "./jcs.mjs";
import {
  MANIFEST_URI, addedPermissions, checkTools, compareSemver, permsDigest, reviewOf, verifyManifest,
} from "./manifest.mjs";
import { loadTrust } from "./trust.mjs";

export const BACKOFF_S = [1, 2, 4, 8, 16, 32, 60];
export const RESTART_WINDOW_MS = 10 * 60 * 1000;
export const MAX_RESTARTS = 6;
const LOG_CAP = 5 * 1024 * 1024;

export const systemClock = { now: () => Date.now(), setTimeout: (f, ms) => setTimeout(f, ms), clearTimeout: (t) => clearTimeout(t) };

const SCHEMA = `
  CREATE TABLE IF NOT EXISTS v1_plugins(
    id TEXT PRIMARY KEY, short TEXT NOT NULL, command TEXT NOT NULL, args TEXT NOT NULL, env TEXT NOT NULL,
    version TEXT NOT NULL, protocol TEXT NOT NULL, publisher_key_id TEXT NOT NULL, plugin_pub TEXT NOT NULL,
    manifest TEXT NOT NULL, manifest_sha256 TEXT NOT NULL, tools TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 1, disabled_tools TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
`;

const parse = (s, d) => {
  try {
    return JSON.parse(s) ?? d;
  } catch {
    return d;
  }
};

/** A spawn spec from a request body: stdio only (14.3). */
export function spawnSpec(body) {
  if (!isObject(body)) refuse("invalid_args", "send {command, args, env?}");

  if (body.url !== undefined || (body.transport !== undefined && body.transport !== "stdio")) refuse("not_supported", "v1 plugins run over stdio only");

  if (!isString(body.command) || !body.command) refuse("invalid_args", "command must be a non-empty string");
  const args = body.args ?? [];

  if (!Array.isArray(args) || !args.every(isString)) refuse("invalid_args", "args must be an array of strings");
  const env = body.env ?? {};

  if (!isObject(env) || !Object.values(env).every(isString)) refuse("invalid_args", "env must map names to strings");

  return { command: body.command, args, env };
}

// ── the MCP connection ───────────────────────────────────────────────────

/** Text of an MCP tool result as JSON: structuredContent, else the first
 *  text parsed. Reserved tools answer this way. */
export function resultData(r) {
  if (isObject(r?.structuredContent)) return r.structuredContent;
  const t = (r?.content || []).find((c) => c.type === "text")?.text;

  return parse(t, null);
}

const isMethodNotFound = (e) => e?.code === -32601;

export function createPlugins({ db, keys, clock = systemClock, logDir, trust = loadTrust, timeouts = {} }) {
  db.exec(SCHEMA);
  const T = { call: 120_000, resolve: 10_000, reserved: 5_000, cancel: 5_000, ...timeouts };
  const events = new EventEmitter();
  const runtimes = new Map(); // id → runtime
  const closers = []; // (id) → Promise: sessions.mjs ends a plugin's opens

  events.setMaxListeners(0);

  const rowOf = (id) => db.prepare("SELECT * FROM v1_plugins WHERE id=?").get(String(id ?? ""));
  const rows = () => db.prepare("SELECT * FROM v1_plugins ORDER BY created_at").all();
  const manifestOf = (row) => parse(row.manifest, {});
  const disabledOf = (row) => new Set(parse(row.disabled_tools, []));

  function runtime(id) {
    if (!runtimes.has(id)) runtimes.set(id, { id, state: "stopped", code: null, message: "", generation: 0, conn: null, tools: [], manifest: null, instructions: "", restarts: [], timer: null, inflight: new Set() });

    return runtimes.get(id);
  }

  function emit(rt) {
    events.emit("plugin", { id: rt.id, state: rt.state, code: rt.code, generation: rt.generation, enabled: !!rowOf(rt.id)?.enabled });
  }

  function setState(rt, state, code = null, message = "") {
    rt.state = state;
    rt.code = code;
    rt.message = String(message).slice(0, 500);
    emit(rt);
  }

  // stderr → the plugin's log (5 MiB, one rotation), with the seed redacted:
  // a plugin that prints its key fails conformance (4.3) and is stopped.
  function stderrSink(id, seed, onLeak) {
    let carry = "";
    const file = logDir && join(logDir, `${id}.log`);
    const variants = [seed, Buffer.from(seed, "base64url").toString("base64").replace(/=+$/, "")];

    return (chunk) => {
      const lines = (carry + chunk).split("\n");

      carry = lines.pop().slice(-4096);

      for (let line of lines) {
        if (variants.some((v) => line.includes(v))) {
          for (const v of variants) line = line.split(v).join("[redacted key]");
          onLeak();
        }

        if (!file) continue;

        try {
          mkdirSync(logDir, { recursive: true });

          if (statSync(file, { throwIfNoEntry: false })?.size > LOG_CAP) renameSync(file, `${file}.1`);
          appendFileSync(file, `${new Date().toISOString()} ${line}\n`);
        } catch {}
      }
    };
  }

  async function connect(spec, seed, id, hooks) {
    const [{ Client }, { StdioClientTransport }, { ResourceListChangedNotificationSchema }] = await Promise.all([
      import("@modelcontextprotocol/sdk/client/index.js"),
      import("@modelcontextprotocol/sdk/client/stdio.js"),
      import("@modelcontextprotocol/sdk/types.js"),
    ]);

    const transport = new StdioClientTransport({ command: spec.command, args: spec.args, env: { PATH: process.env.PATH || "", ...spec.env }, stderr: "pipe" });
    const client = new Client({ name: "rox-broker", version: "1.0.0" });
    const conn = { client, transport, closed: false };
    const sink = stderrSink(id, seed, () => hooks.onLeak?.(conn));

    transport.stderr?.on("data", (d) => sink(String(d)));
    client.onclose = () => {
      conn.closed = true;
      hooks.onClose?.(conn);
    };

    // malformed MCP is a crash (14.4)
    client.onerror = (e) => {
      if (e instanceof SyntaxError || e?.name === "ZodError") transport.close().catch(() => {});
    };

    client.setNotificationHandler(ResourceListChangedNotificationSchema, () => hooks.onListChanged?.(conn));
    await client.connect(transport);

    return conn;
  }

  async function reserved(conn, name, args, ms = T.reserved) {
    const r = await conn.client.callTool({ name, arguments: args }, undefined, { timeout: ms });
    const data = resultData(r);

    if (r.isError) throw new V1Error(isString(data?.code) ? data.code : "invalid_args", `${name}: ${String(data?.message || "refused").slice(0, 200)}`);

    return data;
  }

  // The gate every spawn passes: provision, read, verify (5.2 steps 1-5, 7).
  async function bootstrap(spec, seed, hooks = {}, known = "") {
    const conn = await connect(spec, seed, known || "inspect", hooks);

    try {
      // a first spawn learns the id rox.provision names from the manifest
      // (resources are readable before provision; only tools are gated)
      const id = known || (await peekId(conn));
      let prov;

      try {
        prov = await reserved(conn, "rox.provision", { protocol: "1.0.0", plugin: id, key: { alg: "ed25519", seed }, pub: pubOf(seed) });
      } catch (e) {
        throw new V1Error("plugin_invalid", `rox.provision failed: ${String(e?.message || e).slice(0, 200)}`);
      }

      if (prov?.ok !== true || !/^[0-9a-f]{64}$/.test(String(prov.manifest_sha256))) refuse("plugin_invalid", "rox.provision did not answer {ok: true, manifest_sha256}");
      const v = await readVerified(conn);

      if (prov.manifest_sha256 !== v.sha256) refuse("manifest_invalid", "rox.provision reports a manifest other than the one the plugin serves");

      return { conn, ...v };
    } catch (e) {
      await conn.transport.close().catch(() => {});
      throw e;
    }
  }

  async function peekId(conn) {
    try {
      const { contents = [] } = await conn.client.readResource({ uri: MANIFEST_URI });
      const id = parse(contents.find((c) => isString(c.text))?.text, {})?.id;

      return isString(id) ? id : "";
    } catch {
      return "";
    }
  }

  async function readVerified(conn) {
    const { resources = [] } = await conn.client.listResources();

    if (!resources.some((r) => r.uri === MANIFEST_URI)) refuse("manifest_invalid", "the plugin lists no rox://manifest");
    const { contents = [] } = await conn.client.readResource({ uri: MANIFEST_URI });
    const text = contents.find((c) => isString(c.text))?.text;
    const { manifest, sha256 } = verifyManifest(text, trust());
    const { tools = [] } = await conn.client.listTools();

    checkTools(manifest, tools);

    return { manifest, sha256, tools, instructions: conn.client.getInstructions?.() || "" };
  }

  // 5.2 step 6: one publisher per id; `short` unique among enabled plugins
  function checkInstalled(m, keyId) {
    const row = rowOf(m.id);

    if (row && row.publisher_key_id !== keyId) refuse("manifest_invalid", `${m.id} is installed under publisher key ${row.publisher_key_id}`);
    const clash = db.prepare("SELECT id FROM v1_plugins WHERE short=? AND id<>? AND enabled=1").get(m.short, m.id);

    if (clash) refuse("manifest_invalid", `short ${m.short} is already used by ${clash.id}`);
  }

  // ── running ──────────────────────────────────────────────────────────

  function failInflight(rt) {
    for (const f of rt.inflight) f.reject(new V1Error("plugin_restarted", "the plugin restarted during the call"));
    rt.inflight.clear();
  }

  async function stopConn(rt) {
    const conn = rt.conn;

    rt.conn = null;
    failInflight(rt);

    if (conn) {
      conn.stopping = true;
      await conn.transport.close().catch(() => {});
    }
  }

  function hooksFor(rt) {
    return {
      onClose: (conn) => {
        if (conn.stopping || rt.conn !== conn) return;
        rt.conn = null;
        failInflight(rt);
        scheduleRestart(rt);
      },
      onLeak: (conn) => {
        if (rt.conn !== conn) return;
        stopConn(rt).then(() => setState(rt, "failed", "plugin_invalid", "the plugin wrote its key to stderr (4.3)"));
      },
      onListChanged: (conn) => reread(rt, conn),
    };
  }

  function scheduleRestart(rt) {
    const now = clock.now();

    rt.restarts = rt.restarts.filter((t) => now - t < RESTART_WINDOW_MS);

    if (rt.restarts.length >= MAX_RESTARTS) return setState(rt, "failed", "failed", `restarted ${MAX_RESTARTS} times in 10 minutes`);
    const delay = BACKOFF_S[Math.min(rt.restarts.length, BACKOFF_S.length - 1)] * 1000;

    rt.restarts.push(now);
    rt.nextDelay = delay;
    setState(rt, "restarting", null, `restarting in ${delay / 1000} s`);
    rt.timer = clock.setTimeout(() => {
      rt.timer = null;
      spawn(rt.id).catch(() => {});
    }, delay);
  }

  // The install record against what the plugin now serves (12.1, 13.3).
  function compareInstalled(row, v) {
    if (v.sha256 === row.manifest_sha256) return null;

    if (v.manifest.version === row.version) return new V1Error("manifest_changed", "the manifest changed without a version change");

    return new V1Error("update_available", `the plugin now serves version ${v.manifest.version}; install it to update`);
  }

  function adopt(rt, row, v) {
    rt.conn = v.conn;
    rt.tools = v.tools;
    rt.manifest = v.manifest;
    rt.instructions = v.instructions;
    rt.generation++;
    setState(rt, "running");
  }

  /** Spawn an installed, enabled plugin through the gate. Resolves to its
   *  state; a verification failure leaves it `failed` with the code. */
  async function spawn(id) {
    const rt = runtime(id);
    const row = rowOf(id);

    if (!row?.enabled) return rt.state;

    if (rt.conn) return rt.state;

    if (rt.state !== "restarting") setState(rt, "starting");
    const seed = await keys.get(id);

    if (!seed) return (setState(rt, "failed", "plugin_invalid", "the plugin key is missing from the keystore; reinstall"), rt.state);
    let v;

    try {
      v = await bootstrap({ command: row.command, args: parse(row.args, []), env: parse(row.env, {}) }, seed, hooksFor(rt), id);
    } catch (e) {
      // a process that would not start or answer counts as a crash; a
      // plugin that answered with something invalid does not loop (14.4)
      if (e instanceof V1Error) setState(rt, "failed", e.code, e.message);
      else scheduleRestart(rt);

      return rt.state;
    }

    if (!rowOf(id)?.enabled) {
      await v.conn.transport.close().catch(() => {});

      return rt.state;
    }

    const why = compareInstalled(row, v);

    try {
      if (!why) checkInstalled(v.manifest, row.publisher_key_id);
    } catch (e) {
      await v.conn.transport.close().catch(() => {});

      return (setState(rt, "failed", e.code, e.message), rt.state);
    }

    if (why) {
      await v.conn.transport.close().catch(() => {});

      return (setState(rt, why.code === "update_available" ? "update_available" : "failed", why.code, why.message), rt.state);
    }

    adopt(rt, row, v);

    return rt.state;
  }

  // resources/list_changed: read and verify again (5.1, 12.1)
  async function reread(rt, conn) {
    if (rt.conn !== conn) return;
    const row = rowOf(rt.id);
    let why = null;

    try {
      const v = await readVerified(conn);

      why = compareInstalled(row, v);

      if (!why) {
        rt.tools = v.tools;
        rt.manifest = v.manifest;
      }
    } catch (e) {
      why = e instanceof V1Error ? e : new V1Error("manifest_invalid", String(e?.message || e));
    }

    if (!why || rt.conn !== conn) return;
    await stopConn(rt);
    setState(rt, why.code === "update_available" ? "update_available" : "failed", why.code, why.message);
  }

  // ── host API ─────────────────────────────────────────────────────────

  function updateInfo(row, m) {
    if (!row) return null;
    const prev = manifestOf(row);
    const order = compareSemver(m.version, row.version);

    return {
      from_version: row.version,
      to_version: m.version,
      downgrade: order < 0,
      same_version: order === 0,
      added_permissions: addedPermissions(prev, m),
    };
  }

  // 3.1 rule 5: a command another enabled plugin also declares
  function ambiguity(m) {
    const out = [];

    for (const other of rows()) {
      if (!other.enabled || other.id === m.id) continue;

      for (const c of manifestOf(other).commands || [])
        if ((m.commands || []).some((x) => x.name === c.name)) out.push(`/${c.name} is also a command of ${other.id}; it will need /${m.short}:${c.name}`);
    }

    return out;
  }

  const titleOf = (id) => {
    const row = rowOf(id);

    return row ? manifestOf(row).title || id : id;
  };

  /** POST /v1/host/plugins/inspect: a temporary spawn, verified, with
   *  the review of 13.1 step 3. Nothing is installed or stored. */
  async function inspect(body) {
    const spec = spawnSpec(body);
    const v = await bootstrap(spec, generate().seed);

    await v.conn.transport.close().catch(() => {});
    checkInstalled(v.manifest, v.manifest.publisher.key_id);

    return {
      ok: true,
      manifest: v.manifest,
      manifest_sha256: v.sha256,
      review: reviewOf(v.manifest, titleOf),
      update: updateInfo(rowOf(v.manifest.id), v.manifest),
      warnings: ambiguity(v.manifest),
    };
  }

  function record(m, sha256, spec, pub, tools, prev) {
    const now = clock.now();

    db.prepare(`INSERT OR REPLACE INTO v1_plugins(id,short,command,args,env,version,protocol,publisher_key_id,plugin_pub,manifest,manifest_sha256,tools,enabled,disabled_tools,created_at,updated_at)
      VALUES(?,?,?,?,?,?,?,?,?,?,?,?,1,?,?,?)`).run(
      m.id, m.short, spec.command, JSON.stringify(spec.args), JSON.stringify(spec.env), m.version, m.protocol, m.publisher.key_id, pub,
      JSON.stringify(m), sha256, JSON.stringify(tools.map((t) => ({ name: t.name, description: t.description || "", readOnly: t.annotations?.readOnlyHint === true }))),
      prev?.disabled_tools || "[]", prev?.created_at || now, now,
    );
  }

  /** POST /v1/host/plugins/install: installs what was reviewed (13.1 step
   *  4) or updates an installed plugin (13.3). `manifest_sha256` must be
   *  the reviewed hash. An update that adds permissions needs
   *  `approve_permissions: true`; an older version `allow_downgrade: true`. */
  async function install(body) {
    const spec = spawnSpec(body);

    if (!/^[0-9a-f]{64}$/.test(String(body.manifest_sha256))) refuse("invalid_args", "manifest_sha256 (the reviewed manifest's hash) is required");
    // an update keeps the plugin key; a new install generates one (4.3)
    const fresh = generate();
    const v0 = await bootstrap(spec, fresh.seed);
    const m = v0.manifest;
    const row = rowOf(m.id);
    let v = v0;
    let seed = fresh.seed;

    try {
      if (v.sha256 !== body.manifest_sha256) refuse("manifest_invalid", "the plugin serves a manifest other than the one reviewed; inspect it again");
      checkInstalled(m, m.publisher.key_id);

      if (row) {
        const u = updateInfo(row, m);

        if (v.sha256 === row.manifest_sha256) {
          await v.conn.transport.close().catch(() => {});

          return { ok: true, id: m.id, unchanged: true };
        }

        if (u.same_version) refuse("manifest_changed", "the manifest changed without a version change");

        if (u.downgrade && body.allow_downgrade !== true) refuse("invalid_args", `${m.version} is older than the installed ${row.version}`, { reason: "downgrade" });

        if (u.added_permissions.length && body.approve_permissions !== true)
          refuse("consent_declined", "this update adds permissions the user has not approved", { reason: "permissions_added", added: u.added_permissions });
        // the update runs under the installed key: spawn it again with that
        seed = await keys.get(m.id);
        await v.conn.transport.close().catch(() => {});
        v = null;

        if (!seed) refuse("plugin_invalid", "the installed plugin key is missing; uninstall and install again");
      }
    } catch (e) {
      await v?.conn.transport.close().catch(() => {});
      throw e;
    }

    const rt = runtime(m.id);

    if (row) {
      // 13.3 step 5-6: stop the old version; its opens end with it
      if (rt.timer) clock.clearTimeout(rt.timer);
      await stopConn(rt);
      await Promise.all(closers.map((f) => f(m.id, { stop: false })));
      v = await bootstrap(spec, seed, hooksFor(rt), m.id);

      if (v.sha256 !== body.manifest_sha256) {
        await v.conn.transport.close().catch(() => {});
        refuse("manifest_invalid", "the plugin changed its manifest during the update");
      }
    } else {
      await keys.put(m.id, seed);
      // the reviewed process becomes the running one under the stored key
      v.conn.stopping = true;
      await v.conn.transport.close().catch(() => {});
      v = await bootstrap(spec, seed, hooksFor(rt), m.id).catch(async (e) => {
        await keys.remove(m.id);
        throw e;
      });
    }

    record(m, v.sha256, spec, pubOf(seed), v.tools, row);
    rt.restarts = [];
    adopt(rt, rowOf(m.id), v);

    return { ok: true, id: m.id, state: rt.state, generation: rt.generation, plugin_pub: pubOf(seed), updated: !!row };
  }

  function mustRow(id) {
    const row = rowOf(id);

    if (!row) refuse("not_found", `${String(id).slice(0, 80)} is not installed`);

    return row;
  }

  /** 13.4: stop the process and end its opens; storage, keys, tickets stay. */
  async function disable(id) {
    mustRow(id);
    db.prepare("UPDATE v1_plugins SET enabled=0, updated_at=? WHERE id=?").run(clock.now(), id);
    const rt = runtime(id);

    if (rt.timer) clock.clearTimeout(rt.timer);
    rt.timer = null;
    await Promise.all(closers.map((f) => f(id, { stop: true })));
    await stopConn(rt);
    setState(rt, "stopped");

    return { ok: true, state: rt.state };
  }

  /** 13.5: spawn and verify again. */
  async function enable(id) {
    const row = mustRow(id);

    checkInstalled(manifestOf(row), row.publisher_key_id);
    db.prepare("UPDATE v1_plugins SET enabled=1, updated_at=? WHERE id=?").run(clock.now(), id);
    const rt = runtime(id);

    rt.restarts = [];

    if (rt.conn) return { ok: true, state: rt.state, generation: rt.generation };

    if (rt.timer) clock.clearTimeout(rt.timer);
    rt.timer = null;
    await spawn(id);

    return { ok: rt.state === "running", state: rt.state, code: rt.code, message: rt.message, generation: rt.generation };
  }

  /** 13.6, broker side: rox.uninstall (best effort, 5 s), end the opens,
   *  stop, delete the key, remove the record. */
  async function uninstall(id) {
    const row = mustRow(id);
    const rt = runtime(id);
    let conn = rt.conn;
    let temp = false;

    if (!conn) {
      const seed = await keys.get(id);

      conn = seed && (await bootstrap({ command: row.command, args: parse(row.args, []), env: parse(row.env, {}) }, seed, {}, id).then((v) => v.conn, () => null));
      temp = !!conn;
    }

    if (conn) await reserved(conn, "rox.uninstall", {}, T.reserved).catch(() => {});

    if (temp) await conn.transport.close().catch(() => {});
    await Promise.all(closers.map((f) => f(id, { stop: true })));

    if (rt.timer) clock.clearTimeout(rt.timer);
    await stopConn(rt);
    await keys.remove(id);
    db.prepare("DELETE FROM v1_plugins WHERE id=?").run(id);
    runtimes.delete(id);
    events.emit("plugin", { id, state: "uninstalled", code: null, generation: rt.generation, enabled: false });

    return { ok: true };
  }

  /** 8.3: the per-tool switch. */
  function setTool(id, tool, disabled) {
    const row = mustRow(id);

    if (!parse(row.tools, []).some((t) => t.name === tool)) refuse("not_found", `${id} has no tool ${String(tool).slice(0, 80)}`);

    if (kindOf(disabled) !== "boolean") refuse("invalid_args", "disabled must be a boolean");
    const off = disabledOf(row);

    if (disabled) off.add(tool);
    else off.delete(tool);
    db.prepare("UPDATE v1_plugins SET disabled_tools=?, updated_at=? WHERE id=?").run(JSON.stringify([...off]), clock.now(), id);
    emit(runtime(id));

    return { ok: true, tool, disabled };
  }

  /** A call to a running plugin: `_meta` as given, the broker's timeout
   *  with MCP cancellation, `plugin_restarted` when the process goes away. */
  async function callTool(id, name, args, { meta, timeout = T.call } = {}) {
    const rt = runtime(id);

    if (!rt.conn || rt.state !== "running") refuse("plugin_unavailable", `${id} is not running`);
    const conn = rt.conn;
    const entry = {};
    const gone = new Promise((_, reject) => (entry.reject = reject));

    rt.inflight.add(entry);
    gone.catch(() => {});
    const params = { name, arguments: args };

    if (meta) params._meta = meta;

    try {
      return await Promise.race([conn.client.callTool(params, undefined, { timeout }), gone]);
    } catch (e) {
      if (e instanceof V1Error) throw e;

      if (e?.code === -32001) {
        hangCheck(rt, conn);
        refuse("timeout", `${name} took over ${Math.round(timeout / 1000)} s; the broker cancelled it`);
      }

      if (conn.closed) refuse("plugin_restarted", "the plugin restarted during the call");

      if (isMethodNotFound(e)) refuse("not_found", `${id} has no tool ${name}`);
      refuse("internal", `${name}: ${String(e?.message || e).slice(0, 300)}`);
    } finally {
      rt.inflight.delete(entry);
    }
  }

  // 14.4: a call past the broker limit is cancelled; a plugin that does not
  // answer within 5 s after that is restarted
  function hangCheck(rt, conn) {
    conn.client.ping({ timeout: T.cancel }).catch(() => {
      if (rt.conn === conn) conn.transport.close().catch(() => {});
    });
  }

  /** A reserved tool on a running plugin; -32601 is `null` (7.9). */
  async function callReserved(id, name, args) {
    const rt = runtime(id);

    if (!rt.conn || rt.state !== "running") refuse("plugin_unavailable", `${id} is not running`);

    try {
      return await reserved(rt.conn, name, args);
    } catch (e) {
      if (isMethodNotFound(e)) return null;

      if (e instanceof V1Error) throw e;

      if (e?.code === -32001) refuse("timeout", `${name} did not answer in time`);
      refuse("plugin_unavailable", `${name}: ${String(e?.message || e).slice(0, 200)}`);
    }
  }

  async function readResource(id, uri) {
    const rt = runtime(id);

    if (!rt.conn || rt.state !== "running") refuse("plugin_unavailable", `${id} is not running`);

    return rt.conn.client.readResource({ uri });
  }

  /** Mark a plugin failed (a bad certificate: 4.4 step 4, 14.4). */
  async function fail(id, code, message) {
    const rt = runtime(id);

    await stopConn(rt);
    setState(rt, "failed", code, message);
  }

  /** The plugin as GET /v1/host/plugins lists it. */
  function describe(row) {
    const m = manifestOf(row);
    const rt = runtime(row.id);
    const off = disabledOf(row);
    const tools = (rt.conn ? rt.tools : parse(row.tools, [])).filter((t) => !isV1("reservedTool", t.name));

    return {
      id: row.id,
      short: row.short,
      title: m.title,
      version: row.version,
      protocol: row.protocol,
      publisher: m.publisher,
      state: rt.state,
      code: rt.code,
      message: rt.message,
      enabled: !!row.enabled,
      generation: rt.generation,
      plugin_pub: row.plugin_pub,
      manifest_sha256: row.manifest_sha256,
      apps: (m.apps || []).map((a) => ({ ...a, perms_sha256: permsDigest(a.permissions) })),
      commands: m.commands || [],
      scopes: m.scopes || [],
      routines: (m.routines || []).map((r) => ({ ...r, reads_status: readsStatus(r) })),
      tools: tools.map((t) => ({ name: t.name, description: t.description || "", disabled: off.has(t.name), readOnly: (t.annotations?.readOnlyHint ?? t.readOnly) === true })),
    };
  }

  // 5.9: a routine reads only approved plugins' readOnlyHint tools
  function readsStatus(r) {
    const tr = trust();

    return (r.reads || []).map((x) => {
      const row = rowOf(x.plugin);
      const known = row ? (runtime(x.plugin).conn ? runtime(x.plugin).tools : parse(row.tools, [])) : [];
      const ro = (name) => known.some((t) => t.name === name && (t.annotations?.readOnlyHint ?? t.readOnly) === true);
      let reason = "";

      if (!row) reason = `${x.plugin} is not installed`;
      else if (!tr.approved.some((a) => a.id === x.plugin && a.publisher_key_id === row.publisher_key_id)) reason = `${x.plugin} is not an approved plugin`;
      else if (!row.enabled) reason = `${x.plugin} is disabled`;
      else if (!x.tools.every(ro)) reason = `only read-only tools may be read: ${x.tools.filter((t) => !ro(t)).join(", ")}`;

      return { plugin: x.plugin, tools: x.tools, allowed: !reason, reason: reason || null };
    });
  }

  /** Spawn every enabled plugin (server start). */
  const startAll = () => Promise.all(rows().filter((r) => r.enabled).map((r) => spawn(r.id).catch(() => {})));

  /** Stop every process (tests, shutdown). */
  async function stopAll() {
    for (const rt of runtimes.values()) {
      if (rt.timer) clock.clearTimeout(rt.timer);
      rt.timer = null;
      await stopConn(rt);
    }
  }

  return {
    events, runtime, rowOf, rows, manifestOf, disabledOf, inspect, install, enable, disable, uninstall, setTool, spawn,
    callTool, callReserved, readResource, fail, describe, startAll, stopAll, readsStatus, trust, timeouts: T, clock,
    onClose: (f) => closers.push(f),
    list: () => rows().map(describe),
  };
}
