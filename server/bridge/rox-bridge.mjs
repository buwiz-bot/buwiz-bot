#!/usr/bin/env node
// rox-bridge — a Claude Code session talking with a Rox agent
// (drafts/agent-messaging.md §10.3). An MCP server on stdio that calls the
// Rox broker's /v1/bridge/* routes with a bridge endpoint's token. Make the
// endpoint in Rox (POST /v1/host/agents/endpoints/create {bot, kind:
// "bridge", name}), then add this server to Claude Code:
//
//   claude mcp add rox -e ROX_BRIDGE_TOKEN=<token> -- node <rox>/server/bridge/rox-bridge.mjs
//
// ROX_BRIDGE_URL is the broker (default http://127.0.0.1:8787). The bridge
// is one participant ("Claude Code · <its endpoint name>") in the agent's
// conversations, which Rox shows read-only with a notice in the agent's chat.
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";

const BASE = (process.env.ROX_BRIDGE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const TOKEN = process.env.ROX_BRIDGE_TOKEN || "";

const TOOLS = [
  {
    name: "rox_agents_list",
    description: "The Rox agent this bridge talks with: id, name and what it is for.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "rox_send",
    description:
      "Send the Rox agent a message. Leave out `conversation` to start one (then `subject` is required). With expects_reply the agent works on it and answers; read the answer with rox_receive.",
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
    name: "rox_receive",
    description: "Messages the Rox agent sent this bridge (its answers included), oldest first. wait_s (at most 30) waits for one when there is none yet.",
    inputSchema: { type: "object", properties: { wait_s: { type: "integer" }, conversation: { type: "string" } } },
  },
  {
    name: "rox_conversations",
    description: "This bridge's recent conversations with the Rox agent.",
    inputSchema: { type: "object", properties: { limit: { type: "integer" } } },
  },
];

const ROUTES = {
  rox_agents_list: ["GET", "/v1/bridge/agents"],
  rox_send: ["POST", "/v1/bridge/send"],
  rox_receive: ["POST", "/v1/bridge/receive"],
  rox_conversations: ["GET", "/v1/bridge/conversations"],
};

async function call(name, args) {
  const [method, path] = ROUTES[name] || [];

  if (!method) return { content: [{ type: "text", text: `unknown tool ${name}` }], isError: true };

  if (!TOKEN) return { content: [{ type: "text", text: "ROX_BRIDGE_TOKEN is not set" }], isError: true };

  try {
    const res = await fetch(BASE + path, {
      method,
      headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" },
      ...(method === "POST" ? { body: JSON.stringify(args || {}) } : {}),
      signal: AbortSignal.timeout(40_000),
    });
    const body = await res.json().catch(() => ({}));

    if (!res.ok || body.ok === false) return { content: [{ type: "text", text: `${body.code || res.status}: ${body.message || "refused"}` }], isError: true };
    delete body.ok;

    return { content: [{ type: "text", text: JSON.stringify(body) }] };
  } catch (e) {
    return { content: [{ type: "text", text: `Rox is not reachable at ${BASE}: ${String(e?.message || e).slice(0, 200)}` }], isError: true };
  }
}

const server = new Server({ name: "rox-bridge", version: "1.0.0" }, { capabilities: { tools: {} } });

server.setRequestHandler(ListToolsRequestSchema, () => ({ tools: TOOLS }));
server.setRequestHandler(CallToolRequestSchema, (req) => call(req.params.name, req.params.arguments));
await server.connect(new StdioServerTransport());
