//! Where a saved login's secret lives: the macOS Keychain, generic
//! passwords under service [`super::KEYCHAIN_SERVICE`], account = the
//! `site_logins` row id (a code seed sits beside it as `<id>:otp`).
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
#[cfg_attr(all(target_os = "macos", not(test)), allow(dead_code))]
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

#[cfg(target_os = "macos")]
pub(crate) struct Keychain;

#[cfg(target_os = "macos")]
impl SecretStore for Keychain {
    fn set(&self, account: &str, secret: &Secret) -> Result<(), String> {
        security_framework::passwords::set_generic_password(super::KEYCHAIN_SERVICE, account, secret.expose().as_bytes())
            .map_err(|e| e.to_string())
    }

    fn get(&self, account: &str) -> Result<Option<Secret>, String> {
        use security_framework::passwords::{PasswordOptions, generic_password};
        match generic_password(PasswordOptions::new_generic_password(super::KEYCHAIN_SERVICE, account)) {
            Ok(bytes) => String::from_utf8(bytes).map(|s| Some(Secret::new(s))).map_err(|e| e.to_string()),
            Err(e) if not_found(&e) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(super::KEYCHAIN_SERVICE, account) {
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
            .service(super::KEYCHAIN_SERVICE)
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

/// The store the app uses: the Keychain on macOS, memory elsewhere (no
/// secret is ever written to a file).
pub(crate) fn open() -> Box<dyn SecretStore> {
    #[cfg(target_os = "macos")]
    {
        Box::new(Keychain)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(MemorySecrets::default())
    }
}
