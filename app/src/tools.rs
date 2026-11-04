//! Bot tools and this Mac's command rules (bot controls, PR 1).
//!
//! - The `tools_change` card in the chat (`chat.rs` draws it with
//!   [`notice_row`]): a Bot's widen waits for a click (`pending`); a change
//!   that went through (`applied`) leaves one centered receipt line with
//!   Undo for 8 s from when this app first drew it, then nothing. Declined, superseded, expired and stale
//!   notices draw nothing.
//! - The `/tools` sheet ([`sheet_layer`], 560 wide over a scrim) and its full
//!   view ([`full_layer`], a stack layer like the mini apps), with the tabs
//!   This Bot, This Mac and Activity. Both read `GET /v1/host/tools` on
//!   open, on a tab change and after each change.
//!
//! The human path is the broker's `/v1/host/tools*` and `/v1/host/shell/*`
//! routes (they carry the host token, `client::is_host_route`). A refusal
//! answers `{ok:false, code, message}`.

use std::cell::Cell;
use std::collections::HashMap;

use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::cards::{self, Btn};
use crate::live::store;
use crate::store::ToolsChangeNotice;
use crate::theme;

/// How long an applied change's receipt shows.
pub(crate) const RECEIPT_MS: i64 = 8_000;
/// How long "Put back." shows after an Undo.
pub(crate) const PUT_BACK_MS: i64 = 3_000;
/// A notice first drawn here this long after its change is history (a
/// reload, a restart): no receipt.
pub(crate) const HISTORY_MS: i64 = 5 * 60_000;

const NOT_CONNECTED: &str = "Not connected to an agent server yet";
const STALE_ANSWER: &str = "That changed since you looked. Ask Hex again.";
const STALE_UNDO: &str = "Undo is no longer possible: it changed since.";
const PUT_BACK: &str = "Put back.";

/// The sheet's and the full view's tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tab {
    Bot,
    Mac,
    Activity,
}

/// Which surface is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Sheet,
    Full,
}

/// The open surface and the Bot it is for ("" when the chat has none).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Open {
    pub kind: Kind,
    pub bot: String,
    pub name: String,
}

/// What `/tools` and ⌘K's "Bot tools…" open: the sheet for the chat's
/// Bot (id, name), on This Bot.
pub(crate) fn sheet_for(bot: Option<(String, String)>) -> (Open, Tab) {
    let (bot, name) = bot.unwrap_or_default();
    (Open { kind: Kind::Sheet, bot, name }, Tab::Bot)
}

/// One switch row of This Bot.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct ToolRow {
    pub key: String,
    pub label: String,
    pub sub: String,
    pub on: bool,
}

/// One "Always allowed" rule.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct AllowRule {
    pub pattern: String,
    pub risk: String,
    pub inert: String,
}

/// One Activity row.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct Activity {
    pub at: i64,
    pub bot: String,
    pub bot_name: String,
    pub command: String,
    pub outcome: String,
}

/// One "Recent changes" row (24 h).
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct Recent {
    pub at: i64,
    pub actor: String,
    pub text: String,
    pub undo: String,
}

/// `GET /v1/host/tools`.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct Data {
    pub bot_id: String,
    pub bot_name: String,
    pub tools: Vec<ToolRow>,
    /// `only` or `bypass`.
    pub mode: String,
    pub allow: Vec<AllowRule>,
    pub block: Vec<String>,
    pub activity: Vec<Activity>,
    pub recent: Vec<Recent>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

fn ms(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

impl Data {
    /// The route's answer, or its refusal's words.
    pub(crate) fn of(v: &Value) -> Result<Data, String> {
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(refusal(v, ""));
        }
        let bot = v.get("bot").cloned().unwrap_or_default();
        let mac = v.get("mac").cloned().unwrap_or_default();
        Ok(Data {
            bot_id: s(&bot, "id"),
            bot_name: s(&bot, "name"),
            tools: list(v, "tools")
                .iter()
                .map(|t| ToolRow { key: s(t, "key"), label: s(t, "label"), sub: s(t, "sub"), on: t.get("on").and_then(Value::as_bool).unwrap_or(true) })
                .filter(|t| !t.key.is_empty())
                .collect(),
            mode: Some(s(&mac, "mode")).filter(|m| !m.is_empty()).unwrap_or_else(|| "bypass".to_owned()),
            allow: list(&mac, "allow")
                .iter()
                .map(|r| AllowRule { pattern: s(r, "pattern"), risk: s(r, "risk"), inert: s(r, "inert") })
                .filter(|r| !r.pattern.is_empty())
                .collect(),
            block: list(&mac, "block").iter().map(|r| s(r, "pattern")).filter(|p| !p.is_empty()).collect(),
            activity: list(v, "activity")
                .iter()
                .map(|a| Activity { at: ms(a, "at"), bot: s(a, "bot"), bot_name: s(a, "botName"), command: s(a, "command"), outcome: s(a, "outcome") })
                .collect(),
            recent: list(v, "recent")
                .iter()
                .map(|r| Recent { at: ms(r, "at"), actor: s(r, "actor"), text: s(r, "text"), undo: s(r, "undo") })
                .filter(|r| !r.text.is_empty())
                .collect(),
        })
    }

    /// "n of m on".
    pub(crate) fn on_count(&self) -> (usize, usize) {
        (self.tools.iter().filter(|t| t.on).count(), self.tools.len())
    }
}

/// A human edit's receipt: its `text`, then its `note` (the `*` sentence).
pub(crate) fn receipt_text(v: &Value) -> String {
    [s(v, "text"), s(v, "note")].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
}

/// A rule the form couldn't save, in words: the broker's own sentence for
/// a refusal it words (`refused`, `exists`), else "Couldn't save the rule: …".
pub(crate) fn rule_error(v: Option<&Value>) -> String {
    let Some(v) = v else { return NOT_CONNECTED.to_owned() };
    let words = refusal(v, "");
    if matches!(s(v, "code").as_str(), "refused" | "exists") {
        words
    } else {
        format!("Couldn't save the rule: {}.", words.trim_end_matches('.'))
    }
}

/// A refusal's words: `stale` is the sentence `stale` gives ("" for the
/// server's own), else the server's message, else its code.
pub(crate) fn refusal(v: &Value, stale: &str) -> String {
    let (code, message) = (s(v, "code"), s(v, "message"));
    if code == "stale" && !stale.is_empty() {
        return stale.to_owned();
    }
    if !message.is_empty() {
        message
    } else if !code.is_empty() {
        code
    } else {
        "The server refused it".to_owned()
    }
}

/// A human edit's receipt in the sheet, or an Undo's answer.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Receipt {
    pub text: String,
    pub undo: String,
    /// When it was made (ms).
    pub at: i64,
    /// Shown in red.
    pub error: bool,
}

/// An inline card's state here, before the chat reloads.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct CardLocal {
    /// A two-step card shows its second step.
    pub confirm: bool,
    pub busy: bool,
    pub error: String,
    /// The notice as the answer left it.
    pub settled: Option<ToolsChangeNotice>,
    /// The receipt's Undo was answered (ms): "Put back." or `undo_error`.
    pub undone_at: i64,
    pub undo_error: String,
}

/// The add form of This Mac.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FormKind {
    Allow,
    Block,
}

impl FormKind {
    fn kind(self) -> &'static str {
        match self {
            FormKind::Allow => "allow",
            FormKind::Block => "block",
        }
    }
}

/// What the Tools surfaces keep (`Store::tools`).
pub struct Tools {
    pub(crate) open: Signal<Option<Open>>,
    pub(crate) tab: Signal<Tab>,
    pub(crate) data: Signal<Option<Data>>,
    pub(crate) loading: Signal<bool>,
    /// Why the last read failed.
    pub(crate) error: Signal<String>,
    /// The inline "Yes, bypass" confirm of This Mac.
    pub(crate) mode_ask: Signal<bool>,
    pub(crate) form: Signal<Option<FormKind>>,
    pub(crate) form_text: Signal<String>,
    /// The form's Save asks again (a rule that runs any code).
    pub(crate) form_confirm: Signal<bool>,
    pub(crate) form_error: Signal<String>,
    /// The block whose Remove asks "Stop blocking?".
    pub(crate) rm_ask: Signal<Option<String>>,
    /// What is being changed (a tool key, `allow:…`, `block:…`, `mode`).
    pub(crate) busy: Signal<Option<String>>,
    pub(crate) receipt: Signal<Option<Receipt>>,
    /// The chat's cards by proposal id.
    pub(crate) cards: Signal<HashMap<String, CardLocal>>,
    /// When this app first drew each notice (ms), by notice id: a receipt
    /// counts from there, not from the change.
    pub(crate) seen: std::cell::RefCell<HashMap<String, i64>>,
    /// Now (ms), ticking while a receipt shows.
    pub(crate) clock: Signal<i64>,
}

impl Tools {
    pub fn new() -> Tools {
        Tools {
            open: signal(None),
            tab: signal(Tab::Bot),
            data: signal(None),
            loading: signal(false),
            error: signal(String::new()),
            mode_ask: signal(false),
            form: signal(None),
            form_text: signal(String::new()),
            form_confirm: signal(false),
            form_error: signal(String::new()),
            rm_ask: signal(None),
            busy: signal(None),
            receipt: signal(None),
            cards: signal(HashMap::new()),
            seen: std::cell::RefCell::new(HashMap::new()),
            clock: signal(0),
        }
    }
}

impl Default for Tools {
    fn default() -> Self {
        Self::new()
    }
}

fn tools() -> &'static Tools {
    &store().tools
}

/// Now, in ms since the epoch.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Whether a receipt made at `at` still shows at `now`.
pub(crate) fn receipt_fresh(at: i64, now: i64) -> bool {
    at > 0 && now >= at - 1_000 && now - at < RECEIPT_MS
}

