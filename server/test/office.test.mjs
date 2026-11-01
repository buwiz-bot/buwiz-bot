// Office (Rust broker plan 6.1): the BetterOffice `office_*` tools, office_create,
// the /office/* routes and /file/render for office files, pinned on Node so the
// Rust port (rox-broker's `office` module) can be held to the same answers.
//
// The tools are driven through the office MCP server as the hub calls it:
// Node's child (`node mcp-main.ts --root <dir>`, what office.mjs and
// helper/office.rs spawn) or, with BUWIZ_BROKER_CMD set, rox-broker's own
// (`rox-broker office-mcp --root <dir>`, the same in-process code the hub's
// office tools call). OFF-30 then runs office tools through a chat turn of the
// broker itself, with the fake opencodex scripting the calls, and checks the
// process tree: Node's broker runs the MCP child, rox-broker runs none.
//
// What is pinned, and why: each scripted call's answer (`structuredContent`,
// `isError`, and for an image its PNG header) goes through `norm()` and is
// compared with fixtures/office-golden.json, recorded from Node with
// OFFICE_GOLDEN=write. norm() replaces only what is random per run: the
// session prefix of cell/match/cursor handles (a UUID), and the opaque version
// tokens of xlsx/pptx (a random nonce plus a counter), each renamed by first
// appearance so "the same version" and "a new version" still show. Files with
// bytes (export, create) are pinned by what they hold, not their bytes: the
// file is reopened through the same tools and every paragraph (with its runs'
// formatting), cell and slide paragraph is listed. Byte or XML hashes would pin
// the zip container, part order and serializer whitespace of one engine build,
// which a correct port on BetterOffice's Rust crates (newer than the npm wasm
// builds Node uses) does not reproduce; the text and structure are what the
// file means. A render is pinned by its metadata and the PNG's IHDR (width,
// height), not its pixels, for the same reason.
//
// DIFF: where rox-broker answers differently from Node on purpose, the case
// says so in a `// DIFF:` comment and pins both answers.
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, realpathSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { say, startFakeOpencodex, use } from "./fixtures/fake-opencodex.mjs";
import { ROOT, modelEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const SEEDS = join(ROOT, "src", "office", "seeds");
const GOLDEN = join(ROOT, "test", "fixtures", "office-golden.json");
const TOOLS_JSON = join(ROOT, "..", "broker", "rox-broker", "src", "office", "tools.json");
const RUST = !!(process.env.BUWIZ_BROKER_CMD || "").trim();
const WRITE = process.env.OFFICE_GOLDEN === "write";
const golden = !WRITE && existsSync(GOLDEN) ? JSON.parse(readFileSync(GOLDEN, "utf8")) : {};
const recorded = {};

/** The office MCP server for workspace `root`: Node's child, or rox-broker's. */
function officeCommand(root) {
  if (RUST) {
    const [bin] = process.env.BUWIZ_BROKER_CMD.trim().split(/\s+/);

    return [bin, ["office-mcp", "--root", root]];
  }

  return [process.execPath, ["--experimental-transform-types", join(ROOT, "src", "office", "mcp-main.ts"), "--root", root]];
}

async function connect(root) {
  const [command, args] = officeCommand(root);
  const transport = new StdioClientTransport({ command, args, stderr: "ignore", cwd: ROOT });
  const client = new Client({ name: "office-test", version: "1.0.0" });

  await client.connect(transport);

  return { client, pid: transport.pid };
}

/** A workspace holding the seeds. */
function workspace(label) {
  const dir = tempDir(`office-${label}`);

  cpSync(SEEDS, dir, { recursive: true });

  return dir;
}

/** The width and height a PNG's IHDR declares. */
function pngSize(b64) {
  const b = Buffer.from(b64, "base64");

  assert.equal(b.subarray(1, 4).toString("latin1"), "PNG", "a PNG");

  return { width: b.readUInt32BE(16), height: b.readUInt32BE(20) };
}

/** Run-to-run noise out: handle prefixes and version tokens (see the header). */
function normalizer(root) {
  const versions = new Map();
  const roots = root ? [realpathSync(root), root] : [];
  const walk = (v) => {
    if (typeof v === "string") {
      for (const r of roots) v = v.replaceAll(r, "<root>");
      if (/^[0-9a-f]{16,32}-\d+$/.test(v)) {
        if (!versions.has(v)) versions.set(v, `V${versions.size + 1}`);

        return versions.get(v);
      }

      return v.replace(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}:/, "U:");
    }
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === "object") {
      // a tracked change's id: the session's (client id first) or the file's w:id
      const revision = "author" in v && "date" in v;

      return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, revision && k === "id" ? "REV" : field(k, x)]));
    }

    return v;
  };

  // a file's size is its bytes (see the header)
  const field = (k, x) => {
    if (k === "bytes" && typeof x === "number" && x > 0) return "N";

    return walk(x);
  };

  return walk;
}

