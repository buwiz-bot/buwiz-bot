// Response files, Grok Bot's way (extracted from its renderer):
// - viewer kind per extension (its `jT`): image · video · audio · pdf · table
//   (csv tsv xlsx xls) · json · markdown (md markdown mdx) · html (html htm) ·
//   docx · text (textual extensions) · unknown (download only);
// - code highlighted by Shiki with Grok's cursor-dark / cursor-light syntax
//   palettes; docx through mammoth (as Grok); spreadsheets parsed to rows.
// `renderFile(path)` returns the payload the app draws natively; PDF / HTML /
// media are served over http and shown in a web view / video player.
import { execFileSync } from "node:child_process";
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, readSync, renameSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { deflateSync, inflateSync } from "node:zlib";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHighlighter } from "shiki";
import mammoth from "mammoth";
import ExcelJS from "exceljs";

const IMAGE = new Set(["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "avif", "bmp", "tiff", "svg"]);
const VIDEO = new Set(["mp4", "mov", "webm", "m4v", "mkv", "avi"]);
const AUDIO = new Set(["mp3", "wav", "m4a", "aac", "flac", "ogg", "oga", "opus", "aiff"]);
const TABLE = new Set(["csv", "tsv", "xlsx", "xls"]);
const TEXTUAL = new Set(["md", "markdown", "mdx", "txt", "text", "rtf", "log", "html", "htm", "xml", "yaml", "yml", "toml", "ini", "cfg", "env", "sh", "bash", "zsh", "js", "jsx", "mjs", "cjs", "ts", "tsx", "py", "rb", "go", "rs", "java", "kt", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "php", "sql", "css", "scss", "tex", "json", "jsonc"]);

export const extOf = (name) => {
  const n = String(name).split(/[\\/]/).pop() || "";
  const i = n.lastIndexOf(".");
  return i <= 0 || i === n.length - 1 ? "" : n.slice(i + 1).toLowerCase();
};

/** Grok's viewer kind (`jT`). */
export function viewerKind(name) {
  const t = extOf(name);
  if (!t) return "unknown";
  if (IMAGE.has(t)) return "image";
  if (VIDEO.has(t)) return "video";
  if (AUDIO.has(t)) return "audio";
  if (t === "pdf") return "pdf";
  if (TABLE.has(t)) return "table";
  if (t === "json") return "json";
  if (t === "md" || t === "markdown" || t === "mdx") return "markdown";
  if (t === "html" || t === "htm") return "html";
  if (t === "docx") return "docx";
  if (t === "pptx" || t === "ppt" || t === "odp" || t === "key") return "pptx";
  return TEXTUAL.has(t) ? "text" : "unknown";
}

/** Grok's card illustration variant (`$P` / `FP`). */
const VARIANT = { pdf: "pdf", doc: "docx", docx: "docx", odt: "docx", pages: "docx", xls: "spreadsheet", xlsx: "spreadsheet", xlsm: "spreadsheet", csv: "spreadsheet", tsv: "spreadsheet", ods: "spreadsheet", numbers: "spreadsheet", ppt: "ppt", pptx: "ppt", odp: "ppt", key: "keynote", zip: "zip", tar: "zip", gz: "zip", tgz: "zip", bz2: "zip", xz: "zip", "7z": "zip", rar: "zip", ttf: "font", otf: "font", woff: "font", woff2: "font", mp3: "audio", wav: "audio", m4a: "audio", aac: "audio", flac: "audio", ogg: "audio", oga: "audio", opus: "audio", aiff: "audio", json: "json", jsonc: "json", json5: "json", ndjson: "json", png: "media", jpg: "media", jpeg: "media", gif: "media", webp: "media", heic: "media", heif: "media", avif: "media", bmp: "media", tiff: "media", svg: "media", mp4: "video", mov: "video", webm: "video", m4v: "video", mkv: "video", avi: "video" };
const GENERIC = new Set(["md", "markdown", "mdx", "txt", "text", "rtf", "log", "html", "htm", "xml", "yaml", "yml", "toml", "ini", "cfg", "env", "sh", "bash", "zsh", "js", "jsx", "mjs", "cjs", "ts", "tsx", "py", "rb", "go", "rs", "java", "kt", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "php", "sql", "css", "scss", "tex"]);
export const cardVariant = (name) => {
  const t = extOf(name);
  return VARIANT[t] ?? (GENERIC.has(t) ? "generic" : "unknown");
};

export function fileMeta(path, name = basename(path)) {
  const size = existsSync(path) ? statSync(path).size : 0;
  const meta = { name, ext: extOf(name), size, kind: viewerKind(name), variant: cardVariant(name) };
  if (!size) return meta;
  if (meta.kind === "audio") return { ...meta, ...audioInfo(path) };
  if (meta.kind === "image") return { ...meta, ...imageSize(path) };
  if (meta.kind === "video") return { ...meta, ...videoInfo(path) };
  return meta;
}

// The Library's cheap document facts — a PDF's pages, a deck's slides, a
// workbook's sheet names, a Word file's page count when it recorded one —
// read once per file version (cached by mtime).
const docCache = new Map(); // path → { mtime, meta }
export async function docMeta(path) {
  let mtime;
  try { mtime = statSync(path).mtimeMs; } catch { return {}; }
  const hit = docCache.get(path);
  if (hit?.mtime === mtime) return hit.meta;
  const ext = extOf(path);
  const meta = {};
  try {
    if (ext === "pdf") {
      const n = (readFileSync(path).toString("latin1").match(/\/Type\s*\/Page(?![a-zA-Z])/g) || []).length;
      if (n) meta.pages = n;
    } else if (["docx", "pptx", "xlsx", "xlsm"].includes(ext)) {
      const { default: JSZip } = await import("jszip");
      const z = await JSZip.loadAsync(readFileSync(path));
      if (ext === "pptx") {
        const n = Object.keys(z.files).filter((f) => /^ppt\/slides\/slide\d+\.xml$/.test(f)).length;
        if (n) meta.slides = n;
      } else if (ext === "docx") {
        const n = Number((await z.file("docProps/app.xml")?.async("string"))?.match(/<Pages>(\d+)<\/Pages>/)?.[1]);
        if (n) meta.pages = n;
      } else {
        const wb = (await z.file("xl/workbook.xml")?.async("string")) || "";
        const dec = (t) => t.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, "&");
        const names = [...wb.matchAll(/<sheet\b[^>]*?\bname="([^"]*)"/g)].map((m) => dec(m[1]));
        if (names.length) meta.sheets = names;
      }
    }
  } catch {}
  docCache.set(path, { mtime, meta });
  return meta;
}

export function imageSize(path) {
  // sips can't read SVG: take its width/height, or the viewBox
  if (/\.svg$/i.test(path)) {
    try {
      const head = readFileSync(path, "utf8").slice(0, 2000).match(/<svg[^>]*>/i)?.[0] || "";
      const num = (k) => +(head.match(new RegExp(`\\s${k}="([\\d.]+)`))?.[1] || 0);
      const vb = head.match(/viewBox="[\d.\s-]+?([\d.]+)\s+([\d.]+)"/);
      return { width: num("width") || +(vb?.[1] || 0), height: num("height") || +(vb?.[2] || 0) };
    } catch {
      return {};
    }
  }
  try {
    const o = execFileSync("sips", ["-g", "pixelWidth", "-g", "pixelHeight", path], { encoding: "utf8" });
    return { width: +(o.match(/pixelWidth: (\d+)/)?.[1] || 0), height: +(o.match(/pixelHeight: (\d+)/)?.[1] || 0) };
  } catch {
    return {};
  }
}
// a poster frame (Quick Look) and the duration (AVFoundation via afinfo when
// the movie has an audio track; the player fills it in otherwise)
function videoInfo(path) {
  const dir = path.slice(0, path.lastIndexOf("/"));
  const out = {};
  try {
    execFileSync("qlmanage", ["-t", "-s", "640", "-o", dir, path], { stdio: "ignore", timeout: 20000 });
    if (existsSync(`${path}.png`)) {
      out.poster = `${basename(path)}.png`;
      Object.assign(out, imageSize(`${path}.png`));
    }
  } catch {}
  try {
    const info = execFileSync("afinfo", [path], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    out.duration = +(info.match(/estimated duration:\s*([\d.]+)/)?.[1] || 0);
  } catch {}
  return out;
}

// Grok's audio card: duration and a waveform (48 peaks, 0..1) from the samples
export function audioInfo(path) {
  try {
    const info = execFileSync("afinfo", [path], { encoding: "utf8" });
    const duration = +(info.match(/estimated duration:\s*([\d.]+)/)?.[1] || 0);
    const tmp = join("/tmp", `peaks-${process.pid}-${Date.now()}.wav`);
    execFileSync("afconvert", ["-f", "WAVE", "-d", "LEI16@8000", "-c", "1", path, tmp], { stdio: "ignore" });
    const buf = readFileSync(tmp);
    const data = buf.indexOf("data");
    const pcm = new Int16Array(buf.buffer, buf.byteOffset + data + 8, Math.floor((buf.length - data - 8) / 2));
    const N = 48, step = Math.max(1, Math.floor(pcm.length / N));
    const raw = Array.from({ length: N }, (_, i) => {
      let sum = 0;
      for (let j = i * step; j < Math.min(pcm.length, (i + 1) * step); j++) sum += pcm[j] * pcm[j];
      return Math.sqrt(sum / step);
    });
    const max = Math.max(1, ...raw);
    return { duration, peaks: raw.map((v) => Math.round((v / max) * 100) / 100) };
  } catch {
    return { duration: 0, peaks: [] };
  }
}

// ————— Grok's syntax palettes (cursor-dark from its bundle, cursor-light live) —————
function oklchHex(s) {
  const m = String(s).match(/oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*(?:\/\s*([\d.]+))?\)/);
  if (!m) return s;
  const [L, C, H] = [+m[1], +m[2], (+m[3] * Math.PI) / 180];
  const a = C * Math.cos(H), b = C * Math.sin(H);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const mm = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s3 = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const lin = [4.0767416621 * l - 3.3077115913 * mm + 0.2309699292 * s3, -1.2684380046 * l + 2.6097574011 * mm - 0.3413193965 * s3, -0.0041960863 * l - 0.7034186147 * mm + 1.707614701 * s3];
  const g = (x) => Math.round(255 * Math.min(1, Math.max(0, x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055)));
  const hex = lin.map((x) => g(x).toString(16).padStart(2, "0")).join("");
  const alpha = m[4] != null ? Math.round(+m[4] * 255).toString(16).padStart(2, "0") : "";
  return `#${hex}${alpha}`;
}
const SYNTAX = {
  dark: { foreground: "oklch(0.878 0.01 286.202)", background: "oklch(0.209 0 0)", keyword: "oklch(0.811 0.079 191.639)", string: "oklch(0.769 0.132 329.919)", function: "oklch(0.805 0.097 58.138)", number: "oklch(0.849 0.086 80.222)", comment: "oklch(0.919 0 0 / 0.369)", constant: "oklch(0.854 0.131 83.385)", parameter: "oklch(0.878 0.01 286.202)", punctuation: "oklch(0.878 0.01 286.202)", link: "oklch(0.799 0.106 249.274)" },
  light: { foreground: "oklch(0.191 0 0 / 0.922)", background: "oklch(0.991 0 0)", keyword: "oklch(0.488 0.195 13.082)", string: "oklch(0.7 0.095 290.796)", function: "oklch(0.662 0.144 39.487)", number: "oklch(0.567 0.168 346.061)", comment: "oklch(0.191 0 0 / 0.678)", constant: "oklch(0.488 0.102 243.616)", parameter: "oklch(0.191 0 0 / 0.922)", punctuation: "oklch(0.191 0 0 / 0.922)", link: "oklch(0.488 0.102 243.616)" },
};
// Grok's role map (`oF`): tag→keyword, attribute→parameter, property/variable→
// foreground, type/class/constantVariable→constant, languageVariable→keyword
function shikiTheme(name, p) {
  const c = Object.fromEntries(Object.entries(p).map(([k, v]) => [k, oklchHex(v)]));
  const rule = (scope, color, fontStyle) => ({ scope, settings: { foreground: color, ...(fontStyle ? { fontStyle } : {}) } });
  return {
    name,
    type: name.endsWith("dark") ? "dark" : "light",
    colors: { "editor.foreground": c.foreground, "editor.background": c.background },
    tokenColors: [
      rule(["comment", "punctuation.definition.comment"], c.comment),
      rule(["keyword", "storage", "storage.type", "storage.modifier", "keyword.control", "keyword.operator.new", "entity.name.tag", "variable.language", "support.type.primitive"], c.keyword),
      rule(["string", "string.quoted", "string.template", "punctuation.definition.string"], c.string),
      rule(["constant.numeric"], c.number),
      rule(["constant", "constant.language", "support.constant", "variable.other.constant", "entity.name.type", "entity.name.class", "support.type", "support.class", "entity.other.inherited-class", "entity.name.namespace"], c.constant),
      rule(["entity.name.function", "support.function", "meta.function-call entity.name.function", "entity.name.function.macro"], c.function),
      rule(["variable.parameter", "entity.other.attribute-name"], c.parameter),
      rule(["punctuation", "meta.brace", "keyword.operator"], c.punctuation),
      rule(["markup.underline.link", "string.other.link"], c.link),
      rule(["variable", "support.variable", "meta.property-name", "variable.other.property"], c.foreground),
    ],
  };
}
const LANG = { rs: "rust", ts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx", mjs: "javascript", cjs: "javascript", py: "python", go: "go", swift: "swift", json: "json", jsonc: "jsonc", toml: "toml", yaml: "yaml", yml: "yaml", sh: "bash", bash: "bash", zsh: "bash", css: "css", scss: "scss", html: "html", htm: "html", xml: "xml", sql: "sql", c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", java: "java", kt: "kotlin", rb: "ruby", php: "php", cs: "csharp", md: "markdown", ini: "ini", tex: "latex" };
let highlighter;
async function hl() {
  highlighter ??= await createHighlighter({
    themes: [shikiTheme("cursor-dark", SYNTAX.dark), shikiTheme("cursor-light", SYNTAX.light)],
    langs: [...new Set(Object.values(LANG))],
  });
  return highlighter;
}
/** Lines of tokens: [text, darkColor, lightColor]. */
export async function highlight(code, ext) {
  const lang = LANG[ext];
  if (!lang) return code.split("\n").map((l) => [[l, "", ""]]);
  const h = await hl();
  const lines = h.codeToTokensWithThemes(code, { lang, themes: { dark: "cursor-dark", light: "cursor-light" } });
  return lines.map((line) => line.map((t) => [t.content, t.variants.dark?.color || "", t.variants.light?.color || ""]));
}

function parseDelimited(text, sep) {
  const rows = [];
  let row = [], cell = "", q = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (q) {
      if (ch === '"' && text[i + 1] === '"') { cell += '"'; i++; }
      else if (ch === '"') q = false;
      else cell += ch;
    } else if (ch === '"') q = true;
    else if (ch === sep) { row.push(cell); cell = ""; }
    else if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && text[i + 1] === "\n") i++;
      row.push(cell); rows.push(row); row = []; cell = "";
    } else cell += ch;
  }
  if (cell || row.length) { row.push(cell); rows.push(row); }
  return rows;
}

