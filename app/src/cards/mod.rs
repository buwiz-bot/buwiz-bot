//! Interactive cards in an agent's reply — the client side of nativechat's
//! generative UI, in its wire shapes, so hooking a server up is mechanical.
//!
//! - [`Card::from_wire`] is the one parser: AG-UI `CUSTOM` frames
//!   (`run-awaiting-approval`, `user-form`, `credential.offer_save`,
//!   `opengrok.pluginNeeds`), the `form` frontend tool's call
//!   (`{tool, arguments}`), and the gateway's `send-message` envelope.
//! - A turn's cards ride on `Turn.cards` ([`TurnCard`]), read from an
//!   optional `cards` array on `/turns` (none yet) or added locally by the
//!   dev hook ([`inject`]); `chat.rs` mounts them under the bubble
//!   ([`section`]).
//! - Each card's state (answered, dismissed, superseded, sending, or a
//!   failure's sentence) lives in the store by its key — the call id or the
//!   form's entry id ([`state_of`] / [`set_state`]); a form's typed values
//!   never do (C2 keeps them in its card).
//! - [`transport`] sends the answers as nativechat does.

pub(crate) mod approval;
pub(crate) mod choice;
pub(crate) mod plugin_needs;
pub(crate) mod save_login;
pub(crate) mod transport;
pub(crate) mod user_form;

use std::rc::Rc;

use nsh_core::prelude::*;
use serde_json::Value;

use crate::live::store;
use crate::store::Turn;
use crate::theme;

pub(crate) use approval::ApprovalSpec;
pub(crate) use choice::FormSpec;
pub(crate) use plugin_needs::PluginNeedsSpec;

/// One interactive card of a reply.
pub(crate) enum Card {
    Approval(ApprovalSpec),
    Choice(FormSpec),
    UserForm(user_form::UserFormSpec),
    SaveLogin(save_login::SaveLoginSpec),
    PluginNeeds(PluginNeedsSpec),
}

/// Which card a wire object is, before its spec is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Approval,
    Choice,
    UserForm,
    SaveLogin,
    PluginNeeds,
}

const RUN_AWAITING_APPROVAL: &str = "run-awaiting-approval";

/// The kind of card `v` is, or `None` for a frame that paints no card.
pub(crate) fn kind_of(v: &Value) -> Option<Kind> {
    let name = v.get("name").and_then(Value::as_str).unwrap_or("");
    // The gateway's `send-message` envelope, bare or as a CUSTOM's value.
    let envelope = |v: &Value| {
        v.get("message").and_then(|m| m.get("type")).and_then(Value::as_str) == Some("user-form")
    };
    match name {
        RUN_AWAITING_APPROVAL => {
            let reason = str_at(v, "reason").unwrap_or("exec-consent");
            return if reason.trim().eq_ignore_ascii_case("user-form") {
                Some(Kind::UserForm)
            } else {
                (str_at(v, "runId").is_some() && str_at(v, "callId").is_some()).then_some(Kind::Approval)
            };
        }
        "user-form" => return Some(Kind::UserForm),
        "credential.offer_save" => return Some(Kind::SaveLogin),
        plugin_needs::PLUGIN_NEEDS_CUSTOM => return Some(Kind::PluginNeeds),
        _ => {}
    }
    if envelope(v) || v.get("value").is_some_and(envelope) {
        return Some(Kind::UserForm);
    }
    // A frontend tool's call: `{tool | toolCallName, arguments}`.
    let tool = v.get("tool").or_else(|| v.get("toolCallName")).and_then(Value::as_str);
    if tool.is_some_and(choice::is_form_name) {
        return Some(Kind::Choice);
    }
    // A CUSTOM `form` / `ui` widget, or an inline `{component:"form"}`.
    let component = |v: &Value| {
        v.get("component").or_else(|| v.get("ui")).and_then(Value::as_str).is_some_and(choice::is_form_name)
    };
    if v.get("type").and_then(Value::as_str) == Some("CUSTOM") {
        let value = v.get("value");
        if choice::is_form_name(name) && value.is_some()
            || choice::normalize_name(name) == "ui" && value.is_some_and(component)
        {
            return Some(Kind::Choice);
        }
    }
    component(v).then_some(Kind::Choice)
}

