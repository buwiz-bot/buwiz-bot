// A stand-in for the user's Google Workspace MCP server (workspace-mcp) in
// the gws plugin and routine tests: search_gmail_messages and
// get_gmail_messages_content_batch, answering in workspace-mcp's own text
// format (gmail/gmail_tools.py: _format_gmail_results_plain and the batch
// tool's "Message ID / headers / --- BODY ---" blocks), so the plugin's
// parsing is tested against what the real server prints.
//
// FAKE_GWS_MESSAGES names a JSON file of [{id, threadId?, from, to?,
// subject, date, body}], read on every call (tests add mail between
// ticks). A search returns every message unless the query contains
// "nothing". FAKE_GWS_LOG, when set, gets one JSON line per call
// {tool, args}. FAKE_GWS_FAIL=1 makes every call an error result.
import { appendFileSync, readFileSync } from "node:fs";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";

const messages = () => {
  try {
    return JSON.parse(readFileSync(process.env.FAKE_GWS_MESSAGES, "utf8"));
  } catch {
    return [];
  }
};

function text(t, isError = false) {
  const r = { content: [{ type: "text", text: t }] };

  if (isError) r.isError = true;

  return r;
}

const url = (id) => `https://mail.google.com/mail/u/0/#all/${id}`;

function search({ query, page_size = 10 }) {
  const found = String(query).includes("nothing") ? [] : messages().slice(0, page_size);

  if (!found.length) return text(`No messages found for query: '${query}'`);
  const lines = [`Found ${found.length} messages matching '${query}':`, "", "📧 MESSAGES:"];

  found.forEach((m, i) => {
    const thread = m.threadId || `t-${m.id}`;

    lines.push(`  ${i + 1}. Message ID: ${m.id}`, `     Web Link: ${url(m.id)}`, `     Thread ID: ${thread}`, `     Thread Link: ${url(thread)}`, "");
  });
  lines.push("💡 USAGE:", "  • Pass the Message IDs **as a list** to get_gmail_messages_content_batch()", "    e.g. get_gmail_messages_content_batch(message_ids=[...])");

  return text(lines.join("\n"));
}

function batch({ message_ids = [] }) {
  const all = messages();

  const out = message_ids.map((id) => {
    const m = all.find((x) => x.id === id);

    if (!m) return `⚠️ Message ${id}: <HttpError 404 "Requested entity was not found.">\n`;
    const head = [`Message ID: ${m.id}`, `Subject: ${m.subject}`, `From: ${m.from}`, `Date: ${m.date}`, `Message-ID: <${m.id}@mail.example>`, `To: ${m.to ?? "[not present in Gmail response]"}`, "Cc: [not present in Gmail response]"];

    return `${head.join("\n")}\nWeb Link: ${url(m.id)}\n\n--- BODY ---\n${m.body}\n`;
  });

  return text(`Retrieved ${message_ids.length} messages:\n\n${out.join("\n---\n\n")}`);
}

const TOOLS = [
  {
    name: "search_gmail_messages",
    inputSchema: { type: "object", properties: { query: { type: "string" }, user_google_email: { type: "string" }, page_size: { type: "integer" } }, required: ["query", "user_google_email"] },
    annotations: { readOnlyHint: true },
    run: search,
  },
  {
    name: "get_gmail_messages_content_batch",
    inputSchema: { type: "object", properties: { message_ids: { type: "array", items: { type: "string" } }, user_google_email: { type: "string" }, format: { type: "string" } }, required: ["message_ids", "user_google_email"] },
    annotations: { readOnlyHint: true },
    run: batch,
  },
];

const server = new Server({ name: "fake-workspace-mcp", version: "0.0.0" }, { capabilities: { tools: {} } });

server.setRequestHandler(ListToolsRequestSchema, () => ({ tools: TOOLS.map(({ run: _run, ...t }) => ({ description: t.name, ...t })) }));

server.setRequestHandler(CallToolRequestSchema, (req) => {
  const { name, arguments: args = {} } = req.params;

  if (process.env.FAKE_GWS_LOG) appendFileSync(process.env.FAKE_GWS_LOG, `${JSON.stringify({ tool: name, args })}\n`);

  if (process.env.FAKE_GWS_FAIL === "1") return text("ACTION REQUIRED: Google Authentication Needed", true);
  const tool = TOOLS.find((t) => t.name === name);

  if (!tool) return text(`Unknown tool: ${name}`, true);

  if (!args.user_google_email) return text("user_google_email is required", true);

  return tool.run(args);
});

await server.connect(new StdioServerTransport());