/// When an applied notice's receipt starts: its change's time `at`, or
/// when this app first drew it (`seen`) if that came later, so a reply
/// that outlasts the 8 s still leaves it to see; 0 (none) for a notice
/// first drawn over [`HISTORY_MS`] after its change.
pub(crate) fn receipt_start(at: i64, seen: i64) -> i64 {
    if at <= 0 || seen - at > HISTORY_MS { 0 } else { at.max(seen) }
}

/// When this app first drew notice `id` (now, the first time).
fn first_seen(id: &str) -> i64 {
    *tools().seen.borrow_mut().entry(id.to_owned()).or_insert_with(now_ms)
}

/// The draining bar's share left (1 at `at`, 0 at 8 s).
pub(crate) fn drain(at: i64, now: i64) -> f32 {
    (1.0 - (now - at).max(0) as f32 / RECEIPT_MS as f32).clamp(0.0, 1.0)
}

thread_local! {
    static DEADLINE: Cell<i64> = const { Cell::new(0) };
    static TICKING: Cell<bool> = const { Cell::new(false) };
}

/// Ticks [`Tools::clock`] every 100 ms until `until` (ms).
fn watch(until: i64) {
    DEADLINE.set(DEADLINE.get().max(until));
    if TICKING.replace(true) {
        return;
    }
    crate::store::spawn_detached(async {
        loop {
            timer(std::time::Duration::from_millis(100)).await;
            let now = now_ms();
            tools().clock.set(now);
            if now > DEADLINE.get() {
                TICKING.set(false);
                break;
            }
        }
    });
}

/// The clock, tracked.
fn clock() -> i64 {
    tools().clock.get().max(now_ms())
}

// ---------------------------------------------------------------------------
// Opening and closing
// ---------------------------------------------------------------------------

/// `/tools` and ⌘K: the sheet for the chat's Bot.
pub(crate) fn open_sheet() {
    let bot = store().active_bot_untracked().map(|b| (b.id, b.name));
    let (open, tab) = sheet_for(bot);
    open_at(open, tab);
}

fn open_at(open: Open, tab: Tab) {
    let t = tools();
    reset_inline();
    t.receipt.set(None);
    t.open.set(Some(open));
    t.tab.set(tab);
    load();
}

/// "Open full view ⤢": the same Bot and tab, full screen.
fn open_full() {
    let t = tools();
    t.open.update(|o| {
        if let Some(o) = o {
            o.kind = Kind::Full;
        }
    });
    load();
}

/// ✕, ⌘W, Esc, ⌘[: closed, back to the chat.
pub(crate) fn close() {
    let t = tools();
    if t.open.get_untracked().is_none() {
        return;
    }
    t.open.set(None);
    reset_inline();
    crate::composer::focus();
}

/// Whether a Tools surface is open (tracked).
pub(crate) fn is_open() -> bool {
    tools().open.with(Option::is_some)
}

/// Escape: an open form or confirm closes first, then the surface.
pub(crate) fn escape() {
    let t = tools();
    if t.form.get_untracked().is_some() || t.mode_ask.get_untracked() || t.rm_ask.get_untracked().is_some() {
        reset_inline();
    } else {
        close();
    }
}

fn reset_inline() {
    let t = tools();
    t.mode_ask.set(false);
    t.form.set(None);
    t.form_text.set(String::new());
    t.form_confirm.set(false);
    t.form_error.set(String::new());
    t.rm_ask.set(None);
}

fn set_tab(tab: Tab) {
    reset_inline();
    tools().tab.set(tab);
    load();
}

/// `GET /v1/host/tools` for the open Bot.
fn load() {
    let Some(open) = tools().open.get_untracked() else { return };
    tools().loading.set(true);
    let path = if open.bot.is_empty() { "/v1/host/tools".to_owned() } else { format!("/v1/host/tools?bot={}", crate::client::urlenc(&open.bot)) };
    crate::store::spawn_detached(async move {
        let r = match crate::client::get_any(&path).await {
            None => Err(NOT_CONNECTED.to_owned()),
            Some(v) => Data::of(&v),
        };
        let t = tools();
        t.loading.set(false);
        match r {
            Ok(d) => {
                t.data.set(Some(d));
                t.error.set(String::new());
            }
            Err(e) => t.error.set(e),
        }
    });
}

/// POST `path`: its answer when `ok`, else the refusal's words.
async fn call(path: &str, body: Value, stale: &str) -> Result<Value, String> {
    match crate::client::post_any(path, body).await {
        None => Err(NOT_CONNECTED.to_owned()),
        Some(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Ok(v),
        Some(v) => Err(refusal(&v, stale)),
    }
}

/// A human edit from the sheet: applied at once, its receipt with Undo,
/// then the data again. `fail` words a refusal.
fn edit(busy: String, path: &'static str, body: Value, fail: fn(String) -> String, then: impl FnOnce(bool) + 'static) {
    let t = tools();
    t.busy.set(Some(busy));
    crate::store::spawn_detached(async move {
        let r = call(path, body, "").await;
        let t = tools();
        t.busy.set(None);
        let now = now_ms();
        match r {
            Ok(v) => {
                t.receipt.set(Some(Receipt { text: receipt_text(&v), undo: s(&v, "undo"), at: now, error: false }));
                watch(now + RECEIPT_MS);
                then(true);
            }
            Err(e) => {
                t.receipt.set(Some(Receipt { text: fail(e), undo: String::new(), at: now, error: true }));
                watch(now + RECEIPT_MS);
                then(false);
            }
        }
        load();
    });
}

/// An Undo (a receipt's, or a Recent change's): "Put back." for 3 s.
fn undo_in_sheet(undo: String) {
    crate::store::spawn_detached(async move {
        let r = call("/v1/host/tools/undo", json!({"undo": undo}), STALE_UNDO).await;
        let now = now_ms();
        let (text, error) = match r {
            Ok(v) => (Some(s(&v, "text")).filter(|t| !t.is_empty()).unwrap_or_else(|| PUT_BACK.to_owned()), false),
            Err(e) => (e, true),
        };
        // shown until 3 s from now (the receipt's 8 s, started 5 s ago)
        tools().receipt.set(Some(Receipt { text, undo: String::new(), at: now - (RECEIPT_MS - PUT_BACK_MS), error }));
        watch(now + PUT_BACK_MS);
        load();
    });
}

// ---------------------------------------------------------------------------
// Copy (spec section 7)
// ---------------------------------------------------------------------------

/// A risk class's chip.
pub(crate) fn risk_label(risk: &str) -> &'static str {
    match risk {
        "ro" => "read-only",
        "pc" => "runs project code",
        "any" => "runs any code",
        "never" => "never allowed",
        _ => "",
    }
}

/// A pending card's words: title, notes, the main button, and the second
/// step's sentence and button when it asks twice.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct CardCopy {
    pub title: String,
    pub notes: Vec<String>,
    pub primary: String,
    pub confirm: Option<(String, String)>,
    /// The risk chip ("" for none).
    pub chip: &'static str,
    /// The warning band ("" for none).
    pub warn: String,
}

/// How a tool key reads in a sentence: "the shell", else its label.
fn tool_words(n: &ToolsChangeNotice) -> String {
    if n.key == "shell" {
        "the shell".to_owned()
    } else if !n.label.is_empty() {
        n.label.clone()
    } else {
        n.key.clone()
    }
}

/// The card a pending notice asks with (A1 to A5); `None` for an op that
/// never waits for a click.
pub(crate) fn card_copy(n: &ToolsChangeNotice) -> Option<CardCopy> {
    let bot = n.who().to_owned();
    let two = n.steps == 2;
    let warn = if n.unrequested { format!("{bot} suggested this after reading a web page. You didn't ask for it.") } else { String::new() };
    let mut c = match n.op.as_str() {
        "rule_add" => {
            let r = n.rule.as_ref()?;
            if r.kind != "allow" {
                return None;
            }
            let p = &r.pattern;
            let mut notes = Vec::new();
            if r.risk == "any" {
                if !r.path.is_empty() {
                    notes.push(format!("Resolved to {}", r.path));
                }
                notes.push(format!("It runs commands and changes files itself. Allowing it lets {bot} do the same without asking, for every Bot on this Mac."));
            } else {
                notes.push(format!("{bot} asked for this so it can run the command without asking."));
                notes.push(format!("Covers any command that starts with `{p}`, for every Bot on this Mac."));
            }
            if !n.inert.is_empty() {
                notes.push(format!("Not in effect: {}.", n.inert.trim_end_matches('.')));
            }
            CardCopy {
                title: format!("Always allow `{p}` on this Mac?"),
                notes,
                primary: if two { "Always allow…".to_owned() } else { "Always allow".to_owned() },
                confirm: two.then(|| (format!("Yes, always allow {p}."), "Always allow".to_owned())),
                chip: risk_label(&r.risk),
                warn: String::new(),
            }
        }
        "rule_remove" => {
            let r = n.rule.as_ref()?;
            if r.kind != "block" {
                return None;
            }
            CardCopy {
                title: format!("Stop blocking `{}` on this Mac?", r.pattern),
                notes: vec!["Commands that start with it could run again, for every Bot.".to_owned()],
                primary: "Stop blocking".to_owned(),
                confirm: two.then(|| (format!("Yes, stop blocking {}.", r.pattern), "Stop blocking".to_owned())),
                chip: "",
                warn: String::new(),
            }
        }
        "tool_on" => {
            let what = tool_words(n);
            let note = if n.key == "shell" {
                format!("{bot} can run commands on this Mac again. This Mac's rules still apply.")
            } else {
                format!("{bot} can use {what} again.")
            };
            CardCopy {
                title: format!("Turn {what} back on for {bot}?"),
                notes: vec![note],
                primary: "Turn on".to_owned(),
                confirm: two.then(|| (format!("Yes, turn {what} on for {bot}."), "Turn on".to_owned())),
                chip: "",
                warn: String::new(),
            }
        }
        "mode" => match n.mode.as_str() {
            "bypass" => CardCopy {
                title: format!("Let {bot} run every command on this Mac without asking?"),
                notes: vec!["Blocks still apply. Every command is still logged.".to_owned()],
                primary: if two { "Bypass…".to_owned() } else { "Bypass".to_owned() },
                confirm: two.then(|| ("Yes, run every command without asking.".to_owned(), "Bypass".to_owned())),
                chip: "",
                warn: String::new(),
            },
            _ => return None,
        },
        _ => return None,
    };
    c.warn = warn;
    Some(c)
}