impl Card {
    /// The card a wire object carries; `None` for anything else, or one
    /// whose spec cannot be read.
    pub(crate) fn from_wire(v: &Value) -> Option<Card> {
        Some(match kind_of(v)? {
            Kind::Approval => Card::Approval(ApprovalSpec::from_wire(v)?),
            Kind::Choice => Card::Choice(choice_spec(v)?),
            Kind::UserForm => Card::UserForm(user_form::UserFormSpec::from_wire(v)?),
            Kind::SaveLogin => Card::SaveLogin(save_login::SaveLoginSpec::from_wire(v)?),
            Kind::PluginNeeds => Card::PluginNeeds(PluginNeedsSpec::from_wire(v)?),
        })
    }
}

/// A choice form from a tool call's `arguments`, a CUSTOM's `value`, or
/// the object itself.
fn choice_spec(v: &Value) -> Option<FormSpec> {
    v.get("arguments")
        .or_else(|| v.get("args"))
        .or_else(|| v.get("value"))
        .and_then(FormSpec::from_value)
        .or_else(|| FormSpec::from_value(v))
}

/// The key a card's state is kept under: its call id, or for a user form
/// its gateway entry id; a save-login offer is its form's entry, apart.
pub(crate) fn key_of(v: &Value, kind: Kind) -> String {
    let first = |keys: &[&str]| keys.iter().find_map(|k| str_at(v, k)).map(str::to_owned);
    match kind {
        Kind::Approval => first(&["callId"]),
        Kind::UserForm => first(&["entryId"])
            .or_else(|| {
                v.get("value")
                    .filter(|x| x.get("kind").and_then(Value::as_str) == Some("send-message"))
                    .and_then(|x| x.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| first(&["id", "callId"])),
        Kind::SaveLogin => first(&["formEntryId", "callId"]).map(|id| format!("save-login-{id}")),
        Kind::PluginNeeds => first(&["runId", "callId"]).map(|id| format!("plugin-needs-{id}")),
        Kind::Choice => first(&["toolCallId", "callId", "id"]),
    }
    .unwrap_or_else(|| format!("card-{:016x}", fnv(&v.to_string())))
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// A frame's field, or its `value`'s.
fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .or_else(|| v.get("value").and_then(|x| x.get(k)).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
}

/// A card as a turn holds it: its key, whether the wire already settled it
/// (a replayed form carries its `formResolution`), and the frame it came
/// from — compared, so an unchanged poll leaves the row alone.
#[derive(Clone)]
pub(crate) struct TurnCard {
    pub key: String,
    pub settled: bool,
    pub card: Rc<Card>,
    wire: Rc<Value>,
}

impl PartialEq for TurnCard {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.wire == other.wire
    }
}

impl TurnCard {
    pub(crate) fn of(v: &Value) -> Option<TurnCard> {
        let kind = kind_of(v)?;
        let card = Card::from_wire(v)?;
        let resolved = |x: &Value| x.get("formResolution").and_then(Value::as_str).is_some_and(|r| !r.is_empty());
        Some(TurnCard {
            key: key_of(v, kind),
            settled: resolved(v) || v.get("value").is_some_and(resolved),
            card: Rc::new(card),
            wire: Rc::new(v.clone()),
        })
    }

    /// Unanswered: nothing settled it on the wire or here.
    pub(crate) fn is_open(&self) -> bool {
        !self.settled && matches!(state_of(&self.key), None | Some(CardState::Error(_)))
    }
}

/// The cards of a list of wire objects (a turn's `cards`, a dev file).
pub(crate) fn parse_list(v: &Value) -> Vec<TurnCard> {
    match v {
        Value::Array(items) => items.iter().filter_map(TurnCard::of).collect(),
        Value::Null => Vec::new(),
        one => TurnCard::of(one).into_iter().collect(),
    }
}

// ---------------------------------------------------------------------------
// Per-card state
// ---------------------------------------------------------------------------

/// Where a card stands, kept by its key. No entry: still open.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CardState {
    /// The answer is on its way.
    Sending,
    /// Answered: the line it leaves (an approval's outcome, a choice's
    /// answer text, a form's resolution).
    Answered(String),
    Dismissed,
    /// A later message moved the chat on before it was answered.
    Superseded,
    /// The answer did not go through: the sentence; the card stays open.
    Error(String),
}

/// A card's state (tracked).
pub(crate) fn state_of(key: &str) -> Option<CardState> {
    store().card_states.with(|m| m.get(key).cloned())
}

pub(crate) fn set_state(key: &str, state: CardState) {
    store().card_states.update(|m| {
        m.insert(key.to_owned(), state);
    });
}

/// Whether a press may still answer `key`: open, or its last answer failed.
pub(crate) fn answerable(key: &str) -> bool {
    matches!(untracked(|| state_of(key)), None | Some(CardState::Error(_)))
}

/// Who the cards speak of: the open chat's bot, else the app.
pub(crate) fn bot_name() -> String {
    store().active_bot_untracked().map(|b| b.name).unwrap_or_else(|| crate::app_name().to_owned())
}

/// The `agentId` / coworker id the answers name: the open chat's bot.
pub(crate) fn agent_id() -> String {
    store().active_bot_untracked().map(|b| b.id).unwrap_or_default()
}

/// The composer's status while the chat waits on a card: "Waiting for
/// approval" for an open approval, else "Waiting for you" for an open
/// user form.
pub(crate) fn waiting_label(turns: &[Turn]) -> Option<&'static str> {
    let cards = || turns.iter().flat_map(|t| &t.cards);
    if cards().any(|c| matches!(&*c.card, Card::Approval(_)) && c.is_open()) {
        return Some("Waiting for approval");
    }
    cards().any(|c| matches!(&*c.card, Card::UserForm(_)) && c.is_open()).then_some("Waiting for you")
}

