import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

import { createOfficeMcpServer } from "./agents/mcp.ts";
import type { DocumentRenderer } from "./agents/types.ts";

// grok-shell: the workspace is the generated-files dir (what file cards show),
// overridable with GEN_OFFICE_ROOT for testing.
function resolveRoot(env: NodeJS.ProcessEnv = process.env): string {
  return env.GEN_OFFICE_ROOT?.trim()
    || join(dirname(fileURLToPath(import.meta.url)), "..", "..", "generated", "files");
}

async function main() {
  const { values } = parseArgs({
    options: {
      root: { type: "string" },
      "read-only": { type: "boolean" },
      help: { type: "boolean", short: "h" },
    },
  });
  if (values.help) {
    process.stderr.write("Usage: office-mcp [--root <directory>] [--read-only]\n\ngrok-shell's bundled BetterOffice stdio MCP server for DOCX, XLSX, and PPTX create/read/edit/export.\n");
    return;
  }
  // Rendering needs @napi-rs/canvas and @betteroffice/fonts; when they are
  // unavailable the other tools still work and office_render reports an error.
  const renderer: DocumentRenderer | undefined = await import("./agents/render.ts").then(
    module => module.renderDocxPage,
    () => undefined,
  );
  const server = await createOfficeMcpServer({ root: values.root ?? resolveRoot(), readOnly: values["read-only"] === true, ...(renderer ? { renderer } : {}) });
  await server.connect(new StdioServerTransport(process.stdin, process.stdout, { maxBufferSize: 1024 * 1024 }));
}

main().catch(error => {
  process.stderr.write(`office-mcp: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
