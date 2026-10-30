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

/// Creates what is missing; never drops or changes what is there.
pub fn ensure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(CREATE).map_err(|e| e.to_string())?;
    for sql in TRIED {
        let _ = conn.execute_batch(sql);
    }
    Ok(())
}
