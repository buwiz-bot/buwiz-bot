// The highlighter's themes (files.mjs `cursorThemes()`, Grok's cursor-dark
// and cursor-light palettes) as TextMate .tmTheme files, for buwiz-broker's
// syntect highlighter (broker/buwiz-broker/src/hub/render/).
//
//   node tools/tmtheme.mjs          writes them
//   node tools/tmtheme.mjs --check  exits 1 when the committed ones differ
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "broker", "buwiz-broker", "src", "hub", "render");

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const str = (k, v) => `\t\t\t\t<key>${k}</key>\n\t\t\t\t<string>${esc(v)}</string>\n`;

/** A Shiki theme object as a .tmTheme plist. */
export function tmTheme(theme) {
  const item = (scope, settings) =>
    "\t\t<dict>\n" +
    (scope ? `\t\t\t<key>scope</key>\n\t\t\t<string>${esc(scope)}</string>\n` : "") +
    "\t\t\t<key>settings</key>\n\t\t\t<dict>\n" +
    Object.entries(settings).map(([k, v]) => str(k, v)).join("") +
    "\t\t\t</dict>\n\t\t</dict>\n";
  const global = item(null, { foreground: theme.colors["editor.foreground"], background: theme.colors["editor.background"] });
  const rules = theme.tokenColors.map((r) => item([].concat(r.scope).join(", "), r.settings)).join("");

  return '<?xml version="1.0" encoding="UTF-8"?>\n' +
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' +
    '<plist version="1.0">\n<dict>\n' +
    `\t<key>name</key>\n\t<string>${esc(theme.name)}</string>\n` +
    "\t<key>settings</key>\n\t<array>\n" + global + rules + "\t</array>\n</dict>\n</plist>\n";
}

/** [file, text] for each theme. */
export async function tmThemes() {
  const { cursorThemes } = await import("../src/files.mjs");

  return cursorThemes().map((t) => [join(OUT, `${t.name}.tmTheme`), tmTheme(t)]);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const check = process.argv.includes("--check");
  let stale = 0;

  for (const [file, text] of await tmThemes()) {
    if (!check) writeFileSync(file, text);
    else if (readFileSync(file, "utf8") !== text) {
      console.error(`stale: ${file} (run node tools/tmtheme.mjs)`);
      stale++;
    }
  }
  process.exit(stale ? 1 : 0);
}
