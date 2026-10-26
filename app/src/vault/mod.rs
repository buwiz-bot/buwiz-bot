//! Saved site logins, ported from nativechat `src/site_login`.
//!
//! - Metadata (id, origin, username, label, kind, notes, timestamps) is a
//!   sqlite `site_logins` table in Rox's own data dir ([`data_dir`],
//!   `logins.db`). Rox kept nothing on disk before but its logs
//!   (`~/Library/Logs/Rox`), so the dir is the matching
//!   `~/Library/Application Support/Rox`.
//! - The secret is a Keychain generic password, service
//!   [`KEYCHAIN_SERVICE`], account = row id ([`keychain`]). Never a column,
//!   never a file, never a log line.
//! - Every read of a secret asks the device owner first ([`Vault::unlock`],
//!   [`touch_id`]); there is no other way to get one out.
//!
//! A login is offered on a user-form card for that card's own site
//! (`cards/user_form.rs`), saved from the offer card after a typed
//! submit (`cards/save_login.rs`), and listed on Settings › Logins
//! ([`settings_page`]).

mod db;
mod keychain;
mod origin;
mod page;
pub(crate) mod totp;
mod touch_id;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;

pub(crate) use db::SiteLogin;
use keychain::SecretStore;
pub(crate) use origin::{origins_match, registrable_origin};
pub(crate) use page::settings_page;
use touch_id::{AuthOutcome, OwnerGate};

/// Keychain service; the account is the `site_logins` row id.
pub(crate) const KEYCHAIN_SERVICE: &str = "dev.goldcoders.rox.site-login";

/// The three kinds of row.
pub(crate) const KIND_PASSWORD: &str = "password";
pub(crate) const KIND_CODE: &str = "code";
pub(crate) const KIND_PASSKEY: &str = "passkey";

/// A password, a code or a code seed on its way between the Keychain (or
/// the person's typing) and a REST body. No `Display`, and a `Debug` that
/// says nothing, so it cannot fall into a log or a format string by
/// accident; [`Secret::expose`] is the one way to the text.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// A typed login waiting on the save offer's Save or Not now. Memory only:
/// it is dropped on either answer, and with the process.
#[derive(Clone)]
pub(crate) struct PendingSave {
    pub origin: String,
    pub username: String,
    pub password: Secret,
}

impl std::fmt::Debug for PendingSave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingSave")
            .field("origin", &self.origin)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// What asking for a secret came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unlock {
    Secret(Secret),
    /// The owner passed, but this Mac holds no secret for the row.
    Missing,
    Cancelled,
    /// The sheet could not run, or the OS refused (its sentence).
    Refused(String),
}

pub(crate) struct Vault {
    db: Mutex<Option<Connection>>,
    /// Why the table could not be opened, when it could not.
    open_error: Option<String>,
    secrets: Box<dyn SecretStore>,
    gate: Box<dyn OwnerGate>,
    pending: Mutex<HashMap<String, PendingSave>>,
}

/// `ROX_DATA_DIR`, else `~/Library/Application Support/Rox`.
pub(crate) fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ROX_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/Rox")
}

/// The app's vault: the table in [`data_dir`], the Keychain, the real
/// sheet. Anchored like the store, so a hot patch reaches the same one.
pub(crate) fn vault() -> &'static Vault {
    let at = ra_jit_live::anchor("rox.vault", || Box::leak(Box::new(Vault::open())) as *const Vault as usize);
    // SAFETY: the anchor holds the address of a leaked `Vault` for the life
    // of the process.
    unsafe { &*(at as *const Vault) }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A new row id: `sl_` + the time + 64 random bits.
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i64(now_ms());
    format!("sl_{:x}{:016x}", now_ms(), h.finish())
}

/// The title a row gets when the person gave none.
pub(crate) fn default_label(username: &str, origin: &str) -> String {
    format!("{username} on {origin}")
}

/// The Keychain account of a row's code seed, beside its password.
fn code_account(id: &str) -> String {
    format!("{id}:otp")
}

