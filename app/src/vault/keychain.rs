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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The macOS Keychain, service [`super::KEYCHAIN_SERVICE`].
    Keychain,
    /// [`MemorySecrets`]: a test, a test instance, or not macOS.
    Memory,
}

/// Whether the environment asks for no Keychain: `BUWIZ_NO_KEYCHAIN` set (anything but empty, `0`
/// or `false`), or `BUWIZ_KEYSTORE` naming another store (`file:<dir>`, `memory`), as every test
/// broker and test instance is started with. A test instance must never reach the owner's
/// `com.buwiz.bot.site-login` items.
pub(crate) fn no_keychain_by(var: impl Fn(&str) -> Option<String>) -> bool {
    let set = |k: &str| var(k).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let off = set("BUWIZ_NO_KEYCHAIN").is_some_and(|v| v != "0" && !v.eq_ignore_ascii_case("false"));
    let other = set("BUWIZ_KEYSTORE").is_some_and(|v| v != "keychain");
    off || other
}

/// The store the vault opens, from the environment: never the Keychain in a test build, nor when
/// the environment asks for none ([`no_keychain_by`]), nor off macOS.
pub(crate) fn kind_by(var: impl Fn(&str) -> Option<String>) -> Kind {
    if cfg!(test) || !cfg!(target_os = "macos") || no_keychain_by(var) { Kind::Memory } else { Kind::Keychain }
}

/// [`kind_by`] of this process's environment.
pub(crate) fn kind() -> Kind {
    kind_by(buwiz_protocol::env::var)
}

/// The store the app uses: the Keychain on macOS, memory in a test, a test instance or elsewhere
/// (no secret is ever written to a file).
pub(crate) fn open() -> Box<dyn SecretStore> {
    match kind() {
        #[cfg(target_os = "macos")]
        Kind::Keychain => Box::new(Keychain(super::KEYCHAIN_SERVICE)),
        _ => Box::new(MemorySecrets::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_build_and_a_test_instance_never_open_the_keychain() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned());
        // a test build, whatever the environment says
        assert_eq!(kind_by(env(&[])), Kind::Memory);
        assert_eq!(kind(), Kind::Memory);
        // what a test broker and a test instance are started with
        for set in [&[("BUWIZ_NO_KEYCHAIN", "1")][..], &[("BUWIZ_KEYSTORE", "file:/tmp/x")], &[("BUWIZ_KEYSTORE", "memory")], &[("BUWIZ_NO_KEYCHAIN", "yes"), ("BUWIZ_KEYSTORE", "keychain")]] {
            assert!(no_keychain_by(env(set)), "{set:?}");
        }
        for set in [&[][..], &[("BUWIZ_NO_KEYCHAIN", "0")], &[("BUWIZ_NO_KEYCHAIN", "")], &[("BUWIZ_KEYSTORE", "keychain")], &[("BUWIZ_NO_KEYCHAIN", "false")]] {
            assert!(!no_keychain_by(env(set)), "{set:?}");
        }
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