/** A session: `call` answers {result, isError, png?} as pinned; `raw` keeps
 *  the unnormalized structuredContent for ids the next call needs. */
function session(client, root) {
  const norm = normalizer(root);
  const log = [];
  const call = async (name, args) => {
    const r = await client.callTool({ name, arguments: args });
    const image = (r.content || []).find((c) => c.type === "image");
    const out = { call: [name, norm(args)], isError: !!r.isError, result: norm(r.structuredContent ?? null) };

    if (image) out.png = { mimeType: image.mimeType, ...pngSize(image.data) };
    // the text block is the structured content, as JSON
    assert.deepEqual(JSON.parse(r.content[0].text), r.structuredContent, `${name}: text is the structured content`);
    log.push(out);

    return r.structuredContent;
  };

  return { call, log };
}

/** What rox-broker answers for a pinned Node step, where it differs on
 *  purpose; each difference says why. */
function expected(step, actual) {
  if (!RUST || !step) return step;
  const r = step.result;
  const receipt = r?.receipt?.calculation && actual?.result?.receipt?.calculation;

  // DIFF: an xlsx accept's receipt counts what the engine recalculated, and
  // the engine builds count differently: the npm wasm build Node runs (xlsx
  // 0.3) reports 5 changed cells on 2 sheets for OFF-04's two inputs, the
  // 0.5.1 crate 3 on 1. The cells' values after the accept are the same
  // (the cells and exported contents that follow are compared as pinned).
  if (receipt) {
    const { changedSheets } = actual.result.receipt;
    const { changedCells } = actual.result.receipt.calculation;

    return { ...step, result: { ...r, receipt: { ...r.receipt, changedSheets, calculation: { ...r.receipt.calculation, changedCells } } } };
  }

  // DIFF: rox-broker has no DOCX page renderer. Node paints pages with
  // render.ts (the docx package's JS layout, @napi-rs/canvas and
  // @betteroffice/fonts); the crates' PNG path for DOCX is the `raster`
  // feature of betteroffice-docx, which needs betteroffice-docx-raster
  // 0.5.1 (not fetched, D26). It answers as Node's server does when
  // render.ts cannot load: render false in the capabilities, UNSUPPORTED.
  if (r && r.format === "docx" && r.capabilities?.render === true) return { ...step, result: { ...r, capabilities: { ...r.capabilities, render: false } } };
  const docxRender = step.call[0] === "office_render" && (r?.warnings?.[0]?.startsWith("Preview uses BetterOffice layout") || r?.code === "PAGE_OUT_OF_RANGE");

  if (docxRender) return { call: step.call, isError: true, result: { code: "UNSUPPORTED", message: "No renderer configured. Supply a DocumentRenderer or use the MCP server." } };

  return step;
}

/** Compares (or records) one transcript against the golden file. */
function pin(key, log) {
  if (WRITE) {
    recorded[key] = log;

    return;
  }
  assert.ok(golden[key], `no golden for ${key}: record it on Node with OFFICE_GOLDEN=write`);
  // OFFICE_GOLDEN=diff lists every step that differs instead of stopping at the first
  if (process.env.OFFICE_GOLDEN === "diff") {
    const differ = [];

    for (let i = 0; i < Math.max(log.length, golden[key].length); i++) {
      try {
        assert.deepEqual(log[i], expected(golden[key][i], log[i]));
      } catch {
        differ.push({ step: i, actual: log[i], expected: expected(golden[key][i], log[i]) });
      }
    }
    assert.deepEqual(differ, [], `${key}: ${differ.length} steps differ`);
  }
  for (let i = 0; i < Math.max(log.length, golden[key].length); i++) {
    assert.deepEqual(log[i], expected(golden[key][i], log[i]), `${key} step ${i}: ${JSON.stringify(golden[key][i]?.call ?? log[i]?.call)}`);
  }
}

/** Everything a file holds, read back through the tools: the pinned form of
 *  an exported or created file. */