// mammoth's HTML (Grok renders it as rich text) → our Markdown renderer
function htmlToMd(html) {
  const dec = (t) => t.replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&nbsp;/g, " ");
  const inline = (t) =>
    dec(
      t
        .replace(/<(strong|b)>([\s\S]*?)<\/\1>/g, "**$2**")
        .replace(/<(em|i)>([\s\S]*?)<\/\1>/g, "*$2*")
        .replace(/<a [^>]*href="([^"]*)"[^>]*>([\s\S]*?)<\/a>/g, "[$2]($1)")
        .replace(/<br\s*\/?>/g, "  \n")
        .replace(/<[^>]+>/g, ""),
    ).replace(/^\t?•\t/, "- ");
  let md = html
    .replace(/<table>([\s\S]*?)<\/table>/g, (_, body) => {
      const rows = [...body.matchAll(/<tr>([\s\S]*?)<\/tr>/g)].map((r) => [...r[1].matchAll(/<t[hd][^>]*>([\s\S]*?)<\/t[hd]>/g)].map((c) => inline(c[1]).trim()));
      if (!rows.length) return "";
      const head = rows[0], sep = head.map(() => "---");
      return "\n\n" + [head, sep, ...rows.slice(1)].map((r) => `| ${r.join(" | ")} |`).join("\n") + "\n\n";
    })
    .replace(/<h([1-6])>([\s\S]*?)<\/h\1>/g, (_, n, t) => `\n\n${"#".repeat(+n)} ${inline(t)}\n\n`)
    .replace(/<ol>([\s\S]*?)<\/ol>/g, (_, b) => "\n\n" + [...b.matchAll(/<li>([\s\S]*?)<\/li>/g)].map((m, i) => `${i + 1}. ${inline(m[1])}`).join("\n") + "\n\n")
    .replace(/<ul>([\s\S]*?)<\/ul>/g, (_, b) => "\n\n" + [...b.matchAll(/<li>([\s\S]*?)<\/li>/g)].map((m) => `- ${inline(m[1])}`).join("\n") + "\n\n")
    .replace(/<p>([\s\S]*?)<\/p>/g, (_, t) => `\n\n${inline(t)}\n\n`);
  return md.replace(/<[^>]+>/g, "").replace(/\n{3,}/g, "\n\n").trim();
}

