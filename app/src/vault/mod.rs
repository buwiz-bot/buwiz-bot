//! Saved site logins, ported from nativechat `src/site_login`.
//!
//! - Metadata (id, origin, username, label, kind, notes, timestamps) is a
//!   sqlite `site_logins` table in Rox's own data dir ([`data_dir`],
//!   `logins.db`). Rox kept nothing on disk before but its logs
//!   (`~/Library/Logs/Buwiz Bot`), so the dir is the matching
//!   `~/Library/Application Support/Buwiz Bot` (Rox's `…/Rox` before the
//!   rename, moved over by [`crate::identity::migrate_data`]).
//! - The secret is a Keychain generic password, service
//!   [`KEYCHAIN_SERVICE`], account = row id ([`keychain`]). Never a column,
//!   never a file, never a log line.
//! - Every read of a secret asks the device owner first ([`Vault::unlock`],
//!   [`touch_id`]); there is no other way to get one out.
//!
//! A login is offered on a user-form card for that card's own site: the
//! exact host, and the port on loopback and IP hosts ([`origin`],
//! `cards/user_form.rs`), saved from the offer card after a typed
//! submit (`cards/save_login.rs`), and listed on Settings › Logins
//! ([`settings_page`]).
//!
//! # Sharing
//!
//! The vault is one store on this Mac, for every Bot. Each login carries the
//! Bots it is shared with (`shared_with`, metadata beside the row, never in
//! the Keychain), and one global switch, share-all, offers every login to
//! every Bot. A new login is shared with no Bot, with one exception: a login
//! saved from a Bot's own card ("Save this login?" after a fill on that Bot's
//! computer) is shared with that Bot, which the person just watched use it.
//! Deleting a Bot takes it off every login ([`Vault::forget_bot`]).
//!
//! Who enforces what: this app is the only authority on "shared with this
//! Bot", since the vault and its shares live here and the broker never sees
//! them. A card offers, lists and uses only a login shared with its Bot (or
//! with share-all on) whose site matches the card's ([`Vault::offered`],
//! [`Vault::usable`]), checked again by id before Touch ID and again before
//! anything is sent, so a login named by id alone gets nowhere. The broker is
//! the authority on "this Bot's own computer": it refuses a saved login on a
//! computer several Bots share (`shared-computer`), whatever the app says,
//! and the app asks it first (`POST /v1/host/computer/login/saved`).

pub(crate) mod command;
mod db;
pub(crate) mod keychain;
mod origin;
mod page;
pub(crate) mod totp;
mod touch_id;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;

pub(crate) use db::SiteLogin;
use keychain::SecretStore;
pub(crate) use origin::{SiteMatch, site_match, site_origin};
pub(crate) use page::{SHARE_ALL_MEANS, SITE_CHANGE_UNSHARES, open_add, open_page, settings_page};
#[cfg(test)]
pub(crate) use touch_id::owner_asks;
use touch_id::{AuthOutcome, OwnerGate};

/// Keychain service; the account is the `site_logins` row id.
pub(crate) const KEYCHAIN_SERVICE: &str = "com.buwiz.bot.site-login";
/// Rox's: its items are moved over once (`move_rox_secrets`).
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
pub(crate) const OLD_KEYCHAIN_SERVICE: &str = "dev.goldcoders.rox.site-login";
/// Written in the data dir once Rox's secrets were moved.
#[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
const KEYCHAIN_MARKER: &str = ".keychain-moved-from-rox";

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
    /// The Bot whose card typed it: a Save shares the login with it.
    pub bot: String,
}

impl std::fmt::Debug for PendingSave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingSave")
            .field("origin", &self.origin)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("bot", &self.bot)
            .finish()
    }
}

/// Why a saved login can't be used on a card. Checked before Touch ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// No such row (deleted, or an id no row has).
    Gone,
    /// The card names no Bot, so nothing is shared with it.
    NoBot,
    /// Not shared with the card's Bot, and share-all is off.
    NotShared,
    /// Saved for another site than the card's.
    OtherSite,
    /// For the card's site, but not what it asks for: no authenticator seed
    /// for a code card (`code`), or no password for a password card.
    WrongKind { code: bool },
    /// The table can't be read: nothing is used.
    Unreadable(String),
}

