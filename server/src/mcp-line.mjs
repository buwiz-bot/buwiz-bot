// mcp-line.mjs — a v0 MCP server's client held by rox-broker (Rust broker
// plan 4.1, D12), behind the line: the SDK Client's methods mcp.mjs uses
// (callTool, listResources, readResource, getServerCapabilities, close),
// so mcp.mjs keeps its own logic and folds the answers as before. Results
// go through the SDK's schemas here, as plugins-line.mjs's do. rox-broker
// starts a server whose process died again on its next use.
import { CallToolResultSchema, ListResourcesResultSchema, ListToolsResultSchema, ReadResourceResultSchema } from "@modelcontextprotocol/sdk/types.js";
import { LINE } from "./v1/line-env.mjs";

/** Whether rox-broker hosts the v0 MCP servers (ROX_LINE_HOSTS names mcp). */
export const mcpOnLine = !!LINE.base && LINE.hosts.includes("mcp");

async function post(path, body) {
  const res = await fetch(`${LINE.base}/v1/line/${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-rox-line": LINE.token },
    body: JSON.stringify(body),
  });

  return res.json();
}

/** Connects `id`'s server in rox-broker: {client, tools}. Throws the
 *  connection's error, worded as the SDK words it. */
export async function lineConnect(id) {
  const r = await post("mcp/ensure", { id });

  if (!r.ok) throw new Error(r.err);
  const capabilities = r.capabilities || {};

  const call = async (path, body, Schema) => {
    const a = await post(path, { id, ...body });

    if (!a.ok) throw Object.assign(new Error(a.error), { code: a.code });

    return Schema.parse(a.result);
  };

  const client = {
    callTool: (params, _schema, opts = {}) =>
      call("mcp/call", { tool: params.name, args: params.arguments || {}, ...(opts.timeout ? { timeout_ms: opts.timeout } : {}) }, CallToolResultSchema),
    listResources: () => call("mcp/resources", {}, ListResourcesResultSchema),
    readResource: ({ uri }) => call("mcp/read", { uri }, ReadResourceResultSchema),
    getServerCapabilities: () => capabilities,
    close: () => post("mcp/invalidate", { id }).catch(() => {}),
  };

  return { client, tools: ListToolsResultSchema.parse({ tools: r.tools || [] }).tools };
}
