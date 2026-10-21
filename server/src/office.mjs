// server/office.mjs — BetterOffice (openooxml/betteroffice, Apache-2.0) glue.
//
//   Generation: real .docx/.xlsx/.pptx bytes straight from the wasm cores —
//   the same payload contract the bundled MCP server's office_create uses.
//   Edits/browse/render go through the MCP child (server/office/mcp-main.ts,
//   vendored packages/agents), which the model reaches as office_* tools.
//   Viewing: the app opens GET /office/?path=… in a WebView; the page paints
//   slides/sheets with the engines' own display lists (Canvas2D), so a deck
//   finally has a real renderer. All of it degrades: no packages, no wasm,
//   or a crashed child → the office tools drop out of toolList() and the
//   viewer kinds fall back to the existing renderers.
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SEEDS = join(HERE, "office", "seeds");
const MCP_MAIN = join(HERE, "office", "mcp-main.ts");
// the packaged app ships mcp-main bundled to .js — its SDK/zod deps are
// inlined, so it needs no --experimental-transform-types and no node_modules
// beyond the vendored @betteroffice engines
const MCP_MAIN_MJS = join(HERE, "office", "mcp-main.js");

// The packages only declare "import" export conditions, so CJS
// require.resolve reports ERR_PACKAGE_PATH_NOT_EXPORTED — resolve through
// import.meta.resolve instead and lift the package dir off the entry file.
function pkgEntry(name) {
  try {
    return fileURLToPath(import.meta.resolve(name));
  } catch {
    return null;
  }
}
function pkgRootDir(name) {
  const entry = pkgEntry(name);
  if (!entry) return null;
  // walk up until the package.json whose name matches — covers dist/ and
  // deeper entry files (docx resolves to dist/core.mjs)
  let dir = dirname(entry);
  for (let i = 0; i < 6; i++) {
    try {
      const pj = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
      if (pj.name === name) return dir;
    } catch {}
    const up = dirname(dir);
    if (up === dir) break;
    dir = up;
  }
  return dirname(entry);
}

// ————— capability probe —————
// A kind is "viewable" when its engine package + wasm resolve; "editable" adds
// nothing over what the MCP child already checks — it reports its own errors.
const PKG = { docx: "@betteroffice/docx", xlsx: "@betteroffice/xlsx", pptx: "@betteroffice/pptx" };
let capsCache = null;
export function officeCaps() {
  if (capsCache) return capsCache;
  const caps = { docx: false, xlsx: false, pptx: false };
  for (const [kind, pkg] of Object.entries(PKG)) {
    try {
      const dir = pkgDistDir(kind);
      caps[kind] = !!dir;
    } catch {}
  }
  return (capsCache = caps);
}
/** node_modules/@betteroffice/<kind>/dist — or null when not installed. */
export function pkgDistDir(kind) {
  const root = pkgRootDir(PKG[kind]);
  return root ? join(root, "dist") : null;
}
/** Package dir holding the font .ttf assets (fonts/dist + fonts/assets). */
export function fontsPkgDir() {
  return pkgRootDir("@betteroffice/fonts");
}
// /office/vendor/<key>/… serves the installed packages — dist/ bundles and the
// fonts' assets/ — so the viewer page's ESM imports and wasm self-loads work.
const VENDOR_PKGS = {
  docx: "@betteroffice/docx",
  xlsx: "@betteroffice/xlsx",
  pptx: "@betteroffice/pptx",
  fonts: "@betteroffice/fonts",
  "docx-react": "@betteroffice/docx-react",
  "xlsx-react": "@betteroffice/xlsx-react",
  "pptx-react": "@betteroffice/pptx-react",
};
export function officePkgRoot(key) {
  const name = VENDOR_PKGS[key];
  return name ? pkgRootDir(name) : null;
}

// ————— wasm bytes for the engines (Node can't fetch self-relative wasm URLs) —————
const wasmCache = new Map();
function wasmBytes(kind) {
  let p = wasmCache.get(kind);
  if (!p) {
    const dir = pkgDistDir(kind);
    if (!dir) return Promise.resolve(undefined);
    const file = kind === "docx" ? null : `${kind}_wasm_bg.wasm`;
    p = file
      ? readFile(join(dir, "generated", file)).then((b) => new Uint8Array(b), () => undefined)
      : Promise.resolve(undefined);
    wasmCache.set(kind, p);
  }
  return p;
}