impl Refusal {
    /// The card's sentence; `bot` is the card's Bot's name.
    pub(crate) fn sentence(&self, bot: &str) -> String {
        let bot = if bot.is_empty() { "this Bot" } else { bot };
        match self {
            Self::Gone => "That saved login isn't on this Mac any more, so nothing was used.".to_owned(),
            Self::NoBot => "This card doesn't say which Bot it's for, so no saved login is used on it.".to_owned(),
            Self::NotShared => format!("That saved login isn't shared with {bot}, so it wasn't used. Share it in Settings › Logins."),
            Self::OtherSite => "That saved login is for another site than this card's, so it wasn't used.".to_owned(),
            Self::WrongKind { code: true } => "That saved login has no authenticator code saved, so it wasn't used. Add one in Settings › Logins, or type the code.".to_owned(),
            Self::WrongKind { code: false } => "That saved login has no password saved, so it wasn't used. Type it by hand.".to_owned(),
            Self::Unreadable(why) => format!("Saved logins can't be read right now ({why}), so none was used."),
        }
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

/// `BUWIZ_DATA_DIR`, else `~/Library/Application Support/Buwiz Bot`
/// ([`crate::identity::data_dir`]).
pub(crate) fn data_dir() -> PathBuf {
    crate::identity::data_dir()
}

/// The app's vault: the table in [`data_dir`], the Keychain, the real
/// sheet. Anchored like the store, so a hot patch reaches the same one.
pub(crate) fn vault() -> &'static Vault {
    let at = ra_jit_live::anchor("rox.vault", || Box::leak(Box::new(Vault::open())) as *const Vault as usize);
    // SAFETY: the anchor holds the address of a leaked `Vault` for the life
    // of the process.
    unsafe { &*(at as *const Vault) }
}

/// Counts the vault's changes, on the UI thread, so what lists logins (an
/// open card, Settings › Logins) draws again when a login or a share
/// changes. Anchored like the vault.
pub(crate) fn revision() -> nsh_core::prelude::Signal<u64> {
    use nsh_core::prelude::{Owner, Signal, signal};
    let at = ra_jit_live::anchor("rox.vault.revision", || {
        Box::leak(Box::new(Owner::detached().run(|| signal(0u64)))) as *const Signal<u64> as usize
    });
    // SAFETY: the anchor holds a leaked signal for the life of the process.
    unsafe { *(at as *const Signal<u64>) }
}

/// Says the vault changed ([`revision`]). Call it on the UI thread.
pub(crate) fn changed() {
    revision().update(|n| *n += 1);
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

/// Moves the secrets of the rows in `c` from Rox's Keychain service to
/// ours, once: [`KEYCHAIN_MARKER`] in `dir` says it is done; a failure
/// writes none, so the next launch tries again. macOS may ask once for
/// each item, which Rox's binary saved.
#[cfg(all(target_os = "macos", not(test)))]
fn move_rox_secrets(dir: &std::path::Path, c: &Connection) {
    let marker = dir.join(KEYCHAIN_MARKER);
    if marker.exists() {
        return;
    }
    let accounts: Vec<String> = db::list(c).unwrap_or_default().iter().flat_map(|r| [r.id.clone(), code_account(&r.id)]).collect();
    match keychain::move_secrets(&keychain::Keychain(OLD_KEYCHAIN_SERVICE), &keychain::Keychain(KEYCHAIN_SERVICE), &accounts) {
        Ok(n) => {
            log::info!("rox: {n} saved login secret(s) moved to {KEYCHAIN_SERVICE}");
            let _ = std::fs::write(&marker, format!("{{\"moved\":{n}}}\n"));
        }
        Err(e) => log::warn!("rox: saved login secrets not moved from {OLD_KEYCHAIN_SERVICE} yet: {e}"),
    }
}

/// The owner's gate: the real sheet with the Keychain; with the test store, none that a test
/// instance's run can put up (a sheet takes the screen). A test build's answers what
/// `BUWIZ_TEST_OWNER` says (`verified`, `cancelled`; else unavailable), so its UI tests drive a
/// saved login with no sheet.
fn gate() -> Box<dyn OwnerGate> {
    match keychain::kind() {
        keychain::Kind::Keychain => Box::new(touch_id::DeviceOwner),
        _ => Box::new(touch_id::TestOwner),
    }
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
        // never from a test or a test instance: it would reach the owner's Keychain
        #[cfg(all(target_os = "macos", not(test)))]
        if let (Ok(c), true) = (&conn, keychain::kind() == keychain::Kind::Keychain) {
            move_rox_secrets(&dir, c);
        }
        let (db, open_error) = match conn {
            Ok(c) => (Some(c), None),
            Err(e) => {
                log::warn!("rox: the logins table could not be opened: {e}");
                (None, Some(e))
            }
        };
        Self::with(db, open_error, keychain::open(), gate())
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

    /// The row `id`, read now.
    pub(crate) fn get(&self, id: &str) -> Option<SiteLogin> {
        self.db(|c| db::by_id(c, id)).ok().flatten()
    }

    /// Whether share-all is on. Off when it can't be read.
    pub(crate) fn share_all(&self) -> bool {
        self.db(db::share_all).unwrap_or(false)
    }

    pub(crate) fn set_share_all(&self, on: bool) -> Result<(), String> {
        self.db(|c| db::set_share_all(c, on))
    }

    /// Shares row `id` with `bot` (`on`), or stops sharing it.
    pub(crate) fn share(&self, id: &str, bot: &str, on: bool) -> Result<SiteLogin, String> {
        let bot = bot.trim();
        if bot.is_empty() {
            return Err("a share needs a Bot".into());
        }
        self.db(|c| db::change_share(c, id, bot, on))?;
        self.get(id).ok_or_else(|| "that login isn't saved any more".to_owned())
    }

    /// Takes `bot` off every login (the Bot was deleted).
    pub(crate) fn forget_bot(&self, bot: &str) -> Result<(), String> {
        self.db(|c| db::forget_bot(c, bot))
    }

    /// Whether this Mac holds an authenticator seed for the row (a code row's
    /// own, or one kept beside a password), asked without reading it.
    pub(crate) fn has_code(&self, row: &SiteLogin) -> bool {
        row.kind != KIND_PASSKEY && self.secrets.contains(&code_account(&row.id))
    }

    /// Whether `row` fits a card that wants a code (`code`) or a password.
    fn fits(&self, row: &SiteLogin, code: bool) -> bool {
        if code { self.has_code(row) } else { row.kind == KIND_PASSWORD }
    }

    /// The rows a card for `bot` offers on `site`: shared with that Bot (or
    /// share-all on), saved for that site ([`site_match`]), and of the kind
    /// the card wants. The only list a card shows.
    pub(crate) fn offered(&self, site: &str, bot: &str, code: bool) -> Vec<SiteLogin> {
        if bot.trim().is_empty() {
            return Vec::new();
        }
        let all = self.share_all();
        self.list()
            .into_iter()
            .filter(|r| (all || r.shared_with.contains(bot)) && self.fits(r, code) && site_match(&r.origin, r.exact, site) != SiteMatch::No)
            .collect()
    }

    /// Row `id`, read now, if a card for `bot` on one of `sites` may use it:
    /// it exists, is shared with that Bot (or share-all is on), is for one
    /// of those sites, and is of the kind the card wants. Asked before Touch
    /// ID and again before anything is sent, so an id alone gets nowhere.
    pub(crate) fn usable(&self, id: &str, bot: &str, sites: &[String], code: bool) -> Result<SiteLogin, Refusal> {
        if bot.trim().is_empty() {
            return Err(Refusal::NoBot);
        }
        let row = self.db(|c| db::by_id(c, id)).map_err(Refusal::Unreadable)?.ok_or(Refusal::Gone)?;
        let all = self.db(db::share_all).map_err(Refusal::Unreadable)?;
        if !all && !row.shared_with.contains(bot) {
            return Err(Refusal::NotShared);
        }
        if !sites.iter().any(|s| site_match(&row.origin, row.exact, s) != SiteMatch::No) {
            return Err(Refusal::OtherSite);
        }
        if !self.fits(&row, code) {
            return Err(Refusal::WrongKind { code });
        }
        Ok(row)
    }

    /// The rows of `kind` for the site `origin` names: saved for that exact
    /// site, or legacy rows of its domain ([`SiteMatch::Legacy`]), whoever
    /// they're shared with. The site rule alone, for its tests: a card lists
    /// [`Vault::offered`].
    #[cfg(test)]
    pub(crate) fn rows_for(&self, origin: &str, kind: &str) -> Vec<SiteLogin> {
        self.list()
            .into_iter()
            .filter(|r| r.kind == kind && site_match(&r.origin, r.exact, origin) != SiteMatch::No)
            .collect()
    }

    /// Saves a login: the secret to the Keychain (none for an empty one),
    /// the metadata to the table. A second save of the same site and
    /// username updates that row, under its id.
    pub(crate) fn save(&self, origin: &str, username: &str, kind: &str, secret: &Secret) -> Result<SiteLogin, String> {
        self.save_for(origin, username, kind, secret, None)
    }

    /// [`Vault::save`], and the row shared with `bot` too when one is named
    /// (a login saved from that Bot's card). The row's other shares stay.
    pub(crate) fn save_for(&self, origin: &str, username: &str, kind: &str, secret: &Secret, bot: Option<&str>) -> Result<SiteLogin, String> {
        let origin = site_origin(origin).unwrap_or_else(|| origin.trim().to_owned());
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
            exact: true,
            // not written by the save: the shares are read fresh below
            shared_with: BTreeSet::new(),
        };
        if !secret.is_empty() {
            let account = if kind == KIND_CODE { code_account(&row.id) } else { row.id.clone() };
            self.secrets.set(&account, secret)?;
        }
        self.db(|c| db::upsert(c, &row))?;
        // the card's Bot is added to the shares as they stand now, after the Keychain write
        if let Some(bot) = bot.map(str::trim).filter(|b| !b.is_empty()) {
            self.db(|c| db::change_share(c, &row.id, bot, true))?;
        }
        self.get(&row.id).ok_or_else(|| "the login wasn't saved".to_owned())
    }

    /// A login added by hand (Settings › Logins, `/vault add`): its password,
    /// and an authenticator seed beside it when one is given. A site and name
    /// already saved is refused: that's an edit. A new login is shared with
    /// no Bot.
    pub(crate) fn add(&self, origin: &str, username: &str, password: &Secret, seed: &Secret) -> Result<SiteLogin, String> {
        let site = site_origin(origin).ok_or("a login needs a site")?;
        let username = username.trim();
        if username.is_empty() {
            return Err("a login needs a username".into());
        }
        if password.is_empty() && seed.is_empty() {
            return Err("a login needs a password or an authenticator secret".into());
        }
        check_seed(seed)?;
        if self.db(|c| db::find(c, &site, username))?.is_some() {
            return Err(format!("a login for {site} as {username} is saved already: edit that one"));
        }
        let kind = if password.is_empty() { KIND_CODE } else { KIND_PASSWORD };
        let first = if password.is_empty() { seed } else { password };
        let row = self.save(&site, username, kind, first)?;
        if kind == KIND_PASSWORD && !seed.is_empty() {
            self.secrets.set(&code_account(&row.id), seed)?;
        }
        Ok(row)
    }

    /// Changes row `id`: its site and username, and its password or seed when
    /// one is given (an empty one keeps what's saved: a secret is write-only
    /// here, never read back). A share is for a login on its site: moved to
    /// another site, the login is shared with no Bot until shared again. A
    /// change of username or secret keeps its shares.
    pub(crate) fn edit(&self, id: &str, origin: &str, username: &str, password: &Secret, seed: &Secret) -> Result<SiteLogin, String> {
        let mut row = self.get(id).ok_or("that login isn't saved any more")?;
        let site = site_origin(origin).ok_or("a login needs a site")?;
        let username = username.trim();
        if username.is_empty() {
            return Err("a login needs a username".into());
        }
        check_seed(seed)?;
        if let Some(other) = self.db(|c| db::find(c, &site, username))?.filter(|o| o.id != row.id) {
            return Err(format!("another login is saved for {} as {}", other.origin, other.username));
        }
        if !password.is_empty() {
            self.secrets.set(&row.id, password)?;
            row.kind = KIND_PASSWORD.to_owned();
        }
        if !seed.is_empty() {
            self.secrets.set(&code_account(&row.id), seed)?;
        }
        if row.label == default_label(&row.username, &row.origin) || row.label.trim().is_empty() {
            row.label = default_label(username, &site);
        }
        let moved = row.origin != site;
        row.origin = site;
        row.username = username.to_owned();
        row.exact = true;
        row.updated_at = now_ms();
        self.db(|c| db::update(c, &row))?;
        if moved {
            self.db(|c| db::set_shares(c, &row.id, &BTreeSet::new()))?;
        }
        self.get(&row.id).ok_or_else(|| "that login isn't saved any more".to_owned())
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
        self.unlock_as(row, row.kind == KIND_CODE, reason)
    }

    /// [`Vault::unlock`] of the row's authenticator seed (`code`) or its
    /// password, whatever its kind: a password row may keep a seed beside it.
    pub(crate) fn unlock_as(&self, row: &SiteLogin, code: bool, reason: &str) -> Unlock {
        match self.gate.confirm(reason) {
            AuthOutcome::Verified => {}
            AuthOutcome::Cancelled => return Unlock::Cancelled,
            AuthOutcome::Unavailable(why) | AuthOutcome::Failed(why) => return Unlock::Refused(why),
        }
        let account = if code { code_account(&row.id) } else { row.id.clone() };
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

    /// Binds a legacy row to the exact site its owner just used it on, so
    /// it is offered there only from now on. A row already exact, or one
    /// whose site and name another row holds, stays as it is.
    pub(crate) fn pin_site(&self, id: &str, origin: &str) {
        if let Some(site) = site_origin(origin) {
            let _ = self.db(|c| db::pin_site(c, id, &site));
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

/// An authenticator seed that would mint, or none: a bad one is refused
/// before anything is written (the sentence never repeats the seed).
fn check_seed(seed: &Secret) -> Result<(), String> {
    if seed.is_empty() {
        return Ok(());
    }
    totp::Totp::parse(seed.expose()).map(|_| ())
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
        assert_eq!(row.origin, "accounts.google.com");
        assert!(row.exact);
        assert_eq!(row.label, "ada@example.com on accounts.google.com");
        assert!(!format!("{row:?}").contains("s3cret-pass"));
        assert!(v.has_secret(&row));
        assert_eq!(v.unlock(&row, "test"), Unlock::Secret(Secret::new("s3cret-pass")));
        assert_eq!(v.rows_for("https://accounts.google.com/x", KIND_PASSWORD).len(), 1);
        assert!(v.rows_for("mail.google.com", KIND_PASSWORD).is_empty());
        assert!(v.rows_for("accounts.google.com", KIND_CODE).is_empty());
        // the second save of the same login keeps its id
        let again = v.save("accounts.google.com", "ada@example.com", KIND_PASSWORD, &Secret::new("n3w")).expect("save");
        assert_eq!(again.id, row.id);
        assert_eq!(v.list().len(), 1);
        v.delete(&row.id).expect("delete");
        assert!(v.list().is_empty());
        assert!(!v.has_secret(&row));
    }

    #[test]
    fn exact_rows_are_offered_on_their_own_site_only() {
        let v = memory_vault(AuthOutcome::Verified);
        for (site, user) in [
            ("alice.pythonanywhere.com", "alice"),
            ("alice.hf.space", "alice"),
            ("localhost:3000", "ada"),
            ("127.0.0.1:8443", "ada"),
        ] {
            v.save(site, user, KIND_PASSWORD, &Secret::new("pw")).expect("save");
        }
        for other in ["mallory.pythonanywhere.com", "mallory.hf.space", "http://localhost:8080", "127.0.0.1:9000", "localhost"] {
            assert!(v.rows_for(other, KIND_PASSWORD).is_empty(), "nothing for {other}");
        }
        assert_eq!(v.rows_for("http://localhost:3000/login", KIND_PASSWORD).len(), 1);
        assert_eq!(v.rows_for("https://alice.hf.space/", KIND_PASSWORD).len(), 1);
    }

    #[test]
    fn a_table_from_before_exact_sites_keeps_its_rows_as_legacy_and_pins_them_on_use() {
        let c = Connection::open_in_memory().expect("sqlite");
        // the table as earlier builds made it, with a row they saved
        c.execute_batch(
            "CREATE TABLE site_logins (
                id TEXT PRIMARY KEY NOT NULL, origin TEXT NOT NULL, username TEXT NOT NULL,
                label TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'password', notes TEXT NOT NULL DEFAULT '',
                last_used_at_ms INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             CREATE UNIQUE INDEX site_logins_origin_username ON site_logins (origin, username);
             INSERT INTO site_logins (id, origin, username, label, created_at, updated_at)
                VALUES ('sl_old', 'localhost', 'ada', 'ada on localhost', 1, 1);",
        )
        .expect("old table");
        db::migrate(&c).expect("migrate");
        db::migrate(&c).expect("a second migrate is a no-op");
        let v = Vault::with(Some(c), None, Box::new(MemorySecrets::default()), Box::new(Answer(AuthOutcome::Verified)));
        let old = v.list().pop().expect("the old row");
        assert!(!old.exact);
        assert_eq!(site_match(&old.origin, old.exact, "localhost:3000"), SiteMatch::Legacy);
        assert_eq!(v.rows_for("http://localhost:3000", KIND_PASSWORD).len(), 1);
        v.pin_site("sl_old", "http://localhost:3000/login");
        let pinned = v.list().pop().expect("the row");
        assert_eq!((pinned.id.as_str(), pinned.origin.as_str(), pinned.exact), ("sl_old", "localhost:3000", true));
        assert!(v.rows_for("localhost:8080", KIND_PASSWORD).is_empty());
        assert_eq!(v.rows_for("localhost:3000", KIND_PASSWORD).len(), 1);
        // an exact row is never moved again
        v.pin_site("sl_old", "localhost:8080");
        assert_eq!(v.list().pop().map(|r| r.origin), Some("localhost:3000".to_owned()));
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
    fn a_new_login_is_shared_with_no_bot_and_one_saved_from_a_card_with_that_bot() {
        let v = memory_vault(AuthOutcome::Verified);
        let hand = v.add("shop.example", "ada", &Secret::new("fake-pw"), &Secret::new("")).expect("add");
        assert!(hand.shared_with.is_empty(), "added by hand: no Bot");
        assert!(!v.share_all(), "share-all starts off");
        assert!(v.offered("shop.example", "nova", false).is_empty(), "not offered to any Bot");
        assert_eq!(v.usable(&hand.id, "nova", &["shop.example".into()], false), Err(Refusal::NotShared));
        // saved from Nova's card: Nova's
        let card = v.save_for("shop.example", "kid", KIND_PASSWORD, &Secret::new("fake-pw"), Some("nova")).expect("save");
        assert_eq!(card.shared_with.iter().map(String::as_str).collect::<Vec<_>>(), ["nova"]);
        let ids = |rows: Vec<SiteLogin>| rows.into_iter().map(|r| r.username).collect::<Vec<_>>();
        assert_eq!(ids(v.offered("shop.example", "nova", false)), ["kid"]);
        assert!(v.offered("shop.example", "orbit", false).is_empty(), "another Bot gets nothing");
        // saved again from Orbit's card: both, nothing lost
        let again = v.save_for("shop.example", "kid", KIND_PASSWORD, &Secret::new("fake-pw2"), Some("orbit")).expect("save");
        assert_eq!((again.id.as_str(), again.shared_with.len()), (card.id.as_str(), 2));
        // and a save with no Bot keeps the shares it had
        assert_eq!(v.save("shop.example", "kid", KIND_PASSWORD, &Secret::new("")).expect("save").shared_with.len(), 2);
    }

    #[test]
    fn sharing_offers_a_login_only_to_its_bots_on_its_site_and_share_all_offers_every_one() {
        let v = memory_vault(AuthOutcome::Verified);
        let shop = v.add("shop.example", "ada", &Secret::new("fake-pw"), &Secret::new("")).expect("add");
        let bank = v.add("bank.example", "ada", &Secret::new("fake-pw"), &Secret::new("")).expect("add");
        v.share(&shop.id, "nova", true).expect("share");
        let site = |s: &str| vec![s.to_owned()];
        assert_eq!(v.offered("shop.example", "nova", false).len(), 1);
        assert!(v.offered("bank.example", "nova", false).is_empty(), "shared, but not that login");
        assert!(v.offered("evil.shop.example", "nova", false).is_empty(), "never on another site");
        assert!(v.offered("shop.example", "", false).is_empty(), "a card with no Bot gets nothing");
        assert!(v.usable(&shop.id, "nova", &site("https://shop.example/login"), false).is_ok());
        assert_eq!(v.usable(&shop.id, "nova", &site("evil.example"), false), Err(Refusal::OtherSite));
        assert_eq!(v.usable(&shop.id, "", &site("shop.example"), false), Err(Refusal::NoBot));
        assert_eq!(v.usable(&bank.id, "nova", &site("bank.example"), false), Err(Refusal::NotShared));
        assert_eq!(v.usable("sl_nope", "nova", &site("bank.example"), false), Err(Refusal::Gone));
        // share-all: every login to every Bot, still only on its own site
        v.set_share_all(true).expect("on");
        assert!(v.share_all());
        assert_eq!(v.offered("bank.example", "orbit", false).len(), 1);
        assert!(v.usable(&bank.id, "orbit", &site("bank.example"), false).is_ok());
        assert_eq!(v.usable(&bank.id, "orbit", &site("shop.example"), false), Err(Refusal::OtherSite));
        assert!(v.offered("bank.example", "", false).is_empty(), "never to no Bot");
        v.set_share_all(false).expect("off");
        assert_eq!(v.usable(&bank.id, "orbit", &site("bank.example"), false), Err(Refusal::NotShared));
        // the right site, but a code card and no seed: its own refusal
        assert_eq!(v.usable(&shop.id, "nova", &site("shop.example"), true), Err(Refusal::WrongKind { code: true }));
        assert!(Refusal::WrongKind { code: true }.sentence("Nova").contains("no authenticator code"));
        assert!(!Refusal::WrongKind { code: true }.sentence("Nova").contains("another site"));
        // unshared again: gone from Nova's cards
        v.share(&shop.id, "nova", false).expect("unshare");
        assert!(v.offered("shop.example", "nova", false).is_empty());
    }

    /// A store whose next write runs `hook` first, as a share changed on the UI
    /// thread while a save waited on the Keychain.
    struct Hooked {
        inner: MemorySecrets,
        hook: std::sync::Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>,
    }

    impl SecretStore for Hooked {
        fn set(&self, account: &str, secret: &Secret) -> Result<(), String> {
            if let Some(hook) = self.hook.lock().unwrap().take() {
                hook();
            }
            self.inner.set(account, secret)
        }
        fn get(&self, account: &str) -> Result<Option<Secret>, String> {
            self.inner.get(account)
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            self.inner.delete(account)
        }
        fn contains(&self, account: &str) -> bool {
            self.inner.contains(account)
        }
    }

    #[test]
    fn a_share_changed_while_a_save_or_an_edit_waits_on_the_keychain_stays_changed() {
        let path = std::env::temp_dir().join(format!("buwiz-vault-race-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).expect("sqlite");
        db::migrate(&c).expect("schema");
        let hook = std::sync::Arc::new(Mutex::new(None));
        let v = Vault::with(Some(c), None, Box::new(Hooked { inner: MemorySecrets::default(), hook: hook.clone() }), Box::new(Answer(AuthOutcome::Verified)));
        let arm = |f: Box<dyn FnOnce() + Send>| *hook.lock().unwrap() = Some(f);
        let row = v.save_for("shop.example", "kid", KIND_PASSWORD, &Secret::new("fake-pw"), Some("nova")).expect("save");
        // Orbit's card saves it again; Nova's share is taken back while the Keychain is written
        let (p, id) = (path.clone(), row.id.clone());
        arm(Box::new(move || db::set_shares(&Connection::open(&p).unwrap(), &id, &BTreeSet::new()).unwrap()));
        let again = v.save_for("shop.example", "kid", KIND_PASSWORD, &Secret::new("fake-pw2"), Some("orbit")).expect("save");
        assert_eq!(again.shared_with.iter().map(String::as_str).collect::<Vec<_>>(), ["orbit"], "Nova's unshare isn't put back");
        // an edit of its password while Nova is shared again: the share stays
        let (p, id) = (path.clone(), row.id.clone());
        arm(Box::new(move || {
            let c = Connection::open(&p).unwrap();
            db::change_share(&c, &id, "nova", true).unwrap();
        }));
        let edited = v.edit(&row.id, "shop.example", "kid", &Secret::new("fake-pw3"), &Secret::new("")).expect("edit");
        assert_eq!(edited.shared_with.iter().map(String::as_str).collect::<Vec<_>>(), ["nova", "orbit"], "the share made meanwhile isn't undone");
        drop(v);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn deleting_a_bot_takes_it_off_every_login() {
        let v = memory_vault(AuthOutcome::Verified);
        let a = v.save_for("shop.example", "ada", KIND_PASSWORD, &Secret::new("fake-pw"), Some("nova")).expect("save");
        let b = v.save_for("bank.example", "ada", KIND_PASSWORD, &Secret::new("fake-pw"), Some("nova")).expect("save");
        v.share(&b.id, "orbit", true).expect("share");
        v.forget_bot("nova").expect("forget");
        assert!(v.get(&a.id).expect("row").shared_with.is_empty());
        assert_eq!(v.get(&b.id).expect("row").shared_with.iter().map(String::as_str).collect::<Vec<_>>(), ["orbit"]);
        assert!(v.offered("shop.example", "nova", false).is_empty());
    }

    #[test]
    fn a_table_from_before_sharing_opens_with_nothing_shared_and_share_all_off() {
        let c = Connection::open_in_memory().expect("sqlite");
        // the table as B1 made it: `exact`, no `shared_with`, no `vault_settings`
        c.execute_batch(
            "CREATE TABLE site_logins (
                id TEXT PRIMARY KEY NOT NULL, origin TEXT NOT NULL, username TEXT NOT NULL,
                label TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'password', notes TEXT NOT NULL DEFAULT '',
                last_used_at_ms INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                exact INTEGER NOT NULL DEFAULT 0);
             CREATE UNIQUE INDEX site_logins_origin_username ON site_logins (origin, username);
             INSERT INTO site_logins (id, origin, username, label, created_at, updated_at, exact)
                VALUES ('sl_b1', 'shop.example', 'ada', 'ada on shop.example', 1, 1, 1);",
        )
        .expect("old table");
        db::migrate(&c).expect("migrate");
        db::migrate(&c).expect("a second migrate is a no-op");
        let v = Vault::with(Some(c), None, Box::new(MemorySecrets::default()), Box::new(Answer(AuthOutcome::Verified)));
        let old = v.get("sl_b1").expect("the old row");
        assert!(old.shared_with.is_empty(), "shared with no Bot");
        assert!(old.exact);
        assert!(!v.share_all(), "share-all off");
        assert!(v.offered("shop.example", "nova", false).is_empty(), "offered to no Bot");
        v.share("sl_b1", "nova", true).expect("share");
        assert_eq!(v.offered("shop.example", "nova", false).len(), 1);
    }

    #[test]
    fn add_and_edit_keep_secrets_write_only_and_refuse_what_wont_work() {
        let v = memory_vault(AuthOutcome::Verified);
        let none = Secret::new("");
        assert!(v.add("", "ada", &Secret::new("fake-pw"), &none).is_err());
        assert!(v.add("shop.example", " ", &Secret::new("fake-pw"), &none).is_err());
        assert!(v.add("shop.example", "ada", &none, &none).is_err(), "nothing to keep");
        let bad = v.add("shop.example", "ada", &Secret::new("fake-pw"), &Secret::new("fake-seed-not-base32!")).unwrap_err();
        assert!(!bad.contains("fake-seed"), "the sentence never repeats the seed: {bad}");
        assert!(v.list().is_empty(), "a bad seed writes nothing");
        let row = v.add("https://shop.example/login", "ada", &Secret::new("fake-pw"), &Secret::new("JBSWY3DPEHPK3PXP")).expect("add");
        assert_eq!((row.origin.as_str(), row.kind.as_str()), ("shop.example", KIND_PASSWORD));
        assert!(v.has_secret(&row) && v.has_code(&row), "a password, and a seed beside it");
        assert_eq!(v.offered("shop.example", "nova", true).len(), 0, "not shared yet");
        v.share(&row.id, "nova", true).expect("share");
        assert_eq!(v.offered("shop.example", "nova", true).len(), 1, "offered on a code card too");
        assert!(v.add("shop.example", "ada", &Secret::new("fake-pw"), &none).is_err(), "saved already: an edit");
        // a new username keeps the shares
        let renamed = v.edit(&row.id, "shop.example", "ada2", &none, &none).expect("edit");
        assert_eq!(renamed.shared_with.len(), 1, "a username change keeps its shares");
        // an edit with empty secrets keeps them; a new site stops its shares
        let moved = v.edit(&row.id, "shop2.example", "ada2", &none, &none).expect("edit");
        assert_eq!((moved.id.as_str(), moved.origin.as_str(), moved.username.as_str()), (row.id.as_str(), "shop2.example", "ada2"));
        assert_eq!(moved.label, "ada2 on shop2.example");
        assert!(moved.shared_with.is_empty(), "moved to another site: shared with no Bot");
        assert!(v.offered("shop2.example", "nova", false).is_empty(), "not offered to Nova on the new site");
        v.share(&row.id, "nova", true).expect("share again");
        assert_eq!(v.unlock(&moved, "test"), Unlock::Secret(Secret::new("fake-pw")));
        v.edit(&row.id, "shop2.example", "ada2", &Secret::new("fake-pw-new"), &none).expect("edit");
        assert_eq!(v.unlock(&moved, "test"), Unlock::Secret(Secret::new("fake-pw-new")));
        // a clash with another row changes nothing
        let other = v.add("bank.example", "bob", &Secret::new("fake-pw"), &none).expect("add");
        assert!(v.edit(&other.id, "shop2.example", "ada2", &none, &none).is_err());
        assert_eq!(v.get(&other.id).map(|r| r.origin), Some("bank.example".to_owned()));
        assert!(!format!("{:?}", v.list()).contains("fake-pw"));
    }

    #[test]
    fn pending_saves_and_secrets_never_debug_print() {
        let v = memory_vault(AuthOutcome::Verified);
        let p = PendingSave { origin: "example.com".into(), username: "ada".into(), password: Secret::new("hunter2"), bot: "nova".into() };
        assert!(!format!("{p:?}").contains("hunter2"));
        assert!(!format!("{:?}", Unlock::Secret(Secret::new("hunter2"))).contains("hunter2"));
        v.stash_pending("e_1", p);
        assert!(v.has_pending("e_1"));
        assert_eq!(v.take_pending("e_1").map(|p| p.password), Some(Secret::new("hunter2")));
        assert!(!v.has_pending("e_1"));
    }
}
