#!/usr/bin/env node
// tools/route-inventory.mjs — every route the Node broker serves, for the
// move to Rust: method, path, the handler's file:line, whether Rox calls it
// (app/src), the tests that hit it, and the phase that ports it (the Rust
// broker plan). Markdown on stdout:
//
//   node server/tools/route-inventory.mjs > drafts/rust-broker-routes.md
//
// A route is an `if (...)` on req.method with req.url / path compared to a
// literal (exact, query-stripped or prefix), a "/path": key of a POSTS-like
// table, or v1's /v1/host/plugins/<id>/<action>. A method check with no
// path literal is listed under "Unmatched" for a person to read: the
// counts at the top must add up.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const SERVER = join(dirname(fileURLToPath(import.meta.url)), "..");
const ROOT = join(SERVER, "..");
const SRC = join(SERVER, "src");

const files = [
  ...readdirSync(join(SRC, "routes")).filter((f) => f.endsWith(".mjs")).map((f) => join(SRC, "routes", f)),
  join(SRC, "v1", "routes.mjs"),
];

const EXACT = /(?:req\.url|path|url\.pathname)\??(?:\.split\("\?"\)\[0\])?\s*===\s*"([^"]+)"/g;
const PREFIX = /(?:req\.url|path|url\.pathname)\??\.startsWith\("([^"]+)"/g;
const METHOD = /req\.method\s*===\s*"([A-Z]+)"/g;
// a regex route: /^\/routine\/(update|delete)$/.test(req.url)
const REGEX = /\/(\^(?:\\\/|[^/\n])+?)\/\.test\((?:req\.url|path)\)/g;
const readable = (src) => src.replace(/^\^/, "").replace(/\$$/, "").replace(/\\\//g, "/");

const routes = [];
const unmatched = [];
let checks = 0;

for (const file of files) {
  const text = readFileSync(file, "utf8");
  const lines = text.split("\n");
  const rel = relative(ROOT, file);

  // `if (` conditions: from the line that opens one to the line that ends it with `{`
  for (let i = 0; i < lines.length; i++) {
    if (!/\bif\s*\(/.test(lines[i])) continue;
    let cond = lines[i];
    let j = i;

    while (!/\)\s*\{?\s*$/.test(cond.trimEnd()) && !/\)\s*(return|\{)/.test(cond) && j < i + 8) cond += " " + lines[++j];
    const methods = [...cond.matchAll(METHOD)].map((m) => m[1]);
    const exact = [...cond.matchAll(EXACT)].map((m) => m[1]).filter((p) => p.startsWith("/"));
    const prefix = [...cond.matchAll(PREFIX)].map((m) => m[1]).filter((p) => p.startsWith("/"));
    const regex = [...cond.matchAll(REGEX)].map((m) => readable(m[1]));

    if (!methods.length && !exact.length && !prefix.length && !regex.length) continue;

    if (!exact.length && !prefix.length && !regex.length) {
      if (!methods.length) continue;
      checks++;
      // inside a path check that names no method (`if (req.url === "/prefs") { if (req.method === "GET")`)
      const outer = [...routes].reverse().find((r) => r.file === rel && r.method === "ANY" && i + 1 - r.line <= 60);

      if (outer) outer.methods = [...new Set([...(outer.methods || []), ...methods])];
      // else the dispatcher of a POSTS-like table, or a guard
      else unmatched.push({ file: rel, line: i + 1, text: cond.trim().slice(0, 140) });
      continue;
    }

    for (const p of regex) routes.push({ method: methods[0] || "ANY", path: p, match: "regex", file: rel, line: i + 1 });

    if (!methods.length && !/req\.url|path ===|path\.startsWith/.test(cond)) continue;
    checks++;

    for (const p of exact) routes.push({ method: methods[0] || "ANY", path: p, match: "exact", file: rel, line: i + 1 });

    for (const p of prefix) routes.push({ method: methods[0] || "ANY", path: p, match: "prefix", file: rel, line: i + 1 });
  }

  // POSTS-like tables: `"/path": (b) => …` entries inside `const NAME = {`
  const table = /const\s+[A-Z_]+\s*=\s*\{([\s\S]*?)\n\};/g;

  for (const t of text.matchAll(table)) {
    const start = text.slice(0, t.index).split("\n").length;

    t[1].split("\n").forEach((l, k) => {
      const m = /^\s*"(\/[^"]+)":/.exec(l);

      if (m) {
        checks++;
        routes.push({ method: "POST", path: m[1], match: "exact", file: rel, line: start + k + 1 });
      }
    });
  }

  // v1's plugin actions
  const action = /\/\^\\\/v1\\\/host\\\/plugins\\\/\(\[\^\/\]\+\)\\\/\(([a-z|]+)\)\$\//.exec(text);

  if (action) {
    checks++;
    for (const a of action[1].split("|")) routes.push({ method: "POST", path: `/v1/host/plugins/<id>/${a}`, match: "pattern", file: rel, line: text.slice(0, action.index).split("\n").length });
  }
}

// whether Rox calls it, and which tests hit it
const read = (dir, ext) =>
  readdirSync(dir, { recursive: true })
    .filter((f) => String(f).endsWith(ext))
    .map((f) => readFileSync(join(dir, String(f)), "utf8"))
    .join("\n");
