//! app.db's tables as Node creates them (state.mjs :124-232, then
//! groups.mjs), in the same order: CREATE TABLE IF NOT EXISTS, then each
//! ALTER TABLE tried and its "duplicate column" error ignored. Migrations
//! only add (R6). Other tracks create their own tables (routines, v1,
//! agents) from their own files.

use rusqlite::Connection;

const CREATE: &str = "
  CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
  CREATE TABLE IF NOT EXISTS threads(
    id TEXT PRIMARY KEY, title TEXT NOT NULL, ord INTEGER NOT NULL DEFAULT 0);
  CREATE TABLE IF NOT EXISTS turns(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id TEXT NOT NULL, seq INTEGER NOT NULL, role TEXT NOT NULL,
    text TEXT, name TEXT, label TEXT, caption TEXT, reaction TEXT,
    retries INTEGER DEFAULT 0, widget INTEGER DEFAULT 0);
  CREATE TABLE IF NOT EXISTS widgets(
    name TEXT PRIMARY KEY, title TEXT, prompt TEXT, published_at TEXT);
  CREATE TABLE IF NOT EXISTS bots(
    id TEXT PRIMARY KEY, name TEXT NOT NULL, shape TEXT NOT NULL,
    color TEXT NOT NULL, thread_id TEXT, ord INTEGER NOT NULL DEFAULT 0);
  CREATE TABLE IF NOT EXISTS kv(name TEXT PRIMARY KEY, data TEXT);
  CREATE TABLE IF NOT EXISTS apps(name TEXT PRIMARY KEY, title TEXT, file TEXT);
  -- Artifacts the user deleted: turns keep naming them (the card shows the
  -- \"module missing\" tombstone) but Library must never list them again.
  CREATE TABLE IF NOT EXISTS tombstones(name TEXT PRIMARY KEY);
  -- One artifact, a version tree: generated/<name>/v<k>.tsx is immutable;
  -- heads is the \"current\" pointer (revert moves it, never rewrites).
  CREATE TABLE IF NOT EXISTS versions(
    name TEXT NOT NULL, v INTEGER NOT NULL, parent INTEGER,
    prompt TEXT, created_at INTEGER, PRIMARY KEY(name, v));
  CREATE TABLE IF NOT EXISTS heads(name TEXT PRIMARY KEY, v INTEGER NOT NULL);
  -- User-connected MCP servers: bot NULL = every bot sees the tools,
  -- bot=<id> scopes them to that bot's thread.
  CREATE TABLE IF NOT EXISTS mcp_servers(
    id TEXT PRIMARY KEY, name TEXT, command TEXT, args TEXT, env TEXT,
    enabled INTEGER DEFAULT 1, bot TEXT, created_at INTEGER, url TEXT);
  CREATE TABLE IF NOT EXISTS timeclock(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    clock_in TEXT NOT NULL, clock_out TEXT);
";

/// Each statement on its own, its error ignored (Node's `try {…} catch {}`).
const TRIED: &[&str] = &[
    "ALTER TABLE turns ADD COLUMN meta TEXT",
    "ALTER TABLE versions ADD COLUMN label TEXT",
    "ALTER TABLE apps ADD COLUMN description TEXT",
    "ALTER TABLE apps ADD COLUMN icon TEXT",
    "ALTER TABLE apps ADD COLUMN icon_prompt TEXT",
    "ALTER TABLE apps ADD COLUMN w REAL",
    "ALTER TABLE apps ADD COLUMN h REAL",
    "ALTER TABLE apps ADD COLUMN updated_at INTEGER",
    "CREATE TABLE IF NOT EXISTS artifact_meta(
    id TEXT PRIMARY KEY, title TEXT, file TEXT, description TEXT, icon TEXT, updated_at INTEGER);
  CREATE TABLE IF NOT EXISTS library_hidden(id TEXT PRIMARY KEY, at INTEGER);",
    "CREATE TABLE IF NOT EXISTS attachments(
    id TEXT PRIMARY KEY, server TEXT NOT NULL, token TEXT NOT NULL, scope TEXT,
    title TEXT, open TEXT, key TEXT, details TEXT, thread TEXT NOT NULL, bot TEXT,
    turn TEXT, created_at INTEGER);
  CREATE TABLE IF NOT EXISTS attachment_names(token TEXT PRIMARY KEY, title TEXT, updated_at INTEGER);",
    "ALTER TABLE mcp_servers ADD COLUMN url TEXT",
    "ALTER TABLE mcp_servers ADD COLUMN disabled_tools TEXT",
    "ALTER TABLE mcp_servers ADD COLUMN auth TEXT",
    "ALTER TABLE mcp_servers ADD COLUMN account TEXT",
    "ALTER TABLE mcp_servers ADD COLUMN headers TEXT",
    "ALTER TABLE mcp_servers ADD COLUMN oauth_client TEXT",
    "ALTER TABLE mcp_servers ADD COLUMN transport TEXT",
    "ALTER TABLE mcp_servers ADD COLUMN agent_pending INTEGER",
    "CREATE TABLE IF NOT EXISTS plugin_settings(plugin TEXT PRIMARY KEY, vals TEXT, updated_at INTEGER)",
    "CREATE TABLE IF NOT EXISTS skills(id TEXT PRIMARY KEY, name TEXT, description TEXT, body TEXT, enabled INTEGER DEFAULT 1, created_at INTEGER)",
    "CREATE TABLE IF NOT EXISTS installed_plugins(name TEXT PRIMARY KEY, meta TEXT, created_at INTEGER)",
    "ALTER TABLE turns ADD COLUMN v INTEGER",
    "ALTER TABLE threads ADD COLUMN agents TEXT",
    "ALTER TABLE bots ADD COLUMN extra TEXT",
    "ALTER TABLE threads ADD COLUMN uid TEXT",
    "ALTER TABLE turns ADD COLUMN uid TEXT",
    "ALTER TABLE bots ADD COLUMN pinned INTEGER DEFAULT 0",
    "ALTER TABLE bots ADD COLUMN section TEXT",
    "ALTER TABLE bots ADD COLUMN hidden INTEGER DEFAULT 0",
    "ALTER TABLE bots ADD COLUMN image TEXT",
    "ALTER TABLE bots ADD COLUMN crop TEXT",
    "ALTER TABLE bots ADD COLUMN model TEXT",
    "ALTER TABLE bots ADD COLUMN effort TEXT",
    "ALTER TABLE bots ADD COLUMN fast INTEGER DEFAULT 0",
    "ALTER TABLE bots ADD COLUMN label TEXT",
    "ALTER TABLE bots ADD COLUMN description TEXT",
    "ALTER TABLE bots ADD COLUMN instructions TEXT",
    "ALTER TABLE bots ADD COLUMN memories TEXT",
    "ALTER TABLE bots ADD COLUMN integrations TEXT",
    "ALTER TABLE threads ADD COLUMN hidden INTEGER DEFAULT 0",
    "ALTER TABLE threads ADD COLUMN archived INTEGER DEFAULT 0",
    "ALTER TABLE threads ADD COLUMN routine TEXT",
    // groups.mjs, imported after state.mjs
    "CREATE TABLE IF NOT EXISTS groups(
    id TEXT PRIMARY KEY, name TEXT NOT NULL, color TEXT NOT NULL,
    ord INTEGER NOT NULL DEFAULT 0, collapsed INTEGER DEFAULT 0, created_at INTEGER);
  CREATE TABLE IF NOT EXISTS group_items(
    group_id TEXT NOT NULL, kind TEXT NOT NULL, ref TEXT NOT NULL, thread TEXT,
    ord INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(group_id, kind, ref));",
    "ALTER TABLE group_items ADD COLUMN title TEXT",
];

