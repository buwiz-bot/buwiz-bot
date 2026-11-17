#!/usr/bin/env node
// the conformance fixture plugin (stdio MCP).
//
// Plain Node with no packages: the MCP framing it needs (JSON-RPC lines on
// stdio, initialize, cancellation, MCP error codes) is the few lines at the
// end of this file, and ed.mjs and jcs.mjs beside it are the ed25519 and JCS
// helpers it signs with. Used by the Rust broker's tests and by the Node
// conformance tests.
//
// Plugin id app.buwiz.fixture, short `fx`. It implements every
// reserved tool of spec 10 the way a conforming plugin must (the key in
// memory only, provision first, certificates over JCS, tickets with a
// subject map that never leaves the process), plus tools that exercise the
// broker: fx_echo (echoes args and `_meta["buwiz/caller"]`), fx_slow, fx_fail,
// fx_big, fx_resolve, fx_open, fx_search (readOnlyHint), fx_query (a
// routine's search text), fx_secret (in no app's permissions), fx_stats
// (what it was asked, and each tool's last caller origin) and fx_reload
// (sends resources/list_changed).
//
// The manifest is read from FX_MANIFEST on every request (tests sign one
// per run and may change it); the app sources sit beside this file.
// `--misbehave=<mode>[,<mode>]` breaks one rule on purpose for negative
// tests: skip-provision, bad-cert, log-seed, wrong-perms, crash-after:<n>,
// old-cert (a certificate of another type), no-accounts (no
// buwiz.account.set/clear: -32601). strict-host breaks none: it refuses to
// be provisioned by a host that does not run protocol 1.0.
//
// buwiz.account.set/clear keep the pushed accounts in
// memory; fx_stats shows each one's subject, expiry and the first 12 hex
// of its token's sha256, never the token, and `account_pushes`: each push's
// method, label and `accounts` list (a first-party plugin's); `client` is the
// capabilities the broker sent at initialize.
//
// fx_echo and fx_search answer `args.result_meta` as the result's `_meta`.
// fx_badge answers `_meta["buwiz/badge"]` = `args.badge` (and fails when `args.fail`), with
// `query` and `items` set so a routine may use it as its query tool.
// fx_echo with `reload: true` first sends resources/list_changed.
//
// FX_STATS_FILE names a file the fixture rewrites with what fx_stats answers
// (and its process id) at initialize, before each tool call it serves, and at
// each cancellation: a test reads what the plugin saw without calling a tool.
import { createHash, randomBytes } from "node:crypto";
import { readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { generate, pubOf, sign } from "./ed.mjs";
import { canonicalSigned, isObject, isString, parseIJson, signedBytes } from "./jcs.mjs";

const sha256hex = (s) => createHash("sha256").update(s).digest("hex");
const permsDigest = (perms) => sha256hex(canonicalSigned(perms ?? {}));
const manifestHash = (m) => sha256hex(canonicalSigned(m));
// the v1 reserved tool names: the spec's `reservedTool` grammar
const isReservedTool = (name) => /^buwiz\.[a-z.]+$/.test(String(name ?? ""));

const ErrorCode = { InvalidRequest: -32600, MethodNotFound: -32601, InvalidParams: -32602, InternalError: -32603 };

class McpError extends Error {
  constructor(code, message, data) {
    super(`MCP error ${code}: ${message}`);
    this.code = code;
    this.data = data;
  }
}

const HERE = dirname(fileURLToPath(import.meta.url));
const MANIFEST = process.env.FX_MANIFEST || join(HERE, "manifest.json");
const flags = (process.argv.find((a) => a.startsWith("--misbehave=")) || "").slice(12).split(",").filter(Boolean);
const misbehave = (mode) => flags.some((f) => f === mode || f.startsWith(`${mode}:`));
const crashAfter = Number(flags.find((f) => f.startsWith("crash-after:"))?.split(":")[1] || 0);
const EIGHT_HOURS = 8 * 3600 * 1000;

// memory only (5.4): never logged, written or echoed
let key = null;
const sessions = new Map(); // open_id → app
const tickets = new Map(); // ticket → subject
const bySubject = new Map(); // subject → ticket
const calls = {};
const accounts = new Map(); // `${account}/${label}` → what buwiz.account.set pushed
const accountPushes = []; // {push, label, accounts?}, in the order they came
const origins = {}; // tool → the last caller's origin (16.1)
let cancelled = 0;
let served = 0;
let inflight = 0; // tool calls running now
let maxInflight = 0; // the most at once

// what fx_stats answers; the same snapshot goes to FX_STATS_FILE
const stats = () => ({ pid: process.pid, calls, origins, cancelled, inflight, max_inflight: maxInflight, sessions: sessions.size, tickets: tickets.size, accounts: Object.fromEntries(accounts), account_pushes: accountPushes, client: server.getClientCapabilities() ?? null });

function writeStats() {
  const file = process.env.FX_STATS_FILE;

  if (!file) return;
  // a name of its own per process: every process of a bed shares the file
  const tmp = `${file}.${process.pid}.tmp`;

  // the seed it was given is for a test to look for in the logs (a throwaway key)
  writeFileSync(tmp, JSON.stringify({ ...stats(), seed: key?.seed ?? null }));
  renameSync(tmp, file);
}

const manifest = () => parseIJson(readFileSync(MANIFEST, "utf8"));

// ── tickets: tkt_ + 26 Crockford base32 characters = 130 random bits ────
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

function newTicket() {
  let n = BigInt(`0x${randomBytes(17).toString("hex")}`) >> 6n; // 136 → 130 bits
  let s = "";

  for (let i = 0; i < 26; i++, n >>= 5n) s = CROCKFORD[Number(n & 31n)] + s;

  return `tkt_${s}`;
}

function ticketFor(subject) {
  if (!bySubject.has(subject)) {
    const t = newTicket();

    bySubject.set(subject, t);
    tickets.set(t, subject);
  }

  return bySubject.get(subject);
}

// ── results ──────────────────────────────────────────────────────────────
const ok = (structured) => ({ content: [{ type: "text", text: JSON.stringify(structured) }], structuredContent: structured });
const fail = (code, message) => ({ isError: true, content: [{ type: "text", text: message }], structuredContent: { code, message } });

// ── reserved tools (10) ──────────────────────────────────────────────────
function provision(args) {
  if (misbehave("skip-provision")) throw new McpError(ErrorCode.MethodNotFound, "buwiz.provision is not implemented");

  if (misbehave("strict-host") && !/^1\.0\.\d+$/.test(String(args?.protocol))) return fail("protocol_unsupported", `this plugin needs a protocol 1.0 host, not ${args?.protocol}`);

  if (args?.key?.alg !== "ed25519" || pubOf(args.key.seed) !== args.pub) return fail("invalid_args", "the key and pub do not match");

  if (key && key.seed !== args.key.seed) return fail("invalid_args", "already provisioned with another key");
  key = { seed: args.key.seed, pub: args.pub };

  // the failure log-seed simulates: a plugin that writes its key out
  if (misbehave("log-seed")) process.stderr.write(`provisioned with seed ${args.key.seed}\n`);

  return ok({ ok: true, manifest_sha256: manifestHash(manifest()) });
}

function sessionOpen(args) {
  const app = (manifest().apps || []).find((a) => a.id === args?.app);

  if (!app) return fail("invalid_args", `no app ${String(args?.app).slice(0, 60)}`);
  const digest = permsDigest(app.permissions);

  if (!misbehave("wrong-perms") && args.perms_sha256 !== digest) return fail("invalid_args", "perms_sha256 does not match the manifest");

  if (!Number.isSafeInteger(args.not_after) || args.not_after > Date.now() + EIGHT_HOURS) return fail("invalid_args", "not_after must be within 8 h");

  const cert = {
    v: 1, typ: misbehave("old-cert") ? "other-session" : "buwiz-session", plugin: manifest().id, app: app.id, open_id: args.open_id, ks_pub: args.ks_pub,
    perms_sha256: misbehave("wrong-perms") ? sha256hex("not the permissions") : args.perms_sha256, not_after: args.not_after,
  };

  const signer = misbehave("bad-cert") ? generate().seed : key.seed;

  sessions.set(args.open_id, app.id);

  return ok({ cert, cert_sig: sign(signer, signedBytes(cert)) });
}

function ticketResolve(args) {
  const subject = tickets.get(args?.ticket);

  // a title only, never the subject itself (P3)
  return ok(subject === undefined ? { valid: false } : { valid: true, title: `Item ${subject}` });
}

function uninstall() {
  tickets.clear();
  bySubject.clear();
  sessions.clear();

  return ok({ ok: true });
}

let accountSetFailed = false;
let accountSets = 0;
// `account-set-fails-after:<n>`: every buwiz.account.set after the first n fails
const setsThatWork = Number((flags.find((f) => f.startsWith("account-set-fails-after:")) || "").split(":")[1] ?? Infinity);

function accountSet(args) {
  if (misbehave("no-accounts")) throw new McpError(ErrorCode.MethodNotFound, "buwiz.account.set is not implemented");
  if (misbehave("account-set-fails-once") && !accountSetFailed) {
    accountSetFailed = true;
    throw new McpError(ErrorCode.InternalError, "buwiz.account.set failed");
  }
  if (++accountSets > setsThatWork) throw new McpError(ErrorCode.InternalError, "buwiz.account.set failed");

  if (!isString(args?.account) || !isString(args?.label) || !isString(args?.access_token)) return fail("invalid_args", "send {account, label, access_token, …}");
  accountPushes.push({ push: "set", label: args.label, accounts: args.accounts });
  accounts.set(`${args.account}/${args.label}`, { subject: args.subject ?? "", expires_at: args.expires_at ?? null, token: sha256hex(args.access_token).slice(0, 12), sets: (accounts.get(`${args.account}/${args.label}`)?.sets || 0) + 1 });

  return ok({ ok: true });
}

function accountClear(args) {
  if (misbehave("no-accounts")) throw new McpError(ErrorCode.MethodNotFound, "buwiz.account.clear is not implemented");
  accounts.delete(`${args?.account}/${args?.label}`);
  accountPushes.push({ push: "clear", label: args?.label, accounts: args?.accounts });

  return ok({ ok: true });
}

const RESERVED = {
  "buwiz.provision": provision,
  "buwiz.session.open": sessionOpen,
  "buwiz.session.close": (args) => (sessions.delete(args?.open_id), ok({ ok: true })),
  "buwiz.ticket.resolve": ticketResolve,
  "buwiz.uninstall": uninstall,
  "buwiz.account.set": accountSet,
  "buwiz.account.clear": accountClear,
};

// ── the fixture's own tools ──────────────────────────────────────────────
const sleep = (ms, signal) =>
  new Promise((resolve) => {
    const t = setTimeout(resolve, ms);

    signal?.addEventListener("abort", () => {
      clearTimeout(t);
      cancelled++;
      writeStats();
      resolve();
    });
  });

function resolveCommand(value) {
  if (value === "foreign") return { result: "open", plugin: "app.buwiz.other", app: "elsewhere", title: "Elsewhere" };

  if (value === "card") return { result: "open", app: "card", props: { ticket: ticketFor("card") }, ticket: ticketFor("card"), title: "Fixture Card", placement: "inline" };

  if (value === "bad-props") return { result: "open", app: "card", props: { label: "no ticket" }, title: "Fixture Card" };

  // starter flows (15.2): text for the composer, never sent by Buwiz Bot
  if (value === "prompt") return { result: "prompt", text: "Draft this month's board update:\n- what shipped\n- what is next" };

  if (value === "prompt-long") return { result: "prompt", text: "x".repeat(4001) };

  if (value === "prompt-blank") return { result: "prompt", text: " \n\t " };

  return { result: "open", app: "board", title: "Fixture Board", placement: "full" };
}

function resolveScope(args) {
  const value = String(args.value ?? "");

  if (args.complete) {
    const n = value === "many" ? 25 : 3;

    return {
      result: "choices",
      choices: Array.from({ length: n }, (_, i) => ({
        label: value === "many" ? `Item ${i} ${"l".repeat(100)}` : `Item ${value}${i}`,
        detail: value === "many" ? "d".repeat(200) : `the ${i}th item`,
        value: ticketFor(`${value}${i}`),
      })),
    };
  }

  // a scope never answers prompt (15.2): the broker refuses it
  if (value === "prompt") return { result: "prompt", text: "Attach this item" };

  if (!value || value === "error") return { result: "error", message: `No item matches that. ${"x".repeat(300)}` };

  return { result: "token", ticket: ticketFor(value), title: `Item ${value}` };
}

// a tool result with the `_meta` a test asked for
const withMeta = (r, args) => (isObject(args.result_meta) ? { ...r, _meta: args.result_meta } : r);

const TOOLS = [
  { name: "fx_echo", description: "Echo the arguments and the caller.", inputSchema: { type: "object" }, run: async (args, meta) => (args.reload && (await server.sendResourceListChanged()), withMeta(ok({ args, caller: meta?.["buwiz/caller"] ?? null }), args)) },
  {
    name: "fx_slow", description: "Sleep for ms milliseconds, then answer.", inputSchema: { type: "object", properties: { ms: { type: "integer" } } },
    run: async (args, _meta, extra) => (await sleep(Number(args.ms) || 0, extra.signal), ok({ slept: Number(args.ms) || 0 })),
  },
  { name: "fx_fail", description: "Fail on purpose.", inputSchema: { type: "object" }, run: () => fail("tool_error", "fx_fail: it failed on purpose") },
  {
    name: "fx_big", description: "Answer with bytes characters of text (as: \"resource\" embeds them as an HTML document).",
    inputSchema: { type: "object", properties: { bytes: { type: "integer" }, as: { type: "string" } } },
    run: (args) => {
      const text = "x".repeat(Math.max(0, Number(args.bytes) || 0));

      return { content: [args.as === "resource" ? { type: "resource", resource: { uri: "fixture://doc.html", mimeType: "text/html", text } } : { type: "text", text }] };
    },
  },
  {
    name: "fx_resolve", description: "Resolve the fixture's command and scopes.", inputSchema: { type: "object" },
    run: (args) => ok(args.kind === "command" ? resolveCommand(String(args.value ?? "")) : resolveScope(args)),
  },
  {
    name: "fx_open", description: "Answer with a card for the fixture card app.",
    inputSchema: {
      type: "object",
      properties: { fail: { type: "boolean" }, app: { type: "string" }, size: { type: "string" }, snapshot: { type: "object" }, suggest: { type: "array" }, title: { type: "string" } },
    },
    run: (args) => {
      const t = ticketFor("opened");
      const props = { ticket: t, label: args.title || "Opened" };

      // 16.2: an answer card's snapshot and when it was taken
      if (args.snapshot) Object.assign(props, { snapshot: args.snapshot, as_of: Date.now() });
      const open = { app: args.app || "card", props, ticket: t, title: args.title || "Fixture Card", placement: "inline" };

      if (args.size) open.size = args.size;
      const result = {
        content: [{ type: "text", text: args.fail ? "fx_open failed" : "Here is the card." }],
        _meta: { "buwiz/open": open, ...(args.suggest ? { "buwiz/suggest": args.suggest } : {}) },
      };

      if (args.fail) result.isError = true;

      return result;
    },
  },
  {
    name: "fx_search", description: "Search fixture items (read only).", inputSchema: { type: "object", properties: { query: { type: "string" } } }, annotations: { readOnlyHint: true },
    run: (args) => withMeta(ok({ items: [{ id: String(args.query || "all") }] }), args),
  },
  {
    name: "fx_query", description: "A routine's search text: {query} from args.q.", inputSchema: { type: "object", properties: { q: { type: "string" } } },
    run: (args) => ok({ query: isString(args.q) ? args.q : "from:fixture" }),
  },
  {
    name: "fx_badge", description: "Answer with a buwiz/badge of args.badge (a routine's query tool too).",
    inputSchema: { type: "object", properties: { badge: {}, fail: { type: "boolean" } } },
    run: (args) => ({ ...(args.fail ? fail("tool_error", "fx_badge failed") : ok({ badged: true, query: "from:badge", items: [] })), _meta: { "buwiz/badge": args.badge } }),
  },
  { name: "fx_secret", description: "A tool no app may call.", inputSchema: { type: "object" }, run: () => ok({ secret: true }) },
  { name: "fx_app_only", description: "A tool for apps only (16.5 audience).", inputSchema: { type: "object" }, meta: { "buwiz/audience": ["app"] }, run: () => ok({ app: true }) },
  { name: "fx_stats", description: "What the fixture was asked.", inputSchema: { type: "object" }, run: () => ok(stats()) },
  {
    name: "fx_reload", description: "Tell the host the resources changed.", inputSchema: { type: "object" },
    run: async () => (await server.sendResourceListChanged(), ok({ ok: true })),
  },
];

// ── the MCP framing: what the SDK's low-level Server did for this fixture ──
// JSON-RPC 2.0 lines on stdin and stdout; requests run concurrently and are
// answered when done (a cancelled one is not answered); an unknown method is
// -32601 "Method not found"; a thrown McpError answers its code and its
// "MCP error <code>: <message>" text, any other throw -32603.
function mcpServer(serverInfo, { capabilities, instructions }) {
  const handlers = new Map();
  const running = new Map();
  let clientCapabilities;
  const send = (m) => process.stdout.write(`${JSON.stringify(m)}\n`);
  const SUPPORTED = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];

  handlers.set("initialize", (req) => {
    clientCapabilities = req.params.capabilities;
    queueMicrotask(() => writeStats());

    return {
      protocolVersion: SUPPORTED.includes(req.params.protocolVersion) ? req.params.protocolVersion : SUPPORTED[0],
      capabilities,
      serverInfo,
      ...(instructions && { instructions }),
    };
  });
  handlers.set("ping", () => ({}));

  const onRequest = (req) => {
    const handler = handlers.get(req.method);

    if (!handler) return send({ jsonrpc: "2.0", id: req.id, error: { code: ErrorCode.MethodNotFound, message: "Method not found" } });
    const abort = new AbortController();

    running.set(req.id, abort);
    Promise.resolve()
      .then(() => handler(req, { signal: abort.signal }))
      .then(
        (result) => {
          if (!abort.signal.aborted) send({ result, jsonrpc: "2.0", id: req.id });
        },
        (error) => {
          if (abort.signal.aborted) return;
          const code = Number.isSafeInteger(error?.code) ? error.code : ErrorCode.InternalError;

          send({ jsonrpc: "2.0", id: req.id, error: { code, message: error?.message ?? "Internal error", ...(error?.data !== undefined && { data: error.data }) } });
        },
      )
      .finally(() => {
        if (running.get(req.id) === abort) running.delete(req.id);
      });
  };

  createInterface({ input: process.stdin }).on("line", (line) => {
    let m;

    try {
      m = JSON.parse(line);
    } catch {
      return;
    }

    if (m.method && m.id !== undefined) onRequest(m);
    else if (m.method === "notifications/cancelled") running.get(m.params?.requestId)?.abort(m.params?.reason);
  });

  return {
    setRequestHandler(method, handler) {
      handlers.set(
        method,
        method === "tools/call"
          ? async (req, extra) => {
              if (typeof req.params?.name !== "string") throw new McpError(ErrorCode.InvalidParams, "Invalid tools/call request: name must be a string");
              const result = await handler(req, extra);

              // as the schema's default: a result always has its content list
              return result.content === undefined ? { ...result, content: [] } : result;
            }
          : handler,
      );
    },
    getClientCapabilities: () => clientCapabilities,
    sendResourceListChanged: async () => send({ jsonrpc: "2.0", method: "notifications/resources/list_changed" }),
  };
}

