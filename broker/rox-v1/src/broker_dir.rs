//! The broker's data folder on the owner's Mac:
//! `~/Library/Application Support/Buwiz Broker`, called `Grok Shell` before
//! the rename. [`move_once`] moves the old folder to the new name, once,
//! after a backup of its database; the app runs it before it starts the
//! bundled broker, and the broker when it is given the owner's folder (by
//! either name). Readers that only look ([`dir_in`]) take the new folder,
//! else the old one while it is still there, so a broker still running on
//! `Grok Shell` keeps working until it restarts.
//!
//! The move never runs:
//! - for a data dir that is not the owner's (a test instance, a bench, a
//!   checkout): callers ask [`is_owners`] first;
//! - while a broker answers on the owner's port (the caller's `busy`);
//! - when `Buwiz Broker` is there already: it is used, `Grok Shell` is left
//!   alone.
//!
//! It is a rename (atomic, the files untouched), never a copy-then-delete
//! of a live database: when the rename fails the old folder stays in use
//! and the move is tried again at the next start. A lock file in
//! Application Support keeps the app and the broker from moving it at the
//! same time.
//!
//! The database stores absolute paths into the folder (a plugin's command
//! and env, app icons, attachments in turns' meta, MCP servers' env and
//! auth). Right after the rename, under the lock and before anything opens
//! it, every text value holding the old folder's path is rewritten to the
//! new one, in one transaction ([`rewrite_paths`]); should that fail the
//! folder is renamed back. Then `Grok Shell` is left as a symlink to `Buwiz
//! Broker`, so paths outside the database (plugin folders' own configs,
//! a process still holding the old path) keep resolving. The link can go in
//! a later release, once every install has moved.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The folder's name in Application Support.
pub const DIR_NAME: &str = "Buwiz Broker";
/// Its name before the rename.
pub const OLD_DIR_NAME: &str = "Grok Shell";
/// Written in the new folder once the old one was moved there.
pub const MOVED_MARKER: &str = ".moved-from-grok-shell";
/// Held, in Application Support, while one process moves the folder.
pub const LOCK_FILE: &str = ".buwiz-broker-move.lock";
/// The owner's broker port (the app's default origin).
pub const OWNER_PORT: u16 = 8787;

const SUPPORT: &str = "Library/Application Support";

/// `~/Library/Application Support/Buwiz Broker`.
pub fn new_dir(home: &Path) -> PathBuf {
    home.join(SUPPORT).join(DIR_NAME)
}

/// `~/Library/Application Support/Grok Shell`.
pub fn old_dir(home: &Path) -> PathBuf {
    home.join(SUPPORT).join(OLD_DIR_NAME)
}

/// The folder to read: `Buwiz Broker`, else `Grok Shell` while it is a
/// folder and the new one is missing (not moved yet, or a broker from
/// before the rename still runs on it). Creates nothing.
pub fn dir_in(home: &Path) -> PathBuf {
    let new = new_dir(home);
    let old = old_dir(home);
    if new.symlink_metadata().is_err() && old.is_dir() {
        old
    } else {
        new
    }
}

/// Whether `dir` is the owner's broker folder under `home`, by either name.
pub fn is_owners(home: &Path, dir: &Path) -> bool {
    !home.as_os_str().is_empty() && (dir == new_dir(home) || dir == old_dir(home))
}

/// What [`move_once`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// No `Grok Shell` folder: nothing to move (a fresh install).
    Nothing,
    /// `Buwiz Broker` is there (moved before, or both exist): neither is
    /// touched.
    AlreadyThere,
    /// Renamed; the database's backup is in `backup` (`None`: there was no
    /// database), `rewritten` the values rewritten per `table.column`,
    /// `linked` whether `Grok Shell` was left as a symlink to it.
    Moved { backup: Option<PathBuf>, rewritten: Vec<(String, usize)>, linked: bool },
    /// A broker answers on the owner's port: it may be using the folder.
    Busy,
    /// Another process held the move lock for too long.
    LockHeld,
    /// The backup, the rename or the paths' rewrite failed: `Grok Shell`
    /// stays in use.
    Failed(String),
}

