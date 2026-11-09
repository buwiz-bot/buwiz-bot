//! Setting up a computer from the chat: a `computer_setup` notice the broker posts when a Bot
//! suggests one (its `reason` the Bot's one line; it never sees a key), or a local one `/computer`
//! adds. Both draw the same card, which moves through its steps in place:
//!
//! - Pick a home: the homes with where each stands. One with no key opens Enter a key; one that's
//!   ready is checked. A remote computer opens the Computer tab's form.
//! - Enter a key: the home's key form ([`crate::homes::key_form`]). The broker checks the key
//!   before it saves it, so a yes is a key that works.
//! - Check result: it works, and it can be the default or this Bot's own; or why not, and
//!   Change key.
//!
//! A local card starts where `/computer` said: its notice's `step` (`key` or `check`) and `home`.
//! Where each card is lives here by its id, so a row drawn again picks up where it was. Its key
//! form's typed text lives only in that form until it's sent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nsh_core::prelude::*;
use serde_json::Value;

use crate::cards::{self, Btn};
use crate::homes::{self, Home, KeyForm, Said};
use crate::live::keep;
use crate::store::spawn_detached;
use crate::theme;

/// A `computer_setup` notice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notice {
    pub id: String,
    pub bot: String,
    pub bot_name: String,
    pub chat: String,
    /// the Bot's one line on why
    pub reason: String,
    pub state: String,
    pub at: i64,
    /// a local card's first step: `key` or `check`; "" picks a home
    pub step: String,
    pub home: String,
}

impl Notice {
    pub(crate) fn of(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        (s("kind") == "computer_setup" && !s("id").is_empty()).then(|| Self {
            id: s("id"),
            bot: s("bot"),
            bot_name: s("botName"),
            chat: s("chat"),
            reason: s("reason"),
            state: s("state"),
            at: v.get("at").and_then(Value::as_i64).unwrap_or(0),
            step: s("step"),
            home: s("home"),
        })
    }

    fn first_step(&self) -> Step {
        match self.step.as_str() {
            "key" if !self.home.is_empty() => Step::Key(self.home.clone()),
            "check" => Step::Checking(self.home.clone()),
            _ => Step::Pick,
        }
    }
}

/// Where a card is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Step {
    Pick,
    Key(String),
    /// "" checks the default home
    Checking(String),
    Checked(String, Result<String, String>),
    Done(String),
}

thread_local! {
    static STEPS: Signal<HashMap<String, Step>> = Owner::detached().run(|| keep("computer-setup.steps", HashMap::new));
    /// Each card's key form, by its id.
    static FORMS: RefCell<HashMap<String, KeyForm>> = RefCell::new(HashMap::new());
    /// What a press on the Check result said, by card.
    static NOTES: Signal<HashMap<String, String>> = Owner::detached().run(|| keep("computer-setup.notes", HashMap::new));
}

fn step_of(key: &str) -> Option<Step> {
    STEPS.with(|s| s.with(|m| m.get(key).cloned()))
}

fn go(key: &str, step: Step) {
    STEPS.with(|s| {
        s.update(|m| {
            m.insert(key.to_owned(), step);
        })
    });
}

fn form_of(key: &str) -> KeyForm {
    FORMS.with(|f| *f.borrow_mut().entry(key.to_owned()).or_insert_with(|| Owner::detached().run(KeyForm::new)))
}

fn set_note(key: &str, words: String) {
    NOTES.with(|s| {
        s.update(|m| {
            m.insert(key.to_owned(), words);
        })
    });
}

/// Checks `kind` ("" for the default home), then shows what came back.
fn run_check(key: String, kind: String) {
    go(&key, Step::Checking(kind.clone()));
    spawn_detached(async move {
        let h = homes::loaded().await;
        let kind = if kind.is_empty() { h.default.kind.clone() } else { kind };
        let result = match homes::check(&kind).await {
            Said::Yes(v) => Ok(v.get("message").and_then(Value::as_str).unwrap_or("It works.").to_owned()),
            Said::No(why) => Err(why),
        };
        go(&key, Step::Checked(kind, result));
    });
}