const MAX_TEXT = 400_000;
// Kinds with a BetterOffice engine view: the app opens the bundled editor
// (real editing chrome) when server/office/editor/dist was built, else the
// read-only display-list page, else the classic render when the engines are
// absent (pptx has no other renderer — without them the card shows
// "unavailable").
const OFFICE_VIEW = { pptx: "pptx", xlsx: "xlsx", xlsm: "xlsx", docx: "docx" };
const EDITOR_BUNDLE = join(dirname(fileURLToPath(import.meta.url)), "office", "editor", "dist", "editor.js");
const officeUrl = async (path, rel, ext) => {
  const kind = OFFICE_VIEW[ext];
  if (!kind || !rel) return undefined;
  try {
    const { officeCaps } = await import("./office.mjs");
    if (!officeCaps()[kind]) return undefined;
    const editing = existsSync(EDITOR_BUNDLE);
    const page = editing ? "/office/editor/" : "/office/";
    // WKWebView caches the page URL hard — bake the bundle's mtime into the
    // query so a rebuilt editor always lands on an uncached URL.
    const v = editing ? `&v=${Math.floor(statSync(EDITOR_BUNDLE).mtimeMs)}` : "";
    return `${page}?path=${rel.split("/").map(encodeURIComponent).join("/")}&kind=${kind}${v}`;
  } catch {
    return undefined;
  }
};
/** What the app needs to draw a file in Grok's viewer for its kind. `rel` is
 *  the file's path under generated/files — the key /file/raw serves it by. */