async function contents(call, path) {
  const opened = await call("office_open", { path });
  const doc = opened.document;

  if (opened.format === "xlsx") {
    const { items } = await call("office_outline", { document: doc });

    for (const s of items) await call("office_cells", { document: doc, sheet: s.sheetId, range: "A1:F12", limit: 72 });
  } else {
    let offset = 0;

    do {
      const page = await call("office_outline", { document: doc, offset, limit: 100 });

      if (opened.format === "docx") for (const p of page.items) await call("office_read", { document: doc, ref: p.ref });
      offset = page.nextOffset;
    } while (offset !== null);
  }
  await call("office_close", { document: doc });
}

let fake;

before(async () => {
  fake = await startFakeOpencodex();
});

after(async () => {
  await server?.stop();
  await fake?.close();
  if (WRITE) writeFileSync(GOLDEN, `${JSON.stringify(recorded, null, 1)}\n`);
});

test("OFF-01 tools/list and the server's instructions are the pinned definitions (tools.json)", async () => {
  const root = workspace("list");
  const { client } = await connect(root);

  try {
    const { tools } = await client.listTools();
    if (WRITE) writeFileSync(TOOLS_JSON, `${JSON.stringify({ instructions: client.getInstructions(), serverInfo: client.getServerVersion(), tools }, null, 1)}\n`);
    else {
      const pinned = JSON.parse(readFileSync(TOOLS_JSON, "utf8"));

      assert.deepEqual(tools, pinned.tools);
      assert.equal(client.getInstructions(), pinned.instructions);
      assert.deepEqual(client.getServerVersion(), pinned.serverInfo);
    }
    assert.deepEqual(tools.map((t) => t.name), [
      "office_files", "office_open", "office_outline", "office_grep", "office_read", "office_cells", "office_render", "office_verify", "office_close",
      "office_propose", "office_propose_cells", "office_review", "office_accept", "office_reject", "office_export", "office_create",
    ]);
  } finally {
    await client.close();
  }
});

test("OFF-02 docx: files, open, outline, grep with a cursor, read, propose, review, render, verify, export a proposal, accept, reject, the exported file's contents", async () => {
  const root = workspace("docx");
  const { client } = await connect(root);
  const { call, log } = session(client, root);

  try {
    await call("office_files", {});
    const { document } = await call("office_open", { path: "betteroffice-demo.docx" });

    // opening it again answers the same document
    await call("office_open", { path: "./betteroffice-demo.docx" });
    await call("office_outline", { document, limit: 100 });
    await call("office_outline", { document, headingsOnly: true });
    await call("office_outline", { document, story: "hf:rId3" });
    const g = await call("office_grep", { document, query: "engine", limit: 2 });

    await call("office_grep", { document, query: "engine", limit: 2, cursor: g.nextCursor });
    await call("office_grep", { document, query: "ENGINE", caseSensitive: true });
    await call("office_read", { document, ref: "p3", start: 10, length: 50 });
    await call("office_read", { document, ref: "p1" });
    const p = await call("office_propose", { document, author: "Rox", note: "tighten", edits: [{ match: g.matches[0].match, newText: "core" }, { match: g.matches[1].match, newText: "renderer" }] });

    await call("office_review", { document });
    await call("office_review", { document, proposal: p.id });
    await call("office_render", { document, proposal: p.id });
    await call("office_render", { document, page: 2 });
    await call("office_render", { document, page: 9 });
    await call("office_verify", { document, proposal: p.id });
    await call("office_export", { document, path: "proposed.docx", proposal: p.id });
    await call("office_accept", { document, proposal: p.id });
    await call("office_accept", { document, proposal: p.id });
    await call("office_outline", { document, limit: 4 });
    await call("office_read", { document, ref: "p2" });
    // the old match is stale now; a fresh grep proposes again
    await call("office_propose", { document, author: "Rox", edits: [{ match: g.matches[0].match, newText: "x" }] });
    const g2 = await call("office_grep", { document, query: "Welcome" });
    const q = await call("office_propose", { document, author: "Rox", edits: [{ match: g2.matches[0].match, newText: "Hello" }] });

    await call("office_reject", { document, proposal: q.id });
    await call("office_reject", { document, proposal: q.id });
    await call("office_review", { document });
    await call("office_verify", { document });
    await call("office_export", { document, path: "accepted.docx" });
    await call("office_export", { document, path: "accepted.docx" });
    await call("office_export", { document, path: "accepted.xlsx" });
    await call("office_files", {});
    await contents(call, "proposed.docx");
    await contents(call, "accepted.docx");
    await call("office_close", { document });
    await call("office_close", { document });
  } finally {
    await client.close();
  }
  pin("docx", log);
});

