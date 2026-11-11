//! The user-form card: an agent asks the person to fill a form — a sign-in,
//! a code, a few answers — and the values go to the server by REST, never
//! through chat text. Ported from nativechat `src/opengrok/user_form.rs`
//! (wire) and `src/components/user_form.rs` (card).
//!
//! # Two channels, one card
//!
//! 1. Live: CUSTOM `run-awaiting-approval` with `reason:"user-form"` (tool
//!    `request_user_form`), the form in `arguments` (alias `formRequest`),
//!    the gateway card id in top-level `entryId`, the tool call in `callId`.
//! 2. Replay: the gateway `send-message` envelope (`id` = entry id,
//!    `message.type:"user-form"`, `message.formRequest`, a sibling
//!    `formResolution` once settled), bare or as the `value` of a CUSTOM
//!    `user-form` frame.
//!
//! Both parse with [`UserFormSpec::from_wire`]; [`merge_into`] folds them
//! into one card by `entryId`, else `callId`. Only `entryId` is ever POSTed
//! — never `callId`, never an AG-UI event id: a card without one can be
//! dismissed locally but not sent.
//!
//! # Secrets
//!
//! A field is secret when the wire says `secret:true` or its type is
//! `password`/`otp`. Secret fields are masked, never prefilled, and what
//! is typed into them lives only in this module's view state ([`Local`])
//! until Continue folds it into the REST body — then it is cleared. A
//! saved login's secret is read from the Keychain only after Touch ID
//! (`vault::Vault::unlock`), held here unpainted ([`Held`]), and goes into
//! `values` only inside the submit body (`savedLogin:true, savedLoginId`).
//! Touch ID is asked on every use: nothing read is kept past one submit.
//!
//! # Whose logins
//!
//! A card offers and lists only the saved logins shared with its own Bot
//! (the notice's `bot`), or every login with share-all on, and only those
//! saved for its site ([`vault::Vault::offered_on`]). A pick goes by the row's
//! id, and the vault checks that id again before the broker is asked or
//! Touch ID shows, and once more before the submit leaves
//! ([`vault::Vault::usable`]): a login that isn't shared with this Bot, or is
//! for another site, is refused with nothing sent. The broker, which can't
//! see the vault, decides the other half: that this is the Bot's own
//! computer (its `shared-computer` refusal).
//! That body is sent only to a broker that has just proven it holds the
//! host token (`transport::submit_user_form`, `POST /v1/host/prove`);
//! otherwise it stays in the app and the card says so.
//! Nothing secret is put on a turn, in the store, in a log line or in a
//! `Debug` print: [`Values`] and [`Field`] redact theirs.
//!
//! # The broker's card
//!
//! The broker posts the card as a `user_form` notice in the chat
//! ([`UserFormSpec::from_notice`]): `{id, bot, botName, chat, form, state,
//! note?, outcomes?, takeoverNote}`. It types the values into the Bot's own
//! browser itself; the app only sends them.
//!
//! # Answers
//!
//! `POST /v1/host/computer/login/submit {chat, id, values, savedLogin?,
//! savedLoginId?}` (to a broker that has just proven itself, typed or
//! saved) and `POST /v1/host/computer/login/dismiss {chat, id, mode}`
//! (`dismissed`, or `escalated` for Open the screen, the hand-over), sent
//! and read by `cards/transport.rs`. A saved login is first asked about
//! (`POST /v1/host/computer/login/saved`), so a refusal never follows a
//! fingerprint. A missing route or no broker is a failure the card shows,
//! with nothing typed; never an answer. A typed login is held for the
//! "Save this login?" offer only until the card settles: past an offer the
//! broker made, it's dropped (and on a dismiss or a failure). Where the
//! card stands (sending, settled, a failure's sentence) is the cards'
//! shared state by key (`cards::state_of`), so the composer's "Waiting for
//! you" and a new message's supersede see it; what is typed stays here.

use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::transport::{self, UserFormReply};
use super::{CardState, answerable, close_x, error_line, note, save_login, set_state, state_of, title};
use crate::live::store;
use crate::theme;
use crate::vault::{self, PendingSave, Secret, SiteLogin, SiteMatch, Unlock, off_main, site_match, site_origin, vault};

pub(crate) const RUN_AWAITING_APPROVAL: &str = "run-awaiting-approval";
/// The `reason` of a park that waits on a form, not on a yes.
pub(crate) const USER_FORM_REASON: &str = "user-form";
/// Settled-replay CUSTOM names (after [`normalize_name`]).
const USER_FORM_CUSTOM_NAMES: &[&str] = &["user-form", "form-request", "form-resolution", "request-user-form"];

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Field types; anything else is text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKind {
    Email,
    Tel,
    Password,
    Otp,
    Number,
    Date,
    Select,
    Textarea,
    Checkbox,
    Text,
}

impl FieldKind {
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "email" => Self::Email,
            "tel" | "phone" => Self::Tel,
            "password" => Self::Password,
            "otp" => Self::Otp,
            "number" => Self::Number,
            "date" => Self::Date,
            "select" => Self::Select,
            "textarea" => Self::Textarea,
            "checkbox" => Self::Checkbox,
            _ => Self::Text,
        }
    }

    /// `password` and `otp` are secret even without `secret:true`.
    pub(crate) fn is_secret_kind(self) -> bool {
        matches!(self, Self::Password | Self::Otp)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectOption {
    pub value: String,
    pub label: String,
}

/// One field of the form. What is typed into it is not here: that lives
/// in the card's view state only.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
    /// `secret:true` on the wire, or a password/otp type.
    pub secret: bool,
    pub options: Vec<SelectOption>,
    pub placeholder: Option<String>,
    /// A non-secret default only: a secret field's `value`/`default` is
    /// dropped at parse.
    pub prefill: Option<String>,
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Field")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("kind", &self.kind)
            .field("required", &self.required)
            .field("secret", &self.secret)
            .field("options", &self.options)
            .field("placeholder", &self.placeholder)
            .field("prefill", &self.prefill.as_ref().map(|_| if self.masked() { "<redacted>" } else { "<set>" }))
            .finish()
    }
}

impl Field {
    pub(crate) fn masked(&self) -> bool {
        self.secret || self.kind.is_secret_kind()
    }

    /// A checkbox's wire value.
    pub(crate) fn checkbox_wire(checked: bool) -> &'static str {
        if checked { "true" } else { "false" }
    }
}

/// How a form ended (`formResolution`), plus the client-only Sending and
/// Superseded, and Timed out (`timedOut:true` beside a dismiss).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormResolution {
    Sending,
    Submitted,
    /// Also the paint of a 200 with no resolution: "Not filled".
    FillFailed,
    /// Sent to the computer screen to finish there (`mode:"escalated"`).
    Escalated,
    Dismissed,
    Skipped,
    TimedOut,
    /// A later message moved the chat on.
    Superseded,
    /// The answer didn't say how it went: still being typed, or answered already elsewhere.
    Unsure,
}

impl FormResolution {
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "sending" | "submitting" => Self::Sending,
            "submitted" => Self::Submitted,
            "fill_failed" | "fill-failed" | "not_filled" | "not-filled" => Self::FillFailed,
            "escalated" | "on_screen" | "on-screen" | "on_the_computer" | "on-the-computer" => Self::Escalated,
            "skipped" | "skip" => Self::Skipped,
            "timed_out" | "timed-out" => Self::TimedOut,
            "superseded" => Self::Superseded,
            "unsure" => Self::Unsure,
            _ => Self::Dismissed,
        }
    }

    /// The word the cards' shared state keeps (`CardState::Answered`).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Sending => "sending",
            Self::Submitted => "submitted",
            Self::FillFailed => "fill_failed",
            Self::Escalated => "escalated",
            Self::Dismissed => "dismissed",
            Self::Skipped => "skipped",
            Self::TimedOut => "timed_out",
            Self::Superseded => "superseded",
            Self::Unsure => "unsure",
        }
    }
}

/// How a broker's card's fill went, as its notice states it: the receipt is made of these, and
/// the broker's note shows beside it in its own words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The broker's sentence (a press left to the person, a reason).
    pub note: Option<String>,
    /// Not filled in full, but a value may have gone in (`uncertain`).
    pub uncertain: bool,
    /// That value was a secret's (`secretMaybe`).
    pub secret_maybe: bool,
    /// Return was pressed (`pressed`); `None` when the notice doesn't say.
    pub pressed: Option<bool>,
    /// Each field and whether it went in (`outcomes`), by id: never a value.
    pub outcomes: Vec<(String, bool)>,
}

impl Facts {
    pub(crate) fn of(n: &Value) -> Self {
        Self {
            note: n.get("note").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned),
            uncertain: n.get("uncertain").and_then(Value::as_bool).unwrap_or(false),
            secret_maybe: n.get("secretMaybe").and_then(Value::as_bool).unwrap_or(false),
            pressed: n.get("pressed").and_then(Value::as_bool),
            outcomes: n
                .get("outcomes")
                .and_then(Value::as_array)
                .map(|o| o.iter().filter_map(|o| Some((o.get("id")?.as_str()?.to_owned(), o.get("filled")?.as_bool()?))).collect())
                .unwrap_or_default(),
        }
    }

    /// Nothing but defaults: the notice said nothing of the fill.
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn unsure(&self) -> Unsure {
        match (self.uncertain, self.secret_maybe) {
            (false, _) => Unsure::No,
            (true, false) => Unsure::Value,
            (true, true) => Unsure::Secret,
        }
    }
}

/// A user-form card as the wire describes it. No typed value is ever on it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct UserFormSpec {
    /// The gateway card id (`e_…`): the only id the REST routes take.
    /// Empty when the frame had none.
    pub entry_id: String,
    /// The tool call: for merging frames only, never POSTed.
    pub call_id: String,
    pub run_id: String,
    /// The agent (coworker) the form is for, when the frame names it.
    pub agent_id: String,
    pub title: String,
    pub instruction: Option<String>,
    pub fields: Vec<Field>,
    pub domain: Option<String>,
    pub live_host: Option<String>,
    pub same_page: bool,
    pub submit: bool,
    /// `password`, `otp`, `passkey`, `captcha`, `outside_sandbox`.
    pub challenge_kind: Option<String>,
    /// `use` or `register`, with a passkey card.
    pub passkey_mode: Option<String>,
    pub resolution: Option<FormResolution>,
    pub widget_dismissed: bool,
    /// The chat the broker's card is in (its `chat`); "" for a wire frame.
    pub chat: String,
    pub bot_name: String,
    /// What the broker's card says of how its fill went ([`Facts`]).
    pub facts: Facts,
    /// What Open the screen hands over, as the broker words it.
    pub takeover_note: Option<String>,
    /// What the broker will do with Return (`always`, `never`, `maybe`; its `returnPress`).
    pub return_press: Option<String>,
}

impl UserFormSpec {
    /// The broker's `user_form` notice; `None` for any other.
    pub(crate) fn from_notice(n: &Value) -> Option<Self> {
        let s = |k: &str| n.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_owned();
        if s("kind") != "user_form" || s("id").is_empty() {
            return None;
        }
        // a reply's notice may carry its ending alone, with no form
        let mut spec = Self::parse(&json!({ "formRequest": n.get("form").cloned().unwrap_or(Value::Null) }), Some(s("id"))).unwrap_or_default();
        spec.entry_id = s("id");
        if spec.title.is_empty() {
            spec.title = s("title");
        }
        spec.agent_id = s("bot");
        spec.bot_name = s("botName");
        spec.chat = s("chat");
        spec.resolution = match s("state").as_str() {
            "" | "pending" => None,
            "expired" => Some(FormResolution::TimedOut),
            other => Some(FormResolution::parse(other)),
        };
        spec.facts = Facts::of(n);
        spec.takeover_note = Some(s("takeoverNote")).filter(|t| !t.is_empty());
        spec.return_press = Some(s("returnPress")).filter(|t| !t.is_empty());
        // a card's fields never carry a value from the broker: a login card has none
        for f in &mut spec.fields {
            f.prefill = None;
        }
        Some(spec)
    }

    /// A user form from any wire shape that carries one: the live park
    /// frame, the settled CUSTOM `user-form`, the bare `send-message`
    /// envelope, or a `{tool:"request_user_form", arguments}` call. Any
    /// other frame (an approval park included) is `None`.
    pub(crate) fn from_wire(v: &Value) -> Option<Self> {
        if is_user_form_awaiting(v) {
            return Self::from_awaiting(v);
        }
        if string_field(v, "name").is_some_and(|n| n == RUN_AWAITING_APPROVAL) {
            // another reason: an approval card
            return None;
        }
        let name = string_field(v, "name").map(|n| normalize_name(&n)).unwrap_or_default();
        if USER_FORM_CUSTOM_NAMES.contains(&name.as_str()) {
            let value = v.get("value").filter(|x| x.is_object()).unwrap_or(v);
            let mut spec = Self::parse(value, gateway_entry_id(v, value))?;
            fill_ids(&mut spec, v);
            settle_timeout(&mut spec, &[v, value]);
            return Some(spec);
        }
        if v.get("kind").and_then(Value::as_str) == Some("send-message") && looks_like_user_form(v) {
            let mut spec = Self::parse(v, gateway_entry_id_in(v))?;
            fill_ids(&mut spec, v);
            settle_timeout(&mut spec, &[v]);
            return Some(spec);
        }
        let tool = string_field(v, "tool").or_else(|| string_field(v, "toolName")).unwrap_or_default();
        if normalize_name(&tool) == "request-user-form" {
            let args = arguments_object(v);
            let mut spec = Self::parse(&args, gateway_entry_id(v, &args))
                .or_else(|| Self::parse(&json!({ "formRequest": args }), None))?;
            fill_ids(&mut spec, v);
            return Some(spec);
        }
        None
    }

    /// The live park: paint from `arguments`, take `entryId` from the top
    /// level, and read any settlement a replay folded onto the same frame.
    fn from_awaiting(event: &Value) -> Option<Self> {
        let arguments = arguments_object(event);
        let value = event.get("value").unwrap_or(&Value::Null);
        let hint = gateway_entry_id(event, &arguments).or_else(|| gateway_entry_id(event, event));
        let mut spec = Self::parse(&arguments, hint.clone())
            .or_else(|| Self::parse(&json!({ "formRequest": arguments.clone() }), hint.clone()))
            .or_else(|| Self::parse(event, hint.clone()))
            .or_else(|| looks_like_user_form(value).then(|| Self::parse(value, hint.clone())).flatten())?;
        // A replayed park carries its entry's settlement beside the
        // arguments and in `value` — but only an entry of this call.
        let theirs = string_field(value, "callId").unwrap_or_default();
        let ours = string_field(event, "callId").unwrap_or_default();
        let same = theirs.is_empty() || ours.is_empty() || theirs == ours;
        let value = if same { value } else { &Value::Null };
        if spec.resolution.is_none() {
            spec.resolution = settlement(&[event, value]);
        }
        spec.widget_dismissed |= [event, value].iter().any(|s| bool_at(s, "widgetDismissed") == Some(true));
        settle_timeout(&mut spec, &[event, value]);
        fill_ids(&mut spec, event);
        Some(spec)
    }