export async function renderFile(path, rel) {
  const name = basename(path);
  const meta = fileMeta(path, name.replace(/^[a-z0-9]+-[a-z0-9]{4}-/, ""));
  const { kind, ext } = meta;
  const office = await officeUrl(path, rel, ext);
  if (!existsSync(path)) return { ...meta, error: "missing" };
  if (kind === "pptx") return { ...meta, ...(office ? { office } : {}) };
  if (kind === "markdown") return { ...meta, text: readFileSync(path, "utf8").slice(0, MAX_TEXT) };
  if (kind === "text") {
    const text = readFileSync(path, "utf8").slice(0, MAX_TEXT);
    return { ...meta, lines: await highlight(text, ext) };
  }
  if (kind === "json") {
    // Grok's JSON viewer is a collapsible tree; lines stay for unparsable files
    const text = readFileSync(path, "utf8");
    try {
      return { ...meta, json: JSON.parse(text) };
    } catch {
      return { ...meta, lines: await highlight(text.slice(0, MAX_TEXT), "json") };
    }
  }
  if (kind === "html") {
    // Preview is the web view; Source is the highlighted markup
    return { ...meta, lines: await highlight(readFileSync(path, "utf8").slice(0, MAX_TEXT), "html") };
  }
  if (kind === "table") {
    if (ext === "csv" || ext === "tsv") {
      return { ...meta, sheets: [{ name, rows: parseDelimited(readFileSync(path, "utf8"), ext === "tsv" ? "\t" : ",").slice(0, 2000) }] };
    }
    const wb = new ExcelJS.Workbook();
    await wb.xlsx.readFile(path);
    const sheets = wb.worksheets.map((ws) => {
      const rows = [], bold = [];
      ws.eachRow({ includeEmpty: true }, (r) => {
        const vals = [], b = [];
        for (let c = 1; c <= ws.columnCount; c++) {
          const cell = r.getCell(c);
          const v = cell.value;
          vals.push(v == null ? "" : typeof v === "object" ? String(v.result ?? v.text ?? v.richText?.map((t) => t.text).join("") ?? "") : String(v));
          b.push(!!cell.font?.bold);
        }
        rows.push(vals);
        bold.push(b);
      });
      const widths = Array.from({ length: ws.columnCount }, (_, c) => ws.getColumn(c + 1).width || 0);
      return { name: ws.name, rows: rows.slice(0, 2000), bold: bold.slice(0, 2000), widths };
    });
    return { ...meta, sheets, ...(office ? { office } : {}) };
  }
  if (kind === "docx") {
    const { value } = await mammoth.convertToHtml({ path });
    return { ...meta, text: htmlToMd(value), ...(office ? { office } : {}) };
  }
  return meta; // image · video · audio · pdf · html · unknown: served as-is
}