impl Vault {
    fn open() -> Self {
        let dir = data_dir();
        let conn = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| Connection::open(dir.join("logins.db")).map_err(|e| e.to_string()))
            .and_then(|c| db::migrate(&c).map(|()| c).map_err(|e| e.to_string()));
        let (db, open_error) = match conn {
            Ok(c) => (Some(c), None),
            Err(e) => {
                log::warn!("rox: the logins table could not be opened: {e}");
                (None, Some(e))
            }
        };
        Self::with(db, open_error, keychain::open(), Box::new(touch_id::DeviceOwner))
    }

    fn with(
        db: Option<Connection>,
        open_error: Option<String>,
        secrets: Box<dyn SecretStore>,
        gate: Box<dyn OwnerGate>,
    ) -> Self {
        Self { db: Mutex::new(db), open_error, secrets, gate, pending: Mutex::new(HashMap::new()) }
    }

    fn db<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        let guard = self.db.lock().map_err(|e| e.to_string())?;
        let Some(c) = guard.as_ref() else {
            return Err(self.open_error.clone().unwrap_or_else(|| "the logins table is not open".into()));
        };
        f(c).map_err(|e| e.to_string())
    }

    /// Every row, by site then username.
    pub(crate) fn list(&self) -> Vec<SiteLogin> {
        self.db(db::list).unwrap_or_default()
    }

    /// The rows of `kind` for the site `origin` names (any host on it).
    pub(crate) fn rows_for(&self, origin: &str, kind: &str) -> Vec<SiteLogin> {
        self.list().into_iter().filter(|r| r.kind == kind && origins_match(&r.origin, origin)).collect()
    }

    /// Saves a login: the secret to the Keychain (none for an empty one),
    /// the metadata to the table. A second save of the same site and
    /// username updates that row, under its id.
    pub(crate) fn save(&self, origin: &str, username: &str, kind: &str, secret: &Secret) -> Result<SiteLogin, String> {
        let origin = registrable_origin(origin).unwrap_or_else(|| origin.trim().to_owned());
        let username = username.trim();
        if origin.is_empty() || username.is_empty() {
            return Err("a login needs a site and a username".into());
        }
        let existing = self.db(|c| db::find(c, &origin, username))?;
        let now = now_ms();
        let row = SiteLogin {
            id: existing.as_ref().map(|r| r.id.clone()).unwrap_or_else(new_id),
            label: existing
                .as_ref()
                .map(|r| r.label.clone())
                .filter(|l| !l.trim().is_empty())
                .unwrap_or_else(|| default_label(username, &origin)),
            notes: existing.as_ref().map(|r| r.notes.clone()).unwrap_or_default(),
            kind: kind.to_owned(),
            last_used_at_ms: existing.as_ref().and_then(|r| r.last_used_at_ms),
            created_at: existing.as_ref().map(|r| r.created_at).unwrap_or(now),
            updated_at: now,
            origin,
            username: username.to_owned(),
        };
        if !secret.is_empty() {
            let account = if kind == KIND_CODE { code_account(&row.id) } else { row.id.clone() };
            self.secrets.set(&account, secret)?;
        }
        self.db(|c| db::upsert(c, &row))?;
        Ok(row)
    }

    /// Drops the row and its Keychain items.
    pub(crate) fn delete(&self, id: &str) -> Result<(), String> {
        self.db(|c| db::delete(c, id))?;
        self.secrets.delete(id)?;
        self.secrets.delete(&code_account(id))
    }

    /// Whether this Mac holds the row's secret, asked without reading it.
    pub(crate) fn has_secret(&self, row: &SiteLogin) -> bool {
        if row.kind == KIND_CODE {
            self.secrets.contains(&code_account(&row.id))
        } else {
            self.secrets.contains(&row.id)
        }
    }

    /// Asks the device owner (the sheet reads "Rox is trying to
    /// `reason`"), then reads the row's secret — its password, or for a
    /// code row its `otpauth://` seed. The only way a secret leaves the
    /// Keychain. Blocks: call it off the UI thread.
    pub(crate) fn unlock(&self, row: &SiteLogin, reason: &str) -> Unlock {
        match self.gate.confirm(reason) {
            AuthOutcome::Verified => {}
            AuthOutcome::Cancelled => return Unlock::Cancelled,
            AuthOutcome::Unavailable(why) | AuthOutcome::Failed(why) => return Unlock::Refused(why),
        }
        let account = if row.kind == KIND_CODE { code_account(&row.id) } else { row.id.clone() };
        match self.secrets.get(&account) {
            Ok(Some(s)) => Unlock::Secret(s),
            Ok(None) => Unlock::Missing,
            Err(e) => Unlock::Refused(e),
        }
    }

    /// Asks the owner for a use that reads nothing (the name page of a
    /// two-step sign-in fills only the username).
    pub(crate) fn confirm(&self, reason: &str) -> Result<(), Unlock> {
        match self.gate.confirm(reason) {
            AuthOutcome::Verified => Ok(()),
            AuthOutcome::Cancelled => Err(Unlock::Cancelled),
            AuthOutcome::Unavailable(why) | AuthOutcome::Failed(why) => Err(Unlock::Refused(why)),
        }
    }

    /// Stamps the row as used now.
    pub(crate) fn mark_used(&self, id: &str) {
        let _ = self.db(|c| db::touch_used(c, id, now_ms()));
    }

    /// Holds a typed login for the save offer of form `entry_id`.
    pub(crate) fn stash_pending(&self, entry_id: &str, save: PendingSave) {
        if let Ok(mut m) = self.pending.lock() {
            m.insert(entry_id.to_owned(), save);
        }
    }

    pub(crate) fn has_pending(&self, entry_id: &str) -> bool {
        self.pending.lock().map(|m| m.contains_key(entry_id)).unwrap_or(false)
    }

    /// The site and username of the held login, without its password.
    pub(crate) fn pending_who(&self, entry_id: &str) -> Option<(String, String)> {
        let m = self.pending.lock().ok()?;
        m.get(entry_id).map(|p| (p.origin.clone(), p.username.clone()))
    }

    /// The held login, gone from memory here (Save writes it; Not now drops it).
    pub(crate) fn take_pending(&self, entry_id: &str) -> Option<PendingSave> {
        self.pending.lock().ok()?.remove(entry_id)
    }
}