/// A home picked: a remote computer is the Computer tab's, one with no key needs one, and a
/// ready one is checked.
fn pick(key: &str, bot: &str, home: &Home) {
    if home.kind == "remote" {
        return crate::computer::open_remote(bot);
    }
    if !home.built {
        return;
    }
    if home.ready() {
        run_check(key.to_owned(), home.kind.clone());
    } else {
        form_of(key).open(&home.kind);
        go(key, Step::Key(home.kind.clone()));
    }
}

/// The row `chat.rs` draws for a `computer_setup` notice: the card while it's pending.
pub(crate) fn row(n: Notice) -> impl IntoElement {
    let body = if n.state.is_empty() || n.state == "pending" {
        flow(n.id.clone(), n.bot.clone(), n.bot_name.clone(), n.reason.clone(), n.first_step()).into_element()
    } else {
        cards::outcome_line(format!("{} suggested setting up a computer.", if n.bot_name.is_empty() { "A Bot" } else { &n.bot_name })).into_element()
    };
    view().flex_row().w_full().justify_center().child(body)
}

/// The card under `key`, for `bot` (named `name`; "" for none), starting at `start` the first
/// time it's drawn. `reason`, when there is one, is its first line.
pub(crate) fn flow(key: String, bot: String, name: String, reason: String, start: Step) -> impl IntoElement {
    if untracked(|| step_of(&key)).is_none() {
        match start {
            Step::Checking(kind) => run_check(key.clone(), kind),
            Step::Key(kind) => {
                form_of(&key).open(&kind);
                go(&key, Step::Key(kind));
            }
            other => go(&key, other),
        }
    }
    if !homes::known() {
        homes::refresh();
    }
    dynamic(move || {
        let step = step_of(&key).unwrap_or(Step::Pick);
        let h = homes::homes().unwrap_or_else(homes::fallback);
        let mut c = cards::shell(&format!("computer-setup-{key}")).label("Set up a computer");
        if !reason.is_empty() {
            c = c.child(cards::note(reason.clone()));
        }
        let home = |kind: &str| h.get(kind).cloned().unwrap_or_else(|| Home { kind: kind.to_owned(), label: homes::label_of(kind), built: true, ..Home::default() });
        match step {
            Step::Pick => c = pick_part(c, &key, &bot, &name, &h.list),
            Step::Key(kind) => {
                let home = home(&kind);
                let (k1, k2, kind1) = (key.clone(), key.clone(), kind.clone());
                let saved: Rc<dyn Fn()> = Rc::new(move || go(&k1, Step::Checked(kind1.clone(), Ok("The key works, and it's saved.".to_owned()))));
                let cancelled: Rc<dyn Fn()> = Rc::new(move || go(&k2, Step::Pick));
                c = c.child(cards::title(format!("Enter a key for {}", home.label)));
                for n in &home.notes {
                    c = c.child(cards::note(n.clone()));
                }
                c = c.child(homes::key_form(form_of(&key), home, saved, cancelled));
            }
            Step::Checking(kind) => {
                let label = if kind.is_empty() { h.default.label.clone() } else { home(&kind).label };
                c = c.child(cards::title(format!("Checking {label}…")));
            }
            Step::Checked(kind, result) => c = checked_part(c, &key, &bot, &name, &home(&kind), result),
            Step::Done(words) => return cards::outcome_line(words).into_element(),
        }
        c.into_element()
    })
}

fn pick_part(mut c: View, key: &str, bot: &str, name: &str, list: &[Home]) -> View {
    let whose = if name.is_empty() { "a new computer".to_owned() } else { format!("{name}'s next new computer") };
    c = c.child(cards::title(format!("Where should {whose} be made?")));
    for home in list {
        let (k, b, h) = (key.to_owned(), bot.to_owned(), home.clone());
        let mut row = view().label(home.label.clone()).flex_row().items_center().gap_2().px(px(10.0)).py(px(8.0)).rounded(px(10.0)).bg(theme::SUBTLE);
        row = row.child(text(home.label.clone()).text_size(px(13.0)).text_color(theme::TEXT).flex_1());
        if let Some(words) = home.key_words() {
            row = row.child(text(words).text_size(px(12.0)).text_color(theme::MUTED));
        }
        row = row.child(text(home.state_words()).text_size(px(12.0)).text_color(if home.ready() { theme::UP } else { theme::MUTED }));
        c = c.child(row.on_press(move || pick(&k, &b, &h)));
    }
    c
}