// ————— mock response: one file of every kind, no model call —————
// `only` (a Set of extensions, or null) narrows it to one kind — "/mock pdf".
export function mockFiles(dir, only = null) {
  mkdirSync(dir, { recursive: true });
  const out = [];
  const wantExt = (e) => !only || only.has(e);
  const put = (name, data) => {
    if (!wantExt(name.split(".").pop().toLowerCase())) return null;
    const p = join(dir, name);
    writeFileSync(p, data);
    out.push(p);
    return p;
  };
  // a few images for the gallery (different aspects), and an unknown type
  if (wantExt("png")) try {
    const shots = [["sunset", 640, 400, "#f59e0b", "#ef4444"], ["ocean", 400, 600, "#06b6d4", "#3b82f6"], ["forest", 600, 600, "#22c55e", "#14532d"], ["night", 800, 450, "#6366f1", "#0f172a"]];
    for (const [n, w, h, a, b] of shots) {
      const svg = join(dir, `.${n}.svg`);
      writeFileSync(svg, `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${a}"/><stop offset="1" stop-color="${b}"/></linearGradient></defs><rect width="${w}" height="${h}" fill="url(#g)"/><text x="${w / 2}" y="${h / 2 + 12}" font-family="Helvetica" font-size="36" fill="#fff" text-anchor="middle">${n}</text></svg>`);
      execFileSync("qlmanage", ["-t", "-s", String(Math.max(w, h)), "-o", dir, svg], { stdio: "ignore", timeout: 15000 });
      execFileSync("mv", [`${svg}.png`, join(dir, `${n}.png`)]);
      // Quick Look pads to a square: crop back to the real aspect
      execFileSync("sips", ["-c", String(h), String(w), join(dir, `${n}.png`)], { stdio: "ignore" });
      out.push(join(dir, `${n}.png`));
    }
  } catch {}
  // animated media: a GIF and a WebP (ImageMagick) and an SVG with SMIL animation
  if (wantExt("gif") || wantExt("webp")) try {
    const frames = [];
    for (let i = 0; i < 20; i++) {
      const x = 40 + Math.round(240 * Math.abs(Math.sin((i / 20) * Math.PI)));
      frames.push("(", "-size", "320x180", "xc:#0f172a", "-fill", `hsl(${i * 18},80%,60%)`, "-draw", `circle ${x},110 ${x + 22},110`, "-fill", "#ffffff", "-pointsize", "20", "-draw", `text 16,34 'animated GIF ${i + 1}/20'`, ")");
    }
    // the gif is the webp's source: build it either way, list it only when asked
    execFileSync("magick", ["-delay", "6", "-loop", "0", ...frames, join(dir, "bounce.gif")], { stdio: "ignore", timeout: 20000 });
    if (wantExt("gif")) out.push(join(dir, "bounce.gif"));
    if (wantExt("webp")) {
      execFileSync("magick", [join(dir, "bounce.gif"), "-loop", "0", join(dir, "spin.webp")], { stdio: "ignore", timeout: 20000 });
      out.push(join(dir, "spin.webp"));
    }
  } catch {}
  put("pulse.svg", '<svg xmlns="http://www.w3.org/2000/svg" width="480" height="270" viewBox="0 0 480 270"><defs><linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="#6366f1"/><stop offset="1" stop-color="#ec4899"/></linearGradient></defs><rect width="480" height="270" rx="20" fill="url(#g)"/><text x="32" y="56" font-family="Helvetica" font-size="28" font-weight="bold" fill="#fff">Animated SVG</text><circle cx="380" cy="80" r="30" fill="#fde047"><animate attributeName="r" values="22;38;22" dur="1.6s" repeatCount="indefinite"/></circle><polygon points="120,230 180,130 240,230" fill="#ffffff99"><animateTransform attributeName="transform" type="rotate" from="0 180 196" to="360 180 196" dur="4s" repeatCount="indefinite"/></polygon></svg>');
  put("contact.vcf", "BEGIN:VCARD\nVERSION:3.0\nFN:Kiffy Bot\nEND:VCARD\n");
  // an image (inline in the reply, as Grok's)
  if (wantExt("png")) try {
    const svg = join(dir, ".chip.svg");
    writeFileSync(svg, '<svg xmlns="http://www.w3.org/2000/svg" width="420" height="210"><rect width="420" height="210" rx="18" fill="#2f7bf6"/><text x="210" y="122" font-family="Helvetica" font-size="34" fill="#fff" text-anchor="middle">SVG SAMPLE</text></svg>');
    execFileSync("qlmanage", ["-t", "-s", "420", "-o", dir, svg], { stdio: "ignore", timeout: 15000 });
    execFileSync("mv", [`${svg}.png`, join(dir, "chip.png")]);
    execFileSync("sips", ["-c", "210", "420", join(dir, "chip.png")], { stdio: "ignore" });
    out.push(join(dir, "chip.png"));
  } catch {}
  // a full GitHub-Flavored Markdown showcase (vendored in fixtures/) — every
  // element the markdown viewer must render, not the old stub
  if (wantExt("md")) {
    const readmeFixture = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "README.md");
    copyFileSync(readmeFixture, join(dir, "README.md"));
    out.push(join(dir, "README.md"));
  }
  put("main.rs", `use std::collections::HashMap;\n\n/// Counts words in a sentence.\nfn count_words(text: &str) -> HashMap<&str, usize> {\n    let mut counts = HashMap::new();\n    for word in text.split_whitespace() {\n        *counts.entry(word).or_insert(0) += 1;\n    }\n    counts\n}\n\nfn main() {\n    let counts = count_words("the quick brown fox jumps over the lazy dog the end");\n    let mut pairs: Vec<_> = counts.into_iter().collect();\n    pairs.sort_by(|a, b| b.1.cmp(&a.1));\n    for (word, n) in pairs.iter().take(3) {\n        println!("{word}: {n}");\n    }\n}\n`);
  put("App.tsx", `import { signal, Text, View } from "nsh";\n\nexport default function Counter() {\n  const n = signal(0);\n  return (\n    <View onPress={() => n.update((v) => v + 1)}>\n      <Text>{\`Clicked \${n()} times\`}</Text>\n    </View>\n  );\n}\n`);
  put("data.json", JSON.stringify({ bot: "Kiffy", model: { id: "gpt-6-luna", effort: "max", fast: false }, tags: ["native", "nsh"], count: 3, ok: true }));
  put("budget.csv", "Category,Q1,Q2,Q3,Q4\nRent,3000,3000,3100,3100\nSoftware,420,460,510,505\nTravel,1200,300,900,1500\nMeals,260,310,280,330\n");
  put("notes.txt", "Meeting notes — Oct 19\n\n1. Ship file viewers\n2. Verify parity with Grok Bot\n3. Mock responses so tests cost nothing\n");
  // an interactive analytics dashboard (vendored in fixtures/) — responsive
  // grid, canvas charts, live feed; exercises the web-view preview properly
  if (wantExt("html")) {
    const pageFixture = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "page.html");
    copyFileSync(pageFixture, join(dir, "page.html"));
    out.push(join(dir, "page.html"));
  }
  // docx and pdf via macOS tools; xlsx via exceljs (async, best effort)
  if (wantExt("pdf")) try {
    const html = put(".memo-src.html", "<h1>Quarterly memo</h1><p>Revenue grew <b>18%</b> quarter over quarter.</p><ul><li>Launch the viewer</li><li>Hire two engineers</li></ul><table border=1><tr><th>Region</th><th>Sales</th></tr><tr><td>APAC</td><td>1.2M</td></tr><tr><td>EMEA</td><td>0.9M</td></tr></table>");
    out.splice(out.indexOf(html), 1);
    // a real, element-rich PDF (vendored in fixtures/) — exercises every part
    // of the viewer, unlike the plain-text cupsfilter page it replaces
    const fixture = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "PDF_Elements_Showcase.pdf");
    copyFileSync(fixture, join(dir, "PDF_Elements_Showcase.pdf"));
    out.push(join(dir, "PDF_Elements_Showcase.pdf"));
  } catch {}
  if (wantExt("m4a") || wantExt("wav") || wantExt("aiff")) try {
    // the aiff is the source the others convert from — make it either way
    execFileSync("say", ["-o", join(dir, "voice.aiff"), "This is a mock audio file."], { stdio: "ignore" });
    if (wantExt("aiff")) out.push(join(dir, "voice.aiff"));
    if (wantExt("m4a")) {
      execFileSync("afconvert", ["-f", "m4af", "-d", "aac", join(dir, "voice.aiff"), join(dir, "voice.m4a")], { stdio: "ignore" });
      out.push(join(dir, "voice.m4a"));
    }
    if (wantExt("wav")) {
      execFileSync("afconvert", ["-f", "WAVE", "-d", "LEI16@22050", join(dir, "voice.aiff"), join(dir, "tone.wav")], { stdio: "ignore" });
      out.push(join(dir, "tone.wav"));
    }
  } catch {}
  return out;
}
// a real .pptx via the BetterOffice engine — the deck that used to be a fake
// "previews aren't available" placeholder, now exercising the office viewer
export async function mockPptx(dir) {
  const { officeCreateBytes } = await import("./office.mjs");
  const bytes = await officeCreateBytes("pptx", {
    pptx: {
      slides: [
        { title: "File viewers", bullets: ["One card per reply file", "Tap to open the viewer"], notes: "Kickoff slide." },
        { title: "What's new", bullets: ["Real decks render as slides", "Spreadsheets keep their formulas", "Fallbacks stay for other types"] },
        { title: "Next", bullets: ["Speaker notes land in the file", "Export keeps every part intact"] },
      ],
    },
  });
  const p = join(dir, "deck.pptx");
  writeFileSync(p, bytes);
  return p;
}
export async function mockXlsx(dir) {
  const wb = new ExcelJS.Workbook();
  const ws = wb.addWorksheet("Sales");
  ws.addRows([["Month", "Units", "Revenue"], ["Jan", 120, 2400], ["Feb", 150, 3000], ["Mar", 175, 3500]]);
  const ws2 = wb.addWorksheet("Costs");
  ws2.addRows([["Item", "Cost"], ["Servers", 800], ["Ads", 450]]);
  const p = join(dir, "sales.xlsx");
  await wb.xlsx.writeFile(p);
  return p;
}
// a real .docx (heading, bold run, list, table), so the docx viewer is tested
// against Word structure rather than textutil's flattened output
export async function mockDocx(dir) {
  const { default: JSZip } = await import("jszip");
  const z = new JSZip();
  const W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"';
  const p = (t, style) => `<w:p>${style ? `<w:pPr><w:pStyle w:val="${style}"/></w:pPr>` : ""}${t}</w:p>`;
  const r = (t, b) => `<w:r>${b ? "<w:rPr><w:b/></w:rPr>" : ""}<w:t xml:space="preserve">${t}</w:t></w:r>`;
  const cell = (t) => `<w:tc>${p(r(t))}</w:tc>`;
  const row = (...c) => `<w:tr>${c.map(cell).join("")}</w:tr>`;
  const body = [
    p(r("Quarterly memo"), "Heading1"),
    p(r("Revenue grew ") + r("18%", true) + r(" quarter over quarter.")),
    p(r("Next steps"), "Heading2"),
    p(r("Launch the file viewers"), "ListBullet"),
    p(r("Hire two engineers"), "ListBullet"),
    `<w:tbl>${row("Region", "Sales")}${row("APAC", "1.2M")}${row("EMEA", "0.9M")}</w:tbl>`,
    p(r("Prepared by Kiffy.")),
  ].join("");
  z.file("[Content_Types].xml", '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>');
  z.file("_rels/.rels", '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>');
  z.file("word/_rels/document.xml.rels", '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>');
  z.file("word/styles.xml", `<?xml version="1.0"?><w:styles ${W}><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/></w:style><w:style w:type="paragraph" w:styleId="ListBullet"><w:name w:val="List Bullet"/></w:style></w:styles>`);
  z.file("word/document.xml", `<?xml version="1.0"?><w:document ${W}><w:body>${body}</w:body></w:document>`);
  const out = join(dir, "memo.docx");
  writeFileSync(out, await z.generateAsync({ type: "nodebuffer" }));
  return out;
}
export { copyFileSync };

