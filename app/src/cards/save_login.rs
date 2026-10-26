//! The save-login offer: "Save login for {origin} as {username}?" after a
//! login typed into a user form went through (CUSTOM
//! `credential.offer_save {origin, username, formEntryId}`, ported from
//! nativechat `src/opengrok/credential.rs` and
//! `src/components/save_login.rs`).
//!
//! The frame never carries a password. The one the person typed is held
//! in memory by the vault from the form's submit (`Vault::stash_pending`,
//! keyed by the form's entry id) until Save writes it to the Keychain or
//! Not now drops it; with nothing held (a replay after a restart, a login
//! that came from the vault) the card paints nothing. Where the offer
//! stands is the cards' shared state under its key.

use std::borrow::Borrow;

use nsh_core::ui::prelude::*;
use serde_json::Value;

use super::{Btn, CardState, btn, error_line, note, set_state, shell, state_of, title};
use crate::vault::{self, off_main, vault};

/// The CUSTOM name of the offer.
pub(crate) const CREDENTIAL_OFFER_SAVE: &str = "credential.offer_save";

/// The offer, as the wire states it: never a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SaveLoginSpec {
    pub form_entry_id: String,
    pub origin: String,
    pub username: String,
}

impl SaveLoginSpec {
    /// A `credential.offer_save` frame (fields in its `value` or beside
    /// it); `None` for anything else, or one without a site or a name.
    pub(crate) fn from_wire(v: &Value) -> Option<Self> {
        if v.get("name").and_then(Value::as_str) != Some(CREDENTIAL_OFFER_SAVE) {
            return None;
        }
        let value = v.get("value").unwrap_or(&Value::Null);
        let at = |keys: &[&str]| {
            keys.iter().find_map(|k| {
                [value, v].into_iter().find_map(|o| {
                    o.get(*k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
                })
            })
        };
        Some(Self {
            origin: at(&["origin"])?,
            username: at(&["username"])?,
            form_entry_id: at(&["formEntryId", "form_entry_id", "entryId"]).unwrap_or_default(),
        })
    }
}

/// Save: the held password to the Keychain, the row to the table. The
/// held login's own site and name are used; the frame's are display.
fn save(spec: &SaveLoginSpec, key: &str) {
    let Some(pending) = vault().take_pending(&spec.form_entry_id) else {
        return set_state(key, CardState::Error("The password is no longer held, so it can't be saved.".into()));
    };
    let (key, entry) = (key.to_owned(), spec.form_entry_id.clone());
    set_state(&key, CardState::Sending);
    Owner::detached().run(|| {
        spawn(async move {
            let saved = off_main(move || {
                let r = vault().save(&pending.origin, &pending.username, vault::KIND_PASSWORD, &pending.password);
                (r, pending)
            })
            .await;
            match saved {
                Some((Ok(_), _)) => set_state(&key, CardState::Answered("saved".into())),
                Some((Err(e), pending)) => {
                    // held again, so Save can be pressed once more
                    vault().stash_pending(&entry, pending);
                    set_state(&key, CardState::Error(format!("Couldn't save the login: {e}")));
                }
                None => set_state(&key, CardState::Error("Couldn't save the login".into())),
            }
        });
    });
}

/// Not now: the held password is dropped.
fn skip(spec: &SaveLoginSpec, key: &str) {
    vault().take_pending(&spec.form_entry_id);
    set_state(key, CardState::Dismissed);
}

/// The offer card, its state under `key`.
pub(crate) fn card(spec: impl Borrow<SaveLoginSpec>, key: impl AsRef<str>) -> impl IntoElement {
    let spec = spec.borrow().clone();
    let key = key.as_ref().to_owned();
    dynamic(move || {
        let id = format!("save-login-{}", spec.form_entry_id);
        let who = format!("{} for {}", spec.username, spec.origin);
        let held = vault().has_pending(&spec.form_entry_id);
        let why = match state_of(&key) {
            Some(CardState::Answered(_)) => {
                return shell(&id).child(note(format!("✓ Saved {who} to your logins."))).into_element();
            }
            Some(CardState::Sending) => return shell(&id).child(note("Saving…".into())).into_element(),
            Some(CardState::Dismissed | CardState::Superseded) => return view().into_element(),
            Some(CardState::Error(why)) if !held => return shell(&id).child(error_line(why)).into_element(),
            Some(CardState::Error(why)) => Some(why),
            None if !held => return view().into_element(),
            None => None,
        };
        let (s1, k1, s2, k2) = (spec.clone(), key.clone(), spec.clone(), key.clone());
        shell(&id)
            .label(format!("Save login for {}", spec.origin))
            .child(title(format!("Save login for {} as {}?", spec.origin, spec.username)))
            .child(note("The password stays in the macOS Keychain. Rox never shows it again without Touch ID.".into()))
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .child(btn(Btn::Primary, "Save", move || save(&s1, &k1)))
                    .child(btn(Btn::Secondary, "Not now", move || skip(&s2, &k2))),
            )
            .when(why.is_some(), |v| v.child(error_line(why.unwrap_or_default())))
            .into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_offer_fixture_parses_without_a_password() {
        let path = "/Volumes/goldcoders/OSS/nativechat/fixtures/wire/agui/custom/credential.offer_save/an_agui_only_turn_still_mints_a_user_form_entry_id.json";
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let v: Value = serde_json::from_str(&text).expect("json");
        let spec = SaveLoginSpec::from_wire(&v).expect("offer");
        assert_eq!(spec.origin, "accounts.google.com");
        assert_eq!(spec.username, "ada@example.com");
        assert_eq!(spec.form_entry_id, "e_0199bb4e-0000-7000-8000-000000000002");
    }

    #[test]
    fn other_frames_and_nameless_offers_are_not_offers() {
        assert!(SaveLoginSpec::from_wire(&json!({"name": "user-form", "value": {"origin": "x", "username": "y"}})).is_none());
        assert!(SaveLoginSpec::from_wire(&json!({"name": CREDENTIAL_OFFER_SAVE, "value": {"origin": "x.com"}})).is_none());
        let smuggled = json!({"name": CREDENTIAL_OFFER_SAVE, "value": {"origin": "x.com", "username": "ada", "password": "hunter2"}});
        let spec = SaveLoginSpec::from_wire(&smuggled).expect("offer");
        assert!(!format!("{spec:?}").contains("hunter2"));
    }
}