test("OFF-03 docx: tracked acceptance writes Word tracked changes; errors for formats, paths, arguments and unknown ids", async () => {
  const root = workspace("docx-tracked");
  const { client } = await connect(root);
  const { call, log } = session(client, root);

  try {
    const { document } = await call("office_open", { path: "betteroffice-demo.docx" });
    const g = await call("office_grep", { document, query: "Welcome to" });
    const p = await call("office_propose", { document, author: "Reviewer", edits: [{ match: g.matches[0].match, newText: "Hello from" }] });

    await call("office_accept", { document, proposal: p.id, tracked: true });
    await call("office_accept", { document, proposal: p.id, tracked: true, date: "2026-10-30T00:00:00Z" });
    await call("office_read", { document, ref: "p1" });
    // a range holding a tracked change is protected
    await call("office_grep", { document, query: "Welcome" });
    await call("office_export", { document, path: "tracked.docx" });
    await contents(call, "tracked.docx");
    // errors
    await call("office_cells", { document, sheet: "sheet:0", range: "A1" });
    await call("office_read", { document, ref: "p1", field: "formula" });
    await call("office_read", { document, ref: "p999" });
    await call("office_outline", { document, story: "nope" });
    await call("office_grep", { document, query: "a\nb" });
    await call("office_propose", { document, author: "Rox", edits: [{ match: "m999", newText: "x" }] });
    await call("office_propose_cells", { document, author: "Rox", edits: [{ cell: "c1", input: "1" }] });
    await call("office_review", { document, proposal: "proposal99" });
    await call("office_open", { path: "../outside.docx" });
    await call("office_open", { path: "missing.docx" });
    await call("office_open", { path: "notes.txt" });
    await call("office_outline", { document: "doc99" });
    await call("office_files", { directory: "nope" });
    await call("office_outline", { document, limit: 0 });
    await call("office_open", {});
    await call("office_open", { path: "betteroffice-demo.docx", extra: 1 });
    await call("office_grep", { document, query: "" });
    await call("office_render", { document, page: 0 });
    await call("office_accept", { document, proposal: p.id, date: "yesterday" });
    await call("office_nope", {});
  } finally {
    await client.close();
  }
  pin("docx-tracked", log);
});

test("OFF-04 xlsx: open, outline, cells, grep, read fields, propose cells, review, render, verify, export a proposal, accept, export, the exported file's contents", async () => {
  const root = workspace("xlsx");
  const { client } = await connect(root);
  const { call, log } = session(client, root);

  try {
    const { document } = await call("office_open", { path: "showcase.xlsx" });
    const { items } = await call("office_outline", { document });

    await call("office_outline", { document, story: items[1].sheetId });
    const cells = await call("office_cells", { document, sheet: items[0].sheetId, range: "A1:D10", limit: 40 });

    await call("office_cells", { document, sheet: items[1].sheetId, range: "A1:C6" });
    await call("office_cells", { document, sheet: items[0].sheetId, range: "A1:D10", offset: 38, limit: 5 });
    const g = await call("office_grep", { document, query: "Total" });

    await call("office_grep", { document, query: "e", limit: 3 });
    await call("office_grep", { document, query: "Total", caseSensitive: false });
    await call("office_read", { document, ref: g.matches[0].ref });
    const formula = (await call("office_cells", { document, sheet: items[1].sheetId, range: "A1:E12", limit: 60 })).cells.find((c) => c.formula);

    await call("office_read", { document, ref: formula.ref, field: "formula" });
    await call("office_read", { document, ref: formula.ref, field: "value" });
    const target = cells.cells.find((c) => c.value.kind === "number") || cells.cells[1];
    const p = await call("office_propose_cells", { document, author: "Rox", note: "bump", edits: [{ cell: target.cell, input: "1234" }, { cell: cells.cells[0].cell, input: "=1+2" }] });

    await call("office_review", { document });
    await call("office_review", { document, proposal: p.id });
    await call("office_render", { document, proposal: p.id });
    await call("office_render", { document });
    await call("office_verify", { document, proposal: p.id });
    await call("office_export", { document, path: "proposed.xlsx", proposal: p.id });
    await call("office_accept", { document, proposal: p.id });
    await call("office_cells", { document, sheet: items[0].sheetId, range: "A1:B2" });
    // the handles read before the accept are stale now
    await call("office_propose_cells", { document, author: "Rox", edits: [{ cell: target.cell, input: "1" }] });
    await call("office_propose", { document, author: "Rox", edits: [{ match: "m1", newText: "x" }] });
    await call("office_outline", { document, headingsOnly: true });
    await call("office_cells", { document, sheet: items[0].sheetId, range: "B2:A1" });
    await call("office_cells", { document, sheet: "sheet:99", range: "A1" });
    await call("office_accept", { document, proposal: p.id, tracked: true });
    await call("office_verify", { document });
    await call("office_export", { document, path: "accepted.xlsx" });
    await contents(call, "proposed.xlsx");
    await contents(call, "accepted.xlsx");
  } finally {
    await client.close();
  }
  pin("xlsx", log);
});