/// [`rewrite_paths`]'s shape: (database, old folder, new folder) →
/// (`table.column`, values rewritten).
pub type Rewrite = dyn Fn(&Path, &Path, &Path) -> Result<Vec<(String, usize)>, String>;

/// The hooks [`move_with`] runs; [`move_once`] passes the real ones.
pub struct Hooks<'a> {
    /// Whether a broker answers on the owner's port.
    pub busy: &'a dyn Fn() -> bool,
    /// A consistent copy of the SQLite database `from` into the new file
    /// `to` (sqlite's online backup or `VACUUM INTO`).
    pub backup_db: &'a dyn Fn(&Path, &Path) -> Result<(), String>,
    /// [`rewrite_paths`]: the database, the old folder, the new one.
    pub rewrite: &'a Rewrite,
    pub rename: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>,
    /// How long to wait for another process's move.
    pub lock_wait: Duration,
}

/// Moves `Grok Shell` to `Buwiz Broker` under `home`, once (see the
/// module). `busy` asks whether a broker answers on the owner's port.
/// Then use [`dir_in`].
pub fn move_once(home: &Path, busy: &dyn Fn() -> bool) -> Move {
    move_with(home, &Hooks { busy, backup_db: &backup_db, rewrite: &rewrite_paths, rename: &|from, to| std::fs::rename(from, to), lock_wait: Duration::from_secs(60) })
}

/// [`move_once`] with its hooks.
pub fn move_with(home: &Path, hooks: &Hooks) -> Move {
    let (new, old) = (new_dir(home), old_dir(home));
    // nothing is created unless there is something to move
    if new.symlink_metadata().is_ok() {
        return Move::AlreadyThere;
    }
    if !old.symlink_metadata().is_ok_and(|m| m.is_dir()) {
        return Move::Nothing;
    }
    let lock_path = home.join(SUPPORT).join(LOCK_FILE);
    let lock = match lock(&lock_path, hooks.lock_wait) {
        Ok(Some(f)) => f,
        Ok(None) => return Move::LockHeld,
        Err(e) => return Move::Failed(format!("the move lock {}: {e}", lock_path.display())),
    };
    // another process may have moved it while this one waited
    let done = if new.symlink_metadata().is_ok() {
        Move::AlreadyThere
    } else if !old.is_dir() {
        Move::Nothing
    } else if (hooks.busy)() {
        Move::Busy
    } else {
        move_locked(&old, &new, hooks)
    };
    // whoever waits on this file finds the move done when it gets the lock
    let _ = std::fs::remove_file(&lock_path);
    drop(lock);
    done
}

/// The backup, the rename and the marker, under the lock.
fn move_locked(old: &Path, new: &Path, hooks: &Hooks) -> Move {
    let stamp = utc_stamp(SystemTime::now());
    let db = old.join("generated").join("app.db");
    let backup = if db.is_file() {
        // a fresh folder: a retry within the same second gets its own
        let base = old.join("backups");
        let dir = (1..).map(|i| base.join(if i == 1 { format!("{stamp}-pre-move") } else { format!("{stamp}-pre-move-{i}") })).find(|d| d.symlink_metadata().is_err()).unwrap_or_default();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return Move::Failed(format!("the backup folder {}: {e}", dir.display()));
        }
        if let Err(e) = (hooks.backup_db)(&db, &dir.join("app.db")) {
            return Move::Failed(format!("the backup of {}: {e}", db.display()));
        }
        Some(dir)
    } else {
        None
    };
    if let Err(e) = (hooks.rename)(old, new) {
        return Move::Failed(format!("renaming {} to {}: {e}", old.display(), new.display()));
    }
    // the stored paths follow, before anything opens the database
    let new_db = new.join("generated").join("app.db");
    let rewritten = if new_db.is_file() {
        match (hooks.rewrite)(&new_db, old, new) {
            Ok(r) => r,
            Err(e) => {
                // nothing opened the folder since: back to where it was
                let back = (hooks.rename)(new, old).map(|()| "renamed back".to_owned()).unwrap_or_else(|b| format!("not renamed back: {b}"));
                return Move::Failed(format!("rewriting the paths in {}: {e} ({back})", new_db.display()));
            }
        }
    } else {
        Vec::new()
    };
    // paths outside the database keep resolving
    let linked = std::os::unix::fs::symlink(DIR_NAME, old).is_ok();
    // the backup moved with the folder
    let backup = backup.and_then(|b| b.strip_prefix(old).ok().map(|rel| new.join(rel)));
    let marker = format!(
        "{{\"from\":{},\"at\":\"{stamp}\",\"backup\":{},\"rewritten\":{},\"linked\":{linked}}}\n",
        json_str(&old.to_string_lossy()),
        backup.as_ref().map(|b| json_str(&b.to_string_lossy())).unwrap_or_else(|| "null".into()),
        rewritten.iter().map(|(_, n)| n).sum::<usize>()
    );
    // the move is done either way; a missing marker loses nothing
    let _ = std::fs::write(new.join(MOVED_MARKER), marker);
    Move::Moved { backup, rewritten, linked }
}

