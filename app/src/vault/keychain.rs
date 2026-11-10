//! Where a saved login's secret lives: the macOS Keychain, generic
//! passwords under service [`super::KEYCHAIN_SERVICE`], account = the
//! `site_logins` row id (a code seed sits beside it as `<id>:otp`). Items
//! Rox saved under [`super::OLD_KEYCHAIN_SERVICE`] are moved over once
//! ([`move_secrets`]).
//!
//! The UI never reads a secret to show it: lists ask [`SecretStore::contains`]
//! (an attribute search, which puts up no keychain sheet), and
//! [`super::Vault::unlock`] reads only after device-owner auth.
//! [`MemorySecrets`] stands in for the Keychain in tests and off macOS.

use std::collections::HashMap;
use std::sync::Mutex;

use super::Secret;

pub(crate) trait SecretStore: Send + Sync {
    fn set(&self, account: &str, secret: &Secret) -> Result<(), String>;
    /// Only through [`super::Vault::unlock`], after the owner proved it.
    fn get(&self, account: &str) -> Result<Option<Secret>, String>;
    fn delete(&self, account: &str) -> Result<(), String>;
    fn contains(&self, account: &str) -> bool;
}

/// Secrets in memory, gone with the process.
#[derive(Default)]
pub(crate) struct MemorySecrets {
    inner: Mutex<HashMap<String, Secret>>,
}

