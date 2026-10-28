// sdk/mini-app/lib/rox-plugin.mjs — the reserved half of a v1 plugin.
//
// Every Rox Mini App Protocol v1 plugin answers the same reserved tools
// (spec 7.8) the same way; this module does it once so a plugin's own file
// holds only its tools and apps. What it guarantees:
//
// - rox.provision is answered first; every other tool fails until it has
//   succeeded (P7). The plugin key is kept in memory only, never logged,
//   written or echoed (4.3, P4).
// - rox.session.open checks the app exists and the permissions digest
//   equals the manifest's, then signs the session certificate over JCS
//   with the plugin key (4.4); not_after is at most 8 h ahead.
// - rox.ticket.resolve answers a title, never the subject (P3).
// - rox.uninstall forgets every ticket and session (9.5).
// - Any other rox.* name is MCP error -32601 (7.9).
// - The manifest and app sources are served as resources (5.1, 6.1); the
//   manifest is re-read from disk on every request, so a re-signed file
//   takes effect without a restart.
//
// Tickets (9.5): `tickets.issue(subject)` returns a random tkt_ value and
// keeps the subject on this side; `tickets.subject(t)` maps it back for
// the plugin's own tools. Subjects are strings the plugin chooses.
import { randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema, ErrorCode, ListResourcesRequestSchema, ListToolsRequestSchema, McpError, ReadResourceRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { isV1 } from "../../../server/grammar.mjs";
import { pubOf, sign } from "../../../server/v1/ed.mjs";
import { isObject, parseIJson, signedBytes } from "../../../server/v1/jcs.mjs";
import { MANIFEST_URI, manifestHash, permsDigest } from "../../../server/v1/manifest.mjs";

const EIGHT_HOURS = 8 * 3600 * 1000;
const APP_MIME = "text/x-nsh-tsx";
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** A tool result whose structuredContent is `data` (and its text). */
export const ok = (data) => ({ content: [{ type: "text", text: JSON.stringify(data) }], structuredContent: data });

/** An isError tool result with a spec error code (14.1) and a message. */
export const fail = (code, message) => ({ isError: true, content: [{ type: "text", text: message }], structuredContent: { code, message } });

/** A fresh ticket: tkt_ + 26 Crockford base32 characters, 130 random bits. */
export function newTicket() {
  let n = BigInt(`0x${randomBytes(17).toString("hex")}`) >> 6n;
  let s = "";

  for (let i = 0; i < 26; i++, n >>= 5n) s = CROCKFORD[Number(n & 31n)] + s;

  return `tkt_${s}`;
}

/** The ticket store: subject ↔ ticket, in this process only. */
export function ticketStore() {
  const bySubject = new Map();
  const byTicket = new Map();

  return {
    issue(subject) {
      if (!bySubject.has(subject)) {
        const t = newTicket();

        bySubject.set(subject, t);
        byTicket.set(t, subject);
      }

      return bySubject.get(subject);
    },
    subject: (ticket) => byTicket.get(ticket),
    clear() {
      bySubject.clear();
      byTicket.clear();
    },
  };
}

/**
 * Starts a v1 plugin on stdio.
 *
 *   manifestFile  path of the signed manifest (read on every request)
 *   apps          {appId: path of the .tsx source}
 *   tools         [{name, description, inputSchema, annotations?, run(args, caller, extra)}]
 *                 `caller` is _meta["rox/caller"] (8.4): {origin, plugin, app?, open_id?, ts}
 *   ticketTitle   (subject) => the title rox.ticket.resolve shows
 *   instructions  MCP server instructions (approved text, 10.2), optional
 *   name, version the MCP server identity
 */
export async function startPlugin(opts) {
  const { manifestFile, apps = {}, tools = [], ticketTitle = (s) => String(s), instructions, name = "rox-plugin", version = "1.0.0" } = opts;
  const tickets = opts.tickets || ticketStore();
  const sessions = new Map(); // open_id → app id
  let key = null; // {seed, pub}: memory only

  const manifestText = () => readFileSync(manifestFile, "utf8");
  const manifest = () => parseIJson(manifestText());

  function provision(args) {
    let pub = null;

    try {
      pub = args?.key?.alg === "ed25519" ? pubOf(args.key.seed) : null;
    } catch {}

    if (!pub || pub !== args.pub) return fail("invalid_args", "the key and pub do not match");

    if (key && key.seed !== args.key.seed) return fail("invalid_args", "already provisioned with another key");
    key = { seed: args.key.seed, pub };

    return ok({ ok: true, manifest_sha256: manifestHash(manifest()) });
  }

  function sessionOpen(args) {
    const m = manifest();
    const app = (m.apps || []).find((a) => a.id === args?.app);

    if (!app) return fail("invalid_args", `no app ${String(args?.app).slice(0, 60)}`);

    if (args.perms_sha256 !== permsDigest(app.permissions)) return fail("invalid_args", "perms_sha256 does not match the manifest");

    if (!Number.isSafeInteger(args.not_after) || args.not_after > Date.now() + EIGHT_HOURS) return fail("invalid_args", "not_after must be within 8 h");

    const cert = { v: 1, typ: "rox-session", plugin: m.id, app: app.id, open_id: args.open_id, ks_pub: args.ks_pub, perms_sha256: args.perms_sha256, not_after: args.not_after };

    sessions.set(args.open_id, app.id);

    return ok({ cert, cert_sig: sign(key.seed, signedBytes(cert)) });
  }

  function ticketResolve(args) {
    const subject = tickets.subject(args?.ticket);

    return ok(subject === undefined ? { valid: false } : { valid: true, title: String(ticketTitle(subject)).slice(0, 120) });
  }

  const reserved = {
    "rox.provision": provision,
    "rox.session.open": sessionOpen,
    "rox.session.close": (args) => (sessions.delete(args?.open_id), ok({ ok: true })),
    "rox.ticket.resolve": ticketResolve,
    "rox.uninstall": () => (tickets.clear(), sessions.clear(), ok({ ok: true })),
  };

  const options = { capabilities: { tools: {}, resources: { listChanged: true } } };

  if (instructions) options.instructions = instructions;
  const server = new Server({ name, version }, options);

  server.setRequestHandler(ListToolsRequestSchema, () => ({
    tools: [
      ...Object.keys(reserved).map((n) => ({ name: n, description: "Reserved for Rox.", inputSchema: { type: "object" } })),
      ...tools.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema || { type: "object" }, annotations: t.annotations })),
    ],
  }));

  server.setRequestHandler(CallToolRequestSchema, async (req, extra) => {
    const { name: tool, arguments: args = {}, _meta: meta } = req.params;

    // provision first; no other rox.* tool works without a key either
    if (tool === "rox.provision") return provision(args);

    if (isV1("reservedTool", tool) && !Object.hasOwn(reserved, tool)) throw new McpError(ErrorCode.MethodNotFound, `${tool} is not a tool of this plugin`);

    if (!key) throw new McpError(ErrorCode.InvalidRequest, "rox.provision must come first");

    if (Object.hasOwn(reserved, tool)) return reserved[tool](args);
    const t = tools.find((x) => x.name === tool);

    if (!t) throw new McpError(ErrorCode.InvalidParams, `no tool ${tool}`);

    return t.run(isObject(args) ? args : {}, meta?.["rox/caller"] ?? null, extra);
  });

  server.setRequestHandler(ListResourcesRequestSchema, () => ({
    resources: [
      { uri: MANIFEST_URI, name: "manifest", mimeType: "application/json" },
      ...Object.keys(apps).map((id) => ({ uri: `rox-app://${id}`, name: id, mimeType: APP_MIME })),
    ],
  }));

  server.setRequestHandler(ReadResourceRequestSchema, (req) => {
    const { uri } = req.params;

    if (uri === MANIFEST_URI) return { contents: [{ uri, mimeType: "application/json", text: manifestText() }] };
    const id = uri.startsWith("rox-app://") ? uri.slice(10) : "";

    if (!Object.hasOwn(apps, id)) throw new McpError(ErrorCode.InvalidParams, `no resource ${uri}`);

    return { contents: [{ uri, mimeType: APP_MIME, text: readFileSync(apps[id], "utf8") }] };
  });

  await server.connect(new StdioServerTransport());

  return { server, tickets };
}
