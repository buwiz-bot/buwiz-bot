//! What a bot may make this broker do on the user's own Mac. `local_exec`
//! is the shell consent gate, transcribed from opengrok-server's
//! `opengrok-policy`; this file is its persistence, one policy per
//! `machineId` in app.db (opengrok-server
//! `crates/opengrok-store/src/postgres/computers.rs`, `local_exec_policy`
//! and `local_exec_rule`, without the account column: one person per
//! broker). The store writes rows, never policy: every rule reaches a row
//! through `local_exec::standing_rule_refusal` in the route.

pub mod local_exec;
pub mod shell;

use rusqlite::{params, Connection, OptionalExtension};

use self::local_exec::{LocalExecMode, LocalExecPolicy};
use super::ctx::Db;

/// `local_exec_policy` holds the mode (absent ⇒ the default), `local_exec_rule`
/// the standing allow and deny patterns, each once per machine and kind.
const SCHEMA: &str = r#"
  CREATE TABLE IF NOT EXISTS local_exec_policy(
    machine_id TEXT PRIMARY KEY, mode TEXT NOT NULL, updated_at_ms INTEGER NOT NULL);
  CREATE TABLE IF NOT EXISTS local_exec_rule(
    machine_id TEXT NOT NULL, kind TEXT NOT NULL, pattern TEXT NOT NULL, added_at_ms INTEGER NOT NULL,
    PRIMARY KEY(machine_id, kind, pattern));
"#;

/// The machine `shell_run` runs on: this broker's own Mac. `ROX_MACHINE_ID`
/// when the app set one (its `cards/transport.rs` sends the same variable),
/// else `local`. A request naming no machine means this one; remote
/// machines (slice S3) carry their own ids.
pub fn local_machine_id() -> String {
    std::env::var("ROX_MACHINE_ID").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "local".to_owned())
}

/// `machine_id` as a request named it, or this machine when it named none.
pub fn machine_or_local(named: Option<&str>) -> String {
    match named.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.to_owned(),
        None => local_machine_id(),
    }
}

fn ensure(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(SCHEMA)
}

/// Creates the tables; called when the hub starts and before every write.
pub fn install(db: &Db) -> Result<(), String> {
    db.with(ensure)
}

/// The policy for `machine`. No row is the default (`bypass`, no rules);
/// a row the gate cannot read, or a database it cannot open, is `never`
/// with no rules — a broken store may only ever narrow access (opengrok
/// `LocalExecMode::from_stored`, `load_policy`).
pub fn load(db: &Db, machine: &str) -> LocalExecPolicy {
    let read = db.with(|c| {
        ensure(c)?;
        let mode: Option<String> = c.query_row("SELECT mode FROM local_exec_policy WHERE machine_id = ?1", params![machine], |r| r.get(0)).optional()?;
        let rules = |kind: &str| -> rusqlite::Result<Vec<String>> {
            let mut st = c.prepare("SELECT pattern FROM local_exec_rule WHERE machine_id = ?1 AND kind = ?2 ORDER BY added_at_ms, pattern")?;
            let rows = st.query_map(params![machine, kind], |r| r.get::<_, String>(0))?;
            rows.collect()
        };
        Ok((mode, rules("allow")?, rules("deny")?))
    });
    match read {
        Ok((mode, allow, deny)) => LocalExecPolicy { mode: mode.as_deref().map_or_else(LocalExecMode::default, LocalExecMode::from_stored), allow, deny, session_allow: Vec::new() },
        Err(_) => LocalExecPolicy { mode: LocalExecMode::Never, ..LocalExecPolicy::default() },
    }
}

pub fn set_mode(db: &Db, machine: &str, mode: LocalExecMode, at_ms: i64) -> Result<(), String> {
    db.with(|c| {
        ensure(c)?;
        c.execute(
            "INSERT INTO local_exec_policy(machine_id, mode, updated_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(machine_id) DO UPDATE SET mode = excluded.mode, updated_at_ms = excluded.updated_at_ms",
            params![machine, mode.as_stored(), at_ms],
        )?;
        Ok(())
    })
}

/// Adds a standing rule; one already there stays as it was.
pub fn add_rule(db: &Db, machine: &str, kind: &str, pattern: &str, at_ms: i64) -> Result<(), String> {
    db.with(|c| {
        ensure(c)?;
        c.execute(
            "INSERT INTO local_exec_rule(machine_id, kind, pattern, added_at_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(machine_id, kind, pattern) DO NOTHING",
            params![machine, kind, pattern, at_ms],
        )?;
        Ok(())
    })
}

pub fn remove_rule(db: &Db, machine: &str, kind: &str, pattern: &str) -> Result<(), String> {
    db.with(|c| {
        ensure(c)?;
        c.execute("DELETE FROM local_exec_rule WHERE machine_id = ?1 AND kind = ?2 AND pattern = ?3", params![machine, kind, pattern])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::ctx::HubCtx;
    use crate::hub::traits::Clock;

    fn db(label: &str) -> Db {
        let dir = std::env::temp_dir().join(format!("rox-policy-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
        let ctx = HubCtx::new(dir, Clock::manual());
        ctx.store.load().unwrap();
        ctx.db
    }

    #[test]
    fn policy_store_round_trips_per_machine_and_defaults_to_bypass() {
        let db = db("rt");
        assert_eq!(load(&db, "m1").mode, LocalExecMode::Bypass);
        set_mode(&db, "m1", LocalExecMode::Ask, 1).unwrap();
        add_rule(&db, "m1", "allow", "git status", 2).unwrap();
        add_rule(&db, "m1", "allow", "git status", 3).unwrap();
        add_rule(&db, "m1", "deny", "rm", 4).unwrap();
        add_rule(&db, "m2", "deny", "curl", 5).unwrap();
        let p = load(&db, "m1");
        assert_eq!((p.mode, p.allow.clone(), p.deny.clone()), (LocalExecMode::Ask, vec!["git status".to_owned()], vec!["rm".to_owned()]));
        assert_eq!(load(&db, "m2").mode, LocalExecMode::Bypass);
        assert_eq!(load(&db, "m2").deny, ["curl"]);
        remove_rule(&db, "m1", "deny", "rm").unwrap();
        assert!(load(&db, "m1").deny.is_empty());
        set_mode(&db, "m1", LocalExecMode::Never, 6).unwrap();
        assert_eq!(load(&db, "m1").mode, LocalExecMode::Never);
    }

    #[test]
    fn policy_store_reads_a_bad_mode_word_or_a_broken_db_as_never() {
        let db = db("bad");
        db.with(|c| {
            ensure(c)?;
            c.execute("INSERT INTO local_exec_policy VALUES ('m1', 'open', 1)", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(load(&db, "m1").mode, LocalExecMode::Never);
        let broken = HubCtx::new(std::path::PathBuf::from("/nonexistent/rox-policy-dir"), Clock::manual()).db;
        assert_eq!(load(&broken, "m1").mode, LocalExecMode::Never);
    }

    #[test]
    fn policy_machine_defaults_to_this_one() {
        assert_eq!(machine_or_local(Some(" m9 ")), "m9");
        assert_eq!(machine_or_local(Some("")), local_machine_id());
        assert_eq!(machine_or_local(None), local_machine_id());
        assert!(!local_machine_id().is_empty());
    }
}
