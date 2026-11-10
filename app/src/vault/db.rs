//! The `site_logins` table: metadata only. There is no password column and
//! there must never be one — the secret is in the Keychain under the row id.
//!
//! Sharing is metadata too, kept with the row: `shared_with` is the JSON list
//! of the Bot ids the login is offered to. The one global switch, "share
//! all", is the `vault_settings` row `share_all` (`1` on, anything else off).
//! A table made before sharing gets the column with every row shared with no
//! Bot, and share-all off.

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Row, params};

/// A saved login as the lists show it. No secret.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SiteLogin {
    pub id: String,
    pub origin: String,
    pub username: String,
    pub label: String,
    /// `password`, `code` or `passkey`.
    pub kind: String,
    pub notes: String,
    /// When it was last used to fill a form (unix ms).
    pub last_used_at_ms: Option<i64>,
    /// Unix ms.
    pub created_at: i64,
    pub updated_at: i64,
    /// `origin` is an exact site (`origin::site_origin`). False on rows
    /// saved before that rule, whose origin is a registrable domain.
    pub exact: bool,
    /// The Bots (ids) this login is offered to, besides share-all.
    pub shared_with: BTreeSet<String>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS site_logins (
    id TEXT PRIMARY KEY NOT NULL,
    origin TEXT NOT NULL,
    username TEXT NOT NULL,
    label TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'password',
    notes TEXT NOT NULL DEFAULT '',
    last_used_at_ms INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    exact INTEGER NOT NULL DEFAULT 0,
    shared_with TEXT NOT NULL DEFAULT '[]'
);
CREATE UNIQUE INDEX IF NOT EXISTS site_logins_origin_username
    ON site_logins (origin, username);
CREATE TABLE IF NOT EXISTS vault_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
";

const SELECT: &str =
    "SELECT id, origin, username, label, kind, notes, last_used_at_ms, created_at, updated_at, exact, shared_with FROM site_logins";

/// `shared_with` as stored: a JSON list of Bot ids. Anything else reads as
/// shared with no Bot.
fn shares_from(raw: &str) -> BTreeSet<String> {
    serde_json::from_str::<Vec<String>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(|b| b.trim().to_owned())
        .filter(|b| !b.is_empty())
        .collect()
}

fn shares_to(set: &BTreeSet<String>) -> String {
    serde_json::to_string(set).unwrap_or_else(|_| "[]".to_owned())
}

fn row(r: &Row<'_>) -> rusqlite::Result<SiteLogin> {
    Ok(SiteLogin {
        id: r.get(0)?,
        origin: r.get(1)?,
        username: r.get(2)?,
        label: r.get(3)?,
        kind: r.get(4)?,
        notes: r.get(5)?,
        last_used_at_ms: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        exact: r.get(9)?,
        shared_with: shares_from(&r.get::<_, String>(10)?),
    })
}

fn has_column(c: &Connection, name: &str) -> rusqlite::Result<bool> {
    c.prepare("SELECT 1 FROM pragma_table_info('site_logins') WHERE name = ?1")?.exists([name])
}

/// Makes the tables if they are missing, and adds to a table made before
/// them: `exact` (its rows stay legacy: `exact` 0) and `shared_with` (its
/// rows are shared with no Bot). `vault_settings` starts empty: share-all off.
pub(crate) fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(SCHEMA)?;
    if !has_column(c, "exact")? {
        c.execute_batch("ALTER TABLE site_logins ADD COLUMN exact INTEGER NOT NULL DEFAULT 0")?;
    }
    if !has_column(c, "shared_with")? {
        c.execute_batch("ALTER TABLE site_logins ADD COLUMN shared_with TEXT NOT NULL DEFAULT '[]'")?;
    }
    Ok(())
}

pub(crate) fn list(c: &Connection) -> rusqlite::Result<Vec<SiteLogin>> {
    let mut st = c.prepare(&format!("{SELECT} ORDER BY origin, username"))?;
    let rows = st.query_map([], row)?;
    rows.collect()
}

pub(crate) fn find(c: &Connection, origin: &str, username: &str) -> rusqlite::Result<Option<SiteLogin>> {
    c.query_row(&format!("{SELECT} WHERE origin = ?1 AND username = ?2"), params![origin, username], row)
        .optional()
}

pub(crate) fn by_id(c: &Connection, id: &str) -> rusqlite::Result<Option<SiteLogin>> {
    c.query_row(&format!("{SELECT} WHERE id = ?1"), params![id], row).optional()
}

/// Inserts `r`, or updates the row already saved for its origin and
/// username (which keeps its id, so its Keychain item stays put).
pub(crate) fn upsert(c: &Connection, r: &SiteLogin) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO site_logins (id, origin, username, label, kind, notes, last_used_at_ms, created_at, updated_at, exact, shared_with)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(origin, username) DO UPDATE SET
            label = excluded.label, kind = excluded.kind, notes = excluded.notes,
            updated_at = excluded.updated_at, exact = excluded.exact, shared_with = excluded.shared_with",
        params![r.id, r.origin, r.username, r.label, r.kind, r.notes, r.last_used_at_ms, r.created_at, r.updated_at, r.exact, shares_to(&r.shared_with)],
    )?;
    Ok(())
}

/// Writes row `r` over the row with its id (an edit: its site and name may
/// change). The pair is unique: a clash with another row is an error, and
/// nothing changes.
pub(crate) fn update(c: &Connection, r: &SiteLogin) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE site_logins SET origin = ?1, username = ?2, label = ?3, kind = ?4, exact = ?5, updated_at = ?6, shared_with = ?7 WHERE id = ?8",
        params![r.origin, r.username, r.label, r.kind, r.exact, r.updated_at, shares_to(&r.shared_with), r.id],
    )?;
    Ok(())
}

/// Row `id`'s Bots, as given.
pub(crate) fn set_shares(c: &Connection, id: &str, bots: &BTreeSet<String>) -> rusqlite::Result<()> {
    c.execute("UPDATE site_logins SET shared_with = ?1 WHERE id = ?2", params![shares_to(bots), id])?;
    Ok(())
}

/// `bot` off every row's list (the Bot was deleted).
pub(crate) fn forget_bot(c: &Connection, bot: &str) -> rusqlite::Result<()> {
    for mut r in list(c)? {
        if r.shared_with.remove(bot) {
            set_shares(c, &r.id, &r.shared_with)?;
        }
    }
    Ok(())
}

/// The global share-all switch: off unless set on.
pub(crate) fn share_all(c: &Connection) -> rusqlite::Result<bool> {
    let v: Option<String> = c.query_row("SELECT value FROM vault_settings WHERE key = 'share_all'", [], |r| r.get(0)).optional()?;
    Ok(v.as_deref() == Some("1"))
}

pub(crate) fn set_share_all(c: &Connection, on: bool) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO vault_settings (key, value) VALUES ('share_all', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![if on { "1" } else { "0" }],
    )?;
    Ok(())
}

pub(crate) fn delete(c: &Connection, id: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM site_logins WHERE id = ?1", params![id])?;
    Ok(())
}

/// Binds a legacy row to the exact site `origin`.
pub(crate) fn pin_site(c: &Connection, id: &str, origin: &str) -> rusqlite::Result<()> {
    c.execute("UPDATE site_logins SET origin = ?1, exact = 1 WHERE id = ?2 AND exact = 0", params![origin, id])?;
    Ok(())
}

pub(crate) fn touch_used(c: &Connection, id: &str, at_ms: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE site_logins SET last_used_at_ms = ?1 WHERE id = ?2", params![at_ms, id])?;
    Ok(())
}