impl SecretStore for MemorySecrets {
    fn set(&self, account: &str, secret: &Secret) -> Result<(), String> {
        self.inner.lock().map_err(|e| e.to_string())?.insert(account.to_owned(), secret.clone());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<Secret>, String> {
        Ok(self.inner.lock().map_err(|e| e.to_string())?.get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.inner.lock().map_err(|e| e.to_string())?.remove(account);
        Ok(())
    }

    fn contains(&self, account: &str) -> bool {
        self.inner.lock().map(|m| m.contains_key(account)).unwrap_or(false)
    }
}

/// Moves the secrets of `accounts` from `old` (Rox's service) to `new`,
/// once: each is read, written to `new` unless `new` has one already, and
/// deleted from `old`. → how many moved. The first failure stops it, so
/// the caller tries again and writes no marker.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn move_secrets(old: &dyn SecretStore, new: &dyn SecretStore, accounts: &[String]) -> Result<usize, String> {
    let mut moved = 0;
    for account in accounts {
        if !old.contains(account) {
            continue;
        }
        // a secret saved under the new service since is the newer one
        if !new.contains(account) {
            let Some(secret) = old.get(account)? else { continue };
            new.set(account, &secret)?;
            if new.get(account)?.as_ref() != Some(&secret) {
                return Err("the copy under the new service reads back differently".to_owned());
            }
            moved += 1;
        }
        old.delete(account)?;
    }
    Ok(moved)
}

/// The Keychain's generic passwords under one service.
#[cfg(target_os = "macos")]
pub(crate) struct Keychain(pub &'static str);

#[cfg(target_os = "macos")]
impl SecretStore for Keychain {
    fn set(&self, account: &str, secret: &Secret) -> Result<(), String> {
        security_framework::passwords::set_generic_password(self.0, account, secret.expose().as_bytes())
            .map_err(|e| e.to_string())
    }

    fn get(&self, account: &str) -> Result<Option<Secret>, String> {
        use security_framework::passwords::{PasswordOptions, generic_password};
        match generic_password(PasswordOptions::new_generic_password(self.0, account)) {
            Ok(bytes) => String::from_utf8(bytes).map(|s| Some(Secret::new(s))).map_err(|e| e.to_string()),
            Err(e) if not_found(&e) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(self.0, account) {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Asked by attributes alone: loading no data puts up no keychain sheet.
    fn contains(&self, account: &str) -> bool {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(self.0)
            .account(account)
            .load_attributes(true)
            .limit(Limit::Max(1))
            .search()
            .is_ok_and(|found| !found.is_empty())
    }
}

/// `errSecItemNotFound`.
#[cfg(target_os = "macos")]
fn not_found(e: &security_framework::base::Error) -> bool {
    e.code() == -25300
}

/// Which store the app's vault opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The macOS Keychain, service [`super::KEYCHAIN_SERVICE`].
    Keychain,
    /// [`MemorySecrets`], and why: a test build, a test instance, or not macOS. Nothing saved
    /// outlives the process.
    Memory(&'static str),
    /// The environment names a store this app doesn't know: nothing is saved or read, and the
    /// Logins page says why. Never a silent fallback to memory or the Keychain.
    Refused(String),
}

/// The store the environment names, read as the broker reads it (`helper/keystore.rs`
/// `from_spec`, `main.rs`): `BUWIZ_KEYSTORE` is `keychain` or `file:<dir>` (a test instance's;
/// this app keeps no secret in a file, so its logins are kept in memory), and with it empty,
/// `BUWIZ_NO_KEYCHAIN=1` means no Keychain. Anything else is refused.
pub(crate) fn kind_by(var: impl Fn(&str) -> Option<String>) -> Kind {
    if cfg!(test) {
        return Kind::Memory("this is a test build");
    }
    if !cfg!(target_os = "macos") {
        return Kind::Memory("this isn't macOS");
    }
    spec_kind(var)
}

/// [`kind_by`]'s reading of the environment, for a real build on macOS.
fn spec_kind(var: impl Fn(&str) -> Option<String>) -> Kind {
    let spec = var("BUWIZ_KEYSTORE").unwrap_or_default();
    if spec.starts_with("file:") {
        return Kind::Memory("BUWIZ_KEYSTORE names a test store");
    }
    if !spec.is_empty() && spec != "keychain" {
        return Kind::Refused(format!("BUWIZ_KEYSTORE must be file:<dir> or keychain, not {}", spec.chars().take(40).collect::<String>()));
    }
    if spec.is_empty() && var("BUWIZ_NO_KEYCHAIN").as_deref() == Some("1") {
        return Kind::Memory("BUWIZ_NO_KEYCHAIN=1 is set");
    }
    Kind::Keychain
}

/// [`kind_by`] of this process's environment.
pub(crate) fn kind() -> Kind {
    kind_by(buwiz_protocol::env::var)
}

/// What the Logins page says of the store, when it isn't the Keychain in a real build: `None` for
/// the Keychain and for a test build.
pub(crate) fn warning(kind: &Kind) -> Option<String> {
    match kind {
        Kind::Keychain => None,
        Kind::Memory(_) if cfg!(test) || !cfg!(target_os = "macos") => None,
        Kind::Memory(why) => Some(format!("Saved logins aren't being kept: {why}, so this app uses no Keychain and forgets them when it quits.")),
        Kind::Refused(why) => Some(format!("Saved logins can't be used: {why}.")),
    }
}

/// A store that refuses everything, for an environment naming a store this app doesn't know.
struct RefusedSecrets(String);

impl SecretStore for RefusedSecrets {
    fn set(&self, _: &str, _: &Secret) -> Result<(), String> {
        Err(self.0.clone())
    }

    fn get(&self, _: &str) -> Result<Option<Secret>, String> {
        Err(self.0.clone())
    }

    fn delete(&self, _: &str) -> Result<(), String> {
        Err(self.0.clone())
    }

    fn contains(&self, _: &str) -> bool {
        false
    }
}

/// The store the app uses: the Keychain on macOS, memory in a test, a test instance or elsewhere
/// (no secret is ever written to a file), and none at all for a store it doesn't know.
pub(crate) fn open() -> Box<dyn SecretStore> {
    match kind() {
        #[cfg(target_os = "macos")]
        Kind::Keychain => Box::new(Keychain(super::KEYCHAIN_SERVICE)),
        Kind::Refused(why) => Box::new(RefusedSecrets(why)),
        _ => Box::new(MemorySecrets::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_build_never_opens_the_keychain_and_a_store_it_doesnt_know_is_refused() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned());
        // a test build, whatever the environment says: never the Keychain
        for set in [&[][..], &[("BUWIZ_KEYSTORE", "keychain")], &[("BUWIZ_KEYSTORE", "nonsense")]] {
            assert!(matches!(kind_by(env(set)), Kind::Memory(_)), "{set:?}");
        }
        assert!(matches!(kind(), Kind::Memory(_)));
        assert_eq!(warning(&kind()), None, "a test build says nothing of it");
        // what a real build makes of the words, read by the rule alone: nothing here opens a store
        assert_eq!(spec_kind(env(&[])), Kind::Keychain);
        assert_eq!(spec_kind(env(&[("BUWIZ_KEYSTORE", "keychain")])), Kind::Keychain);
        assert!(matches!(spec_kind(env(&[("BUWIZ_KEYSTORE", "file:/tmp/k")])), Kind::Memory(_)));
        assert!(matches!(spec_kind(env(&[("BUWIZ_NO_KEYCHAIN", "1")])), Kind::Memory(_)));
        assert_eq!(spec_kind(env(&[("BUWIZ_NO_KEYCHAIN", "yes")])), Kind::Keychain, "only 1, as the broker reads it");
        assert!(matches!(spec_kind(env(&[("BUWIZ_KEYSTORE", "memory")])), Kind::Refused(_)), "no silent memory store");
        assert!(matches!(spec_kind(env(&[("BUWIZ_KEYSTORE", "Keychain ")])), Kind::Refused(_)));
        assert_eq!(warning(&Kind::Refused("BUWIZ_KEYSTORE must be file:<dir> or keychain, not x".into())).unwrap(), "Saved logins can't be used: BUWIZ_KEYSTORE must be file:<dir> or keychain, not x.");
        let refused = RefusedSecrets("no".into());
        assert!(refused.set("a", &Secret::new("fake")).is_err() && refused.get("a").is_err() && !refused.contains("a"));
    }

    fn accounts(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Memory stores stand for the two services.
    #[test]
    fn rox_secrets_move_to_the_new_service_once() {
        let (old, new) = (MemorySecrets::default(), MemorySecrets::default());
        old.set("sl_1", &Secret::new("hunter2")).unwrap();
        old.set("sl_1:otp", &Secret::new("seed")).unwrap();
        old.set("sl_other", &Secret::new("not a row")).unwrap();
        let rows = accounts(&["sl_1", "sl_1:otp", "sl_2"]);
        assert_eq!(move_secrets(&old, &new, &rows).unwrap(), 2);
        assert_eq!(new.get("sl_1").unwrap(), Some(Secret::new("hunter2")));
        assert_eq!(new.get("sl_1:otp").unwrap(), Some(Secret::new("seed")));
        assert!(!old.contains("sl_1") && !old.contains("sl_1:otp"), "deleted from the old service");
        assert!(old.contains("sl_other"), "only the rows' accounts are touched");
        assert!(!new.contains("sl_2"));
        // again: nothing left to move
        assert_eq!(move_secrets(&old, &new, &rows).unwrap(), 0);
    }

    #[test]
    fn a_secret_already_under_the_new_service_wins() {
        let (old, new) = (MemorySecrets::default(), MemorySecrets::default());
        old.set("sl_1", &Secret::new("old")).unwrap();
        new.set("sl_1", &Secret::new("new")).unwrap();
        assert_eq!(move_secrets(&old, &new, &accounts(&["sl_1"])).unwrap(), 0);
        assert_eq!(new.get("sl_1").unwrap(), Some(Secret::new("new")));
        assert!(!old.contains("sl_1"));
    }

    /// A store whose writes fail: nothing is deleted from the old one.
    struct Refusing;
    impl SecretStore for Refusing {
        fn set(&self, _: &str, _: &Secret) -> Result<(), String> {
            Err("refused".into())
        }
        fn get(&self, _: &str) -> Result<Option<Secret>, String> {
            Ok(None)
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn contains(&self, _: &str) -> bool {
            false
        }
    }

    #[test]
    fn a_failed_write_keeps_the_old_secret() {
        let old = MemorySecrets::default();
        old.set("sl_1", &Secret::new("hunter2")).unwrap();
        assert!(move_secrets(&old, &Refusing, &accounts(&["sl_1"])).is_err());
        assert!(old.contains("sl_1"));
    }
}