/// What a notice draws now: the card, the receipt (with its Undo while it
/// has one), "Put back." / an Undo's error, or nothing.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Shows {
    Card,
    Receipt { text: String, undo: String, at: i64 },
    Undone { text: String, error: bool },
    Nothing,
}

/// [`Shows`] for `n` as the answer here left it, first drawn here at
/// `seen`, at `now`.
pub(crate) fn shows(n: &ToolsChangeNotice, local: Option<&CardLocal>, seen: i64, now: i64) -> Shows {
    let n = local.and_then(|l| l.settled.as_ref()).unwrap_or(n);
    if let Some(l) = local.filter(|l| l.undone_at > 0) {
        if now - l.undone_at < PUT_BACK_MS {
            return if l.undo_error.is_empty() { Shows::Undone { text: PUT_BACK.to_owned(), error: false } } else { Shows::Undone { text: l.undo_error.clone(), error: true } };
        }
        return Shows::Nothing;
    }
    match n.state.as_str() {
        "pending" if card_copy(n).is_some() => Shows::Card,
        "applied" if receipt_fresh(receipt_start(n.at, seen), now) => {
            // the receipt sentence: the notice's, else the turn's (the same once applied)
            let text = if n.receipt.is_empty() { n.text.clone() } else { n.receipt.clone() };
            if text.is_empty() { Shows::Nothing } else { Shows::Receipt { text, undo: n.undo.clone(), at: receipt_start(n.at, seen) } }
        }
        _ => Shows::Nothing,
    }
}

/// Trims a rule as the broker saves it: spaces folded, a trailing ` *`
/// gone (true when it was there).
pub(crate) fn normalize(raw: &str) -> (String, bool) {
    let folded = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    match folded.strip_suffix(" *") {
        Some(p) => (p.trim_end().to_owned(), true),
        None => (folded, false),
    }
}

/// Programs that run another (`opengrok-policy` `RUNS_ANOTHER`).
const RUNS_ANOTHER: &[&str] = &[
    "eval", "exec", "source", ".", "command", "builtin", "env", "nice", "nohup", "time", "timeout", "xargs", "sudo", "doas", "sh", "bash", "zsh", "dash", "ksh", "fish", "csh",
    "tcsh", "trap", "caffeinate", "arch", "watch",
];
const READ_ONLY: &[&str] = &[
    "git status", "git log", "git diff", "git branch", "git show", "ls", "pwd", "which", "whoami", "date", "cat", "head", "tail", "wc", "claude --version", "node --version", "cargo --version",
];
const PROJECT: &[&str] = &["cargo test", "cargo build", "cargo check", "cargo clippy", "cargo run", "npm test", "npm run", "npm ci", "pnpm test", "yarn test", "make", "swift build", "swift test"];

/// The preview's guess at a rule's risk (the broker decides): `never`,
/// `ro`, `pc` or `any`.
pub(crate) fn guess_risk(p: &str) -> &'static str {
    let t = p.trim();
    let prog = t.split_whitespace().next().unwrap_or_default();
    if t.is_empty() || t.contains([';', '&', '|', '<', '>', '`', '*', '?', '\n']) || t.contains("$(") || RUNS_ANOTHER.contains(&prog) {
        return "never";
    }
    let starts = |r: &&str| t == *r || t.starts_with(&format!("{r} "));
    if READ_ONLY.iter().any(starts) {
        "ro"
    } else if PROJECT.iter().any(starts) {
        "pc"
    } else {
        "any"
    }
}

/// Why an allow can't be kept (a `never` rule).
pub(crate) fn never_words(p: &str) -> String {
    let prog = p.split_whitespace().next().unwrap_or_default();
    if prog == "sudo" || prog == "doas" {
        format!("Not saved: {prog} can't be always allowed.")
    } else {
        format!("Not saved: an allow rule must be one plain command. `{p}` can't be kept. Allow each command on its own.")
    }
}

/// An Activity row's outcome.
pub(crate) fn outcome_words(o: &str) -> &'static str {
    match o {
        "ran" => "Ran",
        "refused_block" => "Refused by a block",
        "refused_mode" => "Refused: mode is Only allowed commands",
        _ => "",
    }
}

/// The Activity rows of Bot `bot` (every Bot's for ""): the broker
/// sends every Bot's.
pub(crate) fn for_bot(rows: &[Activity], bot: &str) -> Vec<Activity> {
    rows.iter().filter(|a| bot.is_empty() || a.bot == bot).cloned().collect()
}

/// A mode's name and line.
fn mode_words(m: &str) -> (&'static str, &'static str) {
    match m {
        "only" => ("Only allowed commands", "Commands with no allow rule are refused without asking. Blocks still apply."),
        _ => ("Bypass", "Every command runs without asking. Blocks still apply. Each command is logged."),
    }
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

/// `s`'s words with its `code` spans in mono, wrapping word by word.
fn rich(s: &str, size: f32, weight: FontWeight, ink: AdaptiveColor) -> View {
    let mut row = view().flex_row().flex_wrap().items_center().gap(px(size * 0.3)).flex_shrink(1.0);
    for (i, part) in s.split('`').enumerate() {
        if i % 2 == 1 {
            if part.is_empty() {
                continue;
            }
            row = row.child(
                text(part.to_owned())
                    .font_family("Menlo")
                    .text_size(px(size - 1.0))
                    .line_height(px(size + 5.0))
                    .text_color(ink)
                    .bg(theme::HOVER)
                    .px(px(4.0))
                    .rounded(px(5.0)),
            );
        } else {
            for w in part.split_whitespace() {
                row = row.child(text(w.to_owned()).text_size(px(size)).line_height(px(size + 5.0)).font_weight(weight).text_color(ink));
            }
        }
    }
    row.label(s.replace('`', ""))
}

fn chip(words: &'static str, risky: bool) -> impl IntoElement {
    text(words)
        .text_size(px(11.0))
        .line_height(px(15.0))
        .text_color(if risky { theme::WARN } else { theme::DIM })
        .bg(if risky { theme::WARN_TINT } else { theme::HOVER })
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
}

fn warn_band(words: String) -> impl IntoElement {
    view()
        .flex_row()
        .gap(px(6.0))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(theme::WARN_TINT)
        .child(text("⚠").text_size(px(12.0)).text_color(theme::WARN))
        .child(text(words).flex_1().text_size(px(12.0)).line_height(px(17.0)).text_color(theme::WARN))
}

/// A small text button ("Remove", "Undo").
fn link(word: String, ink: AdaptiveColor, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    view()
        .label(word.clone())
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(7.0))
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
        .child(text(word).text_size(px(12.0)).text_color(ink))
}

/// The receipt's thin bar, draining over 8 s.
fn drain_bar(at: i64) -> impl IntoElement {
    view().w(px(160.0)).h(px(2.0)).rounded(px(1.0)).bg(theme::BORDER_S).overflow_hidden().child(
        view().h_full().bg(theme::FAINT).w(move || percentage(drain(at, clock()))),
    )
}

/// The centered receipt line: the sentence, Undo while it has one, and
/// the draining bar.
fn receipt_line(text_: String, undo: Option<Box<dyn Fn()>>, at: i64, error: bool) -> impl IntoElement {
    let mut line = view().flex_row().items_center().justify_center().flex_wrap().gap(px(8.0)).child(rich(&text_, 13.0, FontWeight::NORMAL, if error { theme::DOWN } else { theme::DIM }));
    if let Some(on) = undo {
        line = line.child(crate::chat::notice_act("Undo", format!("Undo: {}", text_.replace('`', "")), on));
    }
    view().w_full().max_w(px(560.0)).py(px(6.0)).gap(px(5.0)).items_center().child(line).child(drain_bar(at))
}

// ---------------------------------------------------------------------------
// The chat's card (`tools_change`)
// ---------------------------------------------------------------------------

fn local_of(id: &str) -> Option<CardLocal> {
    tools().cards.with(|m| m.get(id).cloned())
}

fn card_local(id: &str, f: impl FnOnce(&mut CardLocal)) {
    tools().cards.update(|m| f(m.entry(id.to_owned()).or_default()));
}

/// The row `chat.rs` draws for a `tools_change` notice in chat `chat`.
pub(crate) fn notice_row(n: ToolsChangeNotice, chat: String) -> impl IntoElement {
    // a receipt fresh at mount (a Bot's tighten, drawn here just now) ticks
    let seen = first_seen(&n.id);
    let start = receipt_start(n.at, seen);
    if n.state == "applied" && receipt_fresh(start, now_ms()) {
        watch(start + RECEIPT_MS);
    }
    let n2 = n.clone();
    let what = computed(move || {
        let local = local_of(&n2.id);
        let s = shows(&n2, local.as_ref(), seen, now_ms());
        // the clock only matters while something is timed
        if matches!(s, Shows::Receipt { .. } | Shows::Undone { .. }) { shows(&n2, local.as_ref(), seen, clock()) } else { s }
    });
    view().flex_row().w_full().justify_center().child(dynamic(move || match what.get() {
        Shows::Nothing => view().into_element(),
        Shows::Card => card(&n, &chat).into_element(),
        Shows::Receipt { text, undo, at } => {
            let id = n.id.clone();
            let on: Option<Box<dyn Fn()>> = (!undo.is_empty()).then(|| Box::new(move || card_undo(&id, &undo)) as Box<dyn Fn()>);
            receipt_line(text, on, at, false).into_element()
        }
        Shows::Undone { text, error } => {
            let at = local_of(&n.id).map(|l| l.undone_at - (RECEIPT_MS - PUT_BACK_MS)).unwrap_or(0);
            receipt_line(text, None, at, error).into_element()
        }
    }))
}