// ————— media pipeline: sizes, lossless optimize, feed derivatives —————

/** Minimal PNG writer — zlib'd RGBA scanlines, no external tools. */
const CRC_T = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c | 0;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = ~0;
  for (const b of buf) c = CRC_T[(c ^ b) & 0xff] ^ (c >>> 8);
  return ~c | 0;
};
const pngChunk = (type, data) => {
  const b = Buffer.alloc(data.length + 12);
  b.writeUInt32BE(data.length, 0);
  b.write(type, 4, "ascii");
  data.copy(b, 8);
  b.writeInt32BE(crc32(Buffer.concat([Buffer.from(type, "ascii"), data])), data.length + 8);
  return b;
};
/** paint(x, y) → [r,g,b,a] */
export function pngEncode(w, h, paint) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 6; // 8-bit RGBA
  const raw = Buffer.alloc(h * (1 + w * 4));
  for (let y = 0; y < h; y++) {
    const row = y * (1 + w * 4);
    raw[row] = 0; // filter: none
    for (let x = 0; x < w; x++) {
      const [r, g, b, a] = paint(x, y);
      const o = row + 1 + x * 4;
      raw[o] = r;
      raw[o + 1] = g;
      raw[o + 2] = b;
      raw[o + 3] = a;
    }
  }
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", deflateSync(raw, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