test("OFF-05 pptx: open, outline, grep with a cursor, read, propose, review, render (unsupported), verify, export a proposal, accept, reject, the exported file's contents", async () => {
  const root = workspace("pptx");
  const { client } = await connect(root);
  const { call, log } = session(client, root);

  try {
    const { document } = await call("office_open", { path: "presentation.pptx" });

    await call("office_outline", { document, limit: 100 });
    const g = await call("office_grep", { document, query: "office", limit: 2 });

    await call("office_grep", { document, query: "office", limit: 2, cursor: g.nextCursor });
    await call("office_grep", { document, query: "office", limit: 2, cursor: "nope" });
    await call("office_read", { document, ref: g.matches[0].ref, start: 2, length: 5 });
    const p = await call("office_propose", { document, author: "Rox", note: "rename", edits: [{ match: g.matches[1].match, newText: "Docs" }] });

    await call("office_review", { document });
    await call("office_review", { document, proposal: p.id });
    await call("office_render", { document });
    await call("office_verify", { document, proposal: p.id });
    await call("office_export", { document, path: "proposed.pptx", proposal: p.id });
    await call("office_accept", { document, proposal: p.id });
    await call("office_read", { document, ref: g.matches[1].ref });
    const g2 = await call("office_grep", { document, query: "WASM" });
    const q = await call("office_propose", { document, author: "Rox", edits: [{ match: g2.matches[0].match, newText: "Native" }] });

    await call("office_reject", { document, proposal: q.id });
    await call("office_review", { document });
    await call("office_propose", { document, author: "Rox", edits: [{ match: g.matches[0].match, newText: "x" }] });
    await call("office_outline", { document, headingsOnly: true });
    await call("office_export", { document, path: "accepted.pptx" });
    await contents(call, "proposed.pptx");
    await contents(call, "accepted.pptx");
  } finally {
    await client.close();
  }
  pin("pptx", log);
});

test("OFF-06 office_create makes each kind, never overwrites, and checks its payload", async () => {
  const root = workspace("create");
  const { client } = await connect(root);
  const { call, log } = session(client, root);

  mkdirSync(join(root, "made"));
  try {
    await call("office_create", { path: "made/report.docx", docx: { paragraphs: [{ text: "Quarterly report", style: "title" }, { text: "Summary", style: "heading1" }, { text: "Revenue grew.", bold: true, italic: true }, { text: "Plain text." }] } });
    await call("office_create", { path: "made/budget.xlsx", xlsx: { rows: [["Item", "Cost"], ["Rent", "3000"], ["Ads", "450"], ["Total", "=SUM(B2:B3)"]] } });
    await call("office_create", { path: "made/deck.pptx", pptx: { slides: [{ title: "Hello", bullets: ["One", "Two"], notes: "Say hi." }, { title: "Bye" }] } });
    await call("office_create", { path: "made/report.docx", docx: { paragraphs: [{ text: "again" }] } });
    await call("office_create", { path: "made/notes.txt", docx: { paragraphs: [{ text: "x" }] } });
    await call("office_create", { path: "made/x.pptx", docx: { paragraphs: [{ text: "x" }] } });
    await call("office_create", { path: "nope/x.docx", docx: { paragraphs: [{ text: "x" }] } });
    await call("office_create", { path: "../x.docx", docx: { paragraphs: [{ text: "x" }] } });
    await call("office_create", { path: "made/bad.docx", docx: { paragraphs: [{ text: "bad\u0001" }] } });
    await call("office_create", { path: "made/empty.xlsx", xlsx: { rows: [[]] } });
    await call("office_files", { directory: "made" });
    await contents(call, "made/report.docx");
    await contents(call, "made/budget.xlsx");
    await contents(call, "made/deck.pptx");
  } finally {
    await client.close();
  }
  pin("create", log);
});