/// A new message in the chat: its open approvals, choices and forms are
/// superseded (nativechat's steer). Save-login offers stay.
pub(crate) fn supersede_open(turns: &[Turn]) {
    let keys: Vec<String> = turns
        .iter()
        .flat_map(|t| &t.cards)
        .filter(|c| matches!(&*c.card, Card::Approval(_) | Card::Choice(_) | Card::UserForm(_)))
        .filter(|c| untracked(|| c.is_open()))
        .map(|c| c.key.clone())
        .collect();
    if keys.is_empty() {
        return;
    }
    store().card_states.update(|m| {
        for k in keys {
            m.insert(k, CardState::Superseded);
        }
    });
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// A turn's cards in document order, under its bubble. Only the newest
/// still-open approval shows: older open ones would read as two yeses.
pub(crate) fn section(t: &Turn) -> impl IntoElement {
    let cards = Rc::new(t.cards.clone());
    let newest_open = {
        let cards = cards.clone();
        computed(move || {
            cards
                .iter()
                .rev()
                .find(|c| matches!(&*c.card, Card::Approval(_)) && c.is_open())
                .map(|c| c.key.clone())
        })
    };
    let mut col = view().w_full().gap_2().items_start();
    for c in cards.iter() {
        let el = render(c, t.i);
        col = col.child(if matches!(&*c.card, Card::Approval(_)) {
            let key = c.key.clone();
            let open = {
                let c = c.clone();
                move || c.is_open()
            };
            when(move || !open() || newest_open.get().as_deref() == Some(key.as_str()), move || el())
                .into_element()
        } else {
            el()
        });
    }
    col
}

/// One card, by its owner's renderer. A closure, so a card the approval
/// rule hides and shows again is built anew.
pub(crate) fn render(c: &TurnCard, turn: i64) -> Rc<dyn Fn() -> AnyElement> {
    let (card, key) = (c.card.clone(), c.key.clone());
    Rc::new(move || match &*card {
        Card::Approval(spec) => approval::card(spec.clone(), key.clone()).into_element(),
        Card::Choice(spec) => choice::card(spec.clone(), key.clone()).into_element(),
        Card::UserForm(spec) => user_form::card(spec, &key).into_element(),
        Card::SaveLogin(spec) => save_login::card(spec, &key).into_element(),
        Card::PluginNeeds(spec) => plugin_needs::card(spec.clone(), key.clone(), turn).into_element(),
    })
}

/// Rox's card box: the agent column's 560 width, `CARD` on a hairline.
pub(crate) fn shell(id: &str) -> View {
    let v = view()
        .w_full()
        .max_w(px(560.0))
        .gap_2()
        .p_3()
        .bg(theme::CARD)
        .rounded(px(16.0))
        .border(px(0.5))
        .border_color(theme::BORDER_S);
    if id.is_empty() { v } else { v.test_id(id) }
}

/// A card's title: 14 semibold.
pub(crate) fn title(s: String) -> Text {
    text(s).text_size(px(14.0)).line_height(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT)
}

/// A line under the title: 12, dim.
pub(crate) fn note(s: String) -> Text {
    text(s).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::DIM)
}

