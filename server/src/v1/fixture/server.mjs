#!/usr/bin/env node
// v1/fixture/server.mjs — the conformance fixture plugin (stdio MCP).
//
// Plugin id dev.goldcoders.fixture, short `fx`. It implements every
// reserved tool of spec 7.8 the way a conforming plugin must (the key in
// memory only, provision first, certificates over JCS, tickets with a
// subject map that never leaves the process), plus tools that exercise the
// broker: fx_echo (echoes args and `_meta["rox/caller"]`), fx_slow, fx_fail,
// fx_big, fx_resolve, fx_open, fx_search (readOnlyHint), fx_query (a
// routine's search text), fx_secret (in no app's permissions), fx_stats
// (what it was asked, and each tool's last caller origin) and fx_reload
// (sends resources/list_changed).
//
// The manifest is read from FX_MANIFEST on every request (tests sign one
// per run and may change it); the app sources sit beside this file.
// `--misbehave=<mode>[,<mode>]` breaks one rule on purpose for negative
// tests: skip-provision, bad-cert, log-seed, wrong-perms, crash-after:<n>.
import { randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema, ErrorCode, ListResourcesRequestSchema, ListToolsRequestSchema, McpError, ReadResourceRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { isV1 } from "../../grammar.mjs";
import { generate, pubOf, sign } from "../ed.mjs";
import { isObject, isString, parseIJson, signedBytes } from "../jcs.mjs";
import { manifestHash, permsDigest, sha256hex } from "../manifest.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const MANIFEST = process.env.FX_MANIFEST || join(HERE, "manifest.json");
const flags = (process.argv.find((a) => a.startsWith("--misbehave=")) || "").slice(12).split(",").filter(Boolean);
const misbehave = (mode) => flags.some((f) => f === mode || f.startsWith(`${mode}:`));
const crashAfter = Number(flags.find((f) => f.startsWith("crash-after:"))?.split(":")[1] || 0);
const EIGHT_HOURS = 8 * 3600 * 1000;

// memory only (4.3): never logged, written or echoed
let key = null;
const sessions = new Map(); // open_id → app
const tickets = new Map(); // ticket → subject
const bySubject = new Map(); // subject → ticket
const calls = {};
const origins = {}; // tool → the last caller's origin (8.4)
let cancelled = 0;
let served = 0;

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

// ── reserved tools (7.8) ─────────────────────────────────────────────────
function provision(args) {
  if (misbehave("skip-provision")) throw new McpError(ErrorCode.MethodNotFound, "rox.provision is not implemented");

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
    v: 1, typ: "rox-session", plugin: manifest().id, app: app.id, open_id: args.open_id, ks_pub: args.ks_pub,
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

const RESERVED = {
  "rox.provision": provision,
  "rox.session.open": sessionOpen,
  "rox.session.close": (args) => (sessions.delete(args?.open_id), ok({ ok: true })),
  "rox.ticket.resolve": ticketResolve,
  "rox.uninstall": uninstall,
};

// ── the fixture's own tools ──────────────────────────────────────────────
const sleep = (ms, signal) =>
  new Promise((resolve) => {
    const t = setTimeout(resolve, ms);

    signal?.addEventListener("abort", () => {
      clearTimeout(t);
      cancelled++;
      resolve();
    });
  });

function resolveCommand(value) {
  if (value === "foreign") return { result: "open", plugin: "dev.goldcoders.other", app: "elsewhere", title: "Elsewhere" };

  if (value === "card") return { result: "open", app: "card", props: { ticket: ticketFor("card") }, ticket: ticketFor("card"), title: "Fixture Card", placement: "inline" };

  if (value === "bad-props") return { result: "open", app: "card", props: { label: "no ticket" }, title: "Fixture Card" };

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

  if (!value || value === "error") return { result: "error", message: `No item matches that. ${"x".repeat(300)}` };

  return { result: "token", ticket: ticketFor(value), title: `Item ${value}` };
}

const TOOLS = [
  { name: "fx_echo", description: "Echo the arguments and the caller.", inputSchema: { type: "object" }, run: (args, meta) => ok({ args, caller: meta?.["rox/caller"] ?? null }) },
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

      // 9.8: an answer card's snapshot and when it was taken
      if (args.snapshot) Object.assign(props, { snapshot: args.snapshot, as_of: Date.now() });
      const open = { app: args.app || "card", props, ticket: t, title: args.title || "Fixture Card", placement: "inline" };

      if (args.size) open.size = args.size;
      const result = {
        content: [{ type: "text", text: args.fail ? "fx_open failed" : "Here is the card." }],
        _meta: { "rox/open": open, ...(args.suggest ? { "rox/suggest": args.suggest } : {}) },
      };

      if (args.fail) result.isError = true;

      return result;
    },
  },
  {
    name: "fx_search", description: "Search fixture items (read only).", inputSchema: { type: "object", properties: { query: { type: "string" } } }, annotations: { readOnlyHint: true },
    run: (args) => ok({ items: [{ id: String(args.query || "all") }] }),
  },
  {
    name: "fx_query", description: "A routine's search text: {query} from args.q.", inputSchema: { type: "object", properties: { q: { type: "string" } } },
    run: (args) => ok({ query: isString(args.q) ? args.q : "from:fixture" }),
  },
  { name: "fx_secret", description: "A tool no app may call.", inputSchema: { type: "object" }, run: () => ok({ secret: true }) },
  { name: "fx_app_only", description: "A tool for apps only (10.1 audience).", inputSchema: { type: "object" }, meta: { "rox/audience": ["app"] }, run: () => ok({ app: true }) },
  { name: "fx_stats", description: "What the fixture was asked.", inputSchema: { type: "object" }, run: () => ok({ calls, origins, cancelled, sessions: sessions.size, tickets: tickets.size }) },
  {
    name: "fx_reload", description: "Tell the host the resources changed.", inputSchema: { type: "object" },
    run: async () => (await server.sendResourceListChanged(), ok({ ok: true })),
  },
];