test("OFF-07 the office MCP server is no Node process when rox-broker serves it", async () => {
  const root = workspace("proc");
  const { client, pid } = await connect(root);

  try {
    const command = execFileSync("ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" }).trim();

    if (RUST) {
      assert.match(command, /office-mcp/);
      assert.doesNotMatch(command, /\bnode\b/);
    } else assert.match(command, /mcp-main\.ts/);
  } finally {
    await client.close();
  }
});

// ————— over HTTP: the broker's routes and its in-process tools —————

const dir = tempDir("office-http");
const files = join(dir, "generated", "files");
let server;

const hubEnv = () => modelEnv(fake, join(dir, "stubs.log"), RUST ? { BUWIZ_BROKER_HUB: "1", BUWIZ_BROKER_MCP_HOST: "1", BUWIZ_BROKER_V1_HOST: "1" } : {});

test("OFF-20 /office/* pages, caps, vendor files and the editor bundle", async () => {
  mkdirSync(files, { recursive: true });
  cpSync(SEEDS, join(files, "seeds"), { recursive: true });
  server = await startServer(dir, { env: hubEnv() });

  const caps = await request("GET", "/office/caps");

  assert.equal(caps.status, 200);
  assert.equal(caps.headers["content-type"], "application/json");
  assert.equal(caps.text, '{"docx":true,"xlsx":true,"pptx":true}');
  const viewer = await request("GET", "/office/?path=seeds/blank.pptx&kind=pptx");

  assert.equal(viewer.status, 200);
  assert.equal(viewer.headers["content-type"], "text/html; charset=utf-8");
  assert.equal(viewer.headers["cache-control"], "no-cache");
  assert.equal(viewer.text, readFileSync(join(ROOT, "src", "office", "viewer.html"), "utf8"));
  assert.equal((await request("GET", "/office/")).text, viewer.text);
  const editor = await request("GET", "/office/editor/?path=seeds/blank.pptx&kind=pptx");
  const bundle = join(ROOT, "src", "office", "editor", "dist", "editor.js");
  const v = existsSync(bundle) ? Math.floor(statSync(bundle).mtimeMs) : 0;

  assert.equal(editor.status, 200);
  assert.equal(editor.headers["content-type"], "text/html; charset=utf-8");
  assert.equal(editor.text, readFileSync(join(ROOT, "src", "office", "editor", "editor.html"), "utf8").replaceAll("/office/dist/editor.js", `/office/dist/editor.js?v=${v}`));
  const pkg = await request("GET", "/office/vendor/docx/package.json?x=1");

  assert.equal(pkg.status, 200);
  assert.equal(pkg.headers["content-type"], "application/json");
  assert.equal(pkg.headers["cache-control"], "no-cache");
  assert.equal(pkg.text, readFileSync(join(ROOT, "node_modules", "@betteroffice", "docx", "package.json"), "utf8"));
  const wasm = await request("GET", "/office/vendor/xlsx/dist/generated/xlsx_wasm_bg.wasm");

  assert.equal(wasm.status, 200);
  assert.equal(wasm.headers["content-type"], "application/wasm");
  for (const path of ["/office/vendor/nope/x.js", "/office/vendor/docx/nope.js", "/office/vendor/docx/../../../package.json", "/office/vendor/docx/dist", "/office/dist/nope.js", "/office/dist/../editor.html"]) {
    const r = await request("GET", path);

    assert.equal(r.status, 404, path);
    assert.equal(r.text, "", path);
  }
  // DIFF: Node logs which side served nothing; rox-broker's front door logs `hub` for these
  if (RUST) assert.match(server.log(), /GET \/office\/caps 200 \d+ms hub/);
});