    /// The form inside `value` (`formRequest`, `message.formRequest`, a
    /// `message` of type `user-form`, or `value` itself with `fields`).
    pub(crate) fn parse(value: &Value, entry_id: Option<String>) -> Option<Self> {
        let request = form_request_object(value);
        let fields = request
            .and_then(|r| r.get("fields"))
            .or_else(|| value.get("fields"))
            .map(parse_fields)
            .unwrap_or_default();
        let title = request.and_then(|r| string_field(r, "title")).or_else(|| string_field(value, "title")).unwrap_or_default();
        let resolution = settlement(&[value]);
        let widget_dismissed = bool_at(value, "widgetDismissed").unwrap_or(false);
        if fields.is_empty() && title.is_empty() && resolution.is_none() && !widget_dismissed {
            return None;
        }
        let pick = |key: &str| request.and_then(|r| string_field(r, key)).or_else(|| string_field(value, key));
        let flag = |key: &str| request.and_then(|r| bool_at(r, key)).or_else(|| bool_at(value, key)).unwrap_or(false);
        Some(Self {
            entry_id: entry_id.or_else(|| gateway_entry_id_in(value)).unwrap_or_default(),
            title,
            instruction: pick("instruction"),
            fields,
            domain: pick("domain"),
            live_host: pick("liveHost").or_else(|| pick("live_host")),
            same_page: flag("samePage"),
            submit: flag("submit"),
            challenge_kind: pick("challengeKind"),
            passkey_mode: pick("passkeyMode"),
            resolution,
            widget_dismissed,
            ..Self::default()
        })
    }

    /// Where the card is in the view: its entry id, else its call id.
    #[cfg(test)]
    pub(crate) fn card_key(&self) -> &str {
        if !self.entry_id.is_empty() {
            &self.entry_id
        } else if !self.call_id.is_empty() {
            &self.call_id
        } else {
            "user-form"
        }
    }

    pub(crate) fn has_entry_id(&self) -> bool {
        !self.entry_id.is_empty()
    }

    /// Two frames of one card: the same entry id, else the same call id.
    /// Two cards that each carry their own entry id are siblings.
    #[cfg(test)]
    pub(crate) fn same_card(&self, other: &Self) -> bool {
        if self.has_entry_id() && other.has_entry_id() {
            return self.entry_id == other.entry_id;
        }
        !self.call_id.is_empty() && self.call_id == other.call_id
    }

    /// Folds a later frame of the same card onto this one: ids it lacked,
    /// a settlement, fields it adds. A field the card shows is never
    /// dropped by a narrower envelope.
    #[cfg(test)]
    pub(crate) fn merge(&mut self, incoming: UserFormSpec) {
        if incoming.has_entry_id() {
            self.entry_id = incoming.entry_id;
        }
        for (mine, theirs) in [
            (&mut self.call_id, incoming.call_id),
            (&mut self.run_id, incoming.run_id),
            (&mut self.agent_id, incoming.agent_id),
        ] {
            if !theirs.is_empty() {
                *mine = theirs;
            }
        }
        if !incoming.title.is_empty() {
            self.title = incoming.title;
        }
        for field in incoming.fields {
            match self.fields.iter_mut().find(|f| f.id == field.id) {
                Some(mine) => {
                    let prefill = field.prefill.clone().or_else(|| mine.prefill.take());
                    *mine = field;
                    mine.prefill = prefill;
                }
                None => self.fields.push(field),
            }
        }
        for (mine, theirs) in [
            (&mut self.instruction, incoming.instruction),
            (&mut self.domain, incoming.domain),
            (&mut self.live_host, incoming.live_host),
            (&mut self.challenge_kind, incoming.challenge_kind),
            (&mut self.passkey_mode, incoming.passkey_mode),
        ] {
            if theirs.is_some() {
                *mine = theirs;
            }
        }
        self.same_page |= incoming.same_page;
        self.submit |= incoming.submit;
        self.widget_dismissed |= incoming.widget_dismissed;
        // a fill's own outcome is final; a softer word does not undo it
        let firm = |r: Option<FormResolution>| {
            matches!(r, Some(FormResolution::Submitted | FormResolution::FillFailed | FormResolution::Skipped))
        };
        if incoming.resolution.is_some() && (firm(incoming.resolution) || !firm(self.resolution)) {
            self.resolution = incoming.resolution;
        }
    }

    /// How the wire says the card ended, if it did.
    pub(crate) fn effective_resolution(&self) -> Option<FormResolution> {
        self.resolution.or_else(|| self.widget_dismissed.then_some(FormResolution::Dismissed))
    }

    /// "Log in" for a one-page login (a name beside a password that
    /// submits), "Continue" otherwise.
    pub(crate) fn continue_label(&self) -> &'static str {
        let has_password = self.fields.iter().any(|f| f.kind == FieldKind::Password);
        let has_name = self.fields.iter().any(|f| matches!(f.kind, FieldKind::Email | FieldKind::Text | FieldKind::Tel));
        match self.challenge_kind.as_deref() {
            Some("passkey") if self.passkey_mode.as_deref() == Some("register") => "Create passkey",
            Some("passkey") => "Use passkey",
            _ if has_password && has_name && (self.same_page || self.submit) => "Log in",
            _ => "Continue",
        }
    }

    /// Every required field filled; `held` (a saved login's secret held
    /// for the submit) counts as filled.
    pub(crate) fn required_filled(&self, values: &Values, held: &[&str]) -> bool {
        self.fields.iter().filter(|f| f.required).all(|f| held.contains(&f.id.as_str()) || values.filled(f))
    }

    fn field(&self, id: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }
}

/// Folds `incoming` into the card it belongs to in `cards`, or appends it.
#[cfg(test)]
pub(crate) fn merge_into(cards: &mut Vec<UserFormSpec>, incoming: UserFormSpec) {
    match cards.iter_mut().find(|c| c.same_card(&incoming)) {
        Some(card) => card.merge(incoming),
        None => cards.push(incoming),
    }
}

/// Values typed on a card, by field id. View state only: never on a turn,
/// in the store or in a log. `Debug` prints the count.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Values(pub BTreeMap<String, String>);

impl std::fmt::Debug for Values {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Values").field("fields", &self.0.len()).finish()
    }
}

impl Values {
    pub(crate) fn get(&self, id: &str) -> &str {
        self.0.get(id).map(String::as_str).unwrap_or("")
    }

    pub(crate) fn filled(&self, field: &Field) -> bool {
        let raw = self.get(&field.id);
        match field.kind {
            FieldKind::Checkbox => raw == "true",
            _ => !raw.trim().is_empty(),
        }
    }

    /// The values as the submit body's `values` map.
    pub(crate) fn into_map(self) -> serde_json::Map<String, Value> {
        self.0.into_iter().map(|(k, v)| (k, Value::String(v))).collect()
    }
}

/// The ending a settled reply's body states (the card's entry, bare or
/// as its `value`). A body that carries only a Computer handoff is the
/// form sent to the screen.
pub(crate) fn settled_from(body: &Value) -> FormResolution {
    if let Some(r) = body.get("notice").and_then(UserFormSpec::from_notice).and_then(|s| s.resolution) {
        return r;
    }
    [body, body.get("value").unwrap_or(&Value::Null)]
        .into_iter()
        .find_map(|v| UserFormSpec::parse(v, None).and_then(|s| s.effective_resolution()))
        .unwrap_or(FormResolution::Escalated)
}

// ---------------------------------------------------------------------------
// Parse helpers
// ---------------------------------------------------------------------------

fn is_user_form_awaiting(event: &Value) -> bool {
    let inner = event.get("value").unwrap_or(&Value::Null);
    let name = string_field(event, "name").or_else(|| string_field(inner, "name")).unwrap_or_default();
    if name != RUN_AWAITING_APPROVAL {
        return false;
    }
    let reason = string_field(event, "reason").or_else(|| string_field(inner, "reason")).unwrap_or_default();
    if reason.eq_ignore_ascii_case(USER_FORM_REASON) {
        return true;
    }
    let tool = string_field(event, "tool").or_else(|| string_field(inner, "tool")).unwrap_or_default();
    matches!(normalize_name(&tool).as_str(), "request-user-form" | "user-form")
}

fn looks_like_user_form(v: &Value) -> bool {
    if v.get("formRequest").is_some() || v.get("formResolution").is_some() {
        return true;
    }
    let message = v.get("message");
    message.and_then(|m| m.get("type")).and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("user-form"))
        || message.and_then(|m| m.get("formRequest")).is_some()
}

fn form_request_object(v: &Value) -> Option<&Value> {
    v.get("formRequest")
        .or_else(|| v.get("message").and_then(|m| m.get("formRequest")))
        .or_else(|| {
            let m = v.get("message")?;
            m.get("type").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("user-form")).then_some(m)
        })
}

/// The gateway card id: a top-level `entryId`, or one in `value`, or the
/// `id` of a `send-message` envelope. Never a `callId` or an event `id`.
fn gateway_entry_id(event: &Value, value: &Value) -> Option<String> {
    string_field(event, "entryId")
        .or_else(|| gateway_entry_id_in(value))
        .or_else(|| event.get("value").and_then(gateway_entry_id_in))
}

fn gateway_entry_id_in(v: &Value) -> Option<String> {
    string_field(v, "entryId")
        .or_else(|| (v.get("kind").and_then(Value::as_str) == Some("send-message")).then(|| string_field(v, "id")).flatten())
}

/// Run, call and agent ids from the frame (or its `value`).
fn fill_ids(spec: &mut UserFormSpec, event: &Value) {
    let inner = event.get("value").unwrap_or(&Value::Null);
    let first = |keys: &[&str]| {
        keys.iter().find_map(|k| string_field(event, k)).or_else(|| keys.iter().find_map(|k| string_field(inner, k)))
    };
    if spec.run_id.is_empty() {
        spec.run_id = first(&["runId"]).unwrap_or_default();
    }
    if spec.call_id.is_empty() {
        spec.call_id = first(&["callId", "toolCallId"]).unwrap_or_default();
    }
    if spec.agent_id.is_empty() {
        spec.agent_id = first(&["agentId", "coworkerId"])
            .or_else(|| {
                // a coworker's own thread is named for it: `cw_…`, or
                // `gateway-cw_…` for its gateway transcript
                let t = first(&["threadId"])?;
                let t = t.strip_prefix("gateway-").unwrap_or(&t);
                t.starts_with("cw_").then(|| t.to_owned())
            })
            .unwrap_or_default();
    }
}

fn arguments_object(event: &Value) -> Value {
    let raw = event
        .get("arguments")
        .or_else(|| event.get("value").and_then(|v| v.get("arguments")))
        .or_else(|| event.get("formRequest"))
        .cloned()
        .unwrap_or(Value::Null);
    match raw.as_str() {
        Some(s) => serde_json::from_str(s).unwrap_or(raw),
        None => raw,
    }
}

/// The settlement the sources state: `formResolution` (a word, or an
/// object with `status`), with a computer handoff's `boxResolution`
/// deciding an escalated form — handed back is Dismissed, declined or
/// timed out is Skipped.
fn settlement(sources: &[&Value]) -> Option<FormResolution> {
    let said = sources.iter().find_map(|s| parse_resolution(s))?;
    if said == FormResolution::Escalated
        && let Some(word) = sources.iter().find_map(|s| string_field(s, "boxResolution"))
    {
        return Some(match word.to_ascii_lowercase().as_str() {
            "handed_back" => FormResolution::Dismissed,
            _ => FormResolution::Skipped,
        });
    }
    Some(said)
}

/// `timedOut:true` beside a dismissal or a skip reads as Timed out.
fn settle_timeout(spec: &mut UserFormSpec, sources: &[&Value]) {
    if sources.iter().any(|s| bool_at(s, "timedOut") == Some(true))
        && matches!(spec.effective_resolution(), Some(FormResolution::Dismissed | FormResolution::Skipped))
    {
        spec.resolution = Some(FormResolution::TimedOut);
    }
}

fn parse_resolution(v: &Value) -> Option<FormResolution> {
    let raw = v.get("formResolution").or_else(|| v.get("message").and_then(|m| m.get("formResolution")))?;
    if let Some(s) = raw.as_str() {
        return (!s.trim().is_empty()).then(|| FormResolution::parse(s));
    }
    raw.get("status").or_else(|| raw.get("state")).and_then(Value::as_str).map(FormResolution::parse)
}

fn parse_fields(fields: &Value) -> Vec<Field> {
    let Some(rows) = fields.as_array() else { return Vec::new() };
    rows.iter().enumerate().filter_map(|(i, row)| parse_field(row, i)).collect()
}

fn parse_field(row: &Value, index: usize) -> Option<Field> {
    let label = string_field(row, "label").or_else(|| string_field(row, "name"))?;
    let kind = row.get("type").and_then(Value::as_str).map(FieldKind::parse).unwrap_or(FieldKind::Text);
    let secret = bool_at(row, "secret").unwrap_or(false) || kind.is_secret_kind();
    let options = row
        .get("options")
        .and_then(Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|o| {
                    if let Some(s) = o.as_str() {
                        return Some(SelectOption { value: s.to_owned(), label: s.to_owned() });
                    }
                    let value = string_field(o, "value").or_else(|| string_field(o, "id"))?;
                    let label = string_field(o, "label").unwrap_or_else(|| value.clone());
                    Some(SelectOption { value, label })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Field {
        id: string_field(row, "id").unwrap_or_else(|| format!("field-{index}")),
        label,
        kind,
        required: bool_at(row, "required").unwrap_or(false),
        secret,
        options,
        placeholder: string_field(row, "placeholder"),
        // never a secret's: a model-smuggled password must not paint
        prefill: if secret { None } else { string_field(row, "value").or_else(|| string_field(row, "default")) },
    })
}

fn string_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn bool_at(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(|x| x.as_bool().or_else(|| x.as_str().map(|s| s.eq_ignore_ascii_case("true") || s == "1")))
}

fn normalize_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

// ---------------------------------------------------------------------------
// Saved logins: what a card can take from the vault
// ---------------------------------------------------------------------------

/// What a card can take from the vault (nativechat `CardTarget`): a whole
/// login (a name beside one password), the name page or the password page
/// of a two-step sign-in, or a code. Passkeys are not offered here: their
/// key lives on a server Rox does not have yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CardTarget {
    Login { username_id: String, password_id: String },
    Username { username_id: String },
    Password { password_id: String },
    Code { code_id: String },
}

impl CardTarget {
    pub(crate) fn name_field(&self) -> Option<&str> {
        match self {
            Self::Login { username_id, .. } | Self::Username { username_id } => Some(username_id),
            _ => None,
        }
    }

    /// Where a pick's secret goes: never painted, held for the submit.
    pub(crate) fn secret_field(&self) -> Option<&str> {
        match self {
            Self::Login { password_id, .. } | Self::Password { password_id } => Some(password_id),
            Self::Code { code_id } => Some(code_id),
            Self::Username { .. } => None,
        }
    }

    /// The vault kind it lists.
    pub(crate) fn kind(&self) -> &'static str {
        if matches!(self, Self::Code { .. }) { vault::KIND_CODE } else { vault::KIND_PASSWORD }
    }
}

