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
    /// database).
    Moved { backup: Option<PathBuf> },
    /// A broker answers on the owner's port: it may be using the folder.
    Busy,
    /// Another process held the move lock for too long.
    LockHeld,
    /// The backup or the rename failed: `Grok Shell` stays in use.
    Failed(String),
}

/// The hooks [`move_with`] runs; [`move_once`] passes the real ones.
pub struct Hooks<'a> {
    /// Whether a broker answers on the owner's port.
    pub busy: &'a dyn Fn() -> bool,
    /// A consistent copy of the SQLite database `from` into the new file
    /// `to` (sqlite's online backup or `VACUUM INTO`).
    pub backup_db: &'a dyn Fn(&Path, &Path) -> Result<(), String>,
    pub rename: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>,
    /// How long to wait for another process's move.
    pub lock_wait: Duration,
}

/// Moves `Grok Shell` to `Buwiz Broker` under `home`, once (see the
/// module). `busy` asks whether a broker answers on the owner's port;
/// `backup_db` copies the database. Then use [`dir_in`].
pub fn move_once(home: &Path, busy: &dyn Fn() -> bool, backup_db: &dyn Fn(&Path, &Path) -> Result<(), String>) -> Move {
    move_with(home, &Hooks { busy, backup_db, rename: &|from, to| std::fs::rename(from, to), lock_wait: Duration::from_secs(60) })
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
        let dir = old.join("backups").join(format!("{stamp}-pre-move"));
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
    // the backup moved with the folder
    let backup = backup.and_then(|b| b.strip_prefix(old).ok().map(|rel| new.join(rel)));
    let marker = format!(
        "{{\"from\":{},\"at\":\"{stamp}\",\"backup\":{}}}\n",
        json_str(&old.to_string_lossy()),
        backup.as_ref().map(|b| json_str(&b.to_string_lossy())).unwrap_or_else(|| "null".into())
    );
    // the move is done either way; a missing marker loses nothing
    let _ = std::fs::write(new.join(MOVED_MARKER), marker);
    Move::Moved { backup }
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
        /// A `Grok Shell` with a database and a host token.
        fn grok_shell(&self) {
            let old = old_dir(&self.0);
            std::fs::create_dir_all(old.join("generated")).unwrap();
            std::fs::write(old.join("generated/app.db"), b"db").unwrap();
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

    fn copy_db(from: &Path, to: &Path) -> Result<(), String> {
        std::fs::copy(from, to).map(|_| ()).map_err(|e| e.to_string())
    }

    fn hooks<'a>(busy: &'a dyn Fn() -> bool, rename: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>) -> Hooks<'a> {
        Hooks { busy, backup_db: &copy_db, rename, lock_wait: Duration::from_millis(300) }
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
    fn grok_shell_alone_is_backed_up_then_moved_with_a_marker() {
        let h = Home::new("move");
        h.grok_shell();
        let Move::Moved { backup: Some(backup) } = move_with(&h.0, &hooks(&|| false, &real_rename)) else { panic!("not moved") };
        let new = new_dir(&h.0);
        assert_eq!(h.entries(), [DIR_NAME], "the old folder and the lock are gone");
        assert_eq!(std::fs::read(new.join("generated/app.db")).unwrap(), b"db", "the data is as it was");
        assert_eq!(std::fs::read(new.join("host-token")).unwrap(), b"tok");
        assert!(backup.starts_with(new.join("backups")) && backup.file_name().unwrap().to_string_lossy().ends_with("-pre-move"));
        assert_eq!(std::fs::read(backup.join("app.db")).unwrap(), b"db");
        let marker: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(new.join(MOVED_MARKER)).unwrap()).unwrap();
        assert_eq!(marker["from"], old_dir(&h.0).to_string_lossy().as_ref());
        assert_eq!(marker["backup"], backup.to_string_lossy().as_ref());
        assert_eq!(dir_in(&h.0), new);
        // once: the next start finds it there
        assert_eq!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::AlreadyThere);
    }

    #[test]
    fn a_folder_without_a_database_moves_without_a_backup() {
        let h = Home::new("nodb");
        std::fs::create_dir_all(old_dir(&h.0)).unwrap();
        assert_eq!(move_with(&h.0, &hooks(&|| false, &real_rename)), Move::Moved { backup: None });
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
        let hooks = Hooks { busy: &|| false, backup_db: &no, rename: &real_rename, lock_wait: Duration::from_millis(300) };
        assert!(matches!(move_with(&h.0, &hooks), Move::Failed(w) if w.contains("disk full")));
        assert_eq!(dir_in(&h.0), old_dir(&h.0));
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
            move_with(&home, &Hooks { busy: &|| false, backup_db: &copy_db, rename: &rename, lock_wait: Duration::from_secs(10) })
        };
        let (a, b) = (std::thread::spawn(run.clone()), std::thread::spawn(run));
        let mut got = [a.join().unwrap(), b.join().unwrap()];
        got.sort_by_key(|m| matches!(m, Move::Moved { .. }));
        assert_eq!(got[0], Move::AlreadyThere);
        assert!(matches!(got[1], Move::Moved { .. }));
        assert_eq!(std::fs::read_dir(new_dir(&h.0).join("backups")).unwrap().count(), 1, "one backup");
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