/// Creates what is missing; never drops or changes what is there, but
/// for the buwiz names' one-time rename ([`migrate_names`]).
pub fn ensure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(CREATE).map_err(|e| e.to_string())?;
    for sql in TRIED {
        let _ = conn.execute_batch(sql);
    }
    migrate_names(conn).map(|_| ())
}

/// `meta` key marking [`migrate_names`] done (names-migration.mjs's MARKER).
pub const NAMES_MARKER: &str = "names_buwiz";

/// The rox names a stored chat, link, pin or _meta may hold, and their
/// buwiz ones (names-migration.mjs's RENAMES).
pub const RENAMES: [(&str, &str); 7] = [
    ("rox-app://", "buwiz-app://"),
    ("rox://", "buwiz://"),
    ("\"rox-app\"", "\"buwiz-app\""),
    ("\"rox/open\"", "\"buwiz/open\""),
    ("\"rox/suggest\"", "\"buwiz/suggest\""),
    ("\"rox/audience\"", "\"buwiz/audience\""),
    ("\"rox/caller\"", "\"buwiz/caller\""),
];

/// Protocol 1.0 routes and reads only the buwiz names: every text column
/// of every table (keys, `meta` and the v1 install records excepted) is
/// rewritten once, as Node's names-migration.mjs does. Rows changed; 0
/// when the marker is there. Idempotent: no old name survives it.
pub fn migrate_names(conn: &Connection) -> Result<usize, String> {
    let err = |e: rusqlite::Error| e.to_string();
    let done: Option<String> = conn.query_row("SELECT v FROM meta WHERE k=?", [NAMES_MARKER], |r| r.get(0)).ok();
    if done.is_some() {
        return Ok(0);
    }
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let ident = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let tx = conn.unchecked_transaction().map_err(err)?;
    let tables: Vec<String> = {
        let mut q = tx.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'").map_err(err)?;
        let rows = q.query_map([], |r| r.get(0)).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    let mut changed = 0;
    for t in tables.iter().filter(|t| !["meta", "v1_plugins"].contains(&t.as_str())) {
        let cols: Vec<String> = {
            let mut q = tx.prepare(&format!("PRAGMA table_info({})", ident(t))).map_err(err)?;
            let rows = q.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(5)?))).map_err(err)?;
            let all: Vec<(String, String, i64)> = rows.collect::<Result<_, _>>().map_err(err)?;
            let text = |ty: &str| ty.is_empty() || ["TEXT", "CHAR", "CLOB"].iter().any(|k| ty.to_ascii_uppercase().contains(k));
            all.into_iter().filter(|(_, ty, pk)| *pk == 0 && text(ty)).map(|(n, _, _)| n).collect()
        };
        for c in cols {
            let set = RENAMES.iter().fold(ident(&c), |sql, (a, b)| format!("replace({sql}, {}, {})", quote(a), quote(b)));
            let any = RENAMES.iter().map(|(a, _)| format!("instr({}, {}) > 0", ident(&c), quote(a))).collect::<Vec<_>>().join(" OR ");
            let sql = format!("UPDATE {} SET {} = {set} WHERE typeof({}) = 'text' AND ({any})", ident(t), ident(&c), ident(&c));
            changed += tx.execute(&sql, []).map_err(err)?;
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()).to_string();
    tx.execute("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)", [NAMES_MARKER, now.as_str()]).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store written before the buwiz names: rox links in a chat, a card
    /// link, stored _meta, an attachment's open, a /state/rox-* key.
    fn old_store() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            r#"CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
            CREATE TABLE turns(thread TEXT, i INTEGER, content TEXT, links TEXT, meta TEXT, PRIMARY KEY(thread, i));
            CREATE TABLE kv(k TEXT PRIMARY KEY, v TEXT);
            CREATE TABLE v1_plugins(id TEXT PRIMARY KEY, manifest TEXT);
            INSERT INTO turns VALUES('t1', 0, 'see [it](rox://app/app.buwiz.ebirforms/forms) and rox://chat/t2?m=1',
              '[{"provider":"rox-app","link":"rox://app/x/y"}]', '{"rox/caller":{"origin":"agent"},"rox/open":{"app":"forms"}}');
            INSERT INTO kv VALUES('/state/rox-theme', 'rox://view?path=/tmp/a.pdf');
            INSERT INTO v1_plugins VALUES('p', '{"entry":"rox-app://forms"}');"#,
        )
        .unwrap();
        c
    }

    #[test]
    fn names_migration_rewrites_the_rox_names_once_and_leaves_keys_and_install_records() {
        let c = old_store();
        assert_eq!(migrate_names(&c).unwrap(), 4, "content, links, meta; the kv value");
        let (content, links, meta): (String, String, String) = c.query_row("SELECT content, links, meta FROM turns", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!(content, "see [it](buwiz://app/app.buwiz.ebirforms/forms) and buwiz://chat/t2?m=1");
        assert_eq!(links, r#"[{"provider":"buwiz-app","link":"buwiz://app/x/y"}]"#);
        assert_eq!(meta, r#"{"buwiz/caller":{"origin":"agent"},"buwiz/open":{"app":"forms"}}"#);
        let kv: (String, String) = c.query_row("SELECT k, v FROM kv", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(kv, ("/state/rox-theme".to_owned(), "buwiz://view?path=/tmp/a.pdf".to_owned()));
        let m: String = c.query_row("SELECT manifest FROM v1_plugins", [], |r| r.get(0)).unwrap();
        assert_eq!(m, r#"{"entry":"rox-app://forms"}"#, "a 1.x install record is left as installed");
        // marked: never again; and idempotent without the marker
        c.execute("UPDATE turns SET content='rox://typed later'", []).unwrap();
        assert_eq!(migrate_names(&c).unwrap(), 0);
        c.execute("DELETE FROM meta WHERE k=?", [NAMES_MARKER]).unwrap();
        c.execute("UPDATE turns SET content='done'", []).unwrap();
        assert_eq!(migrate_names(&c).unwrap(), 0);
    }
}