/// A consistent copy of the SQLite database `from` in the new file `to`
/// (`VACUUM INTO`: one read transaction).
pub fn backup_db(from: &Path, to: &Path) -> Result<(), String> {
    let db = rusqlite::Connection::open_with_flags(from, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    db.execute("VACUUM INTO ?1", [to.to_string_lossy()]).map(|_| ()).map_err(|e| e.to_string())
}

/// Rewrites, in the database `db`, every text value that holds the folder
/// `old`'s path to hold `new`'s: `<old>/…` (also JSON-escaped, `\/`),
/// `<old>"` (a JSON string ending at the folder) and a value that is the
/// folder itself. Every text column of every table (virtual tables and
/// their shadow tables excepted), in one transaction: all or nothing.
/// Text that only mentions "Grok Shell" is left as it is; blobs are never
/// touched. → (`table.column`, values rewritten), the columns that had any.
pub fn rewrite_paths(db: &Path, old: &Path, new: &Path) -> Result<Vec<(String, usize)>, String> {
    let (Some(old), Some(new)) = (old.to_str(), new.to_str()) else {
        return Err("a folder path that is not UTF-8".into());
    };
    let esc = |p: &str| p.replace('/', "\\/");
    let pairs = [(format!("{old}/"), format!("{new}/")), (format!("{}\\/", esc(old)), format!("{}\\/", esc(new))), (format!("{old}\""), format!("{new}\""))];
    let e = |e: rusqlite::Error| e.to_string();
    let mut conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(e)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(e)?;
    let tx = conn.transaction().map_err(e)?;
    let tables: Vec<(String, String)> = {
        let mut q = tx.prepare("SELECT name, coalesce(sql, '') FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name").map_err(e)?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)?
    };
    let virtuals: Vec<&str> = tables.iter().filter(|(_, sql)| sql.trim_start().to_ascii_uppercase().starts_with("CREATE VIRTUAL")).map(|(n, _)| n.as_str()).collect();
    let quote = |id: &str| format!("\"{}\"", id.replace('"', "\"\""));
    let mut counts = Vec::new();
    for (table, _) in &tables {
        if virtuals.iter().any(|v| table == v || table.starts_with(&format!("{v}_"))) {
            continue;
        }
        let cols: Vec<String> = {
            let mut q = tx.prepare("SELECT name FROM pragma_table_info(?1)").map_err(e)?;
            let rows = q.query_map([table], |r| r.get(0)).map_err(e)?;
            rows.collect::<Result<_, _>>().map_err(e)?
        };
        for col in cols {
            let c = quote(&col);
            let sql = format!(
                "UPDATE {t} SET {c} = CASE WHEN {c} = ?7 THEN ?8 ELSE replace(replace(replace({c}, ?1, ?2), ?3, ?4), ?5, ?6) END \
                 WHERE typeof({c}) = 'text' AND ({c} = ?7 OR instr({c}, ?1) > 0 OR instr({c}, ?3) > 0 OR instr({c}, ?5) > 0)",
                t = quote(table)
            );
            let n = tx
                .execute(&sql, rusqlite::params![pairs[0].0, pairs[0].1, pairs[1].0, pairs[1].1, pairs[2].0, pairs[2].1, old, new])
                .map_err(|err| format!("{table}.{col}: {err}"))?;
            if n > 0 {
                counts.push((format!("{table}.{col}"), n));
            }
        }
    }
    tx.commit().map_err(e)?;
    Ok(counts)
}