// ── the MCP server ───────────────────────────────────────────────────────
const instructions = process.env.FX_INSTRUCTIONS ?? "## Fixture\nUse fx_search to find items; fx_echo answers with what it was sent.";
const server = new Server({ name: "rox-fixture", version: "1.0.0" }, { capabilities: { tools: {}, resources: { listChanged: true } }, instructions });

server.setRequestHandler(ListToolsRequestSchema, () => ({
  tools: [
    ...Object.keys(RESERVED).map((name) => ({ name, description: "Reserved for Rox.", inputSchema: { type: "object" } })),
    ...TOOLS.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema, annotations: t.annotations, ...(t.meta && { _meta: t.meta }) })),
  ],
}));

server.setRequestHandler(CallToolRequestSchema, async (req, extra) => {
  const { name, arguments: args = {}, _meta: meta } = req.params;

  if (Object.hasOwn(RESERVED, name)) return RESERVED[name](args);

  if (isV1("reservedTool", name)) throw new McpError(ErrorCode.MethodNotFound, `${name} is not a tool of this plugin`);

  // P7: nothing before a successful provision
  if (!key && !misbehave("skip-provision")) throw new McpError(ErrorCode.InvalidRequest, "rox.provision must come first");

  const tool = TOOLS.find((t) => t.name === name);

  if (!tool) throw new McpError(ErrorCode.InvalidParams, `no tool ${name}`);
  calls[name] = (calls[name] || 0) + 1;
  origins[name] = meta?.["rox/caller"]?.origin ?? null;
  served++;

  if (crashAfter && served > crashAfter) process.exit(3);

  return tool.run(isObject(args) ? args : {}, meta, extra);
});

const APPS = { board: "board.tsx", card: "card.tsx" };

server.setRequestHandler(ListResourcesRequestSchema, () => ({
  resources: [
    { uri: "rox://manifest", name: "manifest", mimeType: "application/json" },
    ...Object.keys(APPS).map((id) => ({ uri: `rox-app://${id}`, name: id, mimeType: "text/x-nsh-tsx" })),
  ],
}));

server.setRequestHandler(ReadResourceRequestSchema, (req) => {
  const { uri } = req.params;

  if (uri === "rox://manifest") return { contents: [{ uri, mimeType: "application/json", text: readFileSync(MANIFEST, "utf8") }] };
  const id = uri.startsWith("rox-app://") ? uri.slice(10) : "";
  const file = process.env[`FX_APP_${id.toUpperCase()}`] || (APPS[id] && join(HERE, APPS[id]));

  if (!file) throw new McpError(ErrorCode.InvalidParams, `no resource ${uri}`);

  return { contents: [{ uri, mimeType: "text/x-nsh-tsx", text: readFileSync(file, "utf8") }] };
});

await server.connect(new StdioServerTransport());
