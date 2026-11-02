// names-migration.mjs — protocol 1.0's one-time rewrite of app.db (the
// broker's store) from the rox names to the buwiz ones.
//
// Chats, links, pins, attachments and stored _meta held `rox://…` links,
// the `rox-app` link provider, `rox-app://` entries and `rox/<key>` _meta
// keys; protocol 1.0 routes and reads only the buwiz names, so every text
// column of every table (keys excepted) is rewritten once. The marker in
// `meta` makes it run once; the rewrite is idempotent anyway (no old name
// survives it). v1_plugins keeps its records as installed: a 1.x plugin's
// signed manifest is not ours to change, and it is refused until it ships
// the new names. buwiz-broker's hub runs the same rewrite (hub/store/schema.rs).
export const MARKER = "names_buwiz";

export const RENAMES = Object.freeze([
  ["rox-app://", "buwiz-app://"],
  ["rox://", "buwiz://"],
  ['"rox-app"', '"buwiz-app"'],
  ['"rox/open"', '"buwiz/open"'],
  ['"rox/suggest"', '"buwiz/suggest"'],
  ['"rox/audience"', '"buwiz/audience"'],
  ['"rox/caller"', '"buwiz/caller"'],
]);

const SKIP = new Set(["meta", "v1_plugins"]);
const quote = (s) => `'${s.replaceAll("'", "''")}'`;
const ident = (s) => `"${s.replaceAll('"', '""')}"`;

/** The rewrite of `db`, once: how many rows changed (0 when marked). */
export function migrateNames(db) {
  if (db.prepare("SELECT v FROM meta WHERE k=?").get(MARKER)) return 0;
  let changed = 0;

  db.exec("BEGIN");

  try {
    const tables = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'").all().map((t) => t.name);

    for (const t of tables.filter((x) => !SKIP.has(x))) {
      const cols = db.prepare(`PRAGMA table_info(${ident(t)})`).all().filter((c) => !c.pk && (!c.type || /TEXT|CHAR|CLOB/i.test(c.type)));

      for (const { name } of cols) {
        const set = RENAMES.reduce((sql, [a, b]) => `replace(${sql}, ${quote(a)}, ${quote(b)})`, ident(name));
        const where = RENAMES.map(([a]) => `instr(${ident(name)}, ${quote(a)}) > 0`).join(" OR ");

        changed += Number(db.prepare(`UPDATE ${ident(t)} SET ${ident(name)} = ${set} WHERE typeof(${ident(name)}) = 'text' AND (${where})`).run().changes);
      }
    }

    db.prepare("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)").run(MARKER, String(Date.now()));
    db.exec("COMMIT");
  } catch (e) {
    db.exec("ROLLBACK");
    throw e;
  }

  return changed;
}