pub(crate) fn card_target(spec: &UserFormSpec) -> Option<CardTarget> {
    if spec.challenge_kind.as_deref() == Some("passkey") {
        return None;
    }
    let of = |kind: FieldKind| spec.fields.iter().filter(|f| f.kind == kind).collect::<Vec<_>>();
    let (codes, passwords) = (of(FieldKind::Otp), of(FieldKind::Password));
    if let ([password], Some(username_id)) = (passwords.as_slice(), name_field_in(spec)) {
        return Some(CardTarget::Login { username_id, password_id: password.id.clone() });
    }
    // a code, a captcha or a page outside the box is no page of a sign-in
    let other = matches!(spec.challenge_kind.as_deref(), Some("otp" | "captcha" | "outside_sandbox"));
    match (codes.as_slice(), passwords.as_slice()) {
        ([code], []) => Some(CardTarget::Code { code_id: code.id.clone() }),
        ([], [password]) if !other => Some(CardTarget::Password { password_id: password.id.clone() }),
        ([], []) if !other && name_candidates(spec).count() == 1 => {
            name_field_in(spec).map(|username_id| CardTarget::Username { username_id })
        }
        // two codes, a code beside a password, two passwords (a sign-up)
        _ => None,
    }
}

fn name_candidates(spec: &UserFormSpec) -> impl Iterator<Item = &Field> {
    spec.fields
        .iter()
        .filter(|f| matches!(f.kind, FieldKind::Email | FieldKind::Text | FieldKind::Tel) && !f.masked())
}

/// The name field: an email field, else one named like one, else the first
/// plain text or phone field.
fn name_field_in(spec: &UserFormSpec) -> Option<String> {
    let (mut email, mut named, mut first) = (None, None, None);
    for f in name_candidates(spec) {
        if f.kind == FieldKind::Email && email.is_none() {
            email = Some(f.id.clone());
        }
        let key = format!("{} {}", f.id, f.label).to_ascii_lowercase();
        if named.is_none() && ["email", "user", "login", "phone"].iter().any(|w| key.contains(w)) {
            named = Some(f.id.clone());
        }
        if first.is_none() {
            first = Some(f.id.clone());
        }
    }
    email.or(named).or(first)
}

/// The site a card's logins are filed under: the exact site
/// (`vault::site_origin`) of its `domain`, else of its `liveHost`.
pub(crate) fn login_origin(spec: &UserFormSpec) -> Option<String> {
    spec.domain
        .as_deref()
        .and_then(site_origin)
        .or_else(|| spec.live_host.as_deref().and_then(site_origin))
}

/// The page the card says it is on (`liveHost`), when its site is not
/// the one its logins are filed under (an agent's `domain` naming another).
fn other_page(spec: &UserFormSpec) -> Option<String> {
    let page = spec.live_host.as_deref().and_then(site_origin)?;
    (login_origin(spec).as_ref() != Some(&page)).then_some(page)
}

/// The site whose saved logins a card offers: its page's when its
/// `domain` names another site (`other_page`), else [`login_origin`].
pub(crate) fn offered_site(spec: &UserFormSpec) -> Option<String> {
    other_page(spec).or_else(|| login_origin(spec))
}

/// The site a card's `domain` names when its page is on another: its
/// logins are never offered on their own, only listed once the person
/// asks for them, beside the mismatch.
pub(crate) fn asked_site(spec: &UserFormSpec) -> Option<String> {
    other_page(spec).and_then(|_| login_origin(spec))
}

/// The rows a card lists: the page's, and the `domain`'s only once the
/// person asked for them (`shown`), each row once.
fn listed(rows: &[SiteLogin], asked: &[SiteLogin], shown: bool) -> Vec<SiteLogin> {
    let mut out = rows.to_vec();
    if shown {
        out.extend(asked.iter().filter(|a| !rows.iter().any(|r| r.id == a.id)).cloned());
    }
    out
}

/// The Touch ID reason for using `row` on a form for `site`: both sites
/// when they differ, so the sheet shows a mismatch too.
fn use_reason(row: &SiteLogin, site: &str) -> String {
    if site_match(&row.origin, true, site) == SiteMatch::Exact {
        format!("use the saved login for {} as {}", row.origin, row.username)
    } else {
        format!("use the login saved for {} as {} on {site}", row.origin, row.username)
    }
}

/// A typed login worth offering to save: a site, a name, and a password.
pub(crate) fn save_candidate(spec: &UserFormSpec, values: &Values) -> Option<PendingSave> {
    let origin = login_origin(spec)?;
    let password = spec
        .fields
        .iter()
        .filter(|f| f.kind == FieldKind::Password)
        .map(|f| values.get(&f.id).trim())
        .find(|v| !v.is_empty())?;
    let username = name_field_in(spec).map(|id| values.get(&id).trim().to_owned()).filter(|u| !u.is_empty())?;
    Some(PendingSave { origin, username, password: Secret::new(password), bot: spec.agent_id.clone() })
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

/// Where the card stands, read from the cards' shared state by its key
/// (`cards::state_of`): open, sending, or settled with an ending.
#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    Sending,
    Settled(FormResolution),
}

/// The card's phase (tracked) and, while open, its last failure's sentence.
fn phase(key: &str) -> (Phase, String) {
    match state_of(key) {
        None => (Phase::Idle, String::new()),
        Some(CardState::Error(why)) => (Phase::Idle, why),
        Some(CardState::Sending) => (Phase::Sending, String::new()),
        Some(CardState::Answered(word)) => (Phase::Settled(FormResolution::parse(&word)), String::new()),
        Some(CardState::Dismissed) => (Phase::Settled(FormResolution::Dismissed), String::new()),
        Some(CardState::Superseded) => (Phase::Settled(FormResolution::Superseded), String::new()),
    }
}

fn settle(key: &str, r: FormResolution) {
    set_state(key, CardState::Answered(r.as_str().to_owned()));
}

/// Open again: a restored dismiss, a retry.
fn reopen(key: &str) {
    store().card_states.update(|m| {
        m.remove(key);
    });
}

/// Where a saved-login pick is.
#[derive(Debug, Clone, PartialEq, Default)]
enum Pick {
    #[default]
    None,
    /// The Touch ID sheet is up.
    Confirming { username: String },
    /// The name is filled and the secret (if any) is held.
    Ready { username: String },
    /// It could not be used; the sentence says why.
    Unavailable(String),
}

/// One card's own view state, keyed by card key: what is typed, the pick.
/// Signals made outside any component, so they outlive a remount; the
/// registry is anchored, so a hot patch finds the same one.
#[derive(Clone, Copy)]
struct Local {
    values: Signal<Values>,
    pick: Signal<Pick>,
    /// The person chose to type the login by hand: no list.
    by_hand: Signal<bool>,
    /// The person asked to see the logins of the site the card's `domain`
    /// names, which is not its page's.
    show_asked: Signal<bool>,
    /// The broker offered to save the typed login.
    offer: Signal<bool>,
    /// What the broker's answer said of the fill, until its card says it.
    said: Signal<Facts>,
    /// The chat the broker's card is in.
    chat: Signal<String>,
    /// Try again on a card the wire settled as not filled.
    retry: Signal<bool>,
    /// What is on its way while the card is sending: a fill, the screen or a dismiss.
    going: Signal<Going>,
    /// Do it myself was pressed: the card asks once more before the screen is handed over.
    confirm: Signal<bool>,
}

/// What a sending card is waiting on, for its busy line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Going {
    #[default]
    Fill,
    Screen,
    Dismiss,
}

/// A saved login held for the submit. The secret is never painted and
/// leaves memory with the submit (or Type it by hand, or the card's end).
struct Held {
    login_id: String,
    /// The card's Bot and sites, for the check again at submit.
    bot: String,
    sites: Vec<String>,
    /// The field it fills; `None` on a name page.
    field: Option<String>,
    secret: Option<Secret>,
    /// The secret is a code seed: mint at submit.
    code: bool,
}

type Registry = RefCell<HashMap<String, Local>>;
type HeldMap = RefCell<HashMap<String, Held>>;

fn local(key: &str) -> Local {
    let at = ra_jit_live::anchor("rox.cards.user_form", || {
        Box::leak(Box::new(Registry::default())) as *const Registry as usize
    });
    // SAFETY: the anchor holds a leaked registry for the life of the
    // process; it is only touched on the UI thread.
    let reg = unsafe { &*(at as *const Registry) };
    if let Some(l) = reg.borrow().get(key) {
        return *l;
    }
    let l = Owner::detached().run(|| Local {
        values: signal(Values::default()),
        pick: signal(Pick::None),
        by_hand: signal(false),
        show_asked: signal(false),
        offer: signal(false),
        said: signal(Facts::default()),
        chat: signal(String::new()),
        retry: signal(false),
        going: signal(Going::Fill),
        confirm: signal(false),
    });
    reg.borrow_mut().insert(key.to_owned(), l);
    l
}

/// The chat of the broker's card under `key`, once it was drawn.
fn chat_of(key: &str) -> Option<String> {
    Some(local(key).chat.get_untracked()).filter(|c| !c.is_empty())
}

fn held() -> &'static HeldMap {
    let at = ra_jit_live::anchor("rox.cards.user_form.held", || {
        Box::leak(Box::new(HeldMap::default())) as *const HeldMap as usize
    });
    // SAFETY: as in `local`.
    unsafe { &*(at as *const HeldMap) }
}

fn drop_held(key: &str) {
    held().borrow_mut().remove(key);
}

/// Clears what was typed into secret fields.
fn forget_secrets(spec: &UserFormSpec, loc: Local) {
    let keep = |id: &String| spec.field(id).is_some_and(|f| !f.masked());
    if loc.values.with_untracked(|v| v.0.keys().any(|id| !keep(id))) {
        loc.values.update(|v| v.0.retain(|id, _| keep(id)));
    }
}


// ---------------------------------------------------------------------------
// Verbs
// ---------------------------------------------------------------------------

fn submit(spec: Rc<UserFormSpec>, key: String) {
    let loc = local(&key);
    if !spec.has_entry_id() || !answerable(&key) {
        return;
    }
    let held = held().borrow_mut().remove(&key);
    // the login is checked again by id: unshared, deleted or moved since the pick, nothing goes
    if let Some(h) = &held
        && let Err(no) = vault().usable(&h.login_id, &h.bot, &h.sites, h.code)
    {
        loc.pick.set(Pick::None);
        return set_state(&key, CardState::Error(no.sentence(&spec.bot_name)));
    }
    let typed = loc.values.get_untracked();
    let mut values = Values::default();
    for f in &spec.fields {
        let v = typed.get(&f.id);
        if f.kind == FieldKind::Checkbox {
            values.0.insert(f.id.clone(), Field::checkbox_wire(v == "true").to_owned());
        } else if !v.is_empty() {
            values.0.insert(f.id.clone(), v.to_owned());
        }
    }
    drop(typed);
    if held.is_none()
        && let Some(save) = save_candidate(&spec, &values)
    {
        vault().stash_pending(&spec.entry_id, save);
    }
    // what was typed into secret fields leaves the view state now
    forget_secrets(&spec, loc);
    loc.pick.set(Pick::None);
    loc.going.set(Going::Fill);
    set_state(&key, CardState::Sending);
    Owner::detached().run(|| {
        spawn(async move {
            let mut login_id = None;
            if let Some(h) = held {
                if let (Some(field), Some(secret)) = (h.field, h.secret) {
                    let v = if h.code {
                        match vault::totp::Totp::parse(secret.expose()) {
                            Ok(t) => {
                                // mint on a step with time left for the trip
                                let wait = t.wait_before_minting();
                                if wait > 0 {
                                    timer(std::time::Duration::from_secs(wait + 1)).await;
                                }
                                t.current().0
                            }
                            Err(e) => return set_state(&key, CardState::Error(e)),
                        }
                    } else {
                        secret.expose().to_owned()
                    };
                    values.0.insert(field, v);
                }
                login_id = Some(h.login_id);
            }
            // the values exist from here only inside the body, on its way out
            let body = transport::submit_body(&spec.chat, &spec.entry_id, values.into_map(), login_id.as_deref());
            // typed or saved, it goes only to a broker that proves itself
            let reply = transport::submit_user_form(body).await;
            let entry = spec.entry_id.as_str();
            match reply {
                UserFormReply::Settled(body) => {
                    let r = settled_from(&body);
                    if let Some(n) = body.get("notice") {
                        loc.said.set(Facts::of(n));
                    }
                    // the typed login is held past here only for the broker's offer to save it
                    let offered = r == FormResolution::Submitted && login_id.is_none() && body.get("offerSave").is_some_and(Value::is_object);
                    if offered && vault().has_pending(entry) {
                        loc.offer.set(true);
                    } else {
                        vault().take_pending(entry);
                    }
                    settle(&key, r);
                    if r == FormResolution::Submitted
                        && let Some(id) = &login_id
                    {
                        // a legacy row is this site's from its first use (not
                        // on a card whose `domain` and page disagree)
                        if other_page(&spec).is_none()
                            && let Some(site) = login_origin(&spec)
                        {
                            vault().pin_site(id, &site);
                        }
                        vault().mark_used(id);
                    }
                }
                UserFormReply::Refused(why) | UserFormReply::Failed(why) => {
                    vault().take_pending(entry);
                    set_state(&key, CardState::Error(why));
                }
                UserFormReply::Empty | UserFormReply::MissingEntry => {
                    vault().take_pending(entry);
                    settle(&key, FormResolution::FillFailed);
                }
                // it may have been typed: no second answer, the person looks
                UserFormReply::AlreadyAnswered | UserFormReply::StillWorking => {
                    vault().take_pending(entry);
                    settle(&key, FormResolution::Unsure);
                }
                UserFormReply::MissingRoute => {
                    vault().take_pending(entry);
                    set_state(&key, CardState::Error(NO_ROUTE.to_owned()));
                }
            }
        });
    });
}

/// What becomes of Return, as the broker's card says before anything is typed (its
/// `returnPress`): the broker decides it, and the card's words before the press follow it. With
/// no word (an older broker) the card promises nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReturnPress {
    /// Pressed, whichever fields are filled.
    Always,
    Never,
    /// It depends on which fields end up filled: the receipt says what happened.
    Maybe,
}

pub(crate) fn return_press(spec: &UserFormSpec) -> ReturnPress {
    match spec.return_press.as_deref() {
        Some("always") => ReturnPress::Always,
        Some("never") => ReturnPress::Never,
        _ => ReturnPress::Maybe,
    }
}

/// A missing route or no broker: nothing was typed, and the card says so.
pub(crate) const NO_ROUTE: &str = "Buwiz Bot's broker didn't take this, so nothing was typed. Make sure it's running and up to date, then try again.";

fn dismiss(spec: Rc<UserFormSpec>, key: String) {
    answer_card(spec, key, "dismissed");
}

/// Open the screen: the broker hands the Bot's screen to the person, as a
/// hand-over, and the screen opens here.
fn open_screen(spec: Rc<UserFormSpec>, key: String) {
    answer_card(spec, key, "escalated");
}

