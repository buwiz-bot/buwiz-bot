#!/usr/bin/env node
// tools/table-owners.mjs — who touches each app.db table, for the move to
// Rust (rule R2: a table moves together with every module that writes it or
// holds it in memory). Markdown on stdout, appended to the route inventory:
//
//   node server/tools/table-owners.mjs >> drafts/rust-broker-routes.md
//
// Writers, readers and schema lines come from the SQL in server/src (a
// table name after INSERT INTO, REPLACE INTO, UPDATE, DELETE FROM, FROM or
// JOIN). What holds a table in memory, and the phase that moves it, are a
// person's reading of the code: OWNERS below. A table with no OWNERS entry,
// or an entry for a table that no longer exists, fails the run.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const SERVER = join(dirname(fileURLToPath(import.meta.url)), "..");
const SRC = join(SERVER, "src");

// table → [owner phase, what holds it in memory ("" = nothing), note]
const OWNERS = {
  threads: [5, "store.list (state.mjs), rewritten by persist()", "the brain's tables move in one cutover (5.10)"],
  turns: [5, "store.list[].turns, rewritten by persist()", ""],
  bots: [5, "store.bots, rewritten by persist()", ""],
  meta: [5, "persist() rewrites active, sections, seq", "other keys are row-level: /prefs's rows are Rust's from 2.3 (D7)"],
  kv: [2, "", "row-level: /state writes rows (Rust, 2.2); Node deletes a deleted app's, bot's or chat's row until phase 5 (D7)"],
  timeclock: [2, "", "only /data/timeclock* (Rust, 2.3); the read in routes/apps.mjs is an unused copy of tcJson"],
  widgets: [5, "", "written by publish and delete, which belong to the model loop"],
  apps: [5, "", "mini apps: made by the model loop, versions and heads with them"],
  versions: [5, "", ""],
  heads: [5, "", ""],
  artifact_meta: [5, "", "artifacts.mjs, called from the model loop"],
  attachments: [5, "", "written on send, with the turn"],
  attachment_names: [5, "", ""],
  library_hidden: [5, "", ""],
  tombstones: [5, "", "deletes of threads, bots and apps"],
  groups: [5, "", "/groups lists through originOf(), which reads the in-memory store"],
  group_items: [5, "", "artifacts.mjs adds items from the model loop"],
  skills: [5, "", "GET /skills reads it in Rust (2.3); Node writes it until phase 5"],
  routines: [5, "the scheduler (routines.mjs)", ""],
  agent_endpoints: [5, "", "agent messaging, driven by turns"],
  agent_outbox: [5, "", ""],
  agent_hook_seen: [5, "", ""],
  v1_plugins: [3, "the v1 host's running plugins", "the Mac helper"],
  v1_instances: [3, "", ""],
  v1_routines: [3, "the v1 scheduler", ""],
  installed_plugins: [3, "", "market installs"],
  plugin_settings: [3, "", ""],
  mcp_servers: [4, "the MCP clients (mcp.mjs)", "the Mac helper"],
};

const files = [];
(function walk(dir) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) {
      if (!["fixtures", "templates", "node_modules", "editor"].includes(e.name)) walk(p);
    } else if (e.isFile() && e.name.endsWith(".mjs")) files.push(p);
  }
})(SRC);

const tables = new Map(); // name → {schema, write, read}: Set of file:line
const SCHEMA = /CREATE TABLE(?: IF NOT EXISTS)? ([a-z_0-9]+)|ALTER TABLE ([a-z_0-9]+)/g;
const WRITE = /(?:INSERT(?: OR [A-Z]+)? INTO|REPLACE INTO|UPDATE|DELETE FROM) ([a-z_0-9]+)/g;
const READ = /(?<!DELETE )(?:FROM|JOIN) ([a-z_0-9]+)/g;

const sources = files.map((f) => [relative(join(SERVER, ".."), f), readFileSync(f, "utf8")]);
const lineOf = (text, at) => text.slice(0, at).split("\n").length;
for (const [rel, text] of sources) {
  for (const m of text.matchAll(SCHEMA)) {
    const name = m[1] || m[2];
    if (!tables.has(name)) tables.set(name, { schema: new Set(), write: new Set(), read: new Set() });
    tables.get(name).schema.add(`${rel}:${lineOf(text, m.index)}`);
  }
}
for (const [rel, text] of sources) {
  for (const [re, kind] of [[WRITE, "write"], [READ, "read"]]) {
    for (const m of text.matchAll(re)) {
      if (tables.has(m[1])) tables.get(m[1])[kind].add(`${rel}:${lineOf(text, m.index)}`);
    }
  }
}

const missing = [...tables.keys()].filter((t) => !OWNERS[t]);
const stale = Object.keys(OWNERS).filter((t) => !tables.has(t));
if (missing.length || stale.length) {
  console.error(`table-owners: no owner for ${missing.join(", ") || "-"}; owner for a missing table ${stale.join(", ") || "-"}`);
  process.exit(1);
}

// file:line lists, grouped by file: "routes/apps.mjs:551,563"
const where = (set) => {
  const byFile = new Map();
  for (const at of set) {
    const [file, line] = at.split(":");
    byFile.set(file, [...(byFile.get(file) || []), Number(line)]);
  }
  return [...byFile].sort(([a], [b]) => a.localeCompare(b))
    .map(([f, ls]) => `${f.replace(/^server\/src\//, "")}:${ls.sort((a, b) => a - b).join(",")}`).join("<br>") || "-";
};

const names = [...tables.keys()].sort((a, b) => OWNERS[a][0] - OWNERS[b][0] || a.localeCompare(b));
const byPhase = {};
for (const t of names) byPhase[OWNERS[t][0]] = (byPhase[OWNERS[t][0]] || 0) + 1;
console.log(`\n## Tables\n`);
console.log(`Made by \`node server/tools/table-owners.mjs\`. ${names.length} tables, every one with an owner phase (${Object.entries(byPhase).map(([p, n]) => `${p}: ${n}`).join(", ")}). Paths are under \`server/src/\`; line numbers are the SQL's.\n`);
console.log(`| Table | Phase | In memory | Writes | Reads | Note |`);
console.log(`|---|---|---|---|---|---|`);
for (const t of names) {
  const [phase, cache, note] = OWNERS[t];
  const { write, read } = tables.get(t);
  console.log(`| \`${t}\` | ${phase} | ${cache || "-"} | ${where(write)} | ${where(read)} | ${note || ""} |`);
}