// A pptx the model should read comes back as its structured markdown export —
// the deck has no text fallback otherwise. Anything the engine refuses or a
// missing package turns into null, and the caller keeps its own description.
export async function officeMarkdown(path) {
  try {
    const { exportPptxMarkdown, initWasm } = await import("@betteroffice/pptx");
    await initWasm(await wasmBytes("pptx"));
    const r = await exportPptxMarkdown(new Uint8Array(await readFile(path)));
    return (r && (r.markdown ?? r.content?.markdown)) || null;
  } catch {
    return null;
  }
}

// ————— creation (mjs port of the vendored create.ts — the MCP server's
// office_create accepts the same payloads) —————
// Mirrors plainText() in server/office/agents/prototype.ts: reject unpaired
// surrogates and XML control characters before the engines see them.
function plainText(value, label) {
  if (typeof value !== "string" || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/.test(value))
    throw new Error(`${label} must contain complete Unicode characters and no XML control characters`);
}
const HEADINGS = {
  title: { bold: true, fontSize: 64, spaceAfter: 240 },
  heading1: { bold: true, fontSize: 40, outlineLevel: 0, spaceBefore: 240, spaceAfter: 120 },
  heading2: { bold: true, fontSize: 32, outlineLevel: 1, spaceBefore: 160, spaceAfter: 80 },
  normal: { bold: false, fontSize: 24 },
};
// EMU rectangles for a 4:3 seed slide (9144000 x 6858000 EMU).
const TITLE_RECT = { x: 457200, y: 274320, width: 8229600, height: 822960 };
const BODY_RECT = { x: 457200, y: 1371600, width: 8229600, height: 5029200 };

export async function officeCreateBytes(format, input = {}) {
  if (format === "docx") return createDocxBytes(input.docx);
  if (format === "xlsx") return createXlsxBytes(input.xlsx);
  if (format === "pptx") return createPptxBytes(input.pptx);
  throw new Error(`unknown office format ${format}`);
}

async function createDocxBytes(input) {
  if (!input?.paragraphs?.length) throw new Error("docx requires at least one paragraph");
  if (input.paragraphs.length > 5000) throw new Error("docx creation is limited to 5000 paragraphs");
  const { createDocx } = await import("@betteroffice/docx");
  const content = input.paragraphs.map((paragraph) => {
    if (paragraph.text.length > 16000) throw new Error("paragraph text must fit in 16000 UTF-16 units");
    plainText(paragraph.text, "paragraph text");
    const heading = HEADINGS[paragraph.style ?? "normal"] ?? HEADINGS.normal;
    const spacing = {};
    if ("outlineLevel" in heading) spacing.outlineLevel = heading.outlineLevel;
    if ("spaceBefore" in heading) spacing.spaceBefore = heading.spaceBefore;
    if ("spaceAfter" in heading) spacing.spaceAfter = heading.spaceAfter;
    return {
      type: "paragraph",
      formatting: spacing,
      content: [
        {
          type: "run",
          formatting: { bold: paragraph.bold ?? heading.bold, ...(paragraph.italic !== undefined ? { italic: paragraph.italic } : {}), fontSize: heading.fontSize },
          content: [{ type: "text", text: paragraph.text }],
        },
      ],
    };
  });
  return new Uint8Array(await createDocx({ package: { document: { content } } }));
}

async function createXlsxBytes(input) {
  if (!input?.rows?.length || !input.rows.some((row) => row.length)) throw new Error("xlsx requires rows: string[][]");
  if (input.rows.length > 4096 || input.rows.some((row) => row.length > 256)) throw new Error("xlsx creation is limited to 4096x256 cells");
  const { initWasm, openWorkbook } = await import("@betteroffice/xlsx");
  await initWasm(await wasmBytes("xlsx"));
  const workbook = openWorkbook(new Uint8Array(await readFile(join(SEEDS, "blank.xlsx"))));
  try {
    const edits = input.rows.flatMap((row, r) => row.map((cell, c) => {
      if (String(cell).length > 16000) throw new Error("cell text must fit in 16000 UTF-16 units");
      plainText(String(cell), "cell");
      return { row: r, col: c, input: String(cell) };
    }));
    workbook.editCells(0, edits);
    return new Uint8Array(workbook.save());
  } finally {
    workbook.dispose();
  }
}

