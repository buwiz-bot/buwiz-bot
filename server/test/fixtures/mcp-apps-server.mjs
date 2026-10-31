// A stdio MCP server that ships what the /mcp routes serve: a mini app
// resource, a `buwiz://manifest` with a command and an attachable scope, and
// the resolve tool both name. Newline-delimited JSON-RPC, no SDK. The
// resolve tool echoes what it was given, so a test sees the route's
// arguments (including `context.attached`). When FIXTURE_MARK names a file,
// each start appends its pid there.
import { appendFileSync } from "node:fs";
import { createInterface } from "node:readline";

if (process.env.FIXTURE_MARK) appendFileSync(process.env.FIXTURE_MARK, `${process.pid}\n`);

const TOOLS = [
  { name: "echo", description: "Echo the arguments", inputSchema: { type: "object" } },
  { name: "resolve", description: "Resolve /go and @apps:item", inputSchema: { type: "object" } },
];

const MANIFEST = {
  title: "Apps Fixture",
  commands: [{ name: "go", description: "Open the hello app", resolve: "resolve" }],
  scopes: [{ name: "item", description: "An item", attach: true, resolve: "resolve" }],
};

const APP = "export default function R() { return null; }\n";

const send = (msg) => process.stdout.write(`${JSON.stringify(msg)}\n`);

function resolve(args) {
  if (args.complete) return { choices: [{ label: "One", value: "one" }] };

  return { ok: true, open: "buwiz://app/apps/hello", title: `Item ${args.args}`, token: `@apps:item:${args.args}`, details: { got: args } };
}

function answer(method, params) {
  switch (method) {
    case "initialize":
      return {
        protocolVersion: params?.protocolVersion,
        capabilities: { tools: {}, resources: {} },
        serverInfo: { name: "apps-fixture", version: "0.0.0" },
      };
    case "ping":
      return {};
    case "tools/list":
      return { tools: TOOLS };
    case "tools/call": {
      const args = params?.arguments ?? {};
      const out = params?.name === "resolve" ? resolve(args) : { tool: params?.name, args };

      return { content: [{ type: "text", text: JSON.stringify(out) }], structuredContent: out };
    }
    case "resources/list":
      return {
        resources: [
          { uri: "buwiz://manifest", name: "manifest", mimeType: "application/json" },
          { uri: "nsh://apps/hello", name: "hello", title: "Hello", description: "A tiny app", mimeType: "text/x-nsh-tsx" },
        ],
      };
    case "resources/read":
      return params?.uri === "buwiz://manifest"
        ? { contents: [{ uri: params.uri, mimeType: "application/json", text: JSON.stringify(MANIFEST) }] }
        : { contents: [{ uri: params?.uri, mimeType: "text/x-nsh-tsx", text: APP }] };
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

  if (msg.id === undefined) return;
  const result = answer(msg.method, msg.params);

  if (result) send({ jsonrpc: "2.0", id: msg.id, result });
  else send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: `no method ${msg.method}` } });
});

lines.on("close", () => process.exit(0));