/// A pending card (A1 to A5).
fn card(n: &ToolsChangeNotice, chat: &str) -> impl IntoElement {
    let Some(copy) = card_copy(n) else { return view().into_element() };
    let local = local_of(&n.id).unwrap_or_default();
    let (c1, c2, c3) = (chat.to_owned(), chat.to_owned(), chat.to_owned());
    let (i1, i2, i3, i4, i5) = (n.clone(), n.id.clone(), n.clone(), n.id.clone(), n.clone());
    let mut c = cards::shell(&format!("tools-card-{}", n.id))
        .label(copy.title.replace('`', ""))
        .child(
            view()
                .flex_row()
                .items_start()
                .gap_2()
                .child(rich(&copy.title, 14.0, FontWeight::SEMIBOLD, theme::TEXT).flex_1())
                .child(cards::close_x("Not now", move || answer(&c1, &i1, "no"))),
        );
    for note in &copy.notes {
        let ink = if note.starts_with("Not in effect") { theme::WARN } else { theme::DIM };
        c = c.child(rich(note, 12.0, FontWeight::NORMAL, ink));
    }
    if !copy.warn.is_empty() {
        c = c.child(warn_band(copy.warn.clone()));
    }
    if !copy.chip.is_empty() {
        c = c.child(view().flex_row().child(chip(copy.chip, copy.chip == "runs any code")));
    }
    let mut buttons = view().flex_row().items_center().justify_end().gap_2();
    match (&copy.confirm, local.confirm) {
        (Some((sentence, label)), true) => {
            buttons = buttons
                .child(text(sentence.clone()).flex_1().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
                .child(cards::btn(Btn::Secondary, "Cancel", move || card_local(&i2, |l| l.confirm = false)))
                .child(cards::btn(Btn::Primary, label, move || answer(&c2, &i3, "yes")));
        }
        (confirm, _) => {
            let two = confirm.is_some();
            buttons = buttons.child(cards::btn(Btn::Secondary, "Not now", {
                let (c, i) = (chat.to_owned(), n.clone());
                move || answer(&c, &i, "no")
            }));
            buttons = buttons.child(cards::btn(Btn::Primary, &copy.primary, move || {
                if two {
                    card_local(&i4, |l| l.confirm = true);
                } else {
                    answer(&c3, &i5, "yes");
                }
            }));
        }
    }
    c = c.child(buttons);
    if local.busy {
        c = c.child(text("Sending…").text_size(px(12.0)).text_color(theme::MUTED));
    }
    if !local.error.is_empty() {
        c = c.child(cards::error_line(local.error.clone()));
    }
    c.into_element()
}

/// `POST /v1/host/tools/answer`: "no" hides the card at once (back with
/// the error if it fails); "yes" waits for the applied notice.
fn answer(chat: &str, base: &ToolsChangeNotice, answer: &'static str) {
    let (id, base) = (base.id.as_str(), base.clone());
    let busy = local_of(id).is_some_and(|l| l.busy);
    if busy {
        return;
    }
    card_local(id, |l| {
        l.busy = true;
        l.error.clear();
        if answer == "no" {
            l.settled = Some(ToolsChangeNotice { state: "declined".to_owned(), ..base.clone() });
        }
    });
    let (chat, id) = (chat.to_owned(), id.to_owned());
    crate::store::spawn_detached(async move {
        let r = match crate::client::post_any("/v1/host/tools/answer", json!({"chat": chat, "id": id, "answer": answer})).await {
            None => Err(NOT_CONNECTED.to_owned()),
            Some(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Ok(v),
            // no longer pending: the broker's words on the card, then the
            // notice as it is now (not pending: the card goes)
            Some(v) if s(&v, "code") == "stale" => {
                let now = stale_notice(&v, &base);
                card_local(&id, |l| {
                    l.busy = false;
                    l.confirm = false;
                    l.settled = None;
                    l.error = Some(s(&v, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| STALE_ANSWER.to_owned());
                });
                let id = id.clone();
                crate::store::spawn_detached(async move {
                    timer(std::time::Duration::from_millis(PUT_BACK_MS as u64)).await;
                    card_local(&id, |l| l.settled = Some(now));
                });
                return;
            }
            Some(v) => Err(refusal(&v, "")),
        };
        match r {
            Ok(v) => {
                let mut n = v.get("notice").and_then(|x| ToolsChangeNotice::of(x, base.text.clone())).unwrap_or_else(|| ToolsChangeNotice {
                    state: if answer == "yes" { "applied" } else { "declined" }.to_owned(),
                    ..base.clone()
                });
                if n.state == "applied" {
                    // the receipt starts at the click here
                    n.at = now_ms();
                    watch(n.at + RECEIPT_MS);
                }
                card_local(&id, |l| {
                    l.busy = false;
                    l.settled = Some(n);
                });
            }
            Err(e) => card_local(&id, |l| {
                l.busy = false;
                l.confirm = false;
                l.settled = None;
                l.error = e;
            }),
        }
    });
}

/// A `stale` answer's notice as the broker has it now (`details.notice`),
/// else `base` marked `stale`; never `pending` (the card goes).
pub(crate) fn stale_notice(v: &Value, base: &ToolsChangeNotice) -> ToolsChangeNotice {
    let now = v.get("details").and_then(|d| d.get("notice")).and_then(|n| ToolsChangeNotice::of(n, base.text.clone()));
    match now {
        Some(n) if n.state != "pending" => n,
        _ => ToolsChangeNotice { state: "stale".to_owned(), ..base.clone() },
    }
}

/// A receipt's Undo: "Put back." for 3 s, or why not.
fn card_undo(id: &str, undo: &str) {
    let (id, undo) = (id.to_owned(), undo.to_owned());
    crate::store::spawn_detached(async move {
        let r = call("/v1/host/tools/undo", json!({"undo": undo}), STALE_UNDO).await;
        let now = now_ms();
        card_local(&id, |l| {
            l.undone_at = now;
            l.undo_error = r.err().unwrap_or_default();
        });
        watch(now + PUT_BACK_MS);
        if tools().open.get_untracked().is_some() {
            load();
        }
    });
}

// ---------------------------------------------------------------------------
// The sheet and the full view
// ---------------------------------------------------------------------------

/// The `/tools` sheet: 560 wide, centered over a scrim.
pub(crate) fn sheet_layer() -> impl IntoElement {
    dynamic(|| {
        if tools().open.with(|o| o.as_ref().map(|o| o.kind)) != Some(Kind::Sheet) {
            return view().into_element();
        }
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .child(view().size_full().absolute().bg(hsla(0.0, 0.0, 0.0, 0.4)).on_press(close))
            .child(
                view()
                    .w(px(560.0))
                    .h(px(560.0))
                    .max_w(percentage(0.94))
                    .max_h(percentage(0.88))
                    .bg(theme::MENU_BG)
                    .rounded(px(16.0))
                    .border_1()
                    .border_color(theme::BORDER_S)
                    .overflow_hidden()
                    .test_id("tools-sheet")
                    .label("Tools")
                    .child(sheet_header())
                    .child(tabs())
                    .child(scroll().flex_1().child(view().px(px(18.0)).pt(px(6.0)).pb(px(16.0)).child(body())))
                    .child(sheet_footer()),
            )
            .into_element()
    })
}

fn bot_name() -> String {
    tools().open.with(|o| o.as_ref().map(|o| o.name.clone()).unwrap_or_default())
}

fn close_button() -> impl IntoElement {
    let hov = signal(false);
    view()
        .label("Close")
        .size(px(30.0))
        .rounded(px(15.0))
        .items_center()
        .justify_center()
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(close)
        .child(text("✕").text_size(px(13.0)).text_color(theme::MUTED))
}

fn bot_pill() -> impl IntoElement {
    dynamic(|| {
        let name = bot_name();
        if name.is_empty() {
            return view().into_element();
        }
        text(name).text_size(px(12.0)).text_color(theme::DIM).bg(theme::HOVER).px(px(8.0)).py(px(2.0)).rounded(px(999.0)).into_element()
    })
}

fn sheet_header() -> impl IntoElement {
    view()
        .h(px(50.0))
        .flex_row()
        .items_center()
        .gap_2()
        .pl(px(18.0))
        .pr(px(10.0))
        .child(text("Tools").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
        .child(bot_pill())
        .child(view().flex_1())
        .child(close_button())
}

fn sheet_footer() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px(px(14.0))
        .py(px(10.0))
        .border_t(px(0.5))
        .border_t_color(theme::BORDER_F)
        .child(view().flex_1().child(sheet_receipt()))
        .child(cards::btn(Btn::Secondary, "Open full view ⤢", open_full))
}

/// A human edit's receipt in the sheet or the full view.
fn sheet_receipt() -> impl IntoElement {
    let fresh = computed(|| {
        let r = tools().receipt.get();
        r.filter(|r| receipt_fresh(r.at, clock()))
    });
    dynamic(move || match fresh.get() {
        None => view().into_element(),
        Some(r) => {
            let undo = r.undo.clone();
            let on: Option<Box<dyn Fn()>> = (!undo.is_empty()).then(|| Box::new(move || undo_in_sheet(undo.clone())) as Box<dyn Fn()>);
            receipt_line(r.text, on, r.at, r.error).into_element()
        }
    })
}

fn tabs() -> impl IntoElement {
    let tab = |t: Tab, label: &'static str| {
        let hov = signal(false);
        view()
            .label(label)
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(7.0))
            .bg(move || {
                if tools().tab.get() == t {
                    theme::ACTIVE
                } else if hov.get() {
                    theme::HOVER
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
            .on_press(move || set_tab(t))
            .child(text(label).text_size(px(13.0)).text_color(move || if tools().tab.get() == t { theme::TEXT } else { theme::MUTED }))
    };
    view()
        .flex_row()
        .gap(px(4.0))
        .px(px(14.0))
        .pb(px(8.0))
        .border_b(px(0.5))
        .border_b_color(theme::BORDER_F)
        .child(tab(Tab::Bot, "This Bot"))
        .child(tab(Tab::Mac, "This Mac"))
        .child(tab(Tab::Activity, "Activity"))
}

/// The full view: a 52 pt bar ("‹ Chat", the title, the Bot, ✕), the tabs
/// and the open tab at full size.
pub(crate) fn full_layer() -> impl IntoElement {
    dynamic(|| {
        if tools().open.with(|o| o.as_ref().map(|o| o.kind)) != Some(Kind::Full) {
            return view().into_element();
        }
        let back_hov = signal(false);
        view()
            .absolute()
            .size_full()
            .bg(theme::MAIN)
            .test_id("tools-full")
            .label("Tools")
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(52.0))
                    .pl(px(84.0))
                    .pr(px(12.0))
                    .border_b(px(0.5))
                    .border_b_color(theme::BORDER_F)
                    .child(
                        view()
                            .label("Back to the chat")
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(7.0))
                            .bg(move || if back_hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                            .on_mouse_enter(move |_: &MouseEvent| back_hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| back_hov.set(false))
                            .on_press(close)
                            .child(text("‹ Chat").text_size(px(12.5)).text_color(theme::MUTED)),
                    )
                    .child(text("Tools").ml(px(8.0)).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
                    .child(bot_pill())
                    .child(view().flex_1())
                    .child(close_button()),
            )
            .child(view().pt(px(8.0)).child(tabs()))
            .child(view().items_center().pt(px(6.0)).child(view().w_full().max_w(px(720.0)).px(px(24.0)).child(sheet_receipt())))
            .child(scroll().flex_1().child(view().w_full().items_center().child(view().w_full().max_w(px(720.0)).px(px(24.0)).pt(px(8.0)).pb(px(32.0)).child(body()))))
            .into_element()
    })
}

/// The open tab.
fn body() -> impl IntoElement {
    dynamic(|| {
        let t = tools();
        let tab = t.tab.get();
        if t.data.with(Option::is_none) {
            let err = t.error.get();
            let words = if !err.is_empty() { err } else { "Loading…".to_owned() };
            return text(words).text_size(px(13.0)).text_color(theme::MUTED).into_element();
        }
        match tab {
            Tab::Bot => bot_tab().into_element(),
            Tab::Mac => mac_tab().into_element(),
            Tab::Activity => activity_tab().into_element(),
        }
    })
}

fn heading(words: String) -> impl IntoElement {
    text(words).pt(px(12.0)).pb(px(6.0)).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FAINT)
}

fn empty(words: String) -> impl IntoElement {
    text(words).py(px(8.0)).text_size(px(12.5)).line_height(px(18.0)).text_color(theme::MUTED)
}

fn error_note() -> impl IntoElement {
    dynamic(|| {
        let e = tools().error.get();
        if e.is_empty() { view().into_element() } else { cards::error_line(e).into_element() }
    })
}

fn row() -> View {
    view().flex_row().items_center().gap_2().py(px(8.0)).border_b(px(0.5)).border_b_color(theme::BORDER_F)
}

/// This Bot: one switch per tool key.
fn bot_tab() -> impl IntoElement {
    let open_bot = tools().open.with_untracked(|o| o.as_ref().map(|o| o.bot.clone()).unwrap_or_default());
    if open_bot.is_empty() {
        return view()
            .child(empty("This chat has no Bot. Open a Bot's chat to change its tools, or change This Mac's rules here.".to_owned()))
            .into_element();
    }
    view()
        .child(error_note())
        .child(dynamic(move || {
            let d = tools().data.get().unwrap_or_default();
            let (on, all) = d.on_count();
            let mut v = view().child(heading(format!("THIS BOT  ·  {on} of {all} on")));
            for t in d.tools.iter().cloned() {
                let (key, bot, want) = (t.key.clone(), open_bot.clone(), !t.on);
                let shown = t.on;
                let mut name = view().flex_1().child(text(t.label.clone()).text_size(px(13.0)).text_color(theme::TEXT));
                if !t.sub.is_empty() {
                    name = name.child(text(t.sub.clone()).text_size(px(11.5)).text_color(theme::FAINT).number_of_lines(1));
                }
                v = v.child(row().child(name).child(crate::plugins::tool_switch(move || shown, move || set_tool(&bot, &key, want))));
            }
            if !d.tools.iter().any(|t| t.key.starts_with("mcp:") || t.key.starts_with("v1:")) {
                v = v.child(empty("No connected apps. Connect one in Plugins.".to_owned()));
            }
            v.into_element()
        }))
        .into_element()
}

fn set_tool(bot: &str, key: &str, enabled: bool) {
    if tools().busy.get_untracked().is_some() {
        return;
    }
    // the switch moves at once; the read after puts it right
    tools().data.update(|d| {
        if let Some(t) = d.as_mut().and_then(|d| d.tools.iter_mut().find(|t| t.key == key)) {
            t.on = enabled;
        }
    });
    edit(key.to_owned(), "/v1/host/tools/set", json!({"bot": bot, "key": key, "enabled": enabled}), |e| e, |_| ());
}

/// This Mac: the mode, the rules, the add form, the recent changes.
fn mac_tab() -> impl IntoElement {
    view().child(error_note()).child(mode_section()).child(rules_section()).child(form_section()).child(recent_section())
}

fn mode_section() -> impl IntoElement {
    dynamic(|| {
        let t = tools();
        let mode = t.data.with(|d| d.as_ref().map(|d| d.mode.clone()).unwrap_or_default());
        let asking = t.mode_ask.get();
        let seg = |m: &'static str| {
            let on = mode == m;
            let (name, _) = mode_words(m);
            view()
                .label(name)
                .flex_1()
                .items_center()
                .py(px(6.0))
                .rounded(px(7.0))
                .bg(if on { theme::CARD } else { AdaptiveColor::TRANSPARENT })
                .border(px(if on { 0.5 } else { 0.0 }))
                .border_color(theme::BORDER_S)
                .on_press(move || pick_mode(m))
                .child(text(name).text_size(px(12.5)).font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).text_color(if on { theme::TEXT } else { theme::MUTED }))
        };
        let mut v = view()
            .gap(px(6.0))
            .child(heading("MODE".to_owned()))
            .child(view().flex_row().gap(px(2.0)).p(px(2.0)).rounded(px(9.0)).bg(theme::SUBTLE).child(seg("only")).child(seg("bypass")))
            .child(text(mode_words(&mode).1).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::DIM));
        if asking {
            v = v.child(
                view()
                    .gap(px(6.0))
                    .p(px(10.0))
                    .rounded(px(10.0))
                    .bg(theme::WARN_TINT)
                    .child(text("Let every Bot run every command on this Mac without asking?").text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
                    .child(text("Blocks still apply. Every command is logged.").text_size(px(12.0)).text_color(theme::DIM))
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .child(cards::btn(Btn::Secondary, "Cancel", || tools().mode_ask.set(false)))
                            .child(cards::btn(Btn::Primary, "Yes, bypass", || {
                                tools().mode_ask.set(false);
                                set_mode("bypass");
                            })),
                    ),
            );
        }
        v.into_element()
    })
}

