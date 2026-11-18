#!/usr/bin/env node
// buwiz-bridge: lets a coding agent outside Buwiz Bot (Claude Code, say) talk
// with a Buwiz Bot agent. An MCP server on stdio that calls the broker's
// /v1/bridge/* routes with a bridge endpoint's token. Make the endpoint in
// Buwiz Bot (POST /v1/host/agents/endpoints/create {bot, kind: "bridge",
// name}), run `npm ci` once in this folder, then add this server to the
// outside agent, for Claude Code:
//
//   claude mcp add buwiz -e BUWIZ_BRIDGE_TOKEN=<token> -- node <checkout>/bridge/buwiz-bridge.mjs
//
// BUWIZ_BRIDGE_URL is the broker (default http://127.0.0.1:8787). The bridge is
// one participant ("Claude Code · <its endpoint name>") in the agent's
// conversations, which Buwiz Bot shows read-only with a notice in the agent's
// chat.
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";

// a variable of an earlier name is not read: say so and stop, rather than run
// with an empty token
const OLD_NAMES = Object.keys(process.env).filter((k) => k.startsWith("ROX_") || ["GROK_DATA_DIR", "GROK_ALLOWED_ORIGINS", "GROK_BUNDLE_ID"].includes(k)).sort();

if (OLD_NAMES.length) {
  process.stderr.write(`buwiz-bridge: these variables are set and no longer read: ${OLD_NAMES.join(", ")}; use the BUWIZ_ name instead and unset them\n`);
  process.exit(2);
}

const BASE = (process.env.BUWIZ_BRIDGE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const TOKEN = process.env.BUWIZ_BRIDGE_TOKEN || "";

const TOOLS = [
  {
    name: "buwiz_agents_list",
    description: "The Buwiz Bot agent this bridge talks with: id, name and what it is for.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "buwiz_send",
    description:
      "Send the Buwiz Bot agent a message. Leave out `conversation` to start one (then `subject` is required). With expects_reply the agent works on it and answers; read the answer with buwiz_receive.",
    inputSchema: {
      type: "object",
      properties: {
        to: { type: "array", items: { type: "string" }, description: "Agent ids; the bridge's agent when left out." },
        subject: { type: "string" },
        text: { type: "string" },
        data: { type: "object" },
        conversation: { type: "string" },
        in_reply_to: { type: "string" },
        expects_reply: { type: "boolean" },
      },
      required: ["text"],
    },
  },
  {
    name: "buwiz_receive",
    description: "Messages the Buwiz Bot agent sent this bridge (its answers included), oldest first. wait_s (at most 30) waits for one when there is none yet.",
    inputSchema: { type: "object", properties: { wait_s: { type: "integer" }, conversation: { type: "string" } } },
  },
  {
    name: "buwiz_conversations",
    description: "This bridge's recent conversations with the Buwiz Bot agent.",
    inputSchema: { type: "object", properties: { limit: { type: "integer" } } },
  },
];

const ROUTES = new Map(Object.entries({
  buwiz_agents_list: ["GET", "/v1/bridge/agents"],
  buwiz_send: ["POST", "/v1/bridge/send"],
  buwiz_receive: ["POST", "/v1/bridge/receive"],
  buwiz_conversations: ["GET", "/v1/bridge/conversations"],
}));

async function call(name, args) {
  const [method, path] = ROUTES.get(name) || [];

  if (!method) return { content: [{ type: "text", text: `unknown tool ${name}` }], isError: true };

  if (!TOKEN) return { content: [{ type: "text", text: "BUWIZ_BRIDGE_TOKEN is not set" }], isError: true };

  try {
    const res = await fetch(BASE + path, {
      method,
      headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" },
      ...(method === "POST" ? { body: JSON.stringify(args || {}) } : {}),
      signal: AbortSignal.timeout(40_000),
    });
    const body = (await res.json().catch(() => null)) ?? {};

    if (!res.ok || body.ok === false) return { content: [{ type: "text", text: `${body.code || res.status}: ${body.message || "refused"}` }], isError: true };
    delete body.ok;

    return { content: [{ type: "text", text: JSON.stringify(body) }] };
  } catch (e) {
    return { content: [{ type: "text", text: `Buwiz Bot is not reachable at ${BASE}: ${String(e?.message || e).slice(0, 200)}` }], isError: true };
  }
}

const server = new Server({ name: "buwiz-bridge", version: "1.0.0" }, { capabilities: { tools: {} } });

server.setRequestHandler(ListToolsRequestSchema, () => ({ tools: TOOLS }));
server.setRequestHandler(CallToolRequestSchema, (req) => call(req.params.name, req.params.arguments));
await server.connect(new StdioServerTransport());
