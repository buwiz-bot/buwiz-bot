// server/mcp.mjs — user-connected MCP servers (stdio).
//
// app-server owns the mcp_servers table and the endpoints; this module owns
// client lifecycle: lazy spawn on first use, tool discovery, namespaced tool
// defs (mcp_<id>_<tool>), and call routing. A failed spawn caches its error
// and quietly leaves the surface out of toolList() — the model just doesn't
// see those tools. Deleting or editing a server invalidates its client.

const attempts = new Map(); // id -> Promise<{client,tools,err}>
const recs = new Map(); // id -> settled {client,tools,err}

export function statusOf(id) {
  const r = recs.get(id);
  if (!r) return { status: "idle", tools: 0 };
  if (r.client) return { status: "connected", tools: r.tools.length };
  return { status: "error", tools: 0, err: r.err };
}

export function invalidate(id) {
  const r = recs.get(id);
  attempts.delete(id);
  recs.delete(id);
  try {
    r?.client?.close();
  } catch {}
}

// Spawn + handshake + tools/list once per row; every caller shares the one
// attempt. The row is the mcp_servers record (command/args/env as stored).
export function ensureClient(row) {
  let p = attempts.get(row.id);
  if (!p) {
    p = connect(row);
    attempts.set(row.id, p);
    p.then((r) => recs.set(row.id, r)).catch(() => {});
  }
  return p;
}

// Minimal Streamable-HTTP MCP client for `type:"http"` plugin servers:
// POST JSON-RPC with Accept json+sse, carry mcp-session-id between calls.
function httpClient(row) {
  const url = row.url;
  let sessionId = null;
  const rpc = async (method, params, notify = false) => {
    const body = { jsonrpc: "2.0", method, ...(notify ? {} : { id: Math.floor(Math.random() * 1e9) }), ...(params ? { params } : {}) };
    const headers = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      "mcp-protocol-version": "2025-03-26",
    };
    if (sessionId) headers["mcp-session-id"] = sessionId;
    const r = await fetch(url, { method: "POST", headers, body: JSON.stringify(body), signal: AbortSignal.timeout(60_000) });
    const sid = r.headers.get("mcp-session-id");
    if (sid) sessionId = sid;
    if (notify || r.status === 202) return null;
    if (!r.ok) throw new Error(`http ${r.status}`);
    const ct = r.headers.get("content-type") || "";
    if (ct.includes("text/event-stream")) {
      const text = await r.text();
      let last = null;
      for (const line of text.split("\n")) {
        if (line.startsWith("data:")) { try { last = JSON.parse(line.slice(5)); } catch {} }
      }
      if (last?.error) throw new Error(last.error.message || "rpc error");
      return last?.result ?? last;
    }
    const j = await r.json();
    if (j?.error) throw new Error(j.error.message || "rpc error");
    return j?.result ?? j;
  };
  return {
    listTools: async () => {
      await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "grok-shell", version: "1.0.0" } });
      await rpc("notifications/initialized", null, true);
      return rpc("tools/list", {});
    },
    callTool: async ({ name, arguments: args }) => rpc("tools/call", { name, arguments: args || {} }),
    close: () => {},
  };
}

async function connect(row) {
  const rec = { client: null, tools: [], err: null };
  try {
    if (row.url) {
      const client = httpClient(row);
      const { tools } = await client.listTools();
      rec.client = client;
      rec.tools = tools || [];
      console.log(`  mcp ${row.id}: connected (http), ${rec.tools.length} tools`);
      return rec;
    }
    const [{ Client }, { StdioClientTransport }] = await Promise.all([
      import("@modelcontextprotocol/sdk/client/index.js"),
      import("@modelcontextprotocol/sdk/client/stdio.js"),
    ]);
    const transport = new StdioClientTransport({
      command: row.command,
      args: JSON.parse(row.args || "[]"),
      env: { ...process.env, ...JSON.parse(row.env || "{}") },
      stderr: "pipe",
    });
    const client = new Client({ name: "grok-shell", version: "1.0.0" });
    transport.stderr?.on("data", (d) => {
      const line = String(d).trim();
      if (
        line &&
        !line.startsWith("(node:") &&
        !line.includes("ExperimentalWarning")
      )
        console.error(`  mcp ${row.id}: ${line.slice(0, 200)}`);
    });
    await client.connect(transport);
    const { tools } = await client.listTools();
    rec.client = client;
    rec.tools = tools || [];
    console.log(`  mcp ${row.id}: connected, ${rec.tools.length} tools`);
  } catch (e) {
    rec.err = String(e?.message || e).slice(0, 300);
    console.error(`  mcp ${row.id}: ${rec.err}`);
  }
  return rec;
}

// Tool names the model sees: mcp_<id>_<tool>. Ids are [a-z0-9-] (no
// underscores) so the name parses back unambiguously; Anthropic's charset is
// [a-zA-Z0-9_-] ≤64.
export const mcpName = (id, tool) =>
  `mcp_${id}_${tool}`.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64);
export const parseMcpName = (n) =>
  n.match(/^mcp_([a-z0-9-]+)_(.+)$/)?.slice(1) || null;

// Anthropic-shaped defs for a server's discovered tools.
export function anthropicTools(id, tools) {
  return tools.map((t) => ({
    name: mcpName(id, t.name),
    description: `[${id}] ${t.description || t.name}`.slice(0, 1024),
    input_schema: t.inputSchema || { type: "object", properties: {} },
  }));
}

// Fold MCP content blocks into the single string the model reads.
export async function callTool(row, toolName, input) {
  const rec = await ensureClient(row);
  if (!rec.client)
    return {
      content: `mcp server ${row.id} is not connected (${rec.err || "failed to start"})`,
      is_error: true,
    };
  const result = await rec.client.callTool(
    { name: toolName, arguments: input || {} },
    undefined,
    { timeout: 60_000 },
  );
  const parts = (result.content || []).map((c) => {
    if (c.type === "text") return c.text;
    if (c.type === "image")
      return `[image: ${c.mimeType || "image/png"}, ${Math.round((c.data?.length || 0) * 0.75)} bytes]`;
    return JSON.stringify(c);
  });
  return { content: parts.join("\n") || "(empty)", is_error: !!result.isError };
}