/// A segment pressed: stricter applies at once, looser asks first.
fn pick_mode(m: &'static str) {
    let t = tools();
    let now = t.data.with_untracked(|d| d.as_ref().map(|d| d.mode.clone()).unwrap_or_default());
    if now == m {
        return;
    }
    if m == "bypass" {
        t.mode_ask.set(true);
    } else {
        t.mode_ask.set(false);
        set_mode(m);
    }
}

fn set_mode(m: &'static str) {
    edit("mode".to_owned(), "/v1/host/shell/mode", json!({"mode": m}), |e| e, |_| ());
}

fn rules_section() -> impl IntoElement {
    dynamic(|| {
        let t = tools();
        let d = t.data.get().unwrap_or_default();
        let busy = t.busy.get();
        let asking = t.rm_ask.get();
        let bot = bot_name();
        let mut v = view().child(heading(format!("ALWAYS ALLOWED ({})", d.allow.len())));
        if d.allow.is_empty() {
            let who = if bot.is_empty() { "a Bot".to_owned() } else { bot.clone() };
            v = v.child(empty(format!("No commands are always allowed yet. Always allow on a command's card, or ask {who} to allow one.")));
        }
        for r in d.allow.iter().cloned() {
            let id = format!("allow:{}", r.pattern);
            let removing = busy.as_deref() == Some(id.as_str());
            let mut name = view().flex_1().gap(px(2.0)).child(rich(&format!("`{}`", r.pattern), 13.0, FontWeight::NORMAL, theme::TEXT));
            if !r.inert.is_empty() {
                name = name.child(rich(&format!("Not in effect: {}.", r.inert.trim_end_matches('.')), 11.5, FontWeight::NORMAL, theme::WARN));
            }
            let mut acts = view().flex_row().items_center().gap(px(6.0));
            let label = risk_label(&r.risk);
            if !label.is_empty() {
                acts = acts.child(chip(label, r.risk == "any"));
            }
            let p = r.pattern.clone();
            acts = acts.child(link(if removing { "Removing…".to_owned() } else { "Remove".to_owned() }, theme::MUTED, move || remove_rule("allow", &p)));
            v = v.child(row().child(name).child(acts));
        }
        v = v.child(heading(format!("BLOCKED ({})", d.block.len())));
        if d.block.is_empty() {
            v = v.child(empty("Nothing is blocked.".to_owned()));
        }
        for p in d.block.iter().cloned() {
            let id = format!("block:{p}");
            let removing = busy.as_deref() == Some(id.as_str());
            let ask = asking.as_deref() == Some(p.as_str());
            let (word, ink) = if removing {
                ("Removing…".to_owned(), theme::MUTED)
            } else if ask {
                ("Stop blocking?".to_owned(), theme::WARN)
            } else {
                ("Remove".to_owned(), theme::MUTED)
            };
            let p2 = p.clone();
            v = v.child(row().child(view().flex_1().child(rich(&format!("`{p}`"), 13.0, FontWeight::NORMAL, theme::TEXT))).child(link(word, ink, move || {
                if tools().rm_ask.get_untracked().as_deref() == Some(p2.as_str()) {
                    tools().rm_ask.set(None);
                    remove_rule("block", &p2);
                } else {
                    tools().rm_ask.set(Some(p2.clone()));
                }
            })));
        }
        v.into_element()
    })
}