// ── the MCP server ───────────────────────────────────────────────────────
const instructions = process.env.FX_INSTRUCTIONS ?? "## Fixture\nUse fx_search to find items; fx_echo answers with what it was sent.";
const server = mcpServer({ name: "buwiz-fixture", version: "1.0.0" }, { capabilities: { tools: {}, resources: { listChanged: true } }, instructions });

const manifestUri = "buwiz://manifest";

server.setRequestHandler("tools/list", () => ({
  tools: [
    ...Object.keys(RESERVED).map((name) => ({ name, description: "Reserved for Buwiz Bot.", inputSchema: { type: "object" } })),
    ...TOOLS.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema, annotations: t.annotations, ...(t.meta && { _meta: t.meta }) })),
  ],
}));

server.setRequestHandler("tools/call", async (req, extra) => {
  const { name, arguments: args = {}, _meta: meta } = req.params;

  if (Object.hasOwn(RESERVED, name)) {
    const result = RESERVED[name](args);

    writeStats();

    return result;
  }

  if (isReservedTool(name)) throw new McpError(ErrorCode.MethodNotFound, `${name} is not a tool of this plugin`);

  // P7: nothing before a successful provision
  if (!key && !misbehave("skip-provision")) throw new McpError(ErrorCode.InvalidRequest, "buwiz.provision must come first");

  const tool = TOOLS.find((t) => t.name === name);

  if (!tool) throw new McpError(ErrorCode.InvalidParams, `no tool ${name}`);
  calls[name] = (calls[name] || 0) + 1;
  origins[name] = meta?.["buwiz/caller"]?.origin ?? null;
  served++;
  maxInflight = Math.max(maxInflight, ++inflight);
  writeStats();

  if (crashAfter && served > crashAfter) process.exit(3);

  try {
    return await tool.run(isObject(args) ? args : {}, meta, extra);
  } finally {
    inflight--;
  }
});

const APPS = { board: "board.tsx", card: "card.tsx" };

server.setRequestHandler("resources/list", () => ({
  resources: [
    { uri: manifestUri, name: "manifest", mimeType: "application/json" },
    ...Object.keys(APPS).map((id) => ({ uri: `buwiz-app://${id}`, name: id, mimeType: "text/x-nsh-tsx" })),
  ],
}));

server.setRequestHandler("resources/read", (req) => {
  const { uri } = req.params;

  if (uri === manifestUri) return { contents: [{ uri, mimeType: "application/json", text: readFileSync(MANIFEST, "utf8") }] };
  const id = uri.startsWith("buwiz-app://") ? uri.slice(12) : "";
  const file = process.env[`FX_APP_${id.toUpperCase()}`] || (APPS[id] && join(HERE, APPS[id]));

  if (!file) throw new McpError(ErrorCode.InvalidParams, `no resource ${uri}`);

  return { contents: [{ uri, mimeType: "text/x-nsh-tsx", text: readFileSync(file, "utf8") }] };
});

