// mcp-line.mjs — a v0 MCP server's client held by rox-broker (Rust broker
// plan 4.1, 4.2, D12), behind the line: the SDK Client's methods mcp.mjs
// uses (callTool, listResources, readResource, getServerCapabilities,
// close), so mcp.mjs keeps its own logic and folds the answers as before.
// Results go through the SDK's schemas here, as plugins-line.mjs's do.
// rox-broker starts a server whose process or stream died again on its
// next use. A remote server's OAuth runs there too: the sign-in a route
// starts and the callback that finishes it.
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

/** Connects `id`'s server in rox-broker: {client, tools, needsAuth}.
 *  Throws the connection's error, worded as the SDK words it (with its
 *  HTTP status as `code`). */
export async function lineConnect(id) {
  const r = await post("mcp/ensure", { id });

  if (!r.ok) throw Object.assign(new Error(r.err), { code: r.code });
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

  return { client, tools: ListToolsResultSchema.parse({ tools: r.tools || [] }).tools, needsAuth: r.needsAuth || null };
}

/** mcp.mjs's beginAuth in rox-broker: {ok, authorized} or {ok,
 *  redirected, authorizeUrl?}; throws the flow's error. */
export async function lineAuthBegin(id, open) {
  const r = await post("mcp/auth/begin", { id, open });

  if (!r.ok) throw new Error(r.error);

  return r;
}

/** The OAuth callback in rox-broker: "connected", "refused" (no such
 *  flow, a wrong state) or "failed" (with why, when the exchange failed). */
export async function lineAuthFinish(id, { code, state, error }) {
  const r = await post("mcp/auth/finish", { id, code, state, error });

  return { result: r.result || "failed", error: r.error || "" };
}