fn remove_rule(kind: &'static str, pattern: &str) {
    if tools().busy.get_untracked().is_some() {
        return;
    }
    edit(format!("{kind}:{pattern}"), "/v1/host/shell/rules", json!({"op": "remove", "kind": kind, "pattern": pattern}), |e| e, |_| ());
}

/// "+ Allow a command" / "+ Block a command", or the open form.
fn form_section() -> impl IntoElement {
    dynamic(|| match tools().form.get() {
        None => view()
            .flex_row()
            .gap_2()
            .pt(px(12.0))
            .child(cards::btn(Btn::Secondary, "+ Allow a command", || open_form(FormKind::Allow)))
            .child(cards::btn(Btn::Secondary, "+ Block a command", || open_form(FormKind::Block)))
            .into_element(),
        Some(k) => form(k).into_element(),
    })
}

fn open_form(k: FormKind) {
    let t = tools();
    t.form_text.set(String::new());
    t.form_confirm.set(false);
    t.form_error.set(String::new());
    t.form.set(Some(k));
}

/// What the form's preview and Save do with the text now.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct FormState {
    pub pattern: String,
    pub star: bool,
    /// `never`, `ro`, `pc`, `any`; "" for a block.
    pub risk: &'static str,
    /// Save is pressable.
    pub can_save: bool,
    /// Save asks again first.
    pub two_step: bool,
}

pub(crate) fn form_state(kind: FormKind, raw: &str) -> FormState {
    let (pattern, star) = normalize(raw);
    let risk = if kind == FormKind::Allow && !pattern.is_empty() { guess_risk(&pattern) } else { "" };
    FormState { can_save: !pattern.is_empty() && risk != "never", two_step: risk == "any", pattern, star, risk }
}

