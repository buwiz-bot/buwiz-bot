#!/usr/bin/env node
// v1/plugins/google-workspace/server.mjs — the first-party Google Workspace
// plugin (stdio MCP), id dev.goldcoders.google-workspace, short `gws`.
//
// v1 routines read mail only through `reads`, from read-only tools of an
// approved plugin (spec 5.9, Q1); this plugin is that source. It holds no
// Google credentials of its own: it proxies to the user's existing Google
// Workspace MCP server (workspace-mcp, the one v0's gmailAdapter talks to),
// spawned from GWS_UPSTREAM = {"command", "args", "env"} with the account
// from GWS_USER_EMAIL. Both come from the install record's env, so nothing
// secret lives in code. The upstream answers in text ("Message ID: …"
// lines, "--- BODY ---" sections); gws_search_mail and gws_read_mail turn
// that into structured items. A missing or failing upstream is a plain
// isError result, never a crash.
//
// The reserved tools (7.8) follow the fixture: the key in memory only,
// provision first. The plugin has no apps, so session opens are refused,
// and no tickets, so every ticket is invalid. The manifest is read from
// GWS_MANIFEST, else manifest.json beside this file (signed at release).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema, ErrorCode, ListResourcesRequestSchema, ListToolsRequestSchema, McpError, ReadResourceRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { isV1 } from "../../../grammar.mjs";
import { pubOf } from "../../ed.mjs";
import { isObject, isString, parseIJson } from "../../jcs.mjs";
import { manifestHash } from "../../manifest.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const MANIFEST = process.env.GWS_MANIFEST || join(HERE, "manifest.json");
const BODY_MAX = 64 * 1024;
const SNIPPET_MAX = 200;
const UPSTREAM_TIMEOUT_MS = 60_000;
const SEARCH = "search_gmail_messages";
const BATCH = "get_gmail_messages_content_batch";

// memory only (4.3): never logged, written or echoed
let key = null;

const manifestText = () => readFileSync(MANIFEST, "utf8");

// ── results ──────────────────────────────────────────────────────────────
const ok = (structured) => ({ content: [{ type: "text", text: JSON.stringify(structured) }], structuredContent: structured });
const fail = (code, message) => ({ isError: true, content: [{ type: "text", text: message }], structuredContent: { code, message } });
const plainError = (message) => ({ isError: true, content: [{ type: "text", text: message }] });

// ── reserved tools (7.8) ─────────────────────────────────────────────────
function provision(args) {
  if (args?.key?.alg !== "ed25519" || pubOf(args.key.seed) !== args.pub) return fail("invalid_args", "the key and pub do not match");

  if (key && key.seed !== args.key.seed) return fail("invalid_args", "already provisioned with another key");
  key = { seed: args.key.seed, pub: args.pub };

  return ok({ ok: true, manifest_sha256: manifestHash(parseIJson(manifestText())) });
}

const RESERVED = {
  "rox.provision": provision,
  "rox.session.open": (args) => fail("invalid_args", `no app ${String(args?.app).slice(0, 60)}: this plugin has no apps`),
  "rox.session.close": () => ok({ ok: true }),
  "rox.ticket.resolve": () => ok({ valid: false }),
  "rox.uninstall": () => ok({ ok: true }),
};

// ── the upstream Google Workspace MCP server ─────────────────────────────
let upstream = null; // {client, tools, closed}

function upstreamSpec() {
  let spec;

  try {
    spec = JSON.parse(process.env.GWS_UPSTREAM || "");
  } catch {
    spec = null;
  }

  if (!isObject(spec) || !isString(spec.command) || !spec.command)
    throw new Error("Google Workspace is not set up: the plugin's GWS_UPSTREAM does not name the Google Workspace MCP server to use");

  return { command: spec.command, args: Array.isArray(spec.args) ? spec.args.map(String) : [], env: isObject(spec.env) ? spec.env : {} };
}

const within = (p, ms, what) =>
  Promise.race([p, new Promise((_, reject) => setTimeout(() => reject(new Error(`${what} did not answer within ${ms / 1000} s`)), ms).unref())]);