test("OFF-21 /file/render for docx, xlsx and pptx files: the viewer's payload and the office URL", async () => {
  const render = async (path) => (await request("GET", `/file/render?path=${encodeURIComponent(path)}`)).json;
  const bundle = join(ROOT, "src", "office", "editor", "dist", "editor.js");
  const office = (path, kind) => existsSync(bundle)
    ? `/office/editor/?path=${path}&kind=${kind}&v=${Math.floor(statSync(bundle).mtimeMs)}`
    : `/office/?path=${path}&kind=${kind}`;
  const size = (path) => statSync(join(files, path)).size;
  const pptx = await render("seeds/presentation.pptx");

  assert.deepEqual(pptx, { name: "presentation.pptx", ext: "pptx", size: size("seeds/presentation.pptx"), kind: "pptx", variant: "ppt", office: office("seeds/presentation.pptx", "pptx") });
  const xlsx = await render("seeds/showcase.xlsx");

  assert.deepEqual(Object.keys(xlsx), ["name", "ext", "size", "kind", "variant", "sheets", "office"]);
  assert.deepEqual([xlsx.name, xlsx.ext, xlsx.size, xlsx.kind, xlsx.variant, xlsx.office], ["showcase.xlsx", "xlsx", size("seeds/showcase.xlsx"), "table", "spreadsheet", office("seeds/showcase.xlsx", "xlsx")]);
  assert.deepEqual(xlsx.sheets.map((s) => s.name), ["Dashboard", "Formulas", "Data"]);
  for (const s of xlsx.sheets) {
    assert.deepEqual(Object.keys(s), ["name", "rows", "bold", "widths"]);
    assert.equal(s.rows.length, s.bold.length);
    assert.ok(s.rows.every((r) => r.length === s.widths.length), `${s.name}: every row spans the columns`);
  }
  // the cells as ExcelJS reads them: a formula's cached result, numbers as JavaScript prints them
  assert.deepEqual(xlsx.sheets[0].rows.slice(0, 2).map((r) => r.slice(0, 3)), golden.renderXlsx ?? xlsx.sheets[0].rows.slice(0, 2).map((r) => r.slice(0, 3)));
  if (WRITE) recorded.renderXlsx = xlsx.sheets[0].rows.slice(0, 2).map((r) => r.slice(0, 3));
  assert.equal(xlsx.sheets[0].rows[0][0], "Q3 Sales Dashboard");
  assert.equal(xlsx.sheets[0].bold[0][0], true);
  const docx = await render("seeds/betteroffice-demo.docx");

  assert.deepEqual(Object.keys(docx), ["name", "ext", "size", "kind", "variant", "text", "office"]);
  assert.deepEqual([docx.name, docx.kind, docx.variant, docx.office], ["betteroffice-demo.docx", "docx", "docx", office("seeds/betteroffice-demo.docx", "docx")]);
  // mammoth's HTML through htmlToMd: headings by their Word style, bold and
  // italic runs, lists, and a table as a Markdown table
  assert.equal(docx.text, golden.renderDocx ?? docx.text);
  if (WRITE) recorded.renderDocx = docx.text;
  // missing and outside paths
  assert.deepEqual(await render("seeds/gone.docx"), { name: "gone.docx", ext: "docx", size: 0, kind: "docx", variant: "docx", error: "missing" });
  assert.deepEqual(await render("../x.docx"), { error: "bad path" });
  // other kinds stay Node's (the office role) until 6.2
  const md = join(files, "note.md");

  writeFileSync(md, "# hi\n");
  assert.deepEqual(await render("note.md"), { name: "note.md", ext: "md", size: 5, kind: "markdown", variant: "generic", text: "# hi\n" });
  // the front door's log (queries redacted): every kind in Rust, the
  // markdown file too since phase 7 (render.test)
  if (RUST) assert.deepEqual(server.log().match(/GET \/file\/render 200 \d+ms \w+/g).map((l) => l.split(" ").at(-1)), ["hub", "hub", "hub", "hub", "hub", "hub"]);
});