fn form(k: FormKind) -> impl IntoElement {
    let title = match k {
        FormKind::Allow => "Always allow a command",
        FormKind::Block => "Block a command",
    };
    let preview = dynamic(move || {
        let st = form_state(k, &tools().form_text.get());
        let mut v = view().flex_row().flex_wrap().items_center().gap(px(8.0));
        if st.pattern.is_empty() {
            return v.child(text("Type a command.").text_size(px(12.0)).text_color(theme::DIM)).into_element();
        }
        if st.risk == "never" {
            return v
                .child(chip(risk_label("never"), true))
                .child(rich(&never_words(&st.pattern), 12.0, FontWeight::NORMAL, theme::DOWN))
                .into_element();
        }
        v = v.child(rich(&format!("Starts with `{}`", st.pattern), 12.0, FontWeight::NORMAL, theme::DIM));
        if !st.risk.is_empty() {
            v = v.child(chip(risk_label(st.risk), st.risk == "any"));
        }
        if st.star {
            v = v.child(rich(&format!("`*` isn't a wildcard here. Rules match the start of a command, so this is saved as `{}`.", st.pattern), 12.0, FontWeight::NORMAL, theme::DIM));
        }
        if tools().form_confirm.get() {
            v = v.child(text("It can run other code. Every Bot on this Mac can use it.").text_size(px(12.0)).text_color(theme::WARN));
        }
        v.into_element()
    });
    let buttons = dynamic(move || {
        let st = form_state(k, &tools().form_text.get());
        let confirm = tools().form_confirm.get();
        let saving = tools().busy.get().is_some_and(|b| b == "form");
        let save = if saving {
            "Saving…".to_owned()
        } else if confirm {
            format!("Yes, always allow {}", st.pattern)
        } else {
            "Save".to_owned()
        };
        let mut v = view().flex_row().gap_2().child(cards::btn(Btn::Secondary, "Cancel", reset_inline));
        if st.can_save {
            v = v.child(cards::btn(Btn::Primary, &save, move || save_form(k)));
        } else {
            v = v.child(view().opacity(0.4).child(cards::btn(Btn::Primary, "Save", || ())));
        }
        v.into_element()
    });
    view()
        .gap(px(8.0))
        .mt(px(12.0))
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(theme::SUBTLE)
        .border(px(0.5))
        .border_color(theme::BORDER_S)
        .child(text(title).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
        .child(
            view().h(px(32.0)).px(px(10.0)).justify_center().rounded(px(8.0)).bg(theme::CARD).border(px(0.5)).border_color(theme::BORDER_S).child(
                input()
                    .value(|| tools().form_text.get())
                    .placeholder(match k {
                        FormKind::Allow => "e.g. git log",
                        FormKind::Block => "e.g. rm -rf",
                    })
                    .auto_focus(true)
                    .placeholder_color(|| color(theme::GHOST))
                    .text_size(px(13.0))
                    .text_color(theme::TEXT)
                    .on_change(|v: &str| {
                        let t = tools();
                        t.form_text.set(v.to_owned());
                        t.form_confirm.set(false);
                        t.form_error.set(String::new());
                    })
                    .on_submit(move |_: &str| {
                        if form_state(k, &tools().form_text.get_untracked()).can_save {
                            save_form(k);
                        }
                    })
                    .h(px(28.0)),
            ),
        )
        .child(preview)
        .child(dynamic(|| {
            let e = tools().form_error.get();
            if e.is_empty() { view().into_element() } else { cards::error_line(e).into_element() }
        }))
        .child(buttons)
}

fn save_form(k: FormKind) {
    let t = tools();
    if t.busy.get_untracked().is_some() {
        return;
    }
    let st = form_state(k, &t.form_text.get_untracked());
    if !st.can_save {
        return;
    }
    if st.two_step && !t.form_confirm.get_untracked() {
        t.form_confirm.set(true);
        return;
    }
    t.busy.set(Some("form".to_owned()));
    let kind = k.kind();
    crate::store::spawn_detached(async move {
        let r = crate::client::post_any("/v1/host/shell/rules", json!({"op": "add", "kind": kind, "pattern": st.pattern})).await;
        let t = tools();
        t.busy.set(None);
        let r = match r {
            Some(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Ok(v),
            other => Err(rule_error(other.as_ref())),
        };
        match r {
            Ok(v) => {
                let now = now_ms();
                t.receipt.set(Some(Receipt { text: receipt_text(&v), undo: s(&v, "undo"), at: now, error: false }));
                watch(now + RECEIPT_MS);
                reset_inline();
            }
            Err(e) => {
                t.form_confirm.set(false);
                t.form_error.set(e);
            }
        }
        load();
    });
}

fn recent_section() -> impl IntoElement {
    dynamic(|| {
        let d = tools().data.get().unwrap_or_default();
        if d.recent.is_empty() {
            return view().into_element();
        }
        let mut v = view().child(heading("RECENT CHANGES".to_owned()));
        for r in d.recent {
            let mut line = row()
                .child(text(crate::store::checked_at(r.at as f64)).w(px(72.0)).text_size(px(11.5)).text_color(theme::FAINT))
                .child(view().flex_1().child(rich(&r.text, 12.5, FontWeight::NORMAL, theme::DIM)));
            if !r.undo.is_empty() {
                let u = r.undo.clone();
                line = line.child(link("Undo".to_owned(), theme::ACCENT, move || undo_in_sheet(u.clone())));
            }
            v = v.child(line);
        }
        v.into_element()
    })
}

/// Activity: time, Bot, command, outcome.
fn activity_tab() -> impl IntoElement {
    dynamic(|| {
        let d = tools().data.get().unwrap_or_default();
        let bot = tools().open.with(|o| o.as_ref().map(|o| o.bot.clone()).unwrap_or_default());
        let rows = for_bot(&d.activity, &bot);
        let mut v = view().child(error_note()).child(heading("RECENT COMMANDS  ·  KEPT 7 DAYS".to_owned()));
        if rows.is_empty() {
            let who = Some(bot_name()).filter(|n| !n.is_empty()).unwrap_or_else(|| "any Bot".to_owned());
            return v.child(empty(format!("No commands from {who} on this Mac yet."))).into_element();
        }
        for a in rows {
            let ink = if a.outcome == "ran" { theme::UP } else { theme::DOWN };
            v = v.child(
                row()
                    .child(text(crate::store::checked_at(a.at as f64)).w(px(72.0)).text_size(px(11.5)).text_color(theme::FAINT))
                    .child(text(if a.bot_name.is_empty() { a.bot.clone() } else { a.bot_name.clone() }).w(px(72.0)).number_of_lines(1).text_size(px(12.0)).text_color(theme::DIM))
                    .child(text(a.command.clone()).flex_1().number_of_lines(1).font_family("Menlo").text_size(px(12.0)).text_color(theme::TEXT))
                    .child(text(outcome_words(&a.outcome)).text_size(px(11.5)).text_color(ink)),
            );
        }
        v.into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Turn;

    fn notice(extra: Value) -> ToolsChangeNotice {
        let mut n = json!({"kind": "tools_change", "id": "p_1", "bot": "b1", "botName": "Hex"});
        for (k, v) in extra.as_object().unwrap() {
            n[k] = v.clone();
        }
        Turn::of(&json!({"role": "notice", "text": "x", "notice": n})).tools_change.expect("a tools card")
    }

    #[test]
    fn a_tools_change_notice_parses_for_each_op() {
        let n = notice(json!({"op": "rule_add", "rule": {"kind": "allow", "pattern": "claude", "risk": "any", "path": "/u/.local/bin/claude"}, "steps": 2, "unrequested": true, "inert": "blocked by `rm`"}));
        assert_eq!((n.op.as_str(), n.state.as_str(), n.steps, n.unrequested, n.bot_name.as_str()), ("rule_add", "pending", 2, true, "Hex"));
        let r = n.rule.clone().unwrap();
        assert_eq!((r.kind.as_str(), r.pattern.as_str(), r.risk.as_str(), r.path.as_str()), ("allow", "claude", "any", "/u/.local/bin/claude"));
        assert_eq!(n.inert, "blocked by `rm`");

        let n = notice(json!({"op": "tool_on", "key": "shell", "label": "Shell on this Mac"}));
        assert_eq!((n.key.as_str(), n.label.as_str(), n.steps, n.rule.is_none()), ("shell", "Shell on this Mac", 1, true));
        let n = notice(json!({"op": "tool_off", "key": "web", "state": "applied", "at": 1793745245000i64, "undo": "u_1", "text": "Hex can't use the shell now."}));
        assert_eq!((n.state.as_str(), n.at, n.undo.as_str(), n.receipt.as_str(), n.text.as_str()), ("applied", 1793745245000, "u_1", "Hex can't use the shell now.", "x"));
        let n = notice(json!({"op": "rule_remove", "rule": {"kind": "block", "pattern": "rm -rf"}}));
        assert_eq!(n.rule.unwrap().kind, "block");
        let n = notice(json!({"op": "mode", "mode": "bypass", "steps": 2}));
        assert_eq!((n.mode.as_str(), n.steps), ("bypass", 2));
        for state in ["pending", "applied", "declined", "superseded", "expired", "stale"] {
            assert_eq!(notice(json!({"op": "mode", "mode": "bypass", "state": state})).state, state);
        }
        // another kind, no id, or a user turn: no card
        assert!(Turn::of(&json!({"role": "notice", "notice": {"kind": "app_uninstall", "id": "x"}})).tools_change.is_none());
        assert!(Turn::of(&json!({"role": "notice", "notice": {"kind": "tools_change", "op": "mode"}})).tools_change.is_none());
        assert!(Turn::of(&json!({"role": "user", "notice": {"kind": "tools_change", "id": "p_1"}})).tools_change.is_none());
        let t = Turn::of(&json!({"role": "notice", "notice": {"kind": "tools_change", "id": "p_1", "op": "mode"}}));
        assert!(t.app_uninstall.is_none() && t.consent.is_none() && t.notice.is_none());
    }

    #[test]
    fn only_a_pending_widen_is_a_card() {
        let now = 1_000_000;
        let card = |extra: Value| shows(&notice(extra), None, now, now);
        assert_eq!(card(json!({"op": "tool_on", "key": "shell"})), Shows::Card);
        assert_eq!(card(json!({"op": "rule_add", "rule": {"kind": "allow", "pattern": "git status", "risk": "ro"}})), Shows::Card);
        assert_eq!(card(json!({"op": "rule_remove", "rule": {"kind": "block", "pattern": "rm -rf"}})), Shows::Card);
        assert_eq!(card(json!({"op": "mode", "mode": "bypass"})), Shows::Card);
        // a tighten never waits for a click
        assert_eq!(card(json!({"op": "tool_off", "key": "shell"})), Shows::Nothing);
        assert_eq!(card(json!({"op": "rule_add", "rule": {"kind": "block", "pattern": "rm"}})), Shows::Nothing);
        assert_eq!(card(json!({"op": "rule_remove", "rule": {"kind": "allow", "pattern": "claude"}})), Shows::Nothing);
        for state in ["declined", "superseded", "expired", "stale"] {
            assert_eq!(card(json!({"op": "tool_on", "key": "shell", "state": state})), Shows::Nothing, "{state}");
        }
        // answered here: as the answer left it
        let n = notice(json!({"op": "tool_on", "key": "shell"}));
        let local = CardLocal { settled: Some(ToolsChangeNotice { state: "declined".into(), ..n.clone() }), ..Default::default() };
        assert_eq!(shows(&n, Some(&local), now, now), Shows::Nothing);
    }

    #[test]
    fn the_receipt_shows_only_while_fresh() {
        let at = 1_793_745_245_000;
        let n = notice(json!({"op": "tool_off", "key": "shell", "state": "applied", "at": at, "undo": "u_1", "text": "Hex can't use the shell now."}));
        let receipt = Shows::Receipt { text: "Hex can't use the shell now.".into(), undo: "u_1".into(), at };
        assert_eq!(shows(&n, None, at, at), receipt);
        assert_eq!(shows(&n, None, at, at + 7_999), receipt);
        assert_eq!(shows(&n, None, at, at + 8_000), Shows::Nothing, "gone after 8 s");
        assert_eq!(shows(&n, None, at, at + 86_400_000), Shows::Nothing, "a reload shows nothing");
        assert_eq!(shows(&ToolsChangeNotice { at: 0, ..n.clone() }, None, at, at), Shows::Nothing, "no time, no receipt");
        assert!((drain(at, at) - 1.0).abs() < 1e-6);
        assert!((drain(at, at + 4_000) - 0.5).abs() < 1e-6);
        assert_eq!(drain(at, at + 9_000), 0.0);
        // Undo: "Put back." for 3 s, then nothing
        let undone = CardLocal { undone_at: at + 2_000, ..Default::default() };
        assert_eq!(shows(&n, Some(&undone), at, at + 2_500), Shows::Undone { text: "Put back.".into(), error: false });
        assert_eq!(shows(&n, Some(&undone), at, at + 5_000), Shows::Nothing);
        let refused = CardLocal { undone_at: at + 2_000, undo_error: STALE_UNDO.into(), ..Default::default() };
        assert_eq!(shows(&n, Some(&refused), at, at + 2_500), Shows::Undone { text: STALE_UNDO.into(), error: true });
        // a yes answered here: the receipt from the answer's notice
        let p = notice(json!({"op": "tool_on", "key": "shell"}));
        let yes = CardLocal { settled: Some(ToolsChangeNotice { state: "applied".into(), at, undo: String::new(), receipt: "Hex can use the shell again.".into(), ..p.clone() }), ..Default::default() };
        assert_eq!(shows(&p, Some(&yes), at, at + 100), Shows::Receipt { text: "Hex can use the shell again.".into(), undo: String::new(), at });
    }

    #[test]
    fn the_receipt_counts_from_when_it_is_first_drawn_here() {
        let at = 1_793_745_245_000;
        let n = notice(json!({"op": "tool_off", "key": "web", "state": "applied", "at": at, "undo": "u_1", "text": "Hex can't use the web now."}));
        let receipt = |start: i64| Shows::Receipt { text: "Hex can't use the web now.".into(), undo: "u_1".into(), at: start };
        // drawn 20 s after the change (the reply outlasted it): 8 s from then
        let seen = at + 20_000;
        assert_eq!(receipt_start(at, seen), seen);
        assert_eq!(shows(&n, None, seen, seen), receipt(seen));
        assert_eq!(shows(&n, None, seen, seen + 7_999), receipt(seen));
        assert_eq!(shows(&n, None, seen, seen + 8_000), Shows::Nothing);
        // drawn an hour later (a reload, a restart): history, no receipt
        let seen = at + 3_600_000;
        assert_eq!(receipt_start(at, seen), 0);
        assert_eq!(shows(&n, None, seen, seen), Shows::Nothing);
        assert_eq!(shows(&n, None, seen, seen + 1_000), Shows::Nothing);
        assert_eq!(shows(&n, None, at + HISTORY_MS + 1, at + HISTORY_MS + 1), Shows::Nothing);
        // drawn at once: from the change, as before
        assert_eq!(receipt_start(at, at), at);
        assert_eq!(shows(&n, None, at, at + 7_999), receipt(at));
        assert_eq!(shows(&n, None, at, at + 8_000), Shows::Nothing);
        // the clocks a little apart: still from the change
        assert_eq!(shows(&n, None, at - 500, at), receipt(at));
        // Undo after a late first sight: "Put back." for 3 s
        let undone = CardLocal { undone_at: at + 22_000, ..Default::default() };
        assert_eq!(shows(&n, Some(&undone), at + 20_000, at + 23_000), Shows::Undone { text: "Put back.".into(), error: false });
        assert_eq!(shows(&n, Some(&undone), at + 20_000, at + 25_000), Shows::Nothing);
    }

    #[test]
    fn cards_say_section_7_word_for_word() {
        let a1 = card_copy(&notice(json!({"op": "rule_add", "rule": {"kind": "allow", "pattern": "git status", "risk": "ro"}}))).unwrap();
        assert_eq!(a1.title, "Always allow `git status` on this Mac?");
        assert_eq!(a1.notes, [
            "Hex asked for this so it can run the command without asking.",
            "Covers any command that starts with `git status`, for every Bot on this Mac."
        ]);
        assert_eq!((a1.primary.as_str(), a1.confirm.is_none(), a1.chip), ("Always allow", true, "read-only"));

        let a2 = card_copy(&notice(json!({"op": "rule_add", "steps": 2, "rule": {"kind": "allow", "pattern": "claude", "risk": "any", "path": "/Users/u/.local/bin/claude"}}))).unwrap();
        assert_eq!(a2.notes[0], "Resolved to /Users/u/.local/bin/claude");
        assert!(a2.notes[1].ends_with("Allowing it lets Hex do the same without asking, for every Bot on this Mac."));
        assert_eq!(a2.primary, "Always allow…");
        assert_eq!(a2.confirm, Some(("Yes, always allow claude.".to_owned(), "Always allow".to_owned())));
        assert_eq!(a2.chip, "runs any code");

        let a3 = card_copy(&notice(json!({"op": "tool_on", "key": "shell", "label": "Shell on this Mac", "unrequested": true}))).unwrap();
        assert_eq!(a3.title, "Turn the shell back on for Hex?");
        assert_eq!(a3.notes, ["Hex can run commands on this Mac again. This Mac's rules still apply."]);
        assert_eq!(a3.warn, "Hex suggested this after reading a web page. You didn't ask for it.");
        assert_eq!(a3.primary, "Turn on");

        let a5 = card_copy(&notice(json!({"op": "mode", "mode": "bypass", "steps": 2}))).unwrap();
        assert_eq!(a5.title, "Let Hex run every command on this Mac without asking?");
        assert_eq!(a5.notes, ["Blocks still apply. Every command is still logged."]);
        assert_eq!(a5.primary, "Bypass…");
        assert_eq!(a5.confirm.unwrap().0, "Yes, run every command without asking.");

        let stop = card_copy(&notice(json!({"op": "rule_remove", "rule": {"kind": "block", "pattern": "rm -rf"}}))).unwrap();
        assert_eq!(stop.title, "Stop blocking `rm -rf` on this Mac?");
        assert_eq!(stop.notes, ["Commands that start with it could run again, for every Bot."]);
        assert_eq!(stop.primary, "Stop blocking");

        let inert = card_copy(&notice(json!({"op": "rule_add", "inert": "blocked by `rm`", "rule": {"kind": "allow", "pattern": "rm x", "risk": "any"}}))).unwrap();
        assert_eq!(inert.notes.last().unwrap(), "Not in effect: blocked by `rm`.");
        assert!(inert.warn.is_empty(), "requested: no band");
    }

    #[test]
    fn the_tools_route_parses_and_refusals_keep_their_words() {
        let v = json!({"ok": true, "bot": {"id": "b1", "name": "Hex"},
            "tools": [{"key": "shell", "label": "Shell on this Mac", "sub": "shell_run", "on": false}, {"key": "web", "label": "Web search and fetch", "on": true}],
            "mac": {"mode": "only", "allow": [{"pattern": "git status", "risk": "ro"}, {"pattern": "rm x", "risk": "any", "inert": "blocked by `rm`"}], "block": [{"pattern": "rm"}]},
            "activity": [{"at": 5, "bot": "b1", "botName": "Hex", "command": "git status", "outcome": "ran"}],
            "recent": [{"at": 6, "actor": "human", "text": "Blocked on this Mac: `rm`.", "undo": "u_2"}]});
        let d = Data::of(&v).unwrap();
        assert_eq!((d.bot_id.as_str(), d.bot_name.as_str(), d.mode.as_str()), ("b1", "Hex", "only"));
        assert_eq!(d.on_count(), (1, 2));
        assert_eq!(d.allow[1].inert, "blocked by `rm`");
        assert_eq!(d.block, ["rm"]);
        assert_eq!((d.activity[0].bot_name.as_str(), outcome_words(&d.activity[0].outcome)), ("Hex", "Ran"));
        assert_eq!(d.recent[0].undo, "u_2");
        // no Bot: no tools; no mode: bypass
        let d = Data::of(&json!({"ok": true, "tools": [], "mac": {}})).unwrap();
        assert!(d.tools.is_empty() && d.bot_id.is_empty());
        assert_eq!(d.mode, "bypass");
        assert_eq!(Data::of(&json!({"ok": false, "code": "unauthorized", "message": "No host token."})), Err("No host token.".to_owned()));
        let stale = json!({"ok": false, "code": "stale", "message": "changed"});
        assert_eq!(refusal(&stale, STALE_ANSWER), "That changed since you looked. Ask Hex again.");
        assert_eq!(refusal(&stale, ""), "changed");
        assert_eq!(refusal(&json!({"ok": false, "code": "never"}), ""), "never");
    }

    #[test]
    fn the_brokers_answers_as_built() {
        // every notice has `at`; only an applied one is a receipt
        let at = 1_793_745_245_000;
        let pending = notice(json!({"op": "tool_on", "key": "shell", "at": at}));
        assert_eq!(shows(&pending, None, at, at + 10), Shows::Card);
        let declined = notice(json!({"op": "tool_on", "key": "shell", "at": at, "state": "declined"}));
        assert_eq!(shows(&declined, None, at, at + 10), Shows::Nothing);
        // the receipt falls back to the turn's text (the same once applied)
        let mut applied = notice(json!({"op": "tool_off", "key": "shell", "at": at, "state": "applied"}));
        applied.text = "Hex can't use the shell now.".into();
        assert_eq!(shows(&applied, None, at, at + 10), Shows::Receipt { text: "Hex can't use the shell now.".into(), undo: String::new(), at });
        // a chat with no Bot
        let n = notice(json!({"bot": "", "botName": "The assistant", "op": "tool_on", "key": "web", "label": "Web search and fetch"}));
        assert_eq!(card_copy(&n).unwrap().title, "Turn Web search and fetch back on for The assistant?");
        // stale: the notice as it is now, never pending
        let base = notice(json!({"op": "tool_on", "key": "shell"}));
        let v = json!({"ok": false, "code": "stale", "message": STALE_ANSWER,
            "details": {"notice": {"kind": "tools_change", "id": "p_1", "op": "tool_on", "state": "superseded", "at": at}}});
        assert_eq!(refusal(&v, ""), STALE_ANSWER);
        assert_eq!(stale_notice(&v, &base).state, "superseded");
        assert_eq!(shows(&stale_notice(&v, &base), None, at, at), Shows::Nothing);
        let still = json!({"ok": false, "code": "stale", "details": {"notice": {"kind": "tools_change", "id": "p_1", "state": "pending"}}});
        assert_eq!(stale_notice(&still, &base).state, "stale");
        assert_eq!(stale_notice(&json!({"ok": false, "code": "stale"}), &base).state, "stale");
        // rules: the note after the text; the broker's refusals in its words
        assert_eq!(receipt_text(&json!({"ok": true, "text": "Always allowed on this Mac: `claude`.", "undo": null,
            "note": "`*` isn't a wildcard here. Rules match the start of a command, so this is saved as `claude`."})),
            "Always allowed on this Mac: `claude`. `*` isn't a wildcard here. Rules match the start of a command, so this is saved as `claude`.");
        assert_eq!(s(&json!({"undo": null}), "undo"), "", "no undo when nothing changed");
        assert_eq!(rule_error(Some(&json!({"ok": false, "code": "exists", "message": "Already always allowed."}))), "Already always allowed.");
        assert_eq!(rule_error(Some(&json!({"ok": false, "code": "refused", "message": "Not saved: sudo can't be always allowed."}))),
            "Not saved: sudo can't be always allowed.");
        assert_eq!(rule_error(Some(&json!({"ok": false, "code": "not_found", "message": "No such rule."}))), "Couldn't save the rule: No such rule.");
        assert_eq!(rule_error(None), NOT_CONNECTED);
        // GET: `bot` null; activity is every Bot's, filtered here
        let d = Data::of(&json!({"ok": true, "bot": null, "tools": [], "mac": {"mode": "bypass"},
            "activity": [{"at": 1, "bot": "b1", "command": "ls", "outcome": "ran"}, {"at": 2, "bot": "b2", "command": "pwd", "outcome": "refused_mode"}]})).unwrap();
        assert!(d.bot_id.is_empty());
        assert_eq!(for_bot(&d.activity, "b2").iter().map(|a| a.command.as_str()).collect::<Vec<_>>(), ["pwd"]);
        assert_eq!(for_bot(&d.activity, "").len(), 2);
        assert_eq!(Data::of(&json!({"ok": false, "code": "not_found", "message": "No such Bot."})), Err("No such Bot.".to_owned()));
    }

    #[test]
    fn the_add_form_previews_the_rule_it_saves() {
        assert_eq!(normalize("  claude   * "), ("claude".to_owned(), true));
        assert_eq!(normalize("git  log"), ("git log".to_owned(), false));
        let st = form_state(FormKind::Allow, "claude *");
        assert_eq!((st.pattern.as_str(), st.star, st.risk, st.can_save, st.two_step), ("claude", true, "any", true, true));
        let st = form_state(FormKind::Allow, "git status -s");
        assert_eq!((st.risk, st.two_step), ("ro", false));
        assert_eq!(form_state(FormKind::Allow, "cargo test").risk, "pc");
        for never in ["sudo ls", "sh -c x", "ls; rm x", "cat x > ~/.zshrc", "ls *.rs", "echo $(id)", "env", "xargs rm"] {
            let st = form_state(FormKind::Allow, never);
            assert_eq!((st.risk, st.can_save), ("never", false), "{never}");
        }
        assert_eq!(never_words("sudo ls"), "Not saved: sudo can't be always allowed.");
        assert_eq!(never_words("cd src && cargo test"), "Not saved: an allow rule must be one plain command. `cd src && cargo test` can't be kept. Allow each command on its own.");
        // a block takes any plain text, at once
        let st = form_state(FormKind::Block, "curl x | sh");
        assert_eq!((st.risk, st.can_save, st.two_step), ("", true, false));
        assert!(!form_state(FormKind::Block, "  ").can_save);
    }

    #[test]
    fn tools_opens_the_sheet_on_this_bot_for_the_chats_bot() {
        let (open, tab) = sheet_for(Some(("b1".into(), "Hex".into())));
        assert_eq!(open, Open { kind: Kind::Sheet, bot: "b1".into(), name: "Hex".into() });
        assert_eq!(tab, Tab::Bot);
        let (open, _) = sheet_for(None);
        assert!(open.bot.is_empty() && open.kind == Kind::Sheet, "a chat with no Bot still opens it");
    }
}
