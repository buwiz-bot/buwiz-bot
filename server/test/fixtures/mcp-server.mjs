// A tiny stdio MCP server for the tests: newline-delimited JSON-RPC, no
// SDK, no network. Tools `echo` and `secret`; a `rox://manifest` resource
// that mixes valid and invalid command and scope names. When FIXTURE_MARK
// names a file, each start appends its pid there, so a test can tell
// whether the server was ever spawned.
import { appendFileSync } from "node:fs";
import { createInterface } from "node:readline";

if (process.env.FIXTURE_MARK) appendFileSync(process.env.FIXTURE_MARK, `${process.pid}\n`);

const TOOLS = [
  { name: "echo", description: "Echo the arguments", inputSchema: { type: "object" } },
  { name: "secret", description: "A tool the user switches off", inputSchema: { type: "object" } },
];

const MANIFEST = {
  title: "Fixture",
  commands: [{ name: "good" }, { name: "bad cmd" }, { name: "" }],
  scopes: [{ name: "item", attach: true }, { name: "bad.scope" }, { name: "x:y" }, { label: "no name" }],
};

const send = (msg) => process.stdout.write(`${JSON.stringify(msg)}\n`);

function answer(method, params) {
  switch (method) {
    case "initialize":
      return {
        protocolVersion: params?.protocolVersion,
        capabilities: { tools: {}, resources: {} },
        serverInfo: { name: "fixture", version: "0.0.0" },
      };
    case "ping":
      return {};
    case "tools/list":
      return { tools: TOOLS };
    case "tools/call":
      return { content: [{ type: "text", text: JSON.stringify({ tool: params?.name, args: params?.arguments ?? {} }) }] };
    case "resources/list":
      return { resources: [{ uri: "rox://manifest", name: "manifest", mimeType: "application/json" }] };
    case "resources/read":
      return { contents: [{ uri: params?.uri, mimeType: "application/json", text: JSON.stringify(MANIFEST) }] };
    default:
      return null;
  }
}

const lines = createInterface({ input: process.stdin });

lines.on("line", (line) => {
  let msg;

  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }

  if (msg.id === undefined) return; // a notification
  const result = answer(msg.method, msg.params);

  if (result) send({ jsonrpc: "2.0", id: msg.id, result });
  else send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: `no method ${msg.method}` } });
});

lines.on("close", () => process.exit(0));