test("OFF-30 a chat turn's office tools run in the broker: open, cells, propose, accept, export (a card), create (a card); rox-broker runs no office child", async () => {
  const thread = (await postJson("/thread/new", {})).json.id;

  fake.script("office", [
    use("office_open", { path: "seeds/showcase.xlsx" }, { id: "toolu_open" }),
    use("office_cells", { document: "doc1", sheet: "sheet:0", range: "A1:A1" }, { id: "toolu_cells" }),
    use("office_files", { directory: "seeds" }, { id: "toolu_files" }),
    use("office_export", { document: "doc1", path: "seeds/copy.xlsx" }, { id: "toolu_export" }),
    use("office_create", { path: "memo.docx", docx: { paragraphs: [{ text: "Memo", style: "title" }] } }, { id: "toolu_create" }),
    use("office_render", { document: "doc1" }, { id: "toolu_render" }),
    say("Done."),
  ]);
  fake.reset();
  const id = (await postJson("/chat", { prompt: "[[script:office]] work the sheet", thread })).json.id;
  const st = await waitFor(async () => {
    const j = (await request("GET", `/status?id=${id}`)).json;

    return j.done && j;
  }, 60000);

  assert.ok(st, "the turn ends");
  const results = fake.calls().slice(1).map((c) => c.body.messages.at(-1).content.find((b) => b.type === "tool_result"));
  const norm = normalizer();
  const parsed = results.map((r) => {
    try {
      return { ...r, content: norm(JSON.parse(r.content)) };
    } catch {
      return r;
    }
  });

  assert.deepEqual(parsed.map((r) => r.tool_use_id), ["toolu_open", "toolu_cells", "toolu_files", "toolu_export", "toolu_create", "toolu_render"]);
  assert.deepEqual(parsed[0].content, { document: "doc1", name: "showcase.xlsx", format: "xlsx", version: "V1", sheets: 3, capabilities: { grep: "literal, case-sensitive", proposals: "cell inputs and formulas", render: false, export: true, prototype: true }, workflow: "outline lists sheet IDs; grep finds displayed values; cells reads A1 ranges and returns cell handles; proposeCells with cell and input (prefix = for a formula); review; verify; export." });
  assert.deepEqual(parsed[1].content.cells.map((c) => [c.ref, c.cell, c.displayText]), [["sheet:0!A1", "U:cell1", "Q3 Sales Dashboard"]]);
  assert.deepEqual(parsed[2].content.files.map((f) => f.path), ["seeds/betteroffice-demo.docx", "seeds/blank.pptx", "seeds/blank.xlsx", "seeds/presentation.pptx", "seeds/showcase.xlsx"]);
  assert.deepEqual(JSON.parse(results[3].content), { path: "seeds/copy.xlsx", bytes: statSync(join(files, "seeds", "copy.xlsx")).size });
  assert.match(parsed[4].content, /^created o[0-9a-z]+\/memo\.docx — the file card is in the reply\. To revise it, open this path with office_open and export the result\.$/);
  // the folded render: its metadata, then the image's size
  const [meta, image] = results[5].content.split("\n");

  assert.deepEqual(JSON.parse(meta), { page: 1, pageCount: 1, width: JSON.parse(meta).width, height: JSON.parse(meta).height, warnings: ["Preview uses BetterOffice rendering; compare with the target Office application when exact fidelity matters."] });
  assert.match(image, /^\[image: image\/png, \d+ bytes\]$/);
  assert.ok(results.every((r) => !r.is_error), JSON.stringify(results.filter((r) => r.is_error)));
  // the export and the create are cards on their own turns
  const turns = (await request("GET", `/turns?thread=${thread}`)).json.turns;
  const cards = turns.filter((t) => t.label === "office").map((t) => t.files?.map((f) => f.name));

  assert.deepEqual(cards, [["copy.xlsx"], ["memo.docx"]]);
  // the process tree: Node's broker spawns the office MCP child; rox-broker none
  const tree = execFileSync("ps", ["-A", "-o", "pid=,ppid=,command="], { encoding: "utf8" });
  const office = tree.split("\n").filter((l) => /mcp-main\.(ts|js)/.test(l) && l.includes(`--root ${files}`));

  if (RUST) assert.deepEqual(office, [], "no office child");
  else assert.ok(office.length >= 1, "Node's office child");
});

test("OFF-31 a docx, xlsx and pptx round trip in the broker (open, propose, accept, export, render) holds what Node's does", async () => {
  // the transcripts of OFF-02, OFF-04 and OFF-05 are this round trip; here it
  // runs once more on one workspace as the broker's tools see it, and the
  // exported files are compared by contents across the three kinds
  const root = workspace("roundtrip");
  const { client } = await connect(root);
  const { call, log } = session(client, root);

  try {
    for (const [path, query, newText] of [["betteroffice-demo.docx", "Welcome", "Hello"], ["presentation.pptx", "WASM", "Native"]]) {
      const { document } = await call("office_open", { path });
      const g = await call("office_grep", { document, query });
      const p = await call("office_propose", { document, author: "Rox", edits: [{ match: g.matches[0].match, newText }] });

      await call("office_accept", { document, proposal: p.id });
      await call("office_export", { document, path: `rt-${path}` });
      await call("office_render", { document });
      await contents(call, `rt-${path}`);
    }
    const { document } = await call("office_open", { path: "showcase.xlsx" });
    const c = await call("office_cells", { document, sheet: "sheet:0", range: "A1" });
    const p = await call("office_propose_cells", { document, author: "Rox", edits: [{ cell: c.cells[0].cell, input: "Q4 Sales Dashboard" }] });

    await call("office_accept", { document, proposal: p.id });
    await call("office_export", { document, path: "rt-showcase.xlsx" });
    await call("office_render", { document });
    await contents(call, "rt-showcase.xlsx");
  } finally {
    await client.close();
  }
  pin("roundtrip", log);
});
