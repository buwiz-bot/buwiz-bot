// Dev-only oracle: builds upstream Pele at the pinned commit and writes the golden SVGs the
// Rust port is compared against, byte for byte.
//
//   node pele/tools/golden.mjs <scratch-dir>
//
// <scratch-dir> receives the upstream clone and its build; nothing there is tracked. Inputs:
//   - pele/tests/samples/*.mmd (hand-written, every type and a few errors);
//   - every diagram source in upstream's tests/compat specs and tests/corpus docs, pulled out
//     the way upstream's own tests/support/corpus.ts loadCorpus() does.
// Output: pele/tests/golden/<type>/<name>.{mmd,svg,json}. <type> is the detected type, or
// `none` for an input no detector matches. Rendering uses render(code) with no options, as
// app/pele/serve.mjs does. Corpus goldens are written for the types in PORTED (and `none`);
// the samples for every type. `--all` writes every corpus golden (about 26 MB).
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const UPSTREAM = "https://github.com/obsidianmd/pele";
export const PIN = "36e584d59152181927e0d8bf70fddcd2a5f33d6c"; // main after 1.12.0 (1.12.0-8-g36e584d)

// Types the Rust port covers; a later phase adds its types here and reruns the script.
const PORTED = new Set(["flowchart", "none"]);
const ALL = process.argv.includes("--all");

const HERE = dirname(fileURLToPath(import.meta.url));
const CRATE = resolve(HERE, "..");
const scratch = process.argv.slice(2).find((a) => !a.startsWith("--"));
if (!scratch) {
  console.error("usage: node pele/tools/golden.mjs <scratch-dir>");
  process.exit(2);
}
const src = join(resolve(scratch), "upstream");
const run = (cmd, args, cwd) => execFileSync(cmd, args, { cwd, stdio: ["ignore", "inherit", "inherit"] });

if (!existsSync(join(src, ".git"))) run("git", ["clone", "-q", UPSTREAM, src]);
const head = execFileSync("git", ["rev-parse", "HEAD"], { cwd: src }).toString().trim();
if (head !== PIN) {
  run("git", ["fetch", "-q", "origin"], src);
  run("git", ["checkout", "-q", PIN], src);
}
if (!existsSync(join(src, "node_modules", ".bin", "esbuild"))) run("npm", ["ci", "--ignore-scripts"], src);
// The same bundle as upstream's build script, unminified so a stack trace reads.
const bundle = join(resolve(scratch), "build", "pele.mjs");
mkdirSync(dirname(bundle), { recursive: true });
run(join(src, "node_modules", ".bin", "esbuild"), ["src/index.ts", "--bundle", "--format=esm", "--target=es2022", "--charset=utf8", `--outfile=${bundle}`, "--log-level=warning"], src);
const pele = await import(pathToFileURL(bundle).href);

// Keyword per compat directory, as upstream's tests pass it to loadCorpus().
const KEYWORDS = {
  agentflow: /agentflow-beta/, architecture: /architecture/, block: /block/, c4: /C4(?:Context|Container|Component|Dynamic|Deployment)/,
  class: /classDiagram/, cynefin: /cynefin-beta/, er: /erDiagram/, eventmodeling: /eventmodeling/, flowchart: /graph|flowchart/,
  gantt: /gantt/, git: /gitGraph/, info: /info/, ishikawa: /ishikawa/i, journey: /journey/, kanban: /kanban/, mindmap: /mindmap/,
  packet: /packet/, pie: /pie/, quadrant: /quadrantChart/, radar: /radar/, railroad: /railroad/i, requirement: /requirement/,
  sankey: /sankey/i, sequence: /sequenceDiagram/i, state: /stateDiagram/, swimlane: /swimlane-beta/, timeline: /timeline/,
  treemap: /treemap/, treeview: /treeView-beta/, usecase: /usecase-beta/, venn: /venn-beta/, wardley: /wardley-beta/i, xychart: /xychart/i,
};

const inputs = []; // [name, code, source]
for (const file of readdirSync(join(CRATE, "tests", "samples")).filter((f) => f.endsWith(".mmd")).sort()) {
  inputs.push([basename(file, ".mmd"), readFileSync(join(CRATE, "tests", "samples", file), "utf8"), `samples/${file}`]);
}
const seen = new Set(inputs.map((i) => i[1]));
const add = (stem, code, source, counter) => {
  if (seen.has(code)) return;
  seen.add(code);
  const n = (counter[stem] = (counter[stem] ?? 0) + 1);
  inputs.push([`${stem}-${String(n).padStart(3, "0")}`, code, source]);
};
for (const dir of Object.keys(KEYWORDS)) {
  const keyword = KEYWORDS[dir];
  const counter = {};
  const compat = join(src, "tests", "compat", dir);
  if (existsSync(compat)) {
    for (const file of readdirSync(compat).filter((f) => /\.spec\.[jt]s$/.test(f)).sort()) {
      const text = readFileSync(join(compat, file), "utf8");
      const stem = `${dir}-${file.replace(/\.spec\.[jt]s$/, "")}`;
      for (const m of text.matchAll(/`((?:[^`\\]|\\.)*)`|'((?:[^'\\\n]|\\.)*)'|"((?:[^"\\\n]|\\.)*)"/g)) {
        const raw = m[1] ?? m[2] ?? m[3];
        if (!keyword.test(raw)) continue;
        let code;
        try {
          code = new Function("return `" + raw.replace(/`/g, "\\`").replace(/\$\{/g, "\\${") + "`")();
        } catch {
          continue;
        }
        add(stem, code, `tests/compat/${dir}/${file}`, counter);
      }
    }
  }
  const docs = join(src, "tests", "corpus", `${dir}-docs.json`);
  if (existsSync(docs)) for (const code of JSON.parse(readFileSync(docs, "utf8"))) add(`${dir}-docs`, code, `tests/corpus/${dir}-docs.json`, counter);
}

const out = join(CRATE, "tests", "golden");
rmSync(out, { recursive: true, force: true });
const counts = {};
for (const [name, code, source] of inputs) {
  let json;
  let svg;
  let type;
  try {
    const r = pele.render(code);
    type = r.type;
    svg = r.svg;
    json = { source, type: r.type, mermaidType: r.mermaidType, width: r.width, height: r.height, links: r.links };
  } catch (e) {
    if (e?.name !== "PeleError") throw new Error(`${name}: ${e?.stack ?? e}`);
    type = e.type ?? "none";
    json = { source, error: { code: e.code, type: e.type, mermaidType: e.mermaidType, line: e.line, column: e.column, message: e.message, snippet: e.snippet } };
  }
  if (!ALL && !PORTED.has(type) && !source.startsWith("samples/")) continue;
  const dir = join(out, type);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, `${name}.mmd`), code);
  if (svg !== undefined) writeFileSync(join(dir, `${name}.svg`), svg);
  writeFileSync(join(dir, `${name}.json`), JSON.stringify(json, null, 1) + "\n");
  const c = (counts[type] ??= { ok: 0, err: 0 });
  if (svg !== undefined) c.ok++;
  else c.err++;
}
for (const [type, c] of Object.entries(counts).sort()) console.log(`${type.padEnd(16)} ${c.ok} rendered, ${c.err} errors`);
console.log(`${inputs.length} inputs, ${Object.values(counts).reduce((n, c) => n + c.ok + c.err, 0)} written`);