// 3×5 digit glyphs for mock tiles
const DIGIT = {
  "0": [7, 5, 5, 5, 7], "1": [2, 6, 2, 2, 7], "2": [7, 1, 7, 4, 7], "3": [7, 1, 7, 1, 7],
  "4": [5, 5, 7, 1, 1], "5": [7, 4, 7, 1, 7], "6": [7, 4, 7, 5, 7], "7": [7, 1, 1, 2, 2],
  "8": [7, 5, 7, 5, 7], "9": [7, 5, 7, 1, 7], "+": [0, 2, 7, 2, 0],
};
const hexRgb = (s) => [parseInt(s.slice(1, 3), 16), parseInt(s.slice(3, 5), 16), parseInt(s.slice(5, 7), 16)];
const lerp = (a, b, t) => Math.round(a + (b - a) * t);

/** A labeled mock tile PNG at an exact size — no external tools, so n of
 *  them are cheap. Digit drawn 3×5 in white, centered on a soft diagonal. */
function mockTile(w, h, label, c0, c1) {
  const [r0, g0, b0] = hexRgb(c0);
  const [r1, g1, b1] = hexRgb(c1);
  const text = String(label);
  const s = Math.max(2, Math.floor(Math.min(w, h) / 9)); // pixel block
  const gw = text.length * 4 * s - s, gh = 5 * s;
  const gx = Math.floor((w - gw) / 2), gy = Math.floor((h - gh) / 2);
  const on = (x, y) => {
    const dy = Math.floor((y - gy) / s);
    if (dy < 0 || dy > 4) return false;
    const k = Math.floor((x - gx) / s);
    if (k < 0 || k >= text.length * 4 - 1 || k % 4 === 3) return false; // col 3 of each char is the gap
    return !!(DIGIT[text[Math.floor(k / 4)]]?.[dy] & (4 >> (k % 4)));
  };
  return pngEncode(w, h, (x, y) => {
    if (on(x, y)) return [255, 255, 255, 255];
    const t = (x + y) / (w + h);
    return [lerp(r0, r1, t), lerp(g0, g1, t), lerp(b0, b1, t), 255];
  });
}

const MOCK_ASPECTS = [[16, 9], [1, 1], [4, 5], [3, 2], [9, 16], [4, 3], [3, 4], [2, 1]];
const MOCK_COLORS = [["#ef4444", "#b91c1c"], ["#3b82f6", "#1e40af"], ["#22c55e", "#15803d"], ["#f59e0b", "#b45309"], ["#8b5cf6", "#6d28d9"], ["#06b6d4", "#0e7490"], ["#ec4899", "#be185d"], ["#f97316", "#c2410c"]];

/** n numbered tiles; `ar` = [w,h] fixes every tile to that aspect, else they
 *  cycle landscape → square → portrait so one message exercises all rows. */
export function mockImages(dir, n, ar) {
  mkdirSync(dir, { recursive: true });
  const paths = [];
  for (let i = 0; i < n; i++) {
    const [aw, ah] = ar || MOCK_ASPECTS[i % MOCK_ASPECTS.length];
    const longEdge = 640;
    const w = aw >= ah ? longEdge : Math.round((longEdge * aw) / ah);
    const h = aw >= ah ? Math.round((longEdge * ah) / aw) : longEdge;
    const p = join(dir, `mock-${i + 1}.png`);
    writeFileSync(p, mockTile(w, h, i + 1, ...MOCK_COLORS[i % MOCK_COLORS.length]));
    paths.push(p);
  }
  return paths;
}

/** Lossless byte diet: PNG drops ancillary chunks (EXIF/text/etc, keeps
 *  color-critical iCCP/sRGB/gAMA/cHRM) and re-deflates IDAT at level 9;
 *  JPEG drops APPn/COM markers (metadata, embedded thumbs). Pixels are
 *  never touched — and the rewrite only lands when it's actually smaller. */
