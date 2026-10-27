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
//! Nothing secret is put on a turn, in the store, in a log line or in a
//! `Debug` print: [`Values`] and [`Field`] redact theirs.
//!
//! # Answers
//!
//! `POST /ag-ui/user-form/submit {entryId, agentId, values, savedLogin?,
//! savedLoginId?}` and `POST /ag-ui/user-form/dismiss {entryId, agentId,
//! mode}`, sent and read by `cards/transport.rs` (nativechat
//! `user_form_action_from_http`). Sending is optimistic. A server without
//! the routes paints the card's own ending, toasts "Not connected to an
//! agent server yet" and logs the body it would have sent, values
//! redacted — so every flow can be clicked through offline. Where the card
//! stands (sending, settled, a failure's sentence) is the cards' shared
//! state by key (`cards::state_of`), so the composer's "Waiting for you"
//! and a new message's supersede see it; what is typed stays here.

use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::transport::{self, UserFormReply};
use super::{CardState, agent_id, answerable, close_x, error_line, note, save_login, set_state, shell, state_of, title};
use crate::live::store;
use crate::theme;
use crate::vault::{self, PendingSave, Secret, SiteLogin, Unlock, off_main, registrable_origin, vault};

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
        }
    }

    pub(crate) fn pill(self) -> &'static str {
        match self {
            Self::Sending => "Submitting",
            Self::Submitted => "Submitted",
            Self::FillFailed => "Not filled",
            Self::Escalated => "On the computer",
            Self::Dismissed => "Dismissed",
            Self::Skipped => "Skipped",
            Self::TimedOut => "Timed out",
            Self::Superseded => "Superseded",
        }
    }

    pub(crate) fn body(self) -> &'static str {
        match self {
            Self::Sending => "Submitting…",
            Self::Submitted => "Sent to your agent. Secret values were never shown to it.",
            Self::FillFailed => "Nothing was filled — the page may have moved or changed. Secret values were never shown to your agent.",
            Self::Escalated => "Handed to the computer screen to finish there.",
            Self::Dismissed => "Dismissed without filling anything.",
            Self::Skipped => "Skipped without filling anything.",
            Self::TimedOut => "Nobody answered in time, so nothing was filled.",
            Self::Superseded => "Moved on to your next message.",
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
}

impl UserFormSpec {
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
    pub(crate) fn same_card(&self, other: &Self) -> bool {
        if self.has_entry_id() && other.has_entry_id() {
            return self.entry_id == other.entry_id;
        }
        !self.call_id.is_empty() && self.call_id == other.call_id
    }

    /// Folds a later frame of the same card onto this one: ids it lacked,
    /// a settlement, fields it adds. A field the card shows is never
    /// dropped by a narrower envelope.
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

/// The site a card's logins are filed under: the registrable origin of its
/// `domain`, else of its `liveHost`.
pub(crate) fn login_origin(spec: &UserFormSpec) -> Option<String> {
    spec.domain
        .as_deref()
        .and_then(registrable_origin)
        .or_else(|| spec.live_host.as_deref().and_then(registrable_origin))
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
    Some(PendingSave { origin, username, password: Secret::new(password) })
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
    /// The ending was painted here because no server answered.
    offline: Signal<bool>,
    /// Try again on a card the wire settled as not filled.
    retry: Signal<bool>,
}

/// A saved login held for the submit. The secret is never painted and
/// leaves memory with the submit (or Type it by hand, or the card's end).
struct Held {
    login_id: String,
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
        offline: signal(false),
        retry: signal(false),
    });
    reg.borrow_mut().insert(key.to_owned(), l);
    l
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

