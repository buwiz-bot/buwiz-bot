#!/usr/bin/env node
// tools/db-diff.mjs — do two app.db files hold the same rows? (The Rust
// broker plan, phase 5 §6.) Compares every table row by row, as a multiset,
// after leaving out what differs between any two runs of the same writes:
//   - turns.id (a rowid every persist() renews) and sqlite_sequence;
//   - uids: every UUID (threads.uid, turns.uid, memory, attachment ids…);
//   - timestamps: columns named *_at, at, last_run, last_check, clock_in,
//     clock_out, and any 13-digit millisecond time inside a value;
//   - names built from Date.now(): a reply's files folder (`m<base36>/`),
//     bot ids (`b…`) and group ids (`g…`), renamed in table order;
//   - each file's own data dir (the db's ../..), read as {data}.
//
//   node server/tools/db-diff.mjs <a.db> <b.db>
//
// Prints each difference (table, side, row); exits 0 when equal, 1 when
// not, 2 on bad arguments.
import { existsSync, realpathSync } from "node:fs";
import { dirname } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

const TIME_COL = /^(at|last_run|last_check|clock_in|clock_out)$|_at$/;
const DROP = { turns: new Set(["id"]) };
const SKIP_TABLES = new Set(["sqlite_sequence"]);
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;
const REL = /\bm[0-9a-z]{6,10}(?=\/)/g;
const MS = /\b1[0-9]{12}\b/g;

function load(file) {
  const db = new DatabaseSync(file, { readOnly: true });
  const tables = db
    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
    .all()
    .map((r) => r.name)
    .filter((n) => !SKIP_TABLES.has(n));
  const rows = {};

  for (const t of tables) rows[t] = db.prepare(`SELECT * FROM "${t}"`).all();

  // ids built from Date.now(), named by their order instead
  const ids = new Map();
  const order = (table, prefix) => {
    if (!rows[table]) return;

    [...rows[table]].sort((x, y) => (x.ord ?? 0) - (y.ord ?? 0) || String(x.id).localeCompare(String(y.id)))
      .forEach((r, k) => ids.set(String(r.id), `${prefix}#${k}`));
  };

  order("bots", "bot");
  order("groups", "group");
  db.close();
  const data = dirname(dirname(file));
  const dirs = [...new Set([data, realpathSync(data)])].sort((x, y) => y.length - x.length);

  return { tables, rows, ids, dirs };
}

function normText(s, side) {
  for (const d of side.dirs) s = s.split(d).join("{data}");
  s = s.replace(UUID, "{uuid}").replace(REL, "{rel}").replace(MS, "{ms}");

  if (side.ids.size) s = s.replace(/\b[bg][0-9a-z]{8,12}\b/g, (m) => side.ids.get(m) || m);

  return s;
}

function normRow(table, row, side) {
  const out = {};

  for (const [k, v] of Object.entries(row)) {
    if (DROP[table]?.has(k) || k === "uid" || TIME_COL.test(k)) continue;
    out[k] = typeof v === "string" ? normText(v, side) : typeof v === "number" && v >= 1e12 && v < 2e12 ? "{ms}" : v;
  }

  return JSON.stringify(out);
}

/** The differences between two app.db files: [{table, side, row}]. */
export function dbDiff(fileA, fileB) {
  const a = load(fileA);
  const b = load(fileB);
  const out = [];

  for (const t of [...new Set([...a.tables, ...b.tables])].sort()) {
    if (!a.rows[t] || !b.rows[t]) {
      out.push({ table: t, side: a.rows[t] ? "a" : "b", row: "(table only here)" });
      continue;
    }

    const count = new Map();

    for (const r of a.rows[t]) {
      const k = normRow(t, r, a);
      count.set(k, (count.get(k) || 0) + 1);
    }

    for (const r of b.rows[t]) {
      const k = normRow(t, r, b);
      count.set(k, (count.get(k) || 0) - 1);
    }

    for (const [row, n] of count) for (let i = 0; i < Math.abs(n); i++) out.push({ table: t, side: n > 0 ? "a" : "b", row });
  }

  return out;
}

if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [fa, fb] = process.argv.slice(2);

  if (!fa || !fb || !existsSync(fa) || !existsSync(fb)) {
    console.error("usage: db-diff.mjs <a.db> <b.db>");
    process.exit(2);
  }

  const diffs = dbDiff(fa, fb);

  for (const d of diffs) console.log(`${d.table}: only in ${d.side.toUpperCase()}: ${d.row}`);
  console.log(diffs.length ? `\n${diffs.length} rows differ` : "same rows");
  process.exit(diffs.length ? 1 : 0);
}