async function connectUpstream() {
  if (upstream && !upstream.closed) return upstream;
  const spec = upstreamSpec();
  const transport = new StdioClientTransport({ ...spec, stderr: "inherit" });
  const client = new Client({ name: "rox-gws", version: "1.0.0" });
  const up = { client, tools: [], closed: false };

  client.onclose = () => {
    up.closed = true;

    if (upstream === up) upstream = null;
  };

  try {
    await within(client.connect(transport), 30_000, "the Google Workspace server");
    up.tools = (await within(client.listTools(), 30_000, "the Google Workspace server")).tools || [];
  } catch (e) {
    await transport.close().catch(() => {});
    throw new Error(`the Google Workspace server could not start: ${String(e?.message || e).slice(0, 200)}`);
  }

  upstream = up;

  return up;
}

// The upstream tool's text; the account argument when its schema takes one.
async function upstreamCall(name, args) {
  const up = await connectUpstream();
  const tool = up.tools.find((t) => t.name === name);

  if (!tool) throw new Error(`the Google Workspace server has no ${name} tool`);
  const props = tool.inputSchema?.properties || {};
  const email = String(process.env.GWS_USER_EMAIL || "").trim();

  if (props.user_google_email && !email && tool.inputSchema?.required?.includes("user_google_email"))
    throw new Error("Google Workspace is not set up: the plugin's GWS_USER_EMAIL (the Google address to read) is empty");
  const full = props.user_google_email && email ? { ...args, user_google_email: email } : args;
  let r;

  try {
    r = await up.client.callTool({ name, arguments: full }, undefined, { timeout: UPSTREAM_TIMEOUT_MS });
  } catch (e) {
    throw new Error(`the Google Workspace server failed: ${String(e?.message || e).slice(0, 200)}`);
  }

  const text = (r.content || []).filter((c) => c.type === "text").map((c) => c.text).join("\n");

  if (r.isError) throw new Error(`the Google Workspace server refused ${name}: ${text.slice(0, 300) || "no reason given"}`);

  return text;
}

// ── workspace-mcp's text → items ─────────────────────────────────────────
// search_gmail_messages: "  1. Message ID: <id>" per message
const searchIds = (text) => [...String(text).matchAll(/^\s*\d+\.\s+Message ID: (\S+)\s*$/gm)].flatMap((m) => (m[1] === "unknown" ? [] : [m[1]]));

const header = (block, name) => {
  const v = new RegExp(`^${name}: (.*)$`, "m").exec(block)?.[1]?.trim() ?? "";

  return v.startsWith("[not present") ? "" : v;
};

// get_gmail_messages_content_batch: per message "Message ID: <id>", header
// lines, "--- BODY ---", the body, maybe "--- ATTACHMENTS ---"; messages
// joined by "\n---\n\n"; a failed one is "⚠️ Message <id>: <error>".
function batchMessages(text) {
  const s = String(text);
  const starts = [...s.matchAll(/^(?:Message ID: (\S+)|⚠️ Message \S+:.*)$/gm)];

  return starts.flatMap((m, i) => {
    if (!m[1]) return [];
    const block = s.slice(m.index, i + 1 < starts.length ? starts[i + 1].index : s.length).replace(/\n+---\n*$/, "");
    const at = block.indexOf("\n--- BODY ---\n");
    // headers come before the body: a forwarded mail's "From:" is not one
    const head = at < 0 ? block : block.slice(0, at);
    let body = at < 0 ? "" : block.slice(at + 14);
    const att = body.lastIndexOf("\n--- ATTACHMENTS ---\n");

    if (att >= 0) body = body.slice(0, att);

    return [{ id: m[1], from: header(head, "From"), to: header(head, "To"), subject: header(head, "Subject"), date: header(head, "Date"), body: body.replace(/\n+$/, "") }];
  });
}

const snippetOf = (body) => {
  const flat = body.replace(/\s+/g, " ").trim();

  return flat.length > SNIPPET_MAX ? `${flat.slice(0, SNIPPET_MAX - 1)}…` : flat;
};

function capBytes(text, max) {
  if (Buffer.byteLength(text) <= max) return text;
  const marker = "…[truncated]";
  let cut = Buffer.from(text).subarray(0, max - Buffer.byteLength(marker)).toString("utf8");

  // a multi-byte character cut in half decodes to U+FFFD: drop it
  cut = cut.replace(/�$/, "");

  return `${cut}${marker}`;
}

