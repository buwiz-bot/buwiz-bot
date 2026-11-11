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
//!
//! A login saved here is shared with the Bot whose card typed it, by that
//! Bot's id (`Vault::save_for`): that Bot just used it on its own computer.
//! A login saved already keeps the Bots it had. A card that names no Bot id
//! shares it with none. The offer says which of these it will be.

use std::borrow::Borrow;

use nsh_core::prelude::*;
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
    /// The card's Bot (its id), whom a Save shares the login with; "" shares it with none.
    pub bot: String,
    /// That Bot's name, to say so; "" when unknown.
    pub bot_name: String,
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
            // the frame's Bot, for its name; who a Save shares it with is the held login's
            bot: at(&["bot", "agentId", "coworkerId"]).unwrap_or_default(),
            bot_name: at(&["botName"]).unwrap_or_default(),
        })
    }
}

/// Save: the held password to the Keychain, the row to the table. The
/// held login's own site and name are used; the frame's are display.
pub(crate) fn save(spec: &SaveLoginSpec, key: &str) {
    let Some(pending) = vault().take_pending(&spec.form_entry_id) else {
        return set_state(key, CardState::Error("The password is no longer held, so it can't be saved.".into()));
    };
    let (key, entry) = (key.to_owned(), spec.form_entry_id.clone());
    set_state(&key, CardState::Sending);
    Owner::detached().run(|| {
        spawn(async move {
            let saved = off_main(move || {
                // shared with the Bot whose card typed it, and no other
                let bot = Some(pending.bot.as_str()).filter(|b| !b.is_empty());
                let r = vault().save_for(&pending.origin, &pending.username, vault::KIND_PASSWORD, &pending.password, bot);
                (r, pending)
            })
            .await;
            match saved {
                Some((Ok(_), _)) => {
                    vault::changed();
                    set_state(&key, CardState::Answered("saved".into()))
                }
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

/// What Save means: where the password goes, and which Bots it's offered to
/// after. Who it's shared with is read from the login held for the save (its
/// card's Bot id, which Save shares it with) and the row saved already, so the
/// words and the save agree on both paths, the broker's card and a
/// `credential.offer_save` frame.
pub(crate) fn saved_note(spec: &SaveLoginSpec) -> String {
    let bots = crate::live::store().bots.get_untracked();
    let name = |id: &str| crate::agents::name_of(id, &bots);
    let me = vault().pending_bot(&spec.form_entry_id).filter(|b| !b.is_empty());
    let others: Vec<String> = vault()
        .find(&spec.origin, &spec.username)
        .map(|r| r.shared_with.into_iter().filter(|b| Some(b) != me.as_ref()).map(|b| name(&b)).collect())
        .unwrap_or_default();
    let me = me.map(|id| if !spec.bot_name.is_empty() && (spec.bot.is_empty() || spec.bot == id) { spec.bot_name.clone() } else { name(&id) });
    format!("The password stays in the macOS Keychain. Buwiz Bot never shows it again without Touch ID. {}", offer_words(me.as_deref(), &others, vault().share_all()))
}

/// One true sentence for who a saved login is offered to: the card's Bot
/// (`me`, its name), the Bots that keep it already, and Share all.
fn offer_words(me: Option<&str>, others: &[String], share_all: bool) -> String {
    let rest = others.join(", ");
    if share_all {
        let own = match (me, others.is_empty()) {
            (None, true) => return "Saved, every Bot is offered it while Share all is on; with Share all off, no Bot is.".to_owned(),
            (None, false) => format!("{rest} only"),
            (Some(m), true) => format!("{m} only"),
            (Some(m), false) => format!("{m} and {rest}"),
        };
        return format!("Saved, every Bot is offered it while Share all is on; with Share all off, it's offered to {own}.");
    }
    match (me, others.is_empty()) {
        (None, true) => "Saved, it isn't shared with any Bot yet. Share it in Settings › Logins.".to_owned(),
        (None, false) => format!("Saved, it stays shared with {rest} only."),
        (Some(m), true) => format!("Saved, it's offered to {m} only. Share it with other Bots in Settings › Logins."),
        (Some(m), false) => format!("Saved, it's offered to {m}, and still to {rest}."),
    }
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
            .child(note(saved_note(&spec)))
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
        let Some(root) = crate::cards::wire_dir() else { return };
        let path = root.join("agui/custom/credential.offer_save/an_agui_only_turn_still_mints_a_user_form_entry_id.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: Value = serde_json::from_str(&text).expect("json");
        let spec = SaveLoginSpec::from_wire(&v).expect("offer");
        assert_eq!(spec.origin, "accounts.google.com");
        assert_eq!(spec.username, "ada@example.com");
        assert_eq!(spec.form_entry_id, "e_0199bb4e-0000-7000-8000-000000000002");
    }

    #[test]
    fn the_offer_says_one_true_sentence_for_each_case() {
        let o = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(offer_words(Some("Nova"), &[], false), "Saved, it's offered to Nova only. Share it with other Bots in Settings › Logins.");
        assert_eq!(offer_words(Some("Nova"), &o(&["Orbit"]), false), "Saved, it's offered to Nova, and still to Orbit.");
        assert_eq!(offer_words(None, &[], false), "Saved, it isn't shared with any Bot yet. Share it in Settings › Logins.");
        // Share all on: every Bot, and its own shares for when it's off
        assert_eq!(offer_words(Some("Nova"), &[], true), "Saved, every Bot is offered it while Share all is on; with Share all off, it's offered to Nova only.");
        assert_eq!(offer_words(Some("Nova"), &o(&["Orbit"]), true), "Saved, every Bot is offered it while Share all is on; with Share all off, it's offered to Nova and Orbit.");
        assert_eq!(offer_words(None, &[], true), "Saved, every Bot is offered it while Share all is on; with Share all off, no Bot is.");
        for (me, others) in [(Some("Nova"), o(&[])), (None, o(&[]))] {
            let w = offer_words(me, &others, true);
            assert!(!w.contains(" only.") || w.contains("with Share all off"), "{w}");
        }
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