/// A failed answer's sentence.
pub(crate) fn error_line(s: String) -> Text {
    text(s).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::DOWN)
}

/// A command, in a mono box.
pub(crate) fn code_box(s: String) -> impl IntoElement {
    view().w_full().px(px(8.0)).py(px(6.0)).rounded(px(8.0)).bg(theme::SUBTLE).child(
        text(s).font_family("Menlo").text_size(px(12.0)).line_height(px(17.0)).text_color(theme::TEXT),
    )
}

/// What an answered card leaves: one centered line.
pub(crate) fn outcome_line(s: String) -> impl IntoElement {
    view()
        .w_full()
        .max_w(px(560.0))
        .py(px(6.0))
        .items_center()
        .child(text(s).text_size(px(13.0)).line_height(px(18.0)).text_color(theme::DIM))
}

/// A settled card's pill ("Dismissed", "Not answered").
pub(crate) fn pill(words: &str) -> impl IntoElement {
    text(words.to_owned())
        .text_size(px(11.0))
        .line_height(px(15.0))
        .text_color(theme::DIM)
        .bg(theme::HOVER)
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
}

/// A choice's letter in a key-shaped box: bright while its key answers.
pub(crate) fn keycap(letter: char, live: bool) -> impl IntoElement {
    view()
        .size(px(18.0))
        .rounded(px(4.0))
        .border(px(0.5))
        .border_color(theme::BORDER_S)
        .bg(theme::HOVER)
        .items_center()
        .justify_center()
        .child(
            text(letter.to_string())
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if live { theme::TEXT } else { theme::FAINT }),
        )
}

/// The ✕ at a card's top right.
pub(crate) fn close_x(what: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    view()
        .label(what)
        .size(px(22.0))
        .rounded(px(6.0))
        .items_center()
        .justify_center()
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || on())
        .child(text("✕").text_size(px(12.0)).text_color(theme::MUTED))
}

/// A card button's look.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Btn {
    /// Filled in the ink: the card's main answer.
    Primary,
    /// A faint wash.
    Secondary,
}

/// A 28-tall pill button.
pub(crate) fn btn(kind: Btn, label: &str, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    let (bg, bg_hov, ink) = match kind {
        Btn::Primary => (theme::TEXT, theme::DIM, theme::MAIN),
        Btn::Secondary => (theme::HOVER, theme::ACTIVE, theme::TEXT),
    };
    view()
        .label(label.to_owned())
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(move || if hov.get() { bg_hov } else { bg })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || on())
        .child(text(label.to_owned()).number_of_lines(1).text_size(px(12.5)).line_height(px(16.0)).text_color(ink))
}

// ---------------------------------------------------------------------------
// The dev hook (`hot.rs`: `card <path>`, `cards-clear`)
// ---------------------------------------------------------------------------

/// Reads a wire JSON file — one frame, an array of frames, or a recorded
/// fixture's `{…, body}` — and adds its cards to the open chat as a local,
/// unpersisted reply.
pub(crate) fn inject(path: &str) {
    let path = path.trim();
    let name = path.rsplit('/').next().unwrap_or(path);
    let v = match std::fs::read_to_string(path).map(|s| serde_json::from_str::<Value>(&s)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return crate::toast::show(format!("{name}: not JSON ({e})")),
        Err(e) => return crate::toast::show(format!("{name}: {e}")),
    };
    let v = match v.get("body") {
        Some(body) if v.get("status").is_some() => body.clone(),
        _ => v,
    };
    let cards = parse_list(&v);
    if cards.is_empty() {
        return crate::toast::show(format!("No cards in {name}"));
    }
    store().add_local_turn(cards);
}

/// Drops the dev hook's turns and every card's local state.
pub(crate) fn clear_local() {
    let st = store();
    st.drop_local_turns();
    batch(|| {
        st.card_states.set(Default::default());
        st.card_picks.set(Default::default());
        st.card_remember.set(Default::default());
    });
}

#[cfg(test)]
mod tests;