fn checked_part(mut c: View, key: &str, bot: &str, name: &str, home: &Home, result: Result<String, String>) -> View {
    let note = NOTES.with(|s| s.with(|m| m.get(key).cloned())).unwrap_or_default();
    match result {
        Ok(words) => {
            c = c.child(cards::title(format!("{} works", home.label))).child(cards::note(words));
            let mut buttons = view().flex_row().flex_wrap().gap_2().justify_end();
            if home.can_default {
                let (k, kind, label) = (key.to_owned(), home.kind.clone(), home.label.clone());
                buttons = buttons.child(cards::btn(Btn::Secondary, "Make it the default", move || {
                    let (k, kind, label) = (k.clone(), kind.clone(), label.clone());
                    spawn_detached(async move {
                        match homes::set_default(&kind).await {
                            Said::Yes(_) => go(&k, Step::Done(format!("New computers are now made on {label}."))),
                            Said::No(why) => set_note(&k, why),
                        }
                    });
                }));
            }
            if !bot.is_empty() && home.kind != "remote" {
                let (k, b, n, kind, label) = (key.to_owned(), bot.to_owned(), name.to_owned(), home.kind.clone(), home.label.clone());
                let who = if name.is_empty() { "this Bot".to_owned() } else { name.to_owned() };
                buttons = buttons.child(cards::btn(Btn::Primary, &format!("Only for {who}"), move || {
                    let (k, b, n, kind, label) = (k.clone(), b.clone(), n.clone(), kind.clone(), label.clone());
                    spawn_detached(async move {
                        match homes::set_for_bot(&b, Some(&kind)).await {
                            Said::Yes(_) => go(&k, Step::Done(format!("{}'s next new computer will be made on {label}.", if n.is_empty() { "This Bot" } else { &n }))),
                            Said::No(why) => set_note(&k, why),
                        }
                    });
                }));
            }
            c = c.child(buttons);
        }
        Err(why) => {
            c = c.child(cards::title(format!("{} didn't work", home.label))).child(cards::error_line(why));
            let mut buttons = view().flex_row().flex_wrap().gap_2().justify_end();
            let k = key.to_owned();
            buttons = buttons.child(cards::btn(Btn::Secondary, "Pick another", move || go(&k, Step::Pick)));
            if !home.fields.is_empty() {
                let (k, kind) = (key.to_owned(), home.kind.clone());
                buttons = buttons.child(cards::btn(Btn::Primary, "Change key", move || {
                    form_of(&k).open(&kind);
                    go(&k, Step::Key(kind.clone()));
                }));
            }
            c = c.child(buttons);
        }
    }
    if !note.is_empty() {
        c = c.child(cards::error_line(note));
    }
    c
}

/// For a test: whether card `key`'s form still holds anything typed.
#[cfg(test)]
pub(crate) fn form_is_empty(key: &str) -> bool {
    form_of(key).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Turn;
    use serde_json::json;

    #[test]
    fn a_notice_is_a_setup_card_with_the_bots_reason() {
        let t = Turn::of(&json!({"role": "notice", "text": "", "notice": {"kind": "computer_setup", "id": "cs_1", "bot": "b1", "botName": "Ann", "chat": "t1", "reason": "I need a browser for this.", "state": "pending", "at": 5}}));
        let n = t.computer_setup.expect("a setup card");
        assert_eq!((n.id.as_str(), n.bot_name.as_str(), n.reason.as_str(), n.first_step()), ("cs_1", "Ann", "I need a browser for this.", Step::Pick));
        assert!(Turn::of(&json!({"role": "notice", "notice": {"kind": "computer_setup"}})).computer_setup.is_none(), "no id, no card");
        assert!(Turn::of(&json!({"role": "user", "notice": {"kind": "computer_setup", "id": "cs_1"}})).computer_setup.is_none());
    }

    #[test]
    fn a_local_card_starts_where_the_command_said() {
        let n = |step: &str, home: &str| Notice { step: step.into(), home: home.into(), ..Notice::default() }.first_step();
        assert_eq!(n("key", "e2b"), Step::Key("e2b".into()));
        assert_eq!(n("key", ""), Step::Pick, "a key for no home is a pick");
        assert_eq!(n("check", ""), Step::Checking(String::new()));
        assert_eq!(n("", ""), Step::Pick);
    }
}