/// Runs `f` on a thread of its own and waits for it in a task: the
/// Keychain and the Touch ID sheet block, and the UI thread must not.
pub(crate) async fn off_main<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    use nsh_core::prelude::timer;
    use std::sync::mpsc::{TryRecvError, channel};
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    loop {
        match rx.try_recv() {
            Ok(v) => return Some(v),
            Err(TryRecvError::Disconnected) => return None,
            Err(TryRecvError::Empty) => timer(std::time::Duration::from_millis(40)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::keychain::MemorySecrets;
    use super::*;

    struct Answer(AuthOutcome);

    impl OwnerGate for Answer {
        fn confirm(&self, _reason: &str) -> AuthOutcome {
            self.0.clone()
        }
    }

    fn memory_vault(gate: AuthOutcome) -> Vault {
        let c = Connection::open_in_memory().expect("sqlite");
        db::migrate(&c).expect("schema");
        Vault::with(Some(c), None, Box::new(MemorySecrets::default()), Box::new(Answer(gate)))
    }

    #[test]
    fn a_save_keeps_the_password_out_of_the_table_and_unlock_needs_the_owner() {
        let v = memory_vault(AuthOutcome::Verified);
        let row = v.save("https://accounts.google.com/signin", "ada@example.com", KIND_PASSWORD, &Secret::new("s3cret-pass")).expect("save");
        assert_eq!(row.origin, "google.com");
        assert_eq!(row.label, "ada@example.com on google.com");
        assert!(!format!("{row:?}").contains("s3cret-pass"));
        assert!(v.has_secret(&row));
        assert_eq!(v.unlock(&row, "test"), Unlock::Secret(Secret::new("s3cret-pass")));
        assert_eq!(v.rows_for("mail.google.com", KIND_PASSWORD).len(), 1);
        assert!(v.rows_for("google.com", KIND_CODE).is_empty());
        // the second save of the same login keeps its id
        let again = v.save("google.com", "ada@example.com", KIND_PASSWORD, &Secret::new("n3w")).expect("save");
        assert_eq!(again.id, row.id);
        assert_eq!(v.list().len(), 1);
        v.delete(&row.id).expect("delete");
        assert!(v.list().is_empty());
        assert!(!v.has_secret(&row));
    }

    #[test]
    fn a_refused_or_cancelled_owner_reads_nothing() {
        let v = memory_vault(AuthOutcome::Cancelled);
        let row = v.save("example.com", "ada", KIND_PASSWORD, &Secret::new("pw")).expect("save");
        assert_eq!(v.unlock(&row, "test"), Unlock::Cancelled);
        let v = memory_vault(AuthOutcome::Failed("no".into()));
        let row = v.save("example.com", "ada", KIND_PASSWORD, &Secret::new("pw")).expect("save");
        assert_eq!(v.unlock(&row, "test"), Unlock::Refused("no".into()));
    }

    #[test]
    fn a_code_row_keeps_its_seed_beside_the_password_slot() {
        let v = memory_vault(AuthOutcome::Verified);
        let row = v.save("github.com", "ada", KIND_CODE, &Secret::new("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP")).expect("save");
        assert!(v.has_secret(&row));
        let Unlock::Secret(seed) = v.unlock(&row, "test") else { panic!("seed") };
        assert_eq!(totp::Totp::parse(seed.expose()).expect("seed").at(0).len(), 6);
    }

    #[test]
    fn pending_saves_and_secrets_never_debug_print() {
        let v = memory_vault(AuthOutcome::Verified);
        let p = PendingSave { origin: "example.com".into(), username: "ada".into(), password: Secret::new("hunter2") };
        assert!(!format!("{p:?}").contains("hunter2"));
        assert!(!format!("{:?}", Unlock::Secret(Secret::new("hunter2"))).contains("hunter2"));
        v.stash_pending("e_1", p);
        assert!(v.has_pending("e_1"));
        assert_eq!(v.take_pending("e_1").map(|p| p.password), Some(Secret::new("hunter2")));
        assert!(!v.has_pending("e_1"));
    }
}