/// The agent the answer is for: the card's own, else the open chat's bot.
fn agent_of(spec: &UserFormSpec) -> String {
    if spec.agent_id.is_empty() { agent_id() } else { spec.agent_id.clone() }
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
    set_state(&key, CardState::Sending);
    let agent = agent_of(&spec);
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
            let body = transport::submit_body(&spec.entry_id, &agent, values.into_map(), login_id.as_deref(), None);
            let reply = transport::submit_user_form(body).await;
            let entry = spec.entry_id.as_str();
            match reply {
                UserFormReply::Settled(body) => {
                    let r = settled_from(&body);
                    settle(&key, r);
                    if r == FormResolution::Submitted
                        && let Some(id) = &login_id
                    {
                        vault().mark_used(id);
                    }
                }
                UserFormReply::Refused(why) | UserFormReply::Failed(why) => {
                    vault().take_pending(entry);
                    set_state(&key, CardState::Error(why));
                }
                UserFormReply::Empty | UserFormReply::AlreadyAnswered | UserFormReply::MissingEntry => {
                    vault().take_pending(entry);
                    settle(&key, FormResolution::FillFailed);
                }
                UserFormReply::MissingRoute => {
                    // painted here; the transport toasted and logged it
                    loc.offline.set(true);
                    settle(&key, FormResolution::Submitted);
                }
            }
        });
    });
}