export function optimizeImage(path) {
  try {
    const buf = readFileSync(path);
    const out = buf[0] === 0x89 && buf[1] === 0x50 ? optimizePng(buf) : buf[0] === 0xff && buf[1] === 0xd8 ? optimizeJpeg(buf) : null;
    if (out && out.length < buf.length) writeFileSync(path, out);
    return out ? Math.max(0, buf.length - out.length) : 0;
  } catch {
    return 0;
  }
}
const PNG_KEEP = new Set(["IHDR", "PLTE", "tRNS", "iCCP", "sRGB", "gAMA", "cHRM", "IEND"]);
function optimizePng(buf) {
  const segs = [];
  for (let p = 8; p + 8 <= buf.length; ) {
    const len = buf.readUInt32BE(p);
    const type = buf.toString("ascii", p + 4, p + 8);
    const end = p + 12 + len;
    if (end > buf.length) return null;
    segs.push({ type, data: buf.subarray(p + 8, p + 8 + len) });
    p = end;
    if (type === "IEND") break;
  }
  const idat = segs.filter((s) => s.type === "IDAT");
  if (!idat.length || segs[0]?.type !== "IHDR") return null;
  let idatOut;
  try {
    idatOut = deflateSync(inflateSync(Buffer.concat(idat.map((s) => s.data))), { level: 9 });
  } catch {
    return null;
  }
  let wrote = false;
  const out = [buf.subarray(0, 8)];
  for (const s of segs) {
    if (s.type === "IDAT") {
      if (!wrote) {
        out.push(pngChunk("IDAT", idatOut));
        wrote = true;
      }
      continue;
    }
    if (PNG_KEEP.has(s.type)) out.push(pngChunk(s.type, s.data));
  }
  return Buffer.concat(out);
}
function optimizeJpeg(buf) {
  if (buf.length < 4) return null;
  const out = [buf.subarray(0, 2)]; // SOI
  for (let p = 2; p + 4 <= buf.length; ) {
    if (buf[p] !== 0xff) {
      out.push(buf.subarray(p)); // entropy-coded stream — copy verbatim
      break;
    }
    const marker = buf[p + 1];
    if (marker === 0xda) {
      out.push(buf.subarray(p)); // SOS + image data to the end
      break;
    }
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd9)) {
      out.push(buf.subarray(p, p + 2)); // standalone markers
      p += 2;
      continue;
    }
    const len = buf.readUInt16BE(p + 2);
    if (p + 2 + len > buf.length) return null;
    if (!((marker >= 0xe0 && marker <= 0xef) || marker === 0xfe)) out.push(buf.subarray(p, p + 2 + len));
    p += 2 + len;
  }
  return Buffer.concat(out);
}

/** Does this image carry frames? A live decode of an animated tile burns the
 *  scroll budget — the feed shows a static first frame instead (Grok does the
 *  same), the original still plays in the lightbox. */
export function isAnimated(path, ext = extOf(path)) {
  if (ext === "gif") return true;
  try {
    const fd = openSync(path, "r");
    const head = Buffer.alloc(4096);
    const n = readSync(fd, head, 0, 4096, 0);
    closeSync(fd);
    const b = head.subarray(0, n);
    if (ext === "webp") return b.includes(Buffer.from("ANMF")) || b.includes(Buffer.from("ANIM"));
    if (ext === "png") return b.includes(Buffer.from("acTL"));
  } catch {}
  return false;
}

/** Display derivative for the feed: long edge ≤1120px (~560pt @2x), always a
 *  static PNG. A 24MP upload no longer decodes full-res into a 300pt tile, and
 *  animated / vector media (GIF, animated WebP/APNG, SVG) collapse to a first
 *  frame — the feed decodes once and never re-encodes per tick. The original
 *  is untouched for the lightbox. `force` keeps a larger derivative when the
 *  alternative is live decode or a web view; otherwise kept only when smaller. */
export function feedVariant(path, force = false) {
  const feed = path.replace(/\.[^.]+$/, "") + ".feed.png";
  const dir = path.slice(0, path.lastIndexOf("/"));
  try {
    if (extOf(path) === "svg") {
      // sips can't read SVG — Quick Look rasterizes it (first frame)
      execFileSync("qlmanage", ["-t", "-s", "1120", "-o", dir, path], { stdio: "ignore", timeout: 20000 });
      if (existsSync(`${path}.png`)) renameSync(`${path}.png`, feed);
    } else {
      try {
        execFileSync("sips", ["-Z", "1120", path, "--out", feed], { stdio: "ignore", timeout: 20000 });
      } catch {
        // sips can't read animated WebP/AVIF — ImageMagick's first frame
        execFileSync("magick", [`${path}[0]`, feed], { stdio: "ignore", timeout: 20000 });
      }
    }
    if (existsSync(feed) && (force || statSync(feed).size < statSync(path).size)) return feed;
    if (existsSync(feed)) unlinkSync(feed);
  } catch {}
  return null;
}

/** A turn's file record: fileMeta dims + a feed derivative for images.
 *  `relFile` is the file's path relative to the out dir ("/file/raw/" key). */
export function fileRecord(p, relFile) {
  const m = fileMeta(p);
  const dirRel = relFile.slice(0, relFile.length - basename(relFile).length);
  const animated = m.kind === "image" && isAnimated(p, m.ext);
  const feed = m.kind === "image" ? feedVariant(p, m.ext === "svg" || animated) : null;
  return {
    path: relFile,
    ...m,
    ...(animated ? { animated: true } : {}),
    ...(feed ? { feed: dirRel + basename(feed) } : {}),
    ...(m.poster ? { poster: dirRel + m.poster } : {}),
  };
}