async function createPptxBytes(input) {
  if (!input?.slides?.length) throw new Error("pptx requires at least one slide");
  if (input.slides.length > 200) throw new Error("pptx creation is limited to 200 slides");
  const { initWasm, openPresentation } = await import("@betteroffice/pptx");
  await initWasm(await wasmBytes("pptx"));
  const presentation = openPresentation(new Uint8Array(await readFile(join(SEEDS, "blank.pptx"))));
  try {
    input.slides.forEach((slide, index) => {
      for (const text of [slide.title ?? "", ...(slide.bullets ?? []), slide.notes ?? ""]) plainText(String(text), "slide text");
      const { slideId } = presentation.insertSlide(index);
      if (slide.title) presentation.addTextBox(slideId, { name: "Title", rect: TITLE_RECT, text: String(slide.title), style: { fontSizePt: 32, bold: true } });
      if (slide.bullets?.length) presentation.addTextBox(slideId, { name: "Body", rect: BODY_RECT, text: slide.bullets.map(String).join("\n"), style: { fontSizePt: 18 } });
      if (slide.notes) presentation.setSlideNotes(slideId, String(slide.notes));
    });
    return new Uint8Array(presentation.save());
  } finally {
    presentation.dispose();
  }
}

// ————— the MCP child: full office_* surface for follow-up edits —————
// Spawned lazily on the first office tool call; a dead child marks the whole
// surface unavailable rather than looping spawn attempts on a broken install.
let mcpClient = null;
let mcpTools = null; // anthropic-shaped tool defs, once discovered
let mcpFailed = null;

export async function ensureOfficeMcp(root) {
  if (mcpClient) return mcpClient;
  if (mcpFailed) return null;
  try {
    const [{ Client }, { StdioClientTransport }] = await Promise.all([
      import("@modelcontextprotocol/sdk/client/index.js"),
      import("@modelcontextprotocol/sdk/client/stdio.js"),
    ]);
    const bundled = existsSync(MCP_MAIN_MJS);
    const transport = new StdioClientTransport({
      command: process.execPath,
      args: bundled
        ? [MCP_MAIN_MJS, "--root", root]
        : ["--experimental-transform-types", MCP_MAIN, "--root", root],
      stderr: "pipe",
    });
    const client = new Client({ name: "grok-shell", version: "1.0.0" });
    transport.stderr?.on("data", (d) => {
      const line = String(d).trim();
      if (line && !line.startsWith("(node:") && !line.includes("ExperimentalWarning")) console.error(`  office-mcp: ${line}`);
    });
    await client.connect(transport);
    const { tools } = await client.listTools();
    mcpTools = tools.map((t) => ({
      name: t.name,
      description: t.description || "",
      input_schema: t.inputSchema || { type: "object", properties: {} },
    }));
    mcpClient = client;
    return client;
  } catch (e) {
    mcpFailed = String(e?.message || e);
    console.error(`  office mcp unavailable: ${mcpFailed.slice(0, 160)}`);
    return null;
  }
}

export function officeMcpTools() {
  // office_create is the app's own tool — its version attaches the file card.
  return (mcpTools || []).filter((t) => t.name !== "office_create");
}

// MCP results come back as content blocks; fold text (and note images) into
// one string the model reads as the tool result.
export async function callOfficeTool(root, name, input) {
  const client = await ensureOfficeMcp(root);
  if (!client) return { content: `office tools unavailable — the BetterOffice MCP server failed to start (${mcpFailed || "not installed"})`, is_error: true };
  const result = await client.callTool({ name, arguments: input || {} });
  const parts = (result.content || []).map((c) => {
    if (c.type === "text") return c.text;
    if (c.type === "image") return `[image: ${c.mimeType || "image/png"}, ${Math.round((c.data?.length || 0) * 0.75)} bytes]`;
    return JSON.stringify(c);
  });
  return { content: parts.join("\n") || "(empty)", is_error: !!result.isError };
}