// ── the plugin's tools ───────────────────────────────────────────────────
async function searchMail(args) {
  const query = String(args.query ?? "").trim();

  if (!query) return plainError("gws_search_mail needs a query");
  const limit = Math.max(1, Math.min(25, Math.round(Number(args.limit) || 10)));
  const ids = searchIds(await upstreamCall(SEARCH, { query, page_size: limit })).slice(0, limit);

  if (!ids.length) return ok({ items: [] });
  const byId = new Map(batchMessages(await upstreamCall(BATCH, { message_ids: ids, format: "full" })).map((m) => [m.id, m]));

  return ok({
    items: ids.map((id) => {
      const m = byId.get(id);

      return { id, from: m?.from || "", subject: m?.subject || "", date: m?.date || "", snippet: m ? snippetOf(m.body) : "" };
    }),
  });
}

async function readMail(args) {
  const id = String(args.id ?? "").trim();

  if (!id) return plainError("gws_read_mail needs an id");
  const m = batchMessages(await upstreamCall(BATCH, { message_ids: [id], format: "full" })).find((x) => x.id === id);

  if (!m) return plainError(`no message ${id.slice(0, 80)}`);

  return ok({ id, from: m.from, to: m.to, subject: m.subject, date: m.date, body: capBytes(m.body, BODY_MAX) });
}

const READ_ONLY = { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: true };

const TOOLS = [
  {
    name: "gws_search_mail",
    description: "Search the user's Gmail (Gmail search syntax). Returns {items: [{id, from, subject, date, snippet}]}; read one with gws_read_mail.",
    inputSchema: {
      type: "object",
      properties: { query: { type: "string", description: "Gmail search, e.g. from:bir.gov.ph newer_than:2d" }, limit: { type: "integer", minimum: 1, maximum: 25, description: "at most this many messages (default 10)" } },
      required: ["query"],
    },
    annotations: READ_ONLY,
    run: searchMail,
  },
  {
    name: "gws_read_mail",
    description: "Read one Gmail message by id (from gws_search_mail). Returns {id, from, to, subject, date, body}; the body is plain text, at most 64 KiB.",
    inputSchema: { type: "object", properties: { id: { type: "string", description: "the message id" } }, required: ["id"] },
    annotations: READ_ONLY,
    run: readMail,
  },
];

// ── the MCP server ───────────────────────────────────────────────────────
const instructions = "Gmail is read-only here: gws_search_mail finds messages, gws_read_mail reads one. Neither sends, labels or deletes mail.";
const server = new Server({ name: "rox-google-workspace", version: "1.0.0" }, { capabilities: { tools: {}, resources: {} }, instructions });

server.setRequestHandler(ListToolsRequestSchema, () => ({
  tools: [
    ...Object.keys(RESERVED).map((name) => ({ name, description: "Reserved for Rox.", inputSchema: { type: "object" } })),
    ...TOOLS.map(({ run: _run, ...t }) => t),
  ],
}));

server.setRequestHandler(CallToolRequestSchema, async (req) => {
  const { name, arguments: args = {} } = req.params;

  if (Object.hasOwn(RESERVED, name)) return RESERVED[name](args);

  if (isV1("reservedTool", name)) throw new McpError(ErrorCode.MethodNotFound, `${name} is not a tool of this plugin`);

  // P7: nothing before a successful provision
  if (!key) throw new McpError(ErrorCode.InvalidRequest, "rox.provision must come first");
  const tool = TOOLS.find((t) => t.name === name);

  if (!tool) throw new McpError(ErrorCode.InvalidParams, `no tool ${name}`);

  try {
    return await tool.run(isObject(args) ? args : {});
  } catch (e) {
    return plainError(String(e?.message || e).slice(0, 500));
  }
});

server.setRequestHandler(ListResourcesRequestSchema, () => ({ resources: [{ uri: "rox://manifest", name: "manifest", mimeType: "application/json" }] }));

server.setRequestHandler(ReadResourceRequestSchema, (req) => {
  const { uri } = req.params;

  if (uri !== "rox://manifest") throw new McpError(ErrorCode.InvalidParams, `no resource ${uri}`);

  return { contents: [{ uri, mimeType: "application/json", text: manifestText() }] };
});

await server.connect(new StdioServerTransport());
