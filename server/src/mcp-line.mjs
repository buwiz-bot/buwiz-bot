// mcp-line.mjs — a v0 MCP server's client held by buwiz-broker (Rust broker
// plan 4.1, 4.2, D12), behind the line: the SDK Client's methods mcp.mjs
// uses (callTool, listResources, readResource, getServerCapabilities,
// close), so mcp.mjs keeps its own logic and folds the answers as before.
// Results go through the SDK's schemas here, as plugins-line.mjs's do.
// buwiz-broker starts a server whose process or stream died again on its
// next use. A remote server's OAuth runs there too: the sign-in a route
// starts and the callback that finishes it.
import { CallToolResultSchema, ListResourcesResultSchema, ListToolsResultSchema, ReadResourceResultSchema } from "@modelcontextprotocol/sdk/types.js";
import { LINE } from "./v1/line-env.mjs";

/** Whether buwiz-broker hosts the v0 MCP servers (BUWIZ_LINE_HOSTS names mcp). */
export const mcpOnLine = !!LINE.base && LINE.hosts.includes("mcp");

// buwiz-broker owns each server's status (plan 4.3); this is its mirror:
// every line answer carries the status after it, and the line's `mcp`
// events bring the changes made elsewhere (its own /mcp routes).
const mirror = new Map(); // id -> {status, tools, err?}
let onDrop = () => {};

/** buwiz-broker's status of `id` ({status, tools, err?}), as statusOf. */
export const lineStatus = (id) => mirror.get(id) || { status: "idle", tools: 0 };

/** `f(id)` when buwiz-broker drops a server's client (it went idle there). */
export const onLineDrop = (f) => (onDrop = f);

function note(id, st) {
  if (!id || !st || typeof st !== "object") return;
  const was = mirror.get(id)?.status;

  if (st.status === "idle") mirror.delete(id);
  else mirror.set(id, { status: st.status, tools: st.tools || 0, ...(st.err ? { err: st.err } : {}) });
  if (st.status === "idle" && was && was !== "idle") onDrop(id);
}

async function post(path, body) {
  const res = await fetch(`${LINE.base}/v1/line/${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-buwiz-line": LINE.token },
    body: JSON.stringify(body),
  });
  const out = await res.json();

  if (body?.id && out?.status) note(body.id, out.status);

  return out;
}

let listening = new AbortController();

/** Stops following the line's events (a test's teardown). */
export function stopLineEvents() {
  listening.abort();
}

async function listen() {
  while (!listening.signal.aborted) {
    try {
      const res = await fetch(`${LINE.base}/v1/line/events`, { headers: { "x-buwiz-line": LINE.token }, signal: listening.signal });
      const decoder = new TextDecoder();
      let buf = "";

      for await (const chunk of res.body) {
        buf += decoder.decode(chunk, { stream: true });
        let cut;

        while ((cut = buf.indexOf("\n\n")) >= 0) {
          const block = buf.slice(0, cut);

          buf = buf.slice(cut + 2);
          if (/^event: (.*)$/m.exec(block)?.[1] !== "mcp") continue;
          const st = JSON.parse(/^data: (.*)$/m.exec(block)?.[1] || "{}");

          note(st.id, st);
        }
      }
    } catch {}

    if (!listening.signal.aborted) await new Promise((r) => setTimeout(r, 500));
  }
}

if (mcpOnLine) listen();

/** Connects `id`'s server in buwiz-broker: {client, tools, needsAuth}.
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

/** mcp.mjs's beginAuth in buwiz-broker: {ok, authorized} or {ok,
 *  redirected, authorizeUrl?}; throws the flow's error. */
export async function lineAuthBegin(id, open) {
  const r = await post("mcp/auth/begin", { id, open });

  if (!r.ok) throw new Error(r.error);

  return r;
}

/** The OAuth callback in buwiz-broker: "connected", "refused" (no such
 *  flow, a wrong state) or "failed" (with why, when the exchange failed). */
export async function lineAuthFinish(id, { code, state, error }) {
  const r = await post("mcp/auth/finish", { id, code, state, error });

  return { result: r.result || "failed", error: r.error || "" };
}