/// Dismiss or Open the screen. Nothing typed goes with either, and a typed
/// login held for a save offer is dropped.
fn answer_card(spec: Rc<UserFormSpec>, key: String, mode: &'static str) {
    let loc = local(&key);
    drop_held(&key);
    forget_secrets(&spec, loc);
    vault().take_pending(&spec.entry_id);
    loc.pick.set(Pick::None);
    if !spec.has_entry_id() || spec.chat.is_empty() {
        // a card with nothing to answer settles here
        return set_state(&key, CardState::Dismissed);
    }
    if !answerable(&key) {
        return;
    }
    loc.going.set(if mode == "escalated" { Going::Screen } else { Going::Dismiss });
    set_state(&key, CardState::Sending);
    let body = transport::dismiss_body(&spec.chat, &spec.entry_id, mode);
    Owner::detached().run(|| {
        spawn(async move {
            match transport::dismiss_user_form(body).await {
                UserFormReply::Settled(body) => {
                    let r = settled_from(&body);
                    settle(&key, r);
                    if r == FormResolution::Escalated {
                        crate::computer::open_full(&spec.agent_id, &spec.bot_name);
                    }
                }
                UserFormReply::Empty | UserFormReply::MissingEntry => set_state(&key, CardState::Dismissed),
                // the card was answered another way, or is still being typed: never "nothing was filled"
                UserFormReply::AlreadyAnswered | UserFormReply::StillWorking => settle(&key, FormResolution::Unsure),
                UserFormReply::MissingRoute => set_state(&key, CardState::Error(NO_ROUTE.to_owned())),
                UserFormReply::Refused(why) | UserFormReply::Failed(why) => set_state(&key, CardState::Error(why)),
            }
        });
    });
}

/// Try again after a fill that did not land: the fields come back, the
/// secrets empty (they left with the first submit).
fn retry(spec: &UserFormSpec, key: &str) {
    if spec.effective_resolution() == Some(FormResolution::FillFailed) {
        local(key).retry.set(true);
    }
    reopen(key);
}

/// The sites a card's saved logins may be for: its page's, and the one its
/// `domain` names when that is another (listed only once asked for).
pub(crate) fn card_sites(spec: &UserFormSpec) -> Vec<String> {
    offered_site(spec).into_iter().chain(asked_site(spec)).collect()
}

/// The saved login `login_id` on the card under `key`, for the card's Bot
/// `bot` (`bot_name` to say so): the vault checks first that it's shared
/// with that Bot and saved for one of `sites` (nothing is asked or sent
/// otherwise), then the broker says whether it would use one on this
/// computer, then Touch ID, every time. The pick: its name into the name
/// field, its secret held for the submit — never into a field, never on
/// screen.
pub(crate) fn use_saved(key: String, login_id: &str, target: CardTarget, site: String, sites: Vec<String>, bot: String, bot_name: &str) {
    let loc = local(&key);
    if matches!(loc.pick.get_untracked(), Pick::Confirming { .. }) {
        return;
    }
    let code = target.kind() == vault::KIND_CODE;
    let row = match vault().usable(login_id, &bot, &sites, code) {
        Ok(row) => row,
        Err(no) => return loc.pick.set(Pick::Unavailable(no.sentence(bot_name))),
    };
    loc.pick.set(Pick::Confirming { username: row.username.clone() });
    let reason = use_reason(&row, &site);
    let reads = target.secret_field().is_some();
    let r = row.clone();
    let (chat, entry) = (store().active_id.get_untracked(), key.clone());
    let card_chat = chat_of(&key).unwrap_or(chat);
    Owner::detached().run(|| {
        spawn(async move {
            // the broker says first whether it would use one here: a refusal never follows a fingerprint
            if let Err(why) = transport::saved_login_usable(&card_chat, &entry).await {
                return loc.pick.set(Pick::Unavailable(why));
            }
            let got = off_main(move || {
                if reads {
                    vault().unlock_as(&r, code, &reason)
                } else {
                    // the name page reads nothing, but a use is still the owner's
                    vault().confirm(&reason).map(|()| Unlock::Secret(Secret::new(""))).unwrap_or_else(|e| e)
                }
            })
            .await;
            let secret = match got {
                Some(Unlock::Secret(s)) => s,
                Some(Unlock::Missing) => {
                    let what = if target.kind() == vault::KIND_CODE { "code" } else { "password" };
                    return loc.pick.set(Pick::Unavailable(format!(
                        "This Mac's keychain has no {what} for {}. Type it by hand.",
                        row.username
                    )));
                }
                Some(Unlock::Refused(why)) => {
                    return loc.pick.set(Pick::Unavailable(format!("Touch ID didn't confirm: {why}")));
                }
                Some(Unlock::Cancelled) | None => return loc.pick.set(Pick::None),
            };
            if !answerable(&key) {
                // the card ended while the sheet was up: hold nothing
                return loc.pick.set(Pick::None);
            }
            if code && let Err(e) = vault::totp::Totp::parse(secret.expose()) {
                return loc.pick.set(Pick::Unavailable(e));
            }
            if let Some(name) = target.name_field() {
                let (name, user) = (name.to_owned(), row.username.clone());
                loc.values.update(|v| {
                    v.0.insert(name, user);
                });
            }
            held().borrow_mut().insert(
                key.clone(),
                Held {
                    login_id: row.id.clone(),
                    bot,
                    sites,
                    field: target.secret_field().map(str::to_owned),
                    secret: reads.then_some(secret),
                    code,
                },
            );
            loc.pick.set(Pick::Ready { username: row.username.clone() });
        });
    });
}

/// Drops a pick: the held secret goes, the fields are the person's again.
fn by_hand(key: &str) {
    let loc = local(key);
    drop_held(key);
    loc.pick.set(Pick::None);
    loc.by_hand.set(true);
}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------
//
// The broker's card reads as a sign-in sheet. Its header is Buwiz's own: a lock and the site the
// values are typed into (the broker's `domain`, the one fact a person must be able to check),
// never the AI's title. What the AI wrote shows under it as the Bot's words. The fields, then
// the answer: the primary fills (and says whether Return is pressed), the quiet "Do it myself"
// asks once more before the screen is handed over. How it works is behind the ⓘ, beside the ×.
// A settled card is one line: what happened, with a glyph.

/// The login card's widest: it hugs its content, as a message does.
pub(crate) const CARD_MAX_W: f32 = 420.0;
/// The ⓘ's popover's width.
const INFO_W: f32 = 320.0;
/// How long the pointer rests on the ⓘ before its popover peeks, and how long a peek waits for
/// the pointer to come back (on its way from the icon into the popover).
const PEEK_MS: u64 = 300;
const GRACE_MS: u64 = 250;
/// The ⓘ's label.
pub(crate) const INFO_LABEL: &str = "About this card";
/// The popover's label.
pub(crate) const INFO_PANEL: &str = "How this card works";
/// The quiet answer that hands the screen over, after one more press.
pub(crate) const DO_IT_MYSELF: &str = "Do it myself";
pub(crate) const OPEN_THE_SCREEN: &str = "Open the screen";
/// The broker's take-over note (`hub::tools::computer::TAKEOVER_NOTE`), which the popover and
/// the confirm step put in short lines. Any other note is shown in the broker's own words.
pub(crate) const TAKEOVER_NOTE: &str = "You'd use its own browser on its own screen. The AI isn't shown your screen and can't act through its tools until you hand back. Commands it ran can keep running, but they can't see this screen or what you type. The browser and desktop here are a different matter: pages it opened, and anything it set going by clicking on this screen, could. What we can find on the desktop is ended first, not always all of it. Don't type a real password unless you trust this Bot's work so far. Afterwards it can use whatever you signed in to.";

/// The card for `spec`, its view state under `key` (the card key).
pub(crate) fn card(spec: impl Borrow<UserFormSpec>, key: impl AsRef<str>) -> impl IntoElement {
    let spec = Rc::new(spec.borrow().clone());
    let key = key.as_ref().to_owned();
    let loc = local(&key);
    let k = key.clone();
    let ph = computed(move || phase(&k).0);
    dynamic(move || {
        // the wire's ending wins, but for a retry of a fill that failed
        let wire = spec.effective_resolution().filter(|r| !(*r == FormResolution::FillFailed && loc.retry.get()));
        match (wire, ph.get()) {
            (Some(r), _) => settled(spec.clone(), key.clone(), r).into_element(),
            (None, Phase::Settled(r)) => settled(spec.clone(), key.clone(), r).into_element(),
            (None, Phase::Sending) => busy(&spec, &key).into_element(),
            (None, Phase::Idle) => idle(spec.clone(), key.clone()).into_element(),
        }
    })
}

fn title_of(spec: &UserFormSpec) -> String {
    if spec.title.is_empty() { "Form".to_owned() } else { spec.title.clone() }
}

/// The broker's card: one it can answer, in a chat.
fn brokers(spec: &UserFormSpec) -> bool {
    spec.has_entry_id() && !spec.chat.is_empty()
}

/// The site the broker's card types into, as the broker states it (`domain`); `None` for a
/// card that isn't the broker's, whose site no one here checked.
pub(crate) fn site_of(spec: &UserFormSpec) -> Option<String> {
    spec.domain.as_deref().map(str::trim).filter(|d| brokers(spec) && !d.is_empty()).map(str::to_owned)
}

/// "Nova's screen", or "the Bot's screen" with no name.
fn screen_of(spec: &UserFormSpec) -> String {
    if spec.bot_name.is_empty() { "the Bot's screen".to_owned() } else { format!("{}'s screen", spec.bot_name) }
}

/// The header's words: Buwiz's, with the site, on the broker's card; the AI's title on another.
pub(crate) fn headline(spec: &UserFormSpec) -> String {
    match site_of(spec) {
        Some(site) => format!("Sign in on {site}"),
        None => title_of(spec),
    }
}

/// The primary answer's words. The broker's card fills the page, and says whether Return goes
/// with it: it never promises a sign-in. Another card keeps its own ("Log in", "Continue").
pub(crate) fn primary_label(spec: &UserFormSpec) -> &'static str {
    match () {
        _ if !brokers(spec) => spec.continue_label(),
        _ if return_press(spec) == ReturnPress::Always => "Fill and send",
        _ => "Fill in",
    }
}

/// The line under the broker's card's answers: where it's typed, whether Return is pressed, and
/// that nothing goes before the press.
pub(crate) fn caption(spec: &UserFormSpec) -> String {
    let label = primary_label(spec);
    let screen = screen_of(spec);
    match return_press(spec) {
        ReturnPress::Always => format!("Typed on {screen}, then Return is pressed. Nothing is typed until you press {label}."),
        ReturnPress::Never => format!("Typed on {screen}; sending is left to you. Nothing is typed until you press {label}."),
        ReturnPress::Maybe => format!("Typed on {screen}. If the form is complete, Return may be pressed to send it. Nothing is typed until you press {label}."),
    }
}

/// What the AI wrote on the card (its title and instruction), as its Bot's words: "Nova: …".
/// `None` when it says nothing the header doesn't (empty, or only "Sign in to <site>").
pub(crate) fn ai_line(spec: &UserFormSpec, site: &str) -> Option<String> {
    const FILLER: &[&str] = &["sign", "signin", "in", "to", "on", "into", "at", "log", "login", "the", "for", "please"];
    let site = site.to_ascii_lowercase();
    let says = |s: &str| {
        let s = s.to_lowercase().replace(&site, " ");
        s.split(|c: char| !c.is_alphanumeric()).any(|w| !w.is_empty() && !FILLER.contains(&w))
    };
    let (title, said) = (spec.title.trim(), spec.instruction.as_deref().unwrap_or("").trim());
    let stem = |s: &str| s.trim_end_matches(['.', '!', ':']).to_lowercase();
    let mut parts = Vec::new();
    if says(title) && !(says(said) && stem(said).starts_with(&stem(title))) {
        parts.push(title);
    }
    if says(said) {
        parts.push(said);
    }
    if parts.is_empty() {
        return None;
    }
    let words: Vec<String> = parts
        .into_iter()
        .map(|p| if p.ends_with(['.', '!', '?']) { p.to_owned() } else { format!("{p}.") })
        .collect();
    let bot = if spec.bot_name.is_empty() { "The Bot" } else { spec.bot_name.as_str() };
    Some(format!("{bot}: {}", words.join(" ")))
}

/// The caption while the card's saved logins are the choice (no primary shown yet).
pub(crate) fn choosing_caption(spec: &UserFormSpec) -> String {
    format!("Touch ID fills in the card. Nothing is typed on {} until you then press {}.", screen_of(spec), primary_label(spec))
}

/// What the ⓘ's popover says, under its headings: what the primary does and who types, then what
/// handing the screen over means. Nothing of the take-over note is left out; a note this build
/// doesn't know is shown in the broker's own words, a sentence a line.
pub(crate) fn guidance(spec: &UserFormSpec) -> Vec<(String, Vec<String>)> {
    let site = site_of(spec).unwrap_or_else(|| "the page".to_owned());
    let screen = screen_of(spec);
    let label = primary_label(spec);
    let mut out = vec![(
        label.to_owned(),
        vec![
            format!("Buwiz types it into {site} on {screen}, not the AI."),
            match return_press(spec) {
                ReturnPress::Always => "Then it presses Return to send it.",
                ReturnPress::Never => "It doesn't press Return: sending is left to you.",
                ReturnPress::Maybe => "Whether it presses Return depends on which fields are filled; the card says afterwards.",
            }
            .to_owned(),
            "The AI never sees a password or code. It's told the form was filled, and what went into fields that aren't secret, like an email.".to_owned(),
            format!("Nothing is typed until you press {label}."),
        ],
    )];
    if !brokers(spec) {
        return out;
    }
    // with no note from the broker, the note this build knows: as the confirm step says
    let note = spec.takeover_note.as_deref().unwrap_or(TAKEOVER_NOTE);
    let bot = if spec.bot_name.is_empty() { "the Bot".to_owned() } else { spec.bot_name.clone() };
    let lines = if note == TAKEOVER_NOTE {
        vec![
            format!("You use {bot}'s own browser on its own screen yourself."),
            "The AI isn't shown the screen and can't act through its tools until you hand back.".to_owned(),
            "Commands it ran can keep running, but they can't see the screen or what you type.".to_owned(),
            "Pages it opened, and anything it set going by clicking on this screen, could.".to_owned(),
            "What Buwiz can find on the desktop is ended first, not always all of it.".to_owned(),
            "Don't type a real password unless you trust this Bot's work so far.".to_owned(),
            "Afterwards it can use whatever you signed in to.".to_owned(),
        ]
    } else {
        sentences(note)
    };
    out.push((DO_IT_MYSELF.to_owned(), lines));
    out
}

/// The confirm step's words, before the screen is handed over: the short form of the take-over
/// note, or the broker's own note when it isn't the one this build knows.
pub(crate) fn confirm_words(spec: &UserFormSpec) -> String {
    match spec.takeover_note.as_deref() {
        Some(note) if note != TAKEOVER_NOTE => note.to_owned(),
        _ => {
            let bot = if spec.bot_name.is_empty() { "the Bot".to_owned() } else { spec.bot_name.clone() };
            format!("You'll use {bot}'s browser yourself. The AI can't see the screen or act until you hand back; then it can use whatever you signed in to. Pages it already opened can still see what you type: only use a real password if you trust this Bot's work so far.")
        }
    }
}

