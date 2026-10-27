//! The `site_logins` table: metadata only. There is no password column and
//! there must never be one — the secret is in the Keychain under the row id.

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
    exact INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS site_logins_origin_username
    ON site_logins (origin, username);
";

const SELECT: &str =
    "SELECT id, origin, username, label, kind, notes, last_used_at_ms, created_at, updated_at, exact FROM site_logins";

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
    })
}

/// Makes the table if it is missing, and adds `exact` to a table made
/// before it (its rows stay legacy: `exact` 0).
pub(crate) fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(SCHEMA)?;
    let has_exact = c
        .prepare("SELECT 1 FROM pragma_table_info('site_logins') WHERE name = 'exact'")?
        .exists([])?;
    if !has_exact {
        c.execute_batch("ALTER TABLE site_logins ADD COLUMN exact INTEGER NOT NULL DEFAULT 0")?;
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

/// Inserts `r`, or updates the row already saved for its origin and
/// username (which keeps its id, so its Keychain item stays put).
pub(crate) fn upsert(c: &Connection, r: &SiteLogin) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO site_logins (id, origin, username, label, kind, notes, last_used_at_ms, created_at, updated_at, exact)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(origin, username) DO UPDATE SET
            label = excluded.label, kind = excluded.kind, notes = excluded.notes,
            updated_at = excluded.updated_at, exact = excluded.exact",
        params![r.id, r.origin, r.username, r.label, r.kind, r.notes, r.last_used_at_ms, r.created_at, r.updated_at, r.exact],
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
