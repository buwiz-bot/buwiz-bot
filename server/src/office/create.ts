import { readFile } from "node:fs/promises";

import { createDocx, type Document, type Paragraph } from "@betteroffice/docx";
import { initWasm as initPptxWasm, openPresentation } from "@betteroffice/pptx";
import { initWasm as initXlsxWasm, openWorkbook } from "@betteroffice/xlsx";

import { initialize, plainText } from "./agents/prototype.ts";
import { DocumentToolError } from "./agents/types.ts";
import { loadOfficeWasm } from "./wasm.ts";

export interface DocxCreateParagraph {
  text: string;
  style?: "title" | "heading1" | "heading2" | "normal" | undefined;
  bold?: boolean | undefined;
  italic?: boolean | undefined;
}

export interface PptxCreateSlide {
  title?: string | undefined;
  bullets?: string[] | undefined;
  notes?: string | undefined;
}

export interface OfficeCreateInput {
  docx?: { paragraphs: DocxCreateParagraph[] } | undefined;
  xlsx?: { rows: string[][] } | undefined;
  pptx?: { slides: PptxCreateSlide[] } | undefined;
}

export async function officeCreateBytes(format: "docx" | "xlsx" | "pptx", input: OfficeCreateInput): Promise<Uint8Array> {
  if (format === "docx") return createDocxBytes(input.docx);
  if (format === "xlsx") return createXlsxBytes(input.xlsx);
  return createPptxBytes(input.pptx);
}

const HEADINGS = {
  title: { bold: true, fontSize: 64, spaceAfter: 240 },
  heading1: { bold: true, fontSize: 40, outlineLevel: 0, spaceBefore: 240, spaceAfter: 120 },
  heading2: { bold: true, fontSize: 32, outlineLevel: 1, spaceBefore: 160, spaceAfter: 80 },
  normal: { bold: false, fontSize: 24 },
} as const;

async function createDocxBytes(input?: { paragraphs: DocxCreateParagraph[] }): Promise<Uint8Array> {
  if (!input?.paragraphs?.length) throw new DocumentToolError("INVALID_ARGUMENT", "docx requires at least one paragraph.");
  if (input.paragraphs.length > 5000) throw new DocumentToolError("INVALID_ARGUMENT", "docx creation is limited to 5000 paragraphs.");
  const content: Paragraph[] = input.paragraphs.map(paragraph => {
    plainText(paragraph.text);
    if (paragraph.text.length > 16000) throw new DocumentToolError("EDIT_LIMIT", "Paragraph text must fit in 16000 UTF-16 units.");
    const heading = HEADINGS[paragraph.style ?? "normal"];
    const spacing: { outlineLevel?: number; spaceBefore?: number; spaceAfter?: number } = {};
    if ("outlineLevel" in heading) spacing.outlineLevel = heading.outlineLevel;
    if ("spaceBefore" in heading) spacing.spaceBefore = heading.spaceBefore;
    if ("spaceAfter" in heading) spacing.spaceAfter = heading.spaceAfter;
    return {
      type: "paragraph",
      formatting: spacing,
      content: [{
        type: "run",
        formatting: { bold: paragraph.bold ?? heading.bold, ...(paragraph.italic !== undefined ? { italic: paragraph.italic } : {}), fontSize: heading.fontSize },
        content: [{ type: "text", text: paragraph.text }],
      }],
    };
  });
  const document: Document = { package: { document: { content } } };
  return new Uint8Array(await createDocx(document));
}

async function createXlsxBytes(input?: { rows: string[][] }): Promise<Uint8Array> {
  if (!input?.rows?.length || !input.rows.some(row => row.length)) {
    throw new DocumentToolError("INVALID_ARGUMENT", "xlsx requires rows: string[][].");
  }
  if (input.rows.length > 4096 || input.rows.some(row => row.length > 256)) {
    throw new DocumentToolError("INVALID_ARGUMENT", "xlsx creation is limited to 4096x256 cells.");
  }
  for (const row of input.rows) for (const input of row) {
    plainText(input);
    if (input.length > 16000) throw new DocumentToolError("EDIT_LIMIT", "Cell input must fit in 16000 UTF-16 units.");
  }
  await initialize("xlsx", initXlsxWasm, await loadOfficeWasm("xlsx").catch(() => undefined));
  const workbook = openWorkbook(await seedBytes("xlsx"));
  try {
    const edits = input.rows.flatMap((row, r) => row.map((input, c) => ({ row: r, col: c, input })));
    workbook.editCells(0, edits);
    return new Uint8Array(workbook.save());
  } finally {
    workbook.dispose();
  }
}

// EMU rectangles for a 4:3 seed slide (9144000 x 6858000 EMU).
const TITLE_RECT = { x: 457200, y: 274320, width: 8229600, height: 822960 };
const BODY_RECT = { x: 457200, y: 1371600, width: 8229600, height: 5029200 };

async function createPptxBytes(input?: { slides: PptxCreateSlide[] }): Promise<Uint8Array> {
  if (!input?.slides?.length) throw new DocumentToolError("INVALID_ARGUMENT", "pptx requires at least one slide.");
  if (input.slides.length > 200) throw new DocumentToolError("INVALID_ARGUMENT", "pptx creation is limited to 200 slides.");
  for (const slide of input.slides) {
    for (const text of [slide.title ?? "", ...(slide.bullets ?? []), slide.notes ?? ""]) plainText(text);
  }
  await initialize("pptx", initPptxWasm, await loadOfficeWasm("pptx").catch(() => undefined));
  const presentation = openPresentation(await seedBytes("pptx"));
  try {
    input.slides.forEach((slide, index) => {
      const { slideId } = presentation.insertSlide(index);
      if (slide.title) presentation.addTextBox(slideId, { name: "Title", rect: TITLE_RECT, text: slide.title, style: { fontSizePt: 32, bold: true } });
      if (slide.bullets?.length) presentation.addTextBox(slideId, { name: "Body", rect: BODY_RECT, text: slide.bullets.join("\n"), style: { fontSizePt: 18 } });
      if (slide.notes) presentation.setSlideNotes(slideId, slide.notes);
    });
    return new Uint8Array(presentation.save());
  } finally {
    presentation.dispose();
  }
}

const seedCache = new Map<string, Promise<Uint8Array>>();

function seedBytes(format: "xlsx" | "pptx"): Promise<Uint8Array> {
  let pending = seedCache.get(format);
  if (!pending) {
    pending = readFile(new URL(`./seeds/blank.${format}`, import.meta.url)).then(
      bytes => new Uint8Array(bytes),
      () => { throw new DocumentToolError("ENGINE_ERROR", `Missing blank.${format} seed asset.`); },
    );
    seedCache.set(format, pending);
  }
  return pending;
}