/// Takes the lock at `path`, waiting up to `wait` for another holder.
/// `Ok(None)` when it stayed held.
fn lock(path: &Path, wait: Duration) -> std::io::Result<Option<File>> {
    let deadline = Instant::now() + wait;
    loop {
        let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        match f.try_lock() {
            Ok(()) => return Ok(Some(f)),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// Whether something answers on 127.0.0.1:`port` (a broker using the
/// owner's data).
pub fn port_answers(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// `YYYYMMDDTHHMMSSZ`, as `scripts/broker-live backup` names its folders.
pub fn utc_stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn json_str(s: &str) -> String {
    serde_json::Value::String(s.to_owned()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::cell::Cell;

    /// A temp home, removed when dropped.
    struct Home(PathBuf);
    impl Home {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rox-v1-broker-dir-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join(SUPPORT)).unwrap();
            Home(dir)
        }
        fn support(&self) -> PathBuf {
            self.0.join(SUPPORT)
        }
        /// A `Grok Shell` with a WAL database (one row) and a host token.
        fn grok_shell(&self) {
            let old = old_dir(&self.0);
            std::fs::create_dir_all(old.join("generated")).unwrap();
            let db = Connection::open(old.join("generated/app.db")).unwrap();
            db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES ('db');").unwrap();
            std::fs::write(old.join("host-token"), b"tok").unwrap();
        }
        fn entries(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(self.support()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn x_of(db: &Path) -> String {
        Connection::open(db).unwrap().query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap()
    }

    fn hooks<'a>(busy: &'a dyn Fn() -> bool, rename: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>) -> Hooks<'a> {
        Hooks { busy, backup_db: &backup_db, rewrite: &rewrite_paths, rename, lock_wait: Duration::from_millis(300) }
    }
    fn real_rename(a: &Path, b: &Path) -> std::io::Result<()> {
        std::fs::rename(a, b)
    }

    #[test]
    fn a_fresh_install_has_nothing_to_move_and_gets_nothing_made() {
        let h = Home::new("fresh");
        assert_eq!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::Nothing);
        assert!(h.entries().is_empty(), "no folder, no lock: {:?}", h.entries());
        assert_eq!(dir_in(&h.0), new_dir(&h.0));
    }

    #[test]
    fn grok_shell_alone_is_backed_up_moved_and_linked_with_a_marker() {
        let h = Home::new("move");
        h.grok_shell();
        let Move::Moved { backup: Some(backup), linked: true, .. } = move_with(&h.0, &hooks(&|| false, &real_rename)) else { panic!("not moved and linked") };
        let (new, old) = (new_dir(&h.0), old_dir(&h.0));
        assert_eq!(h.entries(), [DIR_NAME, OLD_DIR_NAME], "the lock is gone");
        // Grok Shell is now a link that resolves to the new folder
        assert!(old.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&old).unwrap(), PathBuf::from(DIR_NAME));
        assert_eq!(std::fs::canonicalize(&old).unwrap(), std::fs::canonicalize(&new).unwrap());
        assert_eq!(std::fs::read(old.join("host-token")).unwrap(), b"tok");
        assert_eq!(x_of(&new.join("generated/app.db")), "db", "the data is as it was");
        assert_eq!(std::fs::read(new.join("host-token")).unwrap(), b"tok");
        assert!(backup.starts_with(new.join("backups")) && backup.file_name().unwrap().to_string_lossy().ends_with("-pre-move"));
        assert_eq!(x_of(&backup.join("app.db")), "db");
        let marker: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(new.join(MOVED_MARKER)).unwrap()).unwrap();
        assert_eq!(marker["from"], old.to_string_lossy().as_ref());
        assert_eq!(marker["backup"], backup.to_string_lossy().as_ref());
        assert_eq!(marker["linked"], true);
        assert_eq!(dir_in(&h.0), new);
        // once: the next start finds it there, the link is fine
        assert_eq!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::AlreadyThere);
        assert_eq!(std::fs::read_dir(new.join("backups")).unwrap().count(), 1, "no second backup");
    }

    #[test]
    fn a_folder_without_a_database_moves_without_a_backup() {
        let h = Home::new("nodb");
        std::fs::create_dir_all(old_dir(&h.0)).unwrap();
        assert_eq!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::Moved { backup: None, rewritten: vec![], linked: true });
        assert!(new_dir(&h.0).join(MOVED_MARKER).is_file());
    }

    #[test]
    fn with_both_the_new_one_is_used_and_the_old_left_alone() {
        let h = Home::new("both");
        h.grok_shell();
        std::fs::create_dir_all(new_dir(&h.0)).unwrap();
        let busy_asked = Cell::new(false);
        let busy = || {
            busy_asked.set(true);
            false
        };
        assert_eq!(move_with(&h.0, &hooks(&busy, &real_rename)), Move::AlreadyThere);
        assert!(!busy_asked.get());
        assert!(old_dir(&h.0).join("generated/app.db").is_file());
        assert!(!old_dir(&h.0).join("backups").exists(), "no backup either");
        assert_eq!(dir_in(&h.0), new_dir(&h.0));
    }

    #[test]
    fn a_failed_rename_keeps_the_old_folder_in_use_and_tries_again() {
        let h = Home::new("fail");
        h.grok_shell();
        let fail = |_: &Path, _: &Path| Err(std::io::Error::other("cross-device"));
        let Move::Failed(why) = move_with(&h.0, &hooks(&|| false, &fail)) else { panic!("not failed") };
        assert!(why.contains("cross-device"), "{why}");
        assert!(!new_dir(&h.0).exists(), "no copy was made");
        assert!(old_dir(&h.0).join("generated/app.db").is_file());
        assert_eq!(dir_in(&h.0), old_dir(&h.0));
        assert!(!h.support().join(LOCK_FILE).exists());
        // the next start moves it
        assert!(matches!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::Moved { .. }));
    }

    #[test]
    fn a_failed_backup_moves_nothing() {
        let h = Home::new("nobackup");
        h.grok_shell();
        let no = |_: &Path, _: &Path| Err("disk full".to_owned());
        let hooks = Hooks { busy: &|| false, backup_db: &no, rewrite: &rewrite_paths, rename: &real_rename, lock_wait: Duration::from_millis(300) };
        assert!(matches!(move_with(&h.0, &hooks), Move::Failed(w) if w.contains("disk full")));
        assert_eq!(dir_in(&h.0), old_dir(&h.0));
    }

    /// The paths' rewrite fails: the folder goes back to its old name, no
    /// link, and the old folder stays in use.
    #[test]
    fn a_failed_rewrite_renames_it_back() {
        let h = Home::new("norewrite");
        h.grok_shell();
        let no = |_: &Path, _: &Path, _: &Path| Err("database is locked".to_owned());
        let hooks = Hooks { busy: &|| false, backup_db: &backup_db, rewrite: &no, rename: &real_rename, lock_wait: Duration::from_millis(300) };
        let Move::Failed(why) = move_with(&h.0, &hooks) else { panic!("not failed") };
        assert!(why.contains("database is locked") && why.contains("renamed back"), "{why}");
        let old = old_dir(&h.0);
        assert!(old.symlink_metadata().unwrap().is_dir() && !new_dir(&h.0).exists());
        assert_eq!(x_of(&old.join("generated/app.db")), "db");
        assert_eq!(dir_in(&h.0), old);
    }

    #[test]
    fn a_running_broker_means_no_move() {
        let h = Home::new("busy");
        h.grok_shell();
        let renamed = Cell::new(false);
        let rename = |a: &Path, b: &Path| {
            renamed.set(true);
            std::fs::rename(a, b)
        };
        assert_eq!(move_with(&h.0, &hooks(&|| true, &rename)), Move::Busy);
        assert!(!renamed.get());
        assert!(!old_dir(&h.0).join("backups").exists(), "not even a backup");
        assert_eq!(dir_in(&h.0), old_dir(&h.0));
    }

    #[test]
    fn a_held_lock_waits_then_gives_up_without_moving() {
        let h = Home::new("lock");
        h.grok_shell();
        let held = lock(&h.support().join(LOCK_FILE), Duration::ZERO).unwrap().unwrap();
        assert_eq!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::LockHeld);
        assert_eq!(dir_in(&h.0), old_dir(&h.0));
        drop(held);
    }

    #[test]
    fn two_at_once_move_it_once() {
        let h = Home::new("race");
        h.grok_shell();
        let home = h.0.clone();
        let run = move || {
            let rename = |a: &Path, b: &Path| {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::rename(a, b)
            };
            move_with(&home, &Hooks { busy: &|| false, backup_db: &backup_db, rewrite: &rewrite_paths, rename: &rename, lock_wait: Duration::from_secs(10) })
        };
        let (a, b) = (std::thread::spawn(run.clone()), std::thread::spawn(run));
        let mut got = [a.join().unwrap(), b.join().unwrap()];
        got.sort_by_key(|m| matches!(m, Move::Moved { .. }));
        assert_eq!(got[0], Move::AlreadyThere);
        assert!(matches!(got[1], Move::Moved { .. }));
        assert_eq!(std::fs::read_dir(new_dir(&h.0).join("backups")).unwrap().count(), 1, "one backup");
    }

    /// The owner's tables, as far as their paths go: a plugin installed
    /// from a release (command and env into `<data>/v1/apps`), app icons,
    /// attachments in a turn's meta (JSON), an MCP server's env and auth,
    /// and a bot's description that only mentions Grok Shell.
    #[test]
    fn stored_paths_into_the_folder_follow_it() {
        let h = Home::new("paths");
        h.grok_shell();
        let (old, new) = (old_dir(&h.0), new_dir(&h.0));
        let (o, n) = (old.to_str().unwrap().to_owned(), new.to_str().unwrap().to_owned());
        let db = Connection::open(old.join("generated/app.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE v1_plugins(id TEXT PRIMARY KEY, command TEXT, env TEXT, plugin_pub TEXT);
             CREATE TABLE apps(id INTEGER PRIMARY KEY, icon TEXT, data BLOB);
             CREATE TABLE turns(id INTEGER PRIMARY KEY, meta TEXT, n INTEGER);
             CREATE TABLE mcp_servers(id TEXT, env TEXT, auth TEXT);
             CREATE TABLE bots(id TEXT, description TEXT);
             CREATE TABLE \"odd \"\"name\"(\"a col\" TEXT);",
        )
        .unwrap();
        let gws = format!("{o}/v1/apps/app.buwiz.google-workspace/1.2.0");
        let env = serde_json::json!({"GWS_MANIFEST": format!("{gws}/manifest.json"), "GWS_USER_EMAIL": "x@y"}).to_string();
        db.execute("INSERT INTO v1_plugins VALUES ('app.buwiz.google-workspace', ?1, ?2, 'k')", [format!("{gws}/bin/gws"), env.clone()]).unwrap();
        db.execute("INSERT INTO apps VALUES (1, ?1, ?2)", rusqlite::params![format!("{o}/generated/avatars/a.png"), format!("{o}/x").into_bytes()]).unwrap();
        let meta = serde_json::json!({"attachments": [{"path": format!("{o}/generated/attachments/t1/f.pdf"), "name": "f.pdf"}], "cwd": o}).to_string();
        db.execute("INSERT INTO turns VALUES (1, ?1, 3)", [meta]).unwrap();
        let escaped = format!("{{\"dir\":\"{}\\/chunks\"}}", o.replace('/', "\\/"));
        db.execute("INSERT INTO mcp_servers VALUES ('m', ?1, ?2)", [escaped, format!("{{\"token_file\":\"{o}/mcp/m.json\"}}")]).unwrap();
        let prose = "A bot from the Grok Shell days; it lived in Application Support/Grok Shell.";
        db.execute("INSERT INTO bots VALUES ('b', ?1)", [prose]).unwrap();
        db.execute("INSERT INTO \"odd \"\"name\" VALUES (?1)", [o.clone()]).unwrap();
        // a sibling folder whose name only starts the same is not ours
        db.execute("INSERT INTO bots VALUES ('c', ?1)", [format!("{o} Backup/x")]).unwrap();
        drop(db);

        let Move::Moved { rewritten, linked: true, backup: Some(backup) } = move_with(&h.0, &hooks(&|| false, &real_rename)) else { panic!("not moved") };
        let counts: std::collections::HashMap<&str, usize> = rewritten.iter().map(|(c, k)| (c.as_str(), *k)).collect();
        assert_eq!(counts.get("v1_plugins.command"), Some(&1));
        assert_eq!(counts.get("v1_plugins.env"), Some(&1));
        assert_eq!(counts.get("apps.icon"), Some(&1));
        assert_eq!(counts.get("turns.meta"), Some(&1));
        assert_eq!(counts.get("mcp_servers.env"), Some(&1));
        assert_eq!(counts.get("mcp_servers.auth"), Some(&1));
        assert_eq!(counts.get("odd \"name.a col"), Some(&1));
        assert!(!counts.contains_key("bots.description") && !counts.contains_key("apps.data"), "{counts:?}");

        let db = Connection::open(new.join("generated/app.db")).unwrap();
        let one = |sql: &str| db.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
        let ngws = format!("{n}/v1/apps/app.buwiz.google-workspace/1.2.0");
        assert_eq!(one("SELECT command FROM v1_plugins"), format!("{ngws}/bin/gws"));
        let env: serde_json::Value = serde_json::from_str(&one("SELECT env FROM v1_plugins")).unwrap();
        assert_eq!(env["GWS_MANIFEST"], format!("{ngws}/manifest.json"));
        assert_eq!(env["GWS_USER_EMAIL"], "x@y");
        assert_eq!(one("SELECT plugin_pub FROM v1_plugins"), "k");
        assert_eq!(one("SELECT icon FROM apps"), format!("{n}/generated/avatars/a.png"));
        let blob: Vec<u8> = db.query_row("SELECT data FROM apps", [], |r| r.get(0)).unwrap();
        assert_eq!(blob, format!("{o}/x").into_bytes(), "blobs are never touched");
        let meta: serde_json::Value = serde_json::from_str(&one("SELECT meta FROM turns")).unwrap();
        assert_eq!(meta["attachments"][0]["path"], format!("{n}/generated/attachments/t1/f.pdf"));
        assert_eq!(meta["cwd"], n.as_str(), "a JSON value that is the folder itself");
        let menv: serde_json::Value = serde_json::from_str(&one("SELECT env FROM mcp_servers")).unwrap();
        assert_eq!(menv["dir"], format!("{n}/chunks"), "the escaped form");
        assert_eq!(one("SELECT auth FROM mcp_servers"), format!("{{\"token_file\":\"{n}/mcp/m.json\"}}"));
        assert_eq!(one("SELECT description FROM bots WHERE id = 'b'"), prose);
        assert_eq!(one("SELECT description FROM bots WHERE id = 'c'"), format!("{o} Backup/x"));
        assert_eq!(one("SELECT \"a col\" FROM \"odd \"\"name\""), n);
        // the backup keeps the paths as they were
        let b = Connection::open(backup.join("app.db")).unwrap();
        assert_eq!(b.query_row("SELECT command FROM v1_plugins", [], |r| r.get::<_, String>(0)).unwrap(), format!("{gws}/bin/gws"));
        drop((db, b));
        // a second start does nothing
        assert_eq!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::AlreadyThere);
        assert_eq!(rewrite_paths(&new.join("generated/app.db"), &old, &new).unwrap(), vec![], "nothing left to rewrite");
    }

    #[test]
    fn readers_fall_back_to_grok_shell_until_it_moves() {
        let h = Home::new("read");
        assert_eq!(dir_in(&h.0), new_dir(&h.0));
        h.grok_shell();
        assert_eq!(dir_in(&h.0), old_dir(&h.0));
        std::fs::create_dir_all(new_dir(&h.0)).unwrap();
        assert_eq!(dir_in(&h.0), new_dir(&h.0));
    }

    #[test]
    fn only_the_owners_folder_is_the_owners() {
        let home = Path::new("/Users/u");
        assert!(is_owners(home, Path::new("/Users/u/Library/Application Support/Grok Shell")));
        assert!(is_owners(home, Path::new("/Users/u/Library/Application Support/Buwiz Broker")));
        assert!(!is_owners(home, Path::new("/tmp/bench/data")));
        assert!(!is_owners(home, Path::new("/Users/u/Library/Application Support/Buwiz Bot")));
        assert!(!is_owners(Path::new(""), Path::new("Library/Application Support/Grok Shell")));
    }

    #[test]
    fn the_stamp_is_utc() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(1_793_620_245)), "20261102T115045Z");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "20000229T000000Z");
    }
}