fn dismiss(spec: Rc<UserFormSpec>, key: String) {
    let loc = local(&key);
    drop_held(&key);
    forget_secrets(&spec, loc);
    loc.pick.set(Pick::None);
    set_state(&key, CardState::Dismissed);
    if !spec.has_entry_id() {
        // a call-only card settles here; there is nothing to POST
        return;
    }
    let body = transport::dismiss_body(&spec.entry_id, &agent_of(&spec), "dismissed");
    Owner::detached().run(|| {
        spawn(async move {
            match transport::dismiss_user_form(body).await {
                UserFormReply::Settled(body) => settle(&key, settled_from(&body)),
                UserFormReply::Empty | UserFormReply::AlreadyAnswered => {}
                UserFormReply::MissingRoute => loc.offline.set(true),
                UserFormReply::MissingEntry => reopen(&key),
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

/// Touch ID, then the pick: its name into the name field, its secret held
/// for the submit — never into a field, never on screen.
fn use_saved(key: String, row: SiteLogin, target: CardTarget) {
    let loc = local(&key);
    if matches!(loc.pick.get_untracked(), Pick::Confirming { .. }) {
        return;
    }
    loc.pick.set(Pick::Confirming { username: row.username.clone() });
    let reason = format!("use the saved login for {} as {}", row.origin, row.username);
    let reads = target.secret_field().is_some();
    let r = row.clone();
    Owner::detached().run(|| {
        spawn(async move {
            let got = off_main(move || {
                if reads {
                    vault().unlock(&r, &reason)
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
            let code = target.kind() == vault::KIND_CODE;
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
            (Some(r), _) => settled(spec.clone(), key.clone(), r, false).into_element(),
            (None, Phase::Settled(r)) => settled(spec.clone(), key.clone(), r, true).into_element(),
            (None, Phase::Sending) => collapsed(&spec, &key, FormResolution::Sending).into_element(),
            (None, Phase::Idle) => idle(spec.clone(), key.clone()).into_element(),
        }
    })
}

fn title_of(spec: &UserFormSpec) -> String {
    if spec.title.is_empty() { "Form".to_owned() } else { spec.title.clone() }
}

fn collapsed(spec: &UserFormSpec, key: &str, r: FormResolution) -> View {
    let (fill, ink) = match r {
        FormResolution::Submitted => (theme::UP_TINT, theme::UP),
        FormResolution::FillFailed => (theme::DOWN_TINT, theme::DOWN),
        _ => (theme::HOVER, theme::DIM),
    };
    let pill = if r == FormResolution::Submitted { format!("✓ {}", r.pill()) } else { r.pill().to_owned() };
    shell(&format!("user-form-{key}"))
        .label(format!("{} — {}", title_of(spec), r.pill()))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(title(title_of(spec)).flex_1().min_w(px(0.0)).number_of_lines(1))
                .child(
                    text(pill)
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(ink)
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(999.0))
                        .bg(fill),
                ),
        )
        .child(note(r.body().to_owned()))
}

fn settled(spec: Rc<UserFormSpec>, key: String, r: FormResolution, local_paint: bool) -> View {
    let loc = local(&key);
    // an ending (a supersede included) takes any held or typed secret with it
    drop_held(&key);
    forget_secrets(&spec, loc);
    let mut v = collapsed(&spec, &key, r);
    if r == FormResolution::FillFailed && spec.has_entry_id() {
        let (s1, k1, s2, k2) = (spec.clone(), key.clone(), spec.clone(), key.clone());
        v = v.child(
            view()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(button("Try again", Kind::Primary, || true, move || retry(&s1, &k1)))
                .child(button("Stop for now", Kind::Ghost, || true, move || dismiss(s2.clone(), k2.clone()))),
        );
    }
    // offline, no server will offer to save the login: the card does
    if local_paint && r == FormResolution::Submitted && loc.offline.get_untracked() {
        let entry = spec.entry_id.clone();
        if let Some((origin, username)) = vault().pending_who(&entry) {
            let offer = save_login::SaveLoginSpec { form_entry_id: entry.clone(), origin, username };
            v = v.child(save_login::card(offer, format!("save-login-{entry}")));
        }
    }
    v
}

fn idle(spec: Rc<UserFormSpec>, key: String) -> View {
    let loc = local(&key);
    let target = card_target(&spec);
    let rows: Vec<SiteLogin> = match (&target, login_origin(&spec)) {
        (Some(t), Some(origin)) => vault().rows_for(&origin, t.kind()),
        _ => Vec::new(),
    };
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
    let (s_dis, k_dis) = (spec.clone(), key.clone());
    let mut v = shell(&format!("user-form-{key}")).label(title_of(&spec)).child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(title(title_of(&spec)).flex_1().min_w(px(0.0)).number_of_lines(2))
            .child(close_x("Dismiss", move || dismiss(s_dis.clone(), k_dis.clone()))),
    );
    if let Some(i) = spec.instruction.as_ref().filter(|i| !i.is_empty()) {
        v = v.child(note(i.clone()));
    }
    if let Some(t) = target.clone().filter(|_| !rows.is_empty()) {
        v = v.child(saved_strip(key.clone(), rows, t));
    }
    for f in &spec.fields {
        v = v.child(field_view(key.clone(), f.clone(), target.clone()));
    }
    let k_note = key.clone();
    v = v.child(dynamic(move || {
        let why = phase(&k_note).1;
        if why.is_empty() {
            return view().into_element();
        }
        error_line(why).number_of_lines(4).into_element()
    }));
    let can_post = spec.has_entry_id();
    let (s_go, k_go, s_en) = (spec.clone(), key.clone(), spec.clone());
    let t_en = target.clone();
    let enabled = move || {
        let held: Vec<&str> = match (loc.pick.get(), &t_en) {
            (Pick::Ready { .. }, Some(t)) => t.secret_field().into_iter().collect(),
            _ => Vec::new(),
        };
        can_post && s_en.required_filled(&loc.values.get(), &held)
    };
    let mut actions = view().flex_row().items_center().justify_end().gap_2();
    if !can_post {
        actions = actions.child(
            text("This form has no server entry yet, so it can't be sent.")
                .flex_1()
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        );
    }
    v.child(actions.child(button(spec.continue_label(), Kind::Primary, enabled, move || submit(s_go.clone(), k_go.clone()))))
}

/// The saved logins for the card's site: Use (Touch ID), or Type it by hand.
fn saved_strip(key: String, rows: Vec<SiteLogin>, target: CardTarget) -> impl IntoElement {
    let loc = local(&key);
    dynamic(move || {
        let pick = loc.pick.get();
        let typing = loc.by_hand.get();
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
            CardTarget::Code { .. } => "From your saved logins. Touch ID fills in its code.",
            CardTarget::Username { .. } => "From your saved logins. Touch ID fills in its name.",
            _ => "From your saved logins. Touch ID fills it in; the password is never shown.",
        };
        v = v.child(text(hint).text_size(px(12.0)).text_color(theme::MUTED));
        for row in &rows {
            let (k, r, t) = (key.clone(), row.clone(), target.clone());
            v = v.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(theme::SUBTLE)
                    .label(format!("Use {}", row.username))
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                text(row.username.clone())
                                    .number_of_lines(1)
                                    .text_size(px(13.0))
                                    .text_color(theme::TEXT),
                            )
                            .child(text(row.origin.clone()).number_of_lines(1).text_size(px(11.0)).text_color(theme::MUTED)),
                    )
                    .child(button("Use", Kind::Secondary, || true, move || use_saved(k.clone(), r.clone(), t.clone()))),
            );
        }
        let k = key.clone();
        v.child(view().flex_row().child(link_btn("Type it by hand", move || by_hand(&k)))).into_element()
    })
}

fn field_view(key: String, f: Field, target: Option<CardTarget>) -> AnyElement {
    let loc = local(&key);
    let mut label = view().flex_row().child(
        text(f.label.clone())
            .text_size(px(13.0))
            .line_height(px(18.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::TEXT),
    );
    if !f.required && f.kind != FieldKind::Checkbox {
        label = label.child(text(" (optional)").text_size(px(13.0)).line_height(px(18.0)).text_color(theme::MUTED));
    }
    let is_held = target.as_ref().and_then(CardTarget::secret_field) == Some(f.id.as_str());
    let control = match f.kind {
        FieldKind::Checkbox => return checkbox(key, f).into_element(),
        FieldKind::Select if !f.options.is_empty() => chips(key, f.clone()).into_element(),
        _ if is_held => {
            let (k, f2) = (key.clone(), f.clone());
            dynamic(move || match loc.pick.get() {
                Pick::Ready { username } => locked(&k, &username, target.as_ref()).into_element(),
                _ => text_input(k.clone(), f2.clone()).into_element(),
            })
            .into_element()
        }
        _ => text_input(key, f.clone()).into_element(),
    };
    view().gap(px(4.0)).child(label).child(control).into_element()
}

fn text_input(key: String, f: Field) -> impl IntoElement {
    let loc = local(&key);
    let (id1, id2) = (f.id.clone(), f.id.clone());
    let multiline = f.kind == FieldKind::Textarea;
    let placeholder = f.placeholder.clone().unwrap_or_else(|| match f.kind {
        FieldKind::Date => "YYYY-MM-DD".to_owned(),
        FieldKind::Email => "name@example.com".to_owned(),
        _ => String::new(),
    });
    let h = if multiline { px(72.0) } else { px(32.0) };
    view()
        .h(h)
        .rounded(px(8.0))
        .bg(theme::HOVER)
        .px(px(10.0))
        .justify_center()
        .child(
            input()
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
                }),
        )
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

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Primary,
    Secondary,
    Ghost,
}

fn button(label: &'static str, kind: Kind, enabled: impl Fn() -> bool + 'static, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    let enabled = Rc::new(enabled);
    let (e1, e2, e3) = (enabled.clone(), enabled.clone(), enabled);
    text(label)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(14.0))
        .text_color(move || match kind {
            Kind::Primary => theme::ON_FILL,
            _ => theme::TEXT,
        })
        .bg(move || match (kind, hov.get()) {
            (Kind::Primary, _) => theme::ACCENT,
            (Kind::Secondary, false) => theme::HOVER,
            (Kind::Secondary, true) | (Kind::Ghost, true) => theme::ACTIVE,
            (Kind::Ghost, false) => AdaptiveColor::TRANSPARENT,
        })
        .opacity(move || if e1() { 1.0f32 } else { 0.4 })
        .on_hover_in(move || hov.set(e2()))
        .on_hover_out(move || hov.set(false))
        .on_press(move || {
            if e3() {
                on()
            }
        })
        .label(label)
}

fn link_btn(label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    text(label)
        .text_size(px(12.0))
        .text_color(move || if hov.get() { theme::TEXT } else { theme::ACCENT })
        .on_hover_in(move || hov.set(true))
        .on_hover_out(move || hov.set(false))
        .on_press(move || on())
        .label(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE: &str = "/Volumes/goldcoders/OSS/nativechat/fixtures/wire";

    /// A fixture under nativechat's `fixtures/wire`; `None` (the test
    /// skips) where that checkout is missing.
    fn fixture(rel: &str) -> Option<Value> {
        let text = std::fs::read_to_string(format!("{WIRE}/{rel}")).ok()?;
        Some(serde_json::from_str(&text).expect("fixture json"))
    }

    fn fixtures_in(dir: &str) -> Vec<(String, Value)> {
        let Ok(rd) = std::fs::read_dir(format!("{WIRE}/{dir}")) else { return Vec::new() };
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
        assert_eq!(login_origin(&settled).as_deref(), Some("google.com"));

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
        assert_eq!((save.origin.as_str(), save.username.as_str()), ("example.com", "ada@example.com"));
        assert!(!format!("{save:?}").contains("hunter2"));
        assert_eq!(Field::checkbox_wire(true), "true");
        let body = transport::submit_body("e_1", "cw_1", values.into_map(), Some("sl_1"), None);
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
}