const rox = read(join(ROOT, "app", "src"), ".rs");
const testFiles = readdirSync(join(SERVER, "test")).filter((f) => f.endsWith(".test.mjs"));
const tests = testFiles.map((f) => [f.replace(/\.test\.mjs$/, ""), readFileSync(join(SERVER, "test", f), "utf8")]);

const stem = (r) => r.path.replace(/<id>.*$/, "");
// a regex route's alternatives: /mcp/(attach|detach) → /mcp/attach, /mcp/detach
const variants = (path) => {
  const m = /^(.*?)\(([^()]+)\)(.*)$/.exec(path);

  return m ? m[2].split("|").flatMap((a) => variants(m[1] + a + m[3])) : [path];
};
const used = (src, r) => {
  const p = stem(r);

  if (r.match === "regex") return variants(r.path).some((v) => used(src, { ...r, path: v, match: v.endsWith("/") ? "prefix" : "exact" }));

  if (r.match !== "exact") return src.includes(`"${p}`) || src.includes(`\`${p}`);

  return [`"${p}"`, `"${p}?`, `\`${p}\``, `\`${p}?`, `"${p}/`, `'${p}'`].some((s) => src.includes(s));
};

// the phase that ports it (the plan's §4), by path
const PHASES = [
  [/^\/(health)?$/, "1"],
  [/^\/v1\/host\/(plugins|sessions|call|apps|tickets|resolve|events)/, "3"],
  [/^\/v1\/host\/(routines|agents)/, "5"],
  [/^\/v1\/(hooks|bridge)/, "5"],
  [/^\/v1\/host\/?$/, "3"],
  [/^\/mcp(\/|$)/, "4"],
  [/^\/state/, "2"],
  [/^\/(prefs|keymap|models|status|timeclock)$/, "2"],
  [/^\/data\//, "2"],
  [/^\/(groups|group\/)/, "2"],
  [/^\/(file\/raw|file\/versions|file\/thumb|asset\/|generated|thumb)/, "2"],
  [/^\/(highlight|mermaid|files\/)/, "2"],
  [/^\/oauth\/callback/, "4"],
  [/^\/(copy|share|reuse|unpublish|sources)/, "5"],
  [/^\/office/, "6"],
  [/^\/popout/, "6"],
  [/^\/(threads|turns|chat|thread\/|turn\/|retry|react|bot\/|bots|avatar|jobs?|job\/|status\/)/, "5"],
  [/^\/(routines?|routine\/)/, "5"],
  [/^\/(app\/|apps|artifact|artifacts|module|catalog|publish|serve|widgets?|version|heads|library|attach|attachments?|drive|file\/)/, "5"],
  [/^\/(plugins|market|skills?|marketplace)/, "5"],
];
const phaseOf = (p) => PHASES.find(([re]) => re.test(p))?.[1] || "?";

routes.sort((a, b) => a.path.localeCompare(b.path) || a.method.localeCompare(b.method));
const seen = new Set();
const unique = routes.filter((r) => {
  const k = `${r.method} ${r.path} ${r.match}`;

  if (seen.has(k)) return false;
  seen.add(k);

  return true;
});
const byPhase = {};

for (const r of unique) byPhase[phaseOf(r.path)] = (byPhase[phaseOf(r.path)] || 0) + 1;

const out = [];

out.push("# Node broker routes (for the Rust broker plan)");
out.push("");
out.push(`Generated by \`server/tools/route-inventory.mjs\`. Rerun it after a route moves.`);
out.push("");
out.push(`- Route checks found: ${checks} (conditions with a path, table entries, the v1 action pattern, and ${unmatched.length} method checks without a path, listed under Unmatched).`);
out.push(`- Routes: ${unique.length} (method + path + match), from ${routes.length} handler entries.`);
out.push(`- Rox calls: ${unique.filter((r) => used(rox, r)).length}. Tested: ${unique.filter((r) => tests.some(([, s]) => used(s, r))).length}.`);
out.push(`- By phase: ${Object.entries(byPhase).sort().map(([p, n]) => `${p}: ${n}`).join(", ")}.`);
out.push("");
out.push("| Method | Path | Match | Handler | Rox | Tests | Phase |");
out.push("|---|---|---|---|---|---|---|");

// a path check with method branches inside answers any method; its branches are noted
for (const r of routes) if (r.methods) r.method = `ANY (${r.methods.join(", ")} branch)`;

for (const r of unique) {
  const hit = tests.filter(([, s]) => used(s, r)).map(([n]) => n);

  out.push(`| ${r.method} | \`${r.path}\` | ${r.match} | ${r.file}:${r.line} | ${used(rox, r) ? "yes" : ""} | ${hit.join(", ")} | ${phaseOf(r.path)} |`);
}
out.push("");
out.push("## Unmatched method checks");
out.push("");
out.push("Each is a dispatcher of a table above, or a guard; read them when the counts differ.");
out.push("");

for (const u of unmatched) out.push(`- ${u.file}:${u.line} \`${u.text.replace(/`/g, "'")}\``);
process.stdout.write(out.join("\n") + "\n");
