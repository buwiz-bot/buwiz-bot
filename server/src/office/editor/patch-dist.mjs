#!/usr/bin/env node
// Post-build patch for the bundled BetterOffice editors.
// The packages draw their save command with a download-tray glyph
// (docx maps save → "file_download"; pptx/xlsx hardcode a down-arrow
// path). In this app the button writes back to the open file, so it
// should read as save — a floppy. Patching the emitted bundle keeps
// node_modules untouched.
import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const DIST = join(dirname(fileURLToPath(import.meta.url)), "dist");

// feather-style floppy, same 24×24 stroke convention as the other icons
const SAVE_PATH = "M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2zM17 21v-8H7v8M7 3v5h8";

const PATCHES = [
  // pptx-react / xlsx-react: the save icon's down-arrow-into-tray path
  { from: 'd:"M12 3v11m0 0 4-4m-4 4-4-4M5 19h14"', to: `d:"${SAVE_PATH}"` },
  // docx-react: command→icon map entry
  { from: 'save:"file_download"', to: 'save:"save"' },
];

let touched = 0;
for (const name of readdirSync(DIST)) {
  if (!name.endsWith(".js") && !name.endsWith(".mjs")) continue;
  const p = join(DIST, name);
  let src = readFileSync(p, "utf8");
  let out = src;
  for (const { from, to } of PATCHES) out = out.split(from).join(to);
  if (out !== src) {
    writeFileSync(p, out);
    touched++;
  }
}
console.log(`patch-dist: patched ${touched} file(s)`);