/// `s` a sentence a line.
fn sentences(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, c) in s.char_indices() {
        cur.push(c);
        let next = s[i + c.len_utf8()..].chars().next();
        if matches!(c, '.' | '!' | '?') && next.is_none_or(char::is_whitespace) {
            out.push(cur.trim().to_owned());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

/// The login cards' box: as wide as it needs up to [`CARD_MAX_W`], at the column's left.
pub(crate) fn login_shell(id: &str) -> View {
    let v = view()
        .w_full()
        .max_w(px(CARD_MAX_W))
        .gap(px(12.0))
        .p(px(16.0))
        .bg(theme::CARD)
        .rounded(px(16.0))
        .border(px(0.5))
        .border_color(theme::BORDER_S);
    if id.is_empty() { v } else { v.test_id(id) }
}

/// The header: a lock and the site on the broker's card (the AI's words below it, as its Bot's),
/// the title on another. The corner's buttons sit over its right end ([`corner`]).
fn header(spec: &UserFormSpec, corner_w: f32) -> View {
    let site = site_of(spec);
    let mut row = view().flex_row().items_center().gap(px(6.0));
    if site.is_some() {
        row = row.child(crate::icons::icon_ink("lock", 14.0, || theme::DIM));
    }
    row = row.child(title(headline(spec)).flex_1().min_w(px(0.0)).number_of_lines(1));
    let mut col = view().gap(px(4.0)).pr(px(corner_w)).child(row);
    let words = match &site {
        Some(site) => ai_line(spec, site),
        None => spec.instruction.clone().filter(|i| !i.is_empty()),
    };
    if let Some(w) = words {
        col = col.child(note(w).number_of_lines(2));
    }
    col
}

/// The header's buttons, last in the card so Tab reaches them after its answers, laid over the
/// header's right end: the ⓘ (on the broker's card) and the ×.
fn corner(spec: Rc<UserFormSpec>, key: String) -> View {
    let mut row = view().absolute().top(px(15.0)).right(px(12.0)).flex_row().items_center().gap(px(2.0));
    if brokers(&spec) {
        row = row.child(info_button(Rc::new(guidance(&spec))));
    }
    row.child(close_x("Dismiss", move || dismiss(spec.clone(), key.clone())))
}

/// The ⓘ: the pointer resting on it peeks its popover, the keyboard focus on it opens it, and a
/// press pins it open until another press, a press outside or Escape. The popover is an overlay
/// against the icon, so it takes no room in the card; a screen reader gets its words as the
/// button's hint, and as text while it's open.
fn info_button(lines: Rc<Vec<(String, Vec<String>)>>) -> impl IntoElement {
    // None: closed; Some(pinned)
    let open: Signal<Option<bool>> = signal(None);
    let over = signal(false);
    let hov = signal(false);
    // the focus is coming back to the icon from the popover's Escape: it doesn't open it again
    let quiet = signal(false);
    let anchor = element_ref();
    let owner = Owner::current();
    let close_later = move || {
        owner.run(|| {
            set_timeout(std::time::Duration::from_millis(GRACE_MS), move || {
                if !over.get_untracked() && open.get_untracked() == Some(false) {
                    open.set(None);
                }
            })
        });
    };
    let plain = lines.iter().map(|(h, ls)| format!("{h}: {}", ls.join(" "))).collect::<Vec<_>>().join(" ");
    let panel = lines.clone();
    view()
        .child(
            view()
                .role(AccessibilityRole::Button)
                .label(INFO_LABEL)
                .hint(SharedString::from(plain))
                .accessibility_state(move || AccessibilityState { expanded: Some(open.get().is_some()), has_popup: true, ..Default::default() })
                .size(px(22.0))
                .rounded(px(6.0))
                .items_center()
                .justify_center()
                .bg(move || if hov.get() || open.get().is_some() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .element_ref(&anchor)
                .on_mouse_enter(move |_: &MouseEvent| {
                    hov.set(true);
                    over.set(true);
                    owner.run(|| {
                        set_timeout(std::time::Duration::from_millis(PEEK_MS), move || {
                            if over.get_untracked() && open.get_untracked().is_none() {
                                open.set(Some(false));
                            }
                        })
                    });
                })
                .on_mouse_leave(move |_: &MouseEvent| {
                    hov.set(false);
                    over.set(false);
                    close_later();
                })
                .on_focus(move || {
                    if quiet.get_untracked() {
                        return quiet.set(false);
                    }
                    if open.get_untracked().is_none() {
                        open.set(Some(false));
                    }
                })
                .on_blur(move || {
                    quiet.set(false);
                    if open.get_untracked() == Some(false) {
                        open.set(None);
                    }
                })
                .on_press(move || open.set(if open.get_untracked() == Some(true) { None } else { Some(true) }))
                // an i in a ring
                .child(
                    view()
                        .size(px(14.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(theme::MUTED)
                        .items_center()
                        .justify_center()
                        .child(text("i").text_size(px(9.5)).line_height(px(11.0)).font_weight(FontWeight::BOLD).text_color(theme::MUTED)),
                ),
        )
        .child(when(move || open.get().is_some(), move || info_panel(&panel, &anchor, open, over, quiet, close_later).into_element()))
}

/// The ⓘ's popover: each heading and its short lines.
fn info_panel(lines: &[(String, Vec<String>)], anchor: &ElementRef, open: Signal<Option<bool>>, over: Signal<bool>, quiet: Signal<bool>, close_later: impl Fn() + 'static) -> impl IntoElement {
    let focus = *anchor;
    let mut body = view()
        .label(INFO_PANEL)
        .w(px(INFO_W))
        .gap(px(12.0))
        .p(px(14.0))
        .rounded(px(12.0))
        .bg(theme::POPOVER)
        .border_1()
        .border_color(theme::BORDER_S)
        .shadow_lg()
        .on_mouse_enter(move |_: &MouseEvent| over.set(true))
        .on_mouse_leave(move |_: &MouseEvent| {
            over.set(false);
            close_later();
        });
    for (heading, ls) in lines {
        let mut section = view().gap(px(4.0)).child(
            text(heading.clone()).text_size(px(12.0)).line_height(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT),
        );
        for l in ls {
            section = section.child(
                view()
                    .flex_row()
                    .gap(px(6.0))
                    .child(text("•").text_size(px(12.0)).line_height(px(17.0)).text_color(theme::FAINT))
                    .child(text(l.clone()).flex_1().min_w(px(0.0)).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::DIM)),
            );
        }
        body = body.child(section);
    }
    nsh_core::element::overlay()
        .anchor(anchor)
        .placement(nsh_core::overlay::Placement::Bottom)
        .align(nsh_core::overlay::Align::End)
        .offset(px(6.0))
        .label("About this card, open")
        // the core doesn't count a press on the anchor (or in it) as outside (`overlay_touches`
        // looks for the anchor on the touch's path), so a press on the ⓘ is the ⓘ's alone: it
        // closes a pinned popover rather than closing and pinning it again
        .on_press_outside(move |_: &Point<Points>| open.set(None))
        .on_cancel(move || {
            open.set(None);
            // back where the keyboard was, without that focus opening it again
            quiet.set(true);
            focus.focus();
        })
        .child(body)
}

/// A sending card: its header and what it's waiting on.
fn busy(spec: &UserFormSpec, key: &str) -> View {
    let loc = local(key);
    let on_site = site_of(spec).map(|s| format!(" on {s}")).unwrap_or_default();
    let words = match loc.going.get_untracked() {
        Going::Fill => format!("Filling in{on_site}…"),
        Going::Screen => format!("Handing over {}…", screen_of(spec)),
        Going::Dismiss => "Dismissing…".to_owned(),
    };
    login_shell(&format!("user-form-{key}")).label(headline(spec)).child(header(spec, 0.0)).child(
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .accessibility_state(AccessibilityState { busy: true, ..Default::default() })
            .child(glyph("…", theme::DIM, theme::HOVER))
            .child(text(words).flex_1().min_w(px(0.0)).text_size(px(13.0)).line_height(px(18.0)).text_color(theme::DIM)),
    )
}

/// A status glyph in a small round tint.
fn glyph(g: &'static str, ink: AdaptiveColor, tint: AdaptiveColor) -> impl IntoElement {
    view()
        .size(px(18.0))
        .flex_shrink(0.0)
        .rounded(px(9.0))
        .bg(tint)
        .items_center()
        .justify_center()
        .child(text(g).text_size(px(11.0)).line_height(px(13.0)).font_weight(FontWeight::BOLD).text_color(ink))
}

/// Whether a fill that stopped may have left something in the page: nothing, a value, or a
/// secret's value (the broker's `uncertain` and `secretMaybe`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unsure {
    No,
    Value,
    Secret,
}

/// What a settled card says, in one line, and its glyph, made from the notice's facts (never
/// from the broker's prose, which shows beside it): each ending kept apart (a fill that may have
/// typed is never "nothing was filled", a check-the-screen never "dismissed").
pub(crate) fn receipt(spec: &UserFormSpec, r: FormResolution, f: &Facts) -> (&'static str, AdaptiveColor, AdaptiveColor, String) {
    let on_site = site_of(spec).map(|s| format!(" on {s}")).unwrap_or_default();
    const SECRETS: &str = "Secret values were never shown to the AI.";
    const MAYBE_SECRET: &str = "A secret value may have been typed into the page; it was never shown to the AI.";
    let pressed = if f.pressed == Some(true) { " Return was pressed." } else { "" };
    let (typed, not) = went_in(spec, f);
    match r {
        FormResolution::Sending => ("…", theme::DIM, theme::HOVER, "Sending…".to_owned()),
        FormResolution::Submitted if typed.is_empty() || not.is_empty() => ("✓", theme::UP, theme::UP_TINT, format!("Filled in{on_site}.{pressed} {SECRETS}")),
        FormResolution::Submitted => ("✓", theme::UP, theme::UP_TINT, format!("{}{on_site}; {}.{pressed} {SECRETS}", were(&typed, "typed"), werent(&not))),
        FormResolution::FillFailed => {
            let secrets = if f.unsure() == Unsure::Secret { MAYBE_SECRET } else { SECRETS };
            match f.unsure() {
                Unsure::No if f.note.is_some() => ("✕", theme::DOWN, theme::DOWN_TINT, format!("Nothing was filled{on_site}. {secrets}")),
                Unsure::No => ("✕", theme::DOWN, theme::DOWN_TINT, format!("Nothing was filled{on_site}: the page may have moved or changed. {secrets}")),
                _ => ("!", theme::WARN, theme::WARN_TINT, format!("May have been typed{on_site}: check the screen. {secrets}")),
            }
        }
        FormResolution::Escalated => ("↗", theme::ACCENT, theme::ACCENT_TINT, format!("Handed over: {} is yours. Hand it back when you're done.", screen_of(spec))),
        FormResolution::Dismissed => ("–", theme::MUTED, theme::HOVER, "Dismissed. Nothing was filled.".to_owned()),
        FormResolution::Skipped => ("–", theme::MUTED, theme::HOVER, "Skipped. Nothing was filled.".to_owned()),
        FormResolution::TimedOut => ("–", theme::MUTED, theme::HOVER, "Expired: nobody answered in time, so nothing was filled.".to_owned()),
        FormResolution::Superseded => ("–", theme::MUTED, theme::HOVER, "Moved on to your next message.".to_owned()),
        FormResolution::Unsure => ("!", theme::WARN, theme::WARN_TINT, "Check the screen: Buwiz Bot didn't hear how this went.".to_owned()),
    }
}

/// The labels of the card's fields that went in, and of those that didn't, by the notice's
/// outcomes (none: the notice didn't say).
fn went_in(spec: &UserFormSpec, f: &Facts) -> (Vec<String>, Vec<String>) {
    let label = |id: &str| spec.field(id).map_or_else(|| id.to_owned(), |f| f.label.clone());
    let (yes, no): (Vec<_>, Vec<_>) = f.outcomes.iter().partition(|(_, filled)| *filled);
    (yes.iter().map(|(id, _)| label(id)).collect(), no.iter().map(|(id, _)| label(id)).collect())
}

/// "Email was typed", "Email and Password were typed".
fn were(labels: &[String], what: &str) -> String {
    format!("{} {} {what}", and_list(labels), if labels.len() == 1 { "was" } else { "were" })
}

/// "Password wasn't", "Name and Code weren't".
fn werent(labels: &[String]) -> String {
    format!("{} {}", and_list(labels), if labels.len() == 1 { "wasn't" } else { "weren't" })
}

fn and_list(labels: &[String]) -> String {
    match labels {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn settled(spec: Rc<UserFormSpec>, key: String, r: FormResolution) -> View {
    let loc = local(&key);
    // an ending (a supersede included) takes any held or typed secret with it
    drop_held(&key);
    forget_secrets(&spec, loc);
    // the card's own notice once it has settled, else what the broker's answer said
    let facts = if spec.facts.is_empty() { loc.said.get_untracked() } else { spec.facts.clone() };
    let (g, ink, tint, line) = receipt(&spec, r, &facts);
    let said = facts.note.clone().filter(|_| matches!(r, FormResolution::Submitted | FormResolution::FillFailed));
    // nothing here is cut: a safety sentence is never ellipsised
    let mut words = view().flex_1().min_w(px(0.0)).gap(px(4.0)).child(text(line.clone()).text_size(px(13.0)).line_height(px(18.0)).text_color(theme::TEXT));
    if r == FormResolution::Unsure {
        words = words.child(note(format!("It may still be typing, or it was answered already. Look at {} before doing anything else.", screen_of(&spec))));
    }
    // what the broker said of the fill, in its own block, whole (Return pressed, left to you, a reason)
    if let Some(said) = said {
        words = words.child(note(said));
    }
    let mut receipt = login_shell(&format!("user-form-{key}"))
        .label(line)
        .gap(px(8.0))
        .p(px(12.0))
        .child(view().flex_row().items_start().gap(px(8.0)).child(glyph(g, ink, tint)).child(words));
    // the broker's card is done once it settles: the run has gone on, and a fill that may have
    // typed something isn't retried blind. Its note says what to do; the AI may raise a new card
    if r == FormResolution::FillFailed && spec.has_entry_id() && spec.chat.is_empty() {
        let (s1, k1, s2, k2) = (spec.clone(), key.clone(), spec.clone(), key.clone());
        receipt = receipt.child(
            view()
                .flex_row_reverse()
                .gap_2()
                .child(button("Try again", Kind::Primary, || true, move || retry(&s1, &k1)))
                .child(button("Stop for now", Kind::Ghost, || true, move || dismiss(s2.clone(), k2.clone()))),
        );
    }
    let mut v = view().w_full().gap_2().items_start().child(receipt);
    // the broker offered to save the login it typed: the card asks
    if r == FormResolution::Submitted && loc.offer.get_untracked() {
        let entry = spec.entry_id.clone();
        if let Some((origin, username)) = vault().pending_who(&entry) {
            let offer = save_login::SaveLoginSpec { form_entry_id: entry.clone(), origin, username, bot: spec.agent_id.clone(), bot_name: spec.bot_name.clone() };
            v = v.child(save_login::card(offer, format!("save-login-{entry}")));
        }
    }
    v
}

/// The open card's width for the corner's buttons, so the header's words stop short of them.
fn corner_w(spec: &UserFormSpec) -> f32 {
    if brokers(spec) { 52.0 } else { 28.0 }
}

fn idle(spec: Rc<UserFormSpec>, key: String) -> View {
    let loc = local(&key);
    if !spec.chat.is_empty() && loc.chat.with_untracked(|c| c != &spec.chat) {
        loc.chat.set(spec.chat.clone());
    }
    let target = card_target(&spec);
    // non-secret prefills seed the view state once
    let missing: Vec<(String, String)> = spec
        .fields
        .iter()
        .filter(|f| !f.masked())
        .filter_map(|f| Some((f.id.clone(), f.prefill.clone()?)))
        .filter(|(id, _)| loc.values.with_untracked(|v| !v.0.contains_key(id)))
        .collect();
    if !missing.is_empty() {
        loc.values.update(|v| v.0.extend(missing));
    }
    let v = login_shell(&format!("user-form-{key}")).label(headline(&spec)).relative().child(header(&spec, corner_w(&spec)));
    let (s_form, k_form, t_form) = (spec.clone(), key.clone(), target.clone());
    let (s_confirm, k_confirm) = (spec.clone(), key.clone());
    v.child(when(move || !loc.confirm.get(), move || form_body(s_form.clone(), k_form.clone(), t_form.clone()).into_element()).otherwise(move || confirm_step(s_confirm.clone(), k_confirm.clone(), loc).into_element()))
        .child(corner(spec, key))
}

/// The card's fields and answers. With saved logins offered and nothing typed, the logins come
/// first, as a password manager's: the fields wait behind "Use a different login".
fn form_body(spec: Rc<UserFormSpec>, key: String, target: Option<CardTarget>) -> View {
    let loc = local(&key);
    let (s_rows, t_rows) = (spec.clone(), target.clone());
    let offered = computed(move || {
        let _ = vault::revision().get();
        let Some(t) = &t_rows else { return false };
        let sites = [offered_site(&s_rows), asked_site(&s_rows)];
        vault().offered_on(&sites, &s_rows.agent_id, t.kind() == vault::KIND_CODE).iter().any(|rows| !rows.is_empty())
    });
    let s_typed = spec.clone();
    // the saved logins are the choice: none typed by hand, none picked yet
    let choosing = computed(move || {
        offered.get()
            && !loc.by_hand.get()
            && !matches!(loc.pick.get(), Pick::Ready { .. })
            && loc.values.with(|v| !s_typed.fields.iter().any(|f| v.filled(f)))
    });
    let mut v = view().gap(px(12.0)).child(saved_part(spec.clone(), key.clone(), target.clone()));
    // the last field that takes text: Return there sends, when the form can be sent
    let last = spec
        .fields
        .iter()
        .rposition(|f| !matches!(f.kind, FieldKind::Checkbox) && !(f.kind == FieldKind::Select && !f.options.is_empty()));
    let can_post = spec.has_entry_id();
    let (s_en, t_en) = (spec.clone(), target.clone());
    let enabled = Rc::new(move || {
        let held: Vec<&str> = match (loc.pick.get(), &t_en) {
            (Pick::Ready { .. }, Some(t)) => t.secret_field().into_iter().collect(),
            _ => Vec::new(),
        };
        can_post && s_en.required_filled(&loc.values.get(), &held)
    });
    let (s_go, k_go, e_go) = (spec.clone(), key.clone(), enabled.clone());
    let go: Rc<dyn Fn()> = Rc::new(move || {
        if e_go() {
            submit(s_go.clone(), k_go.clone())
        }
    });
    let (s_f, k_f, t_f, g_f) = (spec.clone(), key.clone(), target.clone(), go.clone());
    v = v.child(when(move || !choosing.get(), move || {
        let mut fields = view().gap(px(10.0));
        for (i, f) in s_f.fields.iter().enumerate() {
            fields = fields.child(field_view(k_f.clone(), f.clone(), t_f.clone(), (Some(i) == last).then(|| g_f.clone())));
        }
        fields.into_element()
    }));
    let k_note = key.clone();
    v = v.child(dynamic(move || {
        let why = phase(&k_note).1;
        if why.is_empty() {
            return view().into_element();
        }
        error_line(why).number_of_lines(5).into_element()
    }));
    // Tab goes to the primary first: the row is laid out from the right
    let mut actions = view().flex_row_reverse().items_center().gap_2();
    let label = primary_label(&spec);
    let hint_words = if brokers(&spec) { caption(&spec) } else { String::new() };
    let (s_why, e_p, g_p) = (spec.clone(), enabled.clone(), go.clone());
    actions = actions.child(when(move || !choosing.get(), move || {
        let (e1, e2, g, s, h) = (e_p.clone(), e_p.clone(), g_p.clone(), s_why.clone(), hint_words.clone());
        primary(label, move || e1(), move || g(), move || if e2() { h.clone() } else { missing_words(&s, loc) }).into_element()
    }));
    actions = actions.child(view().flex_1());
    if !can_post {
        actions = actions.child(
            text("This form has no server entry yet, so it can't be sent.")
                .flex_1()
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        );
    }
    // the broker's card: the person may finish on the Bot's screen instead, after one more press
    if brokers(&spec) {
        actions = actions.child(button(DO_IT_MYSELF, Kind::Ghost, || true, move || loc.confirm.set(true)));
    }
    v = v.child(actions);
    if brokers(&spec) {
        // while the saved logins are the choice there's no primary to press yet: the line says
        // what a pick does
        let (now, picking) = (caption(&spec), choosing_caption(&spec));
        v = v.child(text(move || if choosing.get() { picking.clone() } else { now.clone() }).text_size(px(11.5)).line_height(px(16.0)).text_color(theme::MUTED));
    }
    v
}

/// Why the primary can't be pressed yet: the required fields still empty.
fn missing_words(spec: &UserFormSpec, loc: Local) -> String {
    let values = loc.values.get();
    let held = matches!(loc.pick.get(), Pick::Ready { .. });
    let target = card_target(spec);
    let empty: Vec<String> = spec
        .fields
        .iter()
        .filter(|f| f.required && !values.filled(f))
        .filter(|f| !(held && target.as_ref().and_then(CardTarget::secret_field) == Some(f.id.as_str())))
        .map(|f| f.label.to_lowercase())
        .collect();
    match empty.as_slice() {
        [] if !spec.has_entry_id() => "This form has no server entry yet, so it can't be sent.".to_owned(),
        [] => String::new(),
        [one] => format!("Enter {one}"),
        [rest @ .., last] => format!("Enter {} and {last}", rest.join(", ")),
    }
}

/// The confirm step: what handing the screen over means, then Back or Open the screen.
fn confirm_step(spec: Rc<UserFormSpec>, key: String, loc: Local) -> View {
    let s_open = spec.clone();
    view()
        .gap(px(12.0))
        .child(
            view()
                .gap(px(4.0))
                .child(text(format!("Do it yourself on {}?", screen_of(&spec))).text_size(px(13.0)).line_height(px(18.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
                .child(note(confirm_words(&spec))),
        )
        .child(
            view()
                .flex_row_reverse()
                .items_center()
                .gap_2()
                .child(button(OPEN_THE_SCREEN, Kind::Primary, || true, move || {
                    loc.confirm.set(false);
                    open_screen(s_open.clone(), key.clone());
                }))
                .child(view().flex_1())
                .child(button("Back", Kind::Ghost, || true, move || loc.confirm.set(false))),
        )
}

/// The card's saved-login part, alone drawn again when a login or a share
/// changes (the fields beside it are not, so focus and typing stay): the
/// logins shared with this card's Bot for its site, read once per change, or
/// with none, a refused pick's sentence.
fn saved_part(spec: Rc<UserFormSpec>, key: String, target: Option<CardTarget>) -> impl IntoElement {
    let loc = local(&key);
    dynamic(move || {
        let _ = vault::revision().get();
        let sites = [offered_site(&spec), asked_site(&spec)];
        let [rows, asked] = match &target {
            Some(t) => {
                let mut found = vault().offered_on(&sites, &spec.agent_id, t.kind() == vault::KIND_CODE).into_iter();
                [found.next().unwrap_or_default(), found.next().unwrap_or_default()]
            }
            None => [Vec::new(), Vec::new()],
        };
        if let (Some(t), Some(site)) = (target.clone().filter(|_| !rows.is_empty() || !asked.is_empty()), sites[0].clone()) {
            let asked = sites[1].clone().map(|named| (named, asked));
            let who = (spec.agent_id.clone(), spec.bot_name.clone(), card_sites(&spec));
            // the header names the site on the broker's card
            let say_site = site_of(&spec).is_none();
            return saved_strip(key.clone(), rows, t, site, asked, who, say_site).into_element();
        }
        // no login is offered here, but a refused pick still says why
        dynamic(move || match loc.pick.get() {
            Pick::Unavailable(why) => text(why).number_of_lines(3).text_size(px(12.0)).text_color(theme::DOWN).into_element(),
            _ => view().into_element(),
        })
        .into_element()
    })
}

/// The saved logins for the card's site, each a row to press: Touch ID, then its name in the
/// field and its secret held. "Use a different login" types one by hand. The site the card asks
/// for shows above them where the header doesn't name it, and each row's own site under its name,
/// marked when it is not the same site. `site` is the card's page's; when its `domain` names
/// another (`asked`: that site and its rows), the mismatch shows first and that site's logins are
/// listed only once the person asks for them.
fn saved_strip(
    key: String,
    rows: Vec<SiteLogin>,
    target: CardTarget,
    site: String,
    asked: Option<(String, Vec<SiteLogin>)>,
    (bot, bot_name, sites): (String, String, Vec<String>),
    say_site: bool,
) -> impl IntoElement {
    let loc = local(&key);
    dynamic(move || {
        let pick = loc.pick.get();
        let typing = loc.by_hand.get();
        let shown = loc.show_asked.get();
        let mut v = view().gap(px(6.0));
        match pick {
            Pick::Confirming { username } => {
                return v
                    .child(text(format!("Confirm with Touch ID to use {username}…")).text_size(px(12.0)).text_color(theme::MUTED))
                    .into_element();
            }
            Pick::Ready { .. } => return view().into_element(),
            Pick::Unavailable(why) => {
                v = v.child(text(why).number_of_lines(3).text_size(px(12.0)).text_color(theme::DOWN));
            }
            Pick::None => {}
        }
        if typing {
            let k = key.clone();
            return view()
                .flex_row()
                .child(link_btn("Use a saved login", move || local(&k).by_hand.set(false)))
                .into_element();
        }
        let hint = match target {
            CardTarget::Code { .. } => "Touch ID fills in its code.",
            CardTarget::Username { .. } => "Touch ID fills in its name.",
            _ => "Touch ID fills it in; the password is never shown.",
        };
        v = v.child(text("Saved logins").text_size(px(12.0)).line_height(px(16.0)).font_weight(FontWeight::MEDIUM).text_color(theme::MUTED));
        if let Some((named, _)) = &asked {
            v = v.child(
                text(format!("This form names {named}, but its page is on {site}. Only logins for {site} are offered; check the page before you use another site's."))
                    .number_of_lines(3)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::DOWN),
            );
        } else if say_site {
            v = v.child(text(format!("This form asks for {site}")).text_size(px(12.0)).text_color(theme::MUTED));
        }
        let other: &[SiteLogin] = asked.as_ref().map(|(_, r)| r.as_slice()).unwrap_or_default();
        if rows.is_empty() && !shown {
            v = v.child(text(format!("No login is saved for {site}.")).text_size(px(12.0)).text_color(theme::MUTED));
        }
        for row in &listed(&rows, other, shown) {
            let (k, id, t, s) = (key.clone(), row.id.clone(), target.clone(), site.clone());
            let (b, bn, ss) = (bot.clone(), bot_name.clone(), sites.clone());
            let saved_for = match site_match(&row.origin, true, &site) {
                SiteMatch::Exact => (format!("Saved for {}", row.origin), theme::MUTED),
                _ => (format!("Saved for {}, not {site}: check before you use it", row.origin), theme::DOWN),
            };
            v = v.child(saved_row(format!("Use {}", row.username), row.username.clone(), saved_for, hint, move || {
                use_saved(k.clone(), &id, t.clone(), s.clone(), ss.clone(), b.clone(), &bn)
            }));
        }
        v = v.child(text(hint).text_size(px(11.0)).line_height(px(15.0)).text_color(theme::MUTED));
        let mut links = view().flex_row().gap(px(12.0));
        if let Some((named, _)) = asked.as_ref().filter(|(_, r)| !shown && r.iter().any(|a| !rows.iter().any(|b| b.id == a.id))) {
            let k = key.clone();
            links = links.child(link_btn(format!("Show logins saved for {named}"), move || local(&k).show_asked.set(true)));
        }
        let k = key.clone();
        v.child(links.child(link_btn(USE_ANOTHER, move || by_hand(&k)))).into_element()
    })
}

/// The saved-logins strip's way to type a login by hand.
pub(crate) const USE_ANOTHER: &str = "Use a different login";

/// One saved login, a button: its name, where it's saved for, and Touch ID at its end.
fn saved_row(label: String, username: String, (saved_for, ink): (String, AdaptiveColor), hint: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    view()
        .role(AccessibilityRole::Button)
        .label(label)
        .hint(hint)
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .border(px(0.5))
        .border_color(theme::BORDER_S)
        .bg(move || if hov.get() { theme::HOVER } else { theme::SUBTLE })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(text(username).number_of_lines(1).text_size(px(13.0)).line_height(px(18.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
                .child(text(saved_for).number_of_lines(2).text_size(px(11.0)).line_height(px(15.0)).text_color(ink)),
        )
        .child(text("Touch ID").text_size(px(11.0)).line_height(px(15.0)).text_color(theme::ACCENT))
}

fn field_view(key: String, f: Field, target: Option<CardTarget>, go: Option<Rc<dyn Fn()>>) -> AnyElement {
    let loc = local(&key);
    let mut label = view().flex_row().child(
        text(f.label.clone())
            .text_size(px(12.0))
            .line_height(px(16.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::DIM),
    );
    if !f.required && f.kind != FieldKind::Checkbox {
        label = label.child(text(" (optional)").text_size(px(12.0)).line_height(px(16.0)).text_color(theme::MUTED));
    }
    let is_held = target.as_ref().and_then(CardTarget::secret_field) == Some(f.id.as_str());
    let control = match f.kind {
        FieldKind::Checkbox => return checkbox(key, f).into_element(),
        FieldKind::Select if !f.options.is_empty() => chips(key, f.clone()).into_element(),
        _ if is_held => {
            let (k, f2) = (key.clone(), f.clone());
            dynamic(move || match loc.pick.get() {
                Pick::Ready { username } => locked(&k, &username, target.as_ref()).into_element(),
                _ => text_input(k.clone(), f2.clone(), go.clone()).into_element(),
            })
            .into_element()
        }
        _ => text_input(key, f.clone(), go).into_element(),
    };
    view().gap(px(4.0)).child(label).child(control).into_element()
}

fn text_input(key: String, f: Field, go: Option<Rc<dyn Fn()>>) -> impl IntoElement {
    let loc = local(&key);
    let (id1, id2) = (f.id.clone(), f.id.clone());
    let multiline = f.kind == FieldKind::Textarea;
    let placeholder = f.placeholder.clone().unwrap_or_else(|| match f.kind {
        FieldKind::Date => "YYYY-MM-DD".to_owned(),
        FieldKind::Email => "name@example.com".to_owned(),
        _ => String::new(),
    });
    let h = if multiline { px(72.0) } else { px(32.0) };
    let mut field = input()
        .label(f.label.clone())
        .value(move || loc.values.with(|v| v.get(&id1).to_owned()))
        .placeholder(placeholder)
        .placeholder_color(|| color(theme::GHOST))
        .masked(f.masked())
        .multiline(multiline)
        .auto_correct(!f.masked() && multiline)
        .text_size(px(13.0))
        .text_color(theme::TEXT)
        .h(h)
        .on_change(move |t: &str| {
            let (id, t) = (id2.clone(), t.to_owned());
            loc.values.update(|v| {
                v.0.insert(id, t);
            });
        });
    // Return in the last field sends, as the primary would
    if let Some(go) = go.filter(|_| !multiline) {
        field = field.on_submit(move |_: &str| go());
    }
    view()
        .h(h)
        .rounded(px(8.0))
        .bg(theme::HOVER)
        .border(px(0.5))
        .border_color(theme::BORDER_F)
        .px(px(10.0))
        .justify_center()
        .child(field)
}

/// The held secret's place: dots and where it came from, never the value.
fn locked(key: &str, username: &str, target: Option<&CardTarget>) -> impl IntoElement {
    let k = key.to_owned();
    let what = match target {
        Some(CardTarget::Code { .. }) => format!("Code from your keychain, for {username}; minted when you press Continue"),
        _ => format!("From your keychain, for {username}"),
    };
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(32.0))
        .rounded(px(8.0))
        .bg(theme::SUBTLE)
        .px(px(10.0))
        .child(text("••••••••").text_size(px(13.0)).text_color(theme::TEXT))
        .child(text(what).flex_1().min_w(px(0.0)).number_of_lines(1).text_size(px(11.0)).text_color(theme::MUTED))
        .child(link_btn("Type it by hand", move || by_hand(&k)))
}

fn chips(key: String, f: Field) -> impl IntoElement {
    let loc = local(&key);
    view().flex_row().flex_wrap().gap(px(6.0)).children(f.options.into_iter().map(move |o| {
        let (id, id2, val, val2) = (f.id.clone(), f.id.clone(), o.value.clone(), o.value.clone());
        let on = move || loc.values.with(|v| v.get(&id) == val);
        text(o.label.clone())
            .text_size(px(12.0))
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(999.0))
            .border_1()
            .border_color(theme::BORDER_S)
            .text_color(theme::TEXT)
            .bg({
                let on = on.clone();
                move || if on() { theme::ACCENT_TINT } else { AdaptiveColor::TRANSPARENT }
            })
            .label(o.label)
            .on_press(move || {
                let (id, val) = (id2.clone(), val2.clone());
                loc.values.update(|v| {
                    v.0.insert(id, val);
                });
            })
    }))
}

fn checkbox(key: String, f: Field) -> impl IntoElement {
    let loc = local(&key);
    let (id, id2) = (f.id.clone(), f.id.clone());
    let on = move || loc.values.with(|v| v.get(&id) == "true");
    let on2 = on.clone();
    let mut row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .label(f.label.clone())
        .on_press(move || {
            let id = id2.clone();
            loc.values.update(|v| {
                let next = Field::checkbox_wire(v.get(&id) != "true").to_owned();
                v.0.insert(id, next);
            });
        })
        .child(
            text(move || if on() { "✓".to_owned() } else { String::new() })
                .size(px(16.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(theme::BORDER_S)
                .text_size(px(11.0))
                .text_color(theme::ON_FILL)
                .bg(move || if on2() { theme::ACCENT } else { AdaptiveColor::TRANSPARENT }),
        )
        .child(text(f.label.clone()).text_size(px(13.0)).text_color(theme::TEXT));
    if !f.required {
        row = row.child(text("(optional)").text_size(px(12.0)).text_color(theme::MUTED));
    }
    row
}

/// A login card's button's look.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    /// Filled in the accent: the card's answer.
    Primary,
    /// Words only, a wash on hover: the quiet answer.
    Ghost,
}

pub(crate) fn button(label: &'static str, kind: Kind, enabled: impl Fn() -> bool + 'static, on: impl Fn() + 'static) -> impl IntoElement {
    button_as(label.to_owned(), label, kind, enabled, on)
}

/// The primary answer: disabled (and dim) until the form can be sent, its tooltip and hint
/// saying why (`why`, the fields still empty) or what it does.
fn primary(label: &'static str, enabled: impl Fn() -> bool + 'static, on: impl Fn() + 'static, why: impl Fn() -> String + 'static) -> impl IntoElement {
    let why = Rc::new(why);
    let (w1, w2) = (why.clone(), why);
    button_as(label.to_owned(), label, Kind::Primary, enabled, on).hint(move || SharedString::from(w1())).tooltip(
        dynamic(move || {
            let w = w2();
            if w.is_empty() {
                return view().into_element();
            }
            text(w)
                .max_w(px(260.0))
                .text_size(px(11.5))
                .line_height(px(15.0))
                .text_color(theme::TEXT)
                .bg(theme::POPOVER)
                .border_1()
                .border_color(theme::BORDER_S)
                .rounded(px(6.0))
                .px(px(8.0))
                .py(px(4.0))
                .into_element()
        })
        .into_element(),
    )
}

/// A button a screen reader gets as one: role button, its own `label`, and
/// disabled while it can't be pressed; `words` are what it shows.
fn button_as(label: String, words: &'static str, kind: Kind, enabled: impl Fn() -> bool + 'static, on: impl Fn() + 'static) -> View {
    let hov = signal(false);
    let enabled = Rc::new(enabled);
    let (e1, e2, e3, e4) = (enabled.clone(), enabled.clone(), enabled.clone(), enabled);
    view()
        .role(AccessibilityRole::Button)
        .label(label)
        .disabled(move || !e4())
        .h(px(30.0))
        .px(px(if kind == Kind::Ghost { 10.0 } else { 14.0 }))
        .rounded(px(15.0))
        .items_center()
        .justify_center()
        .child(
            text(words)
                .number_of_lines(1)
                .text_size(px(12.5))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || match kind {
                    Kind::Primary => theme::ON_FILL,
                    Kind::Ghost => theme::DIM,
                }),
        )
        .bg(move || match (kind, hov.get()) {
            (Kind::Primary, _) => theme::ACCENT,
            (Kind::Ghost, true) => theme::ACTIVE,
            (Kind::Ghost, false) => AdaptiveColor::TRANSPARENT,
        })
        .opacity(move || if e1() { 1.0f32 } else { 0.4 })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(e2()))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || {
            if e3() {
                on()
            }
        })
}

fn link_btn(label: impl Into<String>, on: impl Fn() + 'static) -> impl IntoElement {
    let label: String = label.into();
    let hov = signal(false);
    view()
        .role(AccessibilityRole::Button)
        .label(label.clone())
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
        .child(text(label).text_size(px(12.0)).text_color(move || if hov.get() { theme::TEXT } else { theme::ACCENT }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture under nativechat's `fixtures/wire`; `None` (the test
    /// skips) in CI ([`crate::cards::wire_dir`]).
    fn fixture(rel: &str) -> Option<Value> {
        let path = crate::cards::wire_dir()?.join(rel);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        Some(serde_json::from_str(&text).expect("fixture json"))
    }

    fn fixtures_in(dir: &str) -> Vec<(String, Value)> {
        let Some(root) = crate::cards::wire_dir() else { return Vec::new() };
        let path = root.join(dir);
        let rd = std::fs::read_dir(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut out: Vec<(String, Value)> = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .map(|e| {
                let text = std::fs::read_to_string(e.path()).expect("read");
                (e.file_name().to_string_lossy().into_owned(), serde_json::from_str(&text).expect("json"))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn a_live_park_paints_from_arguments_with_its_entry_id() {
        let Some(v) = fixture("agui/custom/run-awaiting-approval/a_form_raised_after_an_answer_has_its_card_and_entry_id.json") else {
            return;
        };
        let spec = UserFormSpec::from_wire(&v).expect("user form");
        assert_eq!(spec.entry_id, "e_0199bb4e-0000-7000-8000-000000000003");
        assert_eq!(spec.call_id, "form-1");
        assert_eq!(spec.title, "Log in to The Internet");
        assert_eq!(spec.fields.len(), 2);
        assert!(!spec.fields[0].masked());
        assert!(spec.fields[1].masked(), "a password field is secret without secret:true");
        assert!(spec.same_page && spec.submit);
        assert_eq!(spec.continue_label(), "Log in");
        assert_eq!(spec.resolution, None);
        assert_eq!(
            card_target(&spec),
            Some(CardTarget::Login { username_id: "username".into(), password_id: "password".into() })
        );
    }

    #[test]
    fn every_park_fixture_is_a_user_form_exactly_when_its_reason_says_so() {
        for (name, v) in fixtures_in("agui/custom/run-awaiting-approval") {
            let is_form = v.get("reason").and_then(Value::as_str) == Some("user-form");
            let spec = UserFormSpec::from_wire(&v);
            assert_eq!(spec.is_some(), is_form, "{name}");
            if let Some(spec) = spec {
                assert!(spec.has_entry_id(), "{name}: the park carries its entry id");
                assert_ne!(spec.entry_id, spec.call_id, "{name}: never the call id");
            }
        }
    }

    #[test]
    fn every_settled_replay_fixture_parses_with_its_ending() {
        let want = [
            ("a_declined_handoff_replays_on_its_form_as_declined.json", FormResolution::Skipped),
            ("a_handed_back_form_replays_as_handed_back_and_leaves_the_approvals_queue.json", FormResolution::Escalated),
            ("a_saved_login_fills_only_a_dedicated_box.json", FormResolution::Submitted),
            ("a_timed_out_handoff_replays_on_its_form_as_timed_out.json", FormResolution::TimedOut),
            ("an_agui_only_turn_still_mints_a_user_form_entry_id.json", FormResolution::Submitted),
        ];
        for (name, v) in fixtures_in("agui/custom/user-form") {
            let spec = UserFormSpec::from_wire(&v).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(spec.entry_id, "e_0199bb4e-0000-7000-8000-000000000002", "{name}");
            assert!(!spec.call_id.is_empty(), "{name}");
            assert!(!spec.fields.is_empty(), "{name}");
            if let Some((_, r)) = want.iter().find(|(n, _)| *n == name) {
                assert_eq!(spec.effective_resolution(), Some(*r), "{name}");
            }
        }
    }

    #[test]
    fn the_send_message_envelope_parses_and_merges_with_the_live_park() {
        let (Some(live), Some(replay)) = (
            fixture("agui/custom/run-awaiting-approval/a_routine_that_asks_for_a_login_says_it_is_waiting_and_its_card_can_be_submitted.json"),
            fixture("agui/custom/user-form/a_saved_login_fills_only_a_dedicated_box.json"),
        ) else {
            return;
        };
        // the bare gateway envelope: `id` is the entry id
        let envelope = replay.get("value").expect("envelope");
        let settled = UserFormSpec::from_wire(envelope).expect("envelope parses");
        assert_eq!(settled.entry_id, "e_0199bb4e-0000-7000-8000-000000000002");
        assert_eq!(settled.call_id, "mock-form-1");
        assert_eq!(settled.resolution, Some(FormResolution::Submitted));
        assert_eq!(login_origin(&settled).as_deref(), Some("accounts.google.com"));

        // the park of the same call, without its entry id, folds onto it
        let mut park = UserFormSpec::from_wire(&live).expect("park");
        park.entry_id.clear();
        assert_eq!(park.call_id, "mock-form-1");
        let mut cards = vec![park];
        merge_into(&mut cards, settled);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].entry_id, "e_0199bb4e-0000-7000-8000-000000000002");
        assert_eq!(cards[0].resolution, Some(FormResolution::Submitted));

        // a card with its own entry id is a sibling, not a twin
        let mut other = cards[0].clone();
        other.entry_id = "e_other".into();
        other.resolution = None;
        merge_into(&mut cards, other);
        assert_eq!(cards.len(), 2);
    }

    #[test]
    fn a_settled_reply_body_states_its_ending() {
        let Some(v) = fixture("rest/POST__ag-ui_user-form_submit/200-a_saved_login_fills_only_a_dedicated_box.json") else {
            return;
        };
        assert_eq!(settled_from(&v["body"]), FormResolution::Submitted);
        assert_eq!(settled_from(&json!({"value": {"formResolution": "fill_failed"}})), FormResolution::FillFailed);
        assert_eq!(settled_from(&json!({"handoffEntryId": "e_h"})), FormResolution::Escalated);
        for r in [FormResolution::Submitted, FormResolution::FillFailed, FormResolution::TimedOut, FormResolution::Skipped] {
            assert_eq!(FormResolution::parse(r.as_str()), r, "the shared state's word reads back");
        }
    }

    #[test]
    fn secrets_are_never_prefilled_or_printed() {
        let spec = UserFormSpec::parse(
            &json!({
                "entryId": "e_1",
                "formRequest": {
                    "title": "Sign in",
                    "domain": "https://accounts.example.com",
                    "fields": [
                        {"id": "email", "label": "Email", "type": "email", "required": true, "value": "ada@example.com"},
                        {"id": "pin", "label": "PIN", "type": "text", "secret": true, "value": "1234"},
                        {"id": "password", "label": "Password", "type": "password", "required": true, "default": "smuggled"},
                        {"id": "ok", "label": "Remember me", "type": "checkbox"}
                    ]
                }
            }),
            None,
        )
        .expect("form");
        assert_eq!(spec.fields[0].prefill.as_deref(), Some("ada@example.com"));
        assert_eq!(spec.fields[1].prefill, None);
        assert_eq!(spec.fields[2].prefill, None);
        assert!(!format!("{spec:?}").contains("smuggled"));
        let mut values = Values::default();
        values.0.insert("email".into(), "ada@example.com".into());
        values.0.insert("password".into(), "hunter2".into());
        assert!(!format!("{values:?}").contains("hunter2"));
        assert!(spec.required_filled(&values, &[]));
        values.0.remove("password");
        assert!(!spec.required_filled(&values, &[]));
        assert!(spec.required_filled(&values, &["password"]), "a held secret counts as filled");
        values.0.insert("password".into(), "hunter2".into());
        let save = save_candidate(&spec, &values).expect("candidate");
        assert_eq!((save.origin.as_str(), save.username.as_str()), ("accounts.example.com", "ada@example.com"));
        assert!(!format!("{save:?}").contains("hunter2"));
        assert_eq!(Field::checkbox_wire(true), "true");
        let body = transport::submit_body("t_1", "e_1", values.into_map(), Some("sl_1"));
        assert_eq!(body["values"]["password"], json!("hunter2"), "only the body carries it");
        assert_eq!(body["savedLogin"], json!(true));
        assert_eq!(body["savedLoginId"], json!("sl_1"));
    }

    #[test]
    fn two_step_and_code_cards_take_their_half() {
        let form = |fields: Value| UserFormSpec::parse(&json!({ "formRequest": { "title": "x", "fields": fields } }), None).expect("form");
        let name_page = form(json!([{"id": "identifier", "label": "Email or phone", "type": "email"}]));
        assert_eq!(card_target(&name_page), Some(CardTarget::Username { username_id: "identifier".into() }));
        let password_page = form(json!([{"id": "Passwd", "label": "Password", "type": "password"}]));
        assert_eq!(card_target(&password_page), Some(CardTarget::Password { password_id: "Passwd".into() }));
        assert_eq!(password_page.continue_label(), "Continue");
        let code = form(json!([{"id": "otp", "label": "Code", "type": "otp"}]));
        assert_eq!(card_target(&code), Some(CardTarget::Code { code_id: "otp".into() }));
        let signup = form(json!([
            {"id": "email", "label": "Email", "type": "email"},
            {"id": "p1", "label": "Password", "type": "password"},
            {"id": "p2", "label": "Confirm", "type": "password"}
        ]));
        assert_eq!(card_target(&signup), None);
    }

    #[test]
    fn an_approval_park_and_other_frames_are_not_user_forms() {
        assert!(UserFormSpec::from_wire(&json!({"type": "CUSTOM", "name": "run-awaiting-approval", "reason": "exec-consent", "callId": "c"})).is_none());
        assert!(UserFormSpec::from_wire(&json!({"type": "CUSTOM", "name": "credential.offer_save", "value": {"origin": "x"}})).is_none());
        let call = UserFormSpec::from_wire(&json!({
            "tool": "request_user_form",
            "callId": "c1",
            "arguments": "{\"title\":\"Code\",\"fields\":[{\"id\":\"otp\",\"label\":\"Code\",\"type\":\"otp\"}]}"
        }))
        .expect("tool call");
        assert_eq!(call.call_id, "c1");
        assert!(!call.has_entry_id());
        assert_eq!(call.card_key(), "c1");
    }

    #[test]
    fn the_approval_names_the_site_asked_for_beside_the_saved_one() {
        let spec = |domain: Option<&str>, live: Option<&str>| UserFormSpec {
            domain: domain.map(str::to_owned),
            live_host: live.map(str::to_owned),
            ..Default::default()
        };
        // an agent's `domain` naming another site than the page it is on
        let odd = spec(Some("alice.pythonanywhere.com"), Some("https://mallory.pythonanywhere.com/login"));
        assert_eq!(login_origin(&odd).as_deref(), Some("alice.pythonanywhere.com"));
        assert_eq!(other_page(&odd).as_deref(), Some("mallory.pythonanywhere.com"));
        let same = spec(Some("https://localhost:3000"), Some("localhost:3000"));
        assert_eq!(login_origin(&same).as_deref(), Some("localhost:3000"));
        assert_eq!(other_page(&same), None);
        assert_eq!(other_page(&spec(None, Some("accounts.google.com"))), None);

        let row = SiteLogin { origin: "google.com".into(), username: "ada".into(), ..Default::default() };
        assert_eq!(use_reason(&row, "google.com"), "use the saved login for google.com as ada");
        // the other site's login, picked by hand on the mismatched card,
        // names both sites on the Touch ID sheet
        let alice = SiteLogin { id: "sl_a".into(), origin: "alice.pythonanywhere.com".into(), username: "alice".into(), ..Default::default() };
        assert_eq!(
            use_reason(&alice, &offered_site(&odd).unwrap()),
            "use the login saved for alice.pythonanywhere.com as alice on mallory.pythonanywhere.com"
        );
        assert_eq!(
            use_reason(&row, "accounts.google.com"),
            "use the login saved for google.com as ada on accounts.google.com"
        );
    }

    #[test]
    fn a_card_whose_domain_names_another_site_offers_only_its_pages_logins() {
        let spec = |domain: Option<&str>, live: Option<&str>| UserFormSpec {
            domain: domain.map(str::to_owned),
            live_host: live.map(str::to_owned),
            ..Default::default()
        };
        let odd = spec(Some("alice.pythonanywhere.com"), Some("https://mallory.pythonanywhere.com/login"));
        assert_eq!(offered_site(&odd).as_deref(), Some("mallory.pythonanywhere.com"), "the page's site is offered");
        assert_eq!(asked_site(&odd).as_deref(), Some("alice.pythonanywhere.com"), "the domain's only on request");
        // no mismatch: as before
        for same in [spec(Some("https://localhost:3000"), Some("localhost:3000")), spec(Some("example.com"), None), spec(None, Some("example.com"))] {
            assert_eq!(offered_site(&same), login_origin(&same));
            assert_eq!(asked_site(&same), None);
        }
        let row = |id: &str, origin: &str| SiteLogin { id: id.into(), origin: origin.into(), username: id.into(), ..Default::default() };
        let page = vec![row("m", "mallory.pythonanywhere.com")];
        let named = vec![row("a", "alice.pythonanywhere.com"), row("m", "mallory.pythonanywhere.com")];
        let ids = |rows: Vec<SiteLogin>| rows.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(listed(&page, &named, false)), ["m"], "the other site's login is not listed by itself");
        assert_eq!(ids(listed(&[], &named[..1], false)), Vec::<String>::new());
        assert_eq!(ids(listed(&page, &named, true)), ["m", "a"], "listed once asked for, each row once");
    }

    fn brokers_card(title: &str, instruction: &str) -> UserFormSpec {
        UserFormSpec {
            entry_id: "uf_1".into(),
            chat: "t_1".into(),
            bot_name: "Nova".into(),
            title: title.into(),
            instruction: Some(instruction.into()).filter(|i: &String| !i.is_empty()),
            domain: Some("shop.example".into()),
            takeover_note: Some(TAKEOVER_NOTE.into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_known_takeover_note_is_the_brokers() {
        let broker = include_str!("../../../broker/buwiz-broker/src/hub/tools/computer.rs");
        assert!(broker.contains(&format!("pub const TAKEOVER_NOTE: &str = \"{TAKEOVER_NOTE}\";")), "the broker's note changed: put its new words in short lines here");
    }

    #[test]
    fn the_popover_keeps_every_fact_of_the_takeover_note() {
        let spec = brokers_card("Sign in", "");
        let g = guidance(&spec);
        assert_eq!(g.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(), ["Fill in", DO_IT_MYSELF]);
        let all = g.iter().flat_map(|(_, l)| l.iter()).cloned().collect::<Vec<_>>().join(" ");
        // each sentence of the note, by what it says
        for fact in ["browser on its own screen", "isn't shown the screen and can't act through its tools until you hand back", "Commands it ran can keep running, but they can't see the screen or what you type", "Pages it opened, and anything it set going by clicking on this screen, could", "ended first, not always all of it", "Don't type a real password unless you trust this Bot's work so far", "Afterwards it can use whatever you signed in to", "shop.example", "not the AI", "never sees a password or code"] {
            assert!(all.contains(fact), "{fact:?} missing from {all}");
        }
        // a note this build doesn't know: the broker's words, a sentence a line
        let mut other = spec.clone();
        other.takeover_note = Some("It's yours. Hand it back after.".into());
        assert_eq!(guidance(&other)[1].1, ["It's yours.", "Hand it back after."]);
        assert_eq!(confirm_words(&other), "It's yours. Hand it back after.");
        assert!(confirm_words(&spec).contains("only use a real password if you trust this Bot's work so far"));
        // no note from the broker: the popover and the confirm step use the same default
        let mut bare = spec.clone();
        bare.takeover_note = None;
        assert_eq!(guidance(&bare), g, "the default note's lines");
        assert_eq!(confirm_words(&bare), confirm_words(&spec));
        // a card that isn't the broker's has no Do it myself
        let mut wire = spec.clone();
        wire.chat.clear();
        assert_eq!(guidance(&wire).len(), 1);
    }

    #[test]
    fn the_ais_words_are_its_bots_and_a_repeat_of_the_site_is_dropped() {
        let line = |t: &str, i: &str| ai_line(&brokers_card(t, i), "shop.example");
        assert_eq!(line("Sign in to the fixture shop", "Sign in to the fixture shop with the fake login."), Some("Nova: Sign in to the fixture shop with the fake login.".into()));
        assert_eq!(line("Sign in to shop.example", ""), None);
        assert_eq!(line("Log in", "Please sign in on shop.example"), None);
        assert_eq!(line("", ""), None);
        assert_eq!(line("Sign in to Shop", ""), Some("Nova: Sign in to Shop.".into()));
        // another domain in the AI's words stays visible, attributed, under the verified header
        assert_eq!(line("Sign in to bank.example", ""), Some("Nova: Sign in to bank.example.".into()));
        assert_eq!(line("Shop", "Use the test login"), Some("Nova: Shop. Use the test login.".into()));
        assert_eq!(headline(&brokers_card("Sign in to bank.example", "")), "Sign in on shop.example");
        let mut wire = brokers_card("Sign in to bank.example", "");
        wire.chat.clear();
        assert_eq!(headline(&wire), "Sign in to bank.example", "a card that isn't the broker's has no verified site");
    }

    #[test]
    fn the_caption_and_the_primary_say_what_the_broker_will_do_with_return() {
        let mut spec = brokers_card("Sign in", "");
        spec.return_press = Some("always".into());
        assert_eq!(primary_label(&spec), "Fill and send");
        assert_eq!(caption(&spec), "Typed on Nova's screen, then Return is pressed. Nothing is typed until you press Fill and send.");
        spec.return_press = Some("never".into());
        assert_eq!(primary_label(&spec), "Fill in");
        assert_eq!(caption(&spec), "Typed on Nova's screen; sending is left to you. Nothing is typed until you press Fill in.");
        // it depends on what's filled (a one-page card whose password may be left empty), or the
        // broker didn't say: nothing promised before the press
        for press in [Some("maybe"), None] {
            spec.return_press = press.map(str::to_owned);
            assert_eq!(primary_label(&spec), "Fill in");
            assert_eq!(caption(&spec), "Typed on Nova's screen. If the form is complete, Return may be pressed to send it. Nothing is typed until you press Fill in.");
            assert_eq!(guidance(&spec)[0].1[1], "Whether it presses Return depends on which fields are filled; the card says afterwards.");
        }
        let n = json!({"kind": "user_form", "id": "uf_1", "chat": "t_1", "state": "pending", "returnPress": "always"});
        assert_eq!(return_press(&UserFormSpec::from_notice(&n).unwrap()), ReturnPress::Always);
        // a card that isn't the broker's keeps its own words
        spec.chat.clear();
        spec.fields = parse_fields(&json!([{"id": "e", "label": "Email", "type": "email"}, {"id": "p", "label": "Password", "type": "password"}]));
        spec.same_page = true;
        assert_eq!(primary_label(&spec), "Log in");
    }

    #[test]
    fn every_ending_keeps_its_own_words() {
        let spec = brokers_card("Sign in", "");
        let f = |uncertain: bool, secret_maybe: bool, note: bool| Facts { uncertain, secret_maybe, note: note.then(|| "why".to_owned()), ..Facts::default() };
        let line = |r, f: Facts| receipt(&spec, r, &f).3;
        let none = || f(false, false, false);
        let want = [
            (FormResolution::Submitted, none(), "Filled in on shop.example. Secret values were never shown to the AI."),
            (FormResolution::FillFailed, f(true, true, false), "May have been typed on shop.example: check the screen. A secret value may have been typed into the page; it was never shown to the AI."),
            (FormResolution::FillFailed, f(true, false, false), "May have been typed on shop.example: check the screen. Secret values were never shown to the AI."),
            (FormResolution::FillFailed, none(), "Nothing was filled on shop.example: the page may have moved or changed. Secret values were never shown to the AI."),
            (FormResolution::Escalated, none(), "Handed over: Nova's screen is yours. Hand it back when you're done."),
            (FormResolution::Dismissed, none(), "Dismissed. Nothing was filled."),
            (FormResolution::Skipped, none(), "Skipped. Nothing was filled."),
            (FormResolution::TimedOut, none(), "Expired: nobody answered in time, so nothing was filled."),
            (FormResolution::Superseded, none(), "Moved on to your next message."),
            (FormResolution::Unsure, none(), "Check the screen: Buwiz Bot didn't hear how this went."),
        ];
        for (r, facts, words) in want.clone() {
            assert_eq!(line(r, facts), words, "{r:?}");
        }
        let lines: std::collections::BTreeSet<String> = want.iter().map(|(r, f, _)| line(*r, f.clone())).collect();
        assert_eq!(lines.len(), want.len(), "no two endings read alike");
        // with the broker's reason beside it, a failure doesn't guess at one
        assert_eq!(line(FormResolution::FillFailed, f(false, false, true)), "Nothing was filled on shop.example. Secret values were never shown to the AI.");
        // a may-have-typed ending is never "nothing was filled"
        assert!(!line(FormResolution::FillFailed, f(true, false, true)).contains("Nothing was filled") && !line(FormResolution::FillFailed, f(true, true, true)).contains("Nothing was filled"));
        assert!(!line(FormResolution::Unsure, none()).contains("Nothing"));
    }

    #[test]
    fn a_fills_receipt_is_made_of_the_notices_facts() {
        let mut spec = brokers_card("Sign in", "");
        spec.fields = parse_fields(&json!([{"id": "email", "label": "Email", "type": "email"}, {"id": "password", "label": "Password", "type": "password"}]));
        let facts = |n: Value| Facts::of(&n);
        let line = |r, f: &Facts| receipt(&spec, r, f).3;
        let both = json!([{"id": "email", "filled": true}, {"id": "password", "filled": true}]);
        assert_eq!(line(FormResolution::Submitted, &facts(json!({"pressed": true, "outcomes": both, "note": "anything"}))), "Filled in on shop.example. Return was pressed. Secret values were never shown to the AI.");
        // not pressed, or not said: the receipt doesn't guess, the note beside it says
        assert_eq!(line(FormResolution::Submitted, &facts(json!({"pressed": false, "outcomes": both}))), "Filled in on shop.example. Secret values were never shown to the AI.");
        // a stepped card: what went in, by label
        let first = json!([{"id": "email", "filled": true}, {"id": "password", "filled": false}]);
        assert_eq!(line(FormResolution::Submitted, &facts(json!({"pressed": false, "outcomes": first}))), "Email was typed on shop.example; Password wasn't. Secret values were never shown to the AI.");
        // the broker's prose never changes the line
        let a = facts(json!({"pressed": true, "outcomes": both, "note": "Typed, and Return was pressed."}));
        let b = facts(json!({"pressed": true, "outcomes": both, "note": "Something else entirely."}));
        assert_eq!(line(FormResolution::Submitted, &a), line(FormResolution::Submitted, &b));
    }

    #[test]
    fn a_notice_says_when_a_value_may_have_gone_in() {
        let n = json!({"kind": "user_form", "id": "uf_1", "chat": "t_1", "state": "fill_failed", "uncertain": true, "secretMaybe": true, "note": "x"});
        let spec = UserFormSpec::from_notice(&n).unwrap();
        assert!(spec.facts.uncertain && spec.facts.secret_maybe && spec.facts.note.as_deref() == Some("x"));
        let n = json!({"kind": "user_form", "id": "uf_1", "chat": "t_1", "state": "fill_failed"});
        assert!(!UserFormSpec::from_notice(&n).unwrap().facts.uncertain);
        assert_eq!(sentences("A. B! c? d"), ["A.", "B!", "c?", "d"]);
        assert_eq!(sentences("Go to shop.example now."), ["Go to shop.example now."]);
    }
}
