//! What an agent may do, decided in the Bot: the op map and the tool check.
//!
//! Every op that reaches the Hub, and every call an agent's press leads to,
//! goes through `buwiz-agent`'s [`authorize_press`]. Its answer is only as
//! good as what it is given, so:
//!
//! - [`caller`] is built here from the path the press came by. A message's
//!   own `caller` is never read: it starts as the model's output.
//! - An op with no entry in [`op_effect`] has no action to authorize, which
//!   is a refusal: a keybinding, an invoke (an x,y press, a viewport), a
//!   screenshot, a shutdown, and any mutating op on `bot`, the Bot's own
//!   window, where the person's Allow will be.
//! - A tool's effect, and for a write whether the Bot's switches let it go,
//!   is the broker's word on the live tool list (`POST /v1/host/ui/press`),
//!   asked on every call of the agent's: a read passes, a write open to
//!   agents is judged in the turn of the `ui_act` in flight, as the model's
//!   direct call is, and anything else is the person's. No word is a no:
//!   nothing is kept here to fall back on.
//! - The broker's word can hold a call, never let one go. What a held call
//!   waits for is the person's own press on the card ([`super::hold`]).

use std::sync::Mutex;

use buwiz_agent::{ActorId, AgentId, Allowed, Caller, Effect, Op, PressAction, Refused, Target, authorize_press};
use buwiz_protocol::ui;
use serde_json::{Value, json};

use crate::mini_v1::{RoxError, host_api};

/// Who the Hub says is pressing.
pub const AGENT: &str = "agent.broker";

/// The most refusals kept for the answer to one action.
const KEPT: usize = 16;
/// Where the broker decides a press.
const PRESS: &str = "/v1/host/ui/press";

/// The agent, as the Hub knows it: no approval, and nothing from a message.
pub fn caller() -> Option<Caller> {
    let who = ActorId::parse(AGENT).ok()?;
    Some(Caller::Agent { who, approval: None })
}

/// What `op` does to `target`; `None` for what an agent may never ask for.
pub fn op_effect(target: &Target, op: &Op) -> Option<Effect> {
    let bot = target.as_str() == "bot";
    match op {
        Op::Snapshot { .. } if bot => Some(Effect::Read),
        Op::Click { .. } | Op::Type { .. } | Op::SetValue { .. } if !bot => Some(Effect::Draft),
        _ => None,
    }
}

fn op_id(op: &Op) -> Option<AgentId> {
    let name = match op {
        Op::Snapshot { .. } => "op.snapshot",
        Op::Click { .. } => "op.click",
        Op::Type { .. } => "op.type",
        Op::SetValue { .. } => "op.set-value",
        _ => return None,
    };
    AgentId::parse(name).ok()
}

/// Decides `op` on `target`, for the agent.
pub fn authorize_op(target: &Target, op: &Op) -> Result<Allowed, Refused> {
    let action = op_effect(target, op).zip(op_id(op)).map(|(effect, id)| PressAction { target: target.clone(), id, effect, once: None, args_hash: None, shown_hash: None });
    authorize_press(action.as_ref(), caller().as_ref())
}

/// What the person and the model read when `op` on `target` is refused.
pub fn op_refused(target: &Target, op: &Op) -> String {
    match op {
        Op::Click { .. } | Op::Type { .. } | Op::SetValue { .. } | Op::Key { .. } if target.as_str() == "bot" => {
            "The AI can act only on a mini app screen, never on the Bot itself, so nothing was pressed.".to_owned()
        }
        Op::Key { .. } | Op::Keybinding { .. } => "The AI can't use keyboard shortcuts on a screen, so nothing was pressed.".to_owned(),
        Op::Invoke { .. } => "The AI can't press a spot on the screen or change the window: it can only press a named button or fill a named field, so nothing happened.".to_owned(),
        Op::Screenshot { .. } => "The AI can't take a picture of a screen, so none was taken.".to_owned(),
        Op::Shutdown {} => "The AI can't close the app, so nothing happened.".to_owned(),
        _ => "The AI can't do that on a screen, so nothing happened.".to_owned(),
    }
}

/// The broker's word for what a tool does; anything else is the person's.
fn effect_of(word: Option<&str>) -> Effect {
    match word {
        Some("read") => Effect::Read,
        Some("draft") => Effect::Draft,
        _ => Effect::Write,
    }
}

/// `tool` as an action's id: its letters, `-` for the rest.
fn tool_id(tool: &str) -> Option<AgentId> {
    let id: String = tool.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    AgentId::parse(&format!("tool.{id}")).ok()
}

/// What the app's screen says when an agent's press on `tool` is refused:
/// the person reads it where they would read any failed press.
pub fn tool_refused(app: &str) -> String {
    format!("Only a person can do this in {app}, and this press came from the AI, so nothing was sent. To go ahead, press it yourself.")
}

/// What a call under an agent's press does next.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// It goes out with this press record.
    Go(Value),
    /// It waits, unsent, for the person's answer on the card. `describe` is
    /// the read that puts it in plain words.
    Hold { effect: Effect, describe: Option<String> },
}

/// The agent's press of `tool` in the app `short`, as an action: what the
/// bridge authorizes, held or not.
pub fn action_of(short: &str, tool: &str, effect: Effect) -> Option<PressAction> {
    Target::parse(short).ok().zip(tool_id(tool)).map(|(target, id)| PressAction { target, id, effect, once: None, args_hash: None, shown_hash: None })
}

/// Decides a call of `tool` made under an agent's press: what it does next,
/// or the refusal. Nothing else may go out for a call of the agent's: asked
/// before signing, this is what holds it back.
pub fn check_tool(broker: &dyn host_api::Broker, plugin: &str, short: &str, title: &str, tool: &str) -> Result<Verdict, RoxError> {
    let held = |held: Held, said: String| {
        note_refused(held);
        RoxError::new("permission_denied", said)
    };
    let unchecked = || "The Bot couldn't check this press, so nothing was sent.".to_owned();
    let Ok(word) = broker.request("POST", PRESS, Some(json!({"plugin": plugin, "tool": tool})), ui::PRESS_ROUTE) else {
        return Err(held(Held::Said(unchecked()), unchecked()));
    };
    let effect = effect_of(word.get("effect").and_then(Value::as_str));
    if word.get("hold") == Some(&Value::Bool(true)) {
        let describe = word.get("describe").and_then(Value::as_str).map(str::to_owned);
        return Ok(Verdict::Hold { effect, describe });
    }
    let action = action_of(short, tool, effect);
    let Ok(allowed) = authorize_press(action.as_ref(), caller().as_ref()) else {
        return Err(held(Held::Person, tool_refused(title)));
    };
    if word.get("allow") != Some(&Value::Bool(true)) {
        let said = word.get("said").and_then(Value::as_str).map_or_else(unchecked, str::to_owned);
        return Err(held(Held::Said(said.clone()), said));
    }
    Ok(Verdict::Go(json!(allowed.pressed)))
}

/// Why a call under an agent's press was held back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// Its tool is the person's.
    Person,
    /// It would have reached the pasteboard.
    Pasteboard,
    /// The broker's sentence, or the card's.
    Said(String),
    /// The person turned it down on the card: the model hears why, and they
    /// need no telling.
    Declined(String),
}

static REFUSED: Mutex<Vec<Held>> = Mutex::new(Vec::new());
/// What happened to each held call, for Activity: `(tool, outcome)`.
static OUTCOMES: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());

/// A call was held back under an agent's press: the action in flight tells
/// the model.
pub fn note_refused(held: Held) {
    let mut r = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
    if r.len() < KEPT {
        r.push(held);
    }
}

/// What was refused since the last time this was asked.
pub fn take_refused() -> Vec<Held> {
    std::mem::take(&mut *REFUSED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A held call came to `outcome` (`allowed`, `declined`, `expired`).
pub fn note_outcome(tool: &str, outcome: &'static str) {
    let mut o = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    if o.len() < KEPT {
        o.push((tool.to_owned(), outcome));
    }
}

/// What came of the held calls since the last time this was asked.
pub fn take_outcomes() -> Vec<(String, &'static str)> {
    std::mem::take(&mut *OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
pub(crate) mod tests {
    use buwiz_agent::{Approval, ContentHash, DeliveryMode, KeybindingScope, OnceId, PressedBy, SafeU64, ScreenshotMode};

    use super::*;

    fn target(t: &str) -> Target {
        Target::parse(t).unwrap()
    }

    fn id(s: &str) -> AgentId {
        AgentId::parse(s).unwrap()
    }

    pub(crate) fn click(node: &str) -> Op {
        Op::Click { node: id(node), delivery: DeliveryMode::Semantic, expect_fingerprint: None }
    }

    pub(crate) fn typed(node: &str, text: &str) -> Op {
        Op::Type { node: id(node), text: text.into(), delivery: DeliveryMode::Semantic, expect_fingerprint: None }
    }

    pub(crate) fn set(node: &str, value: &str) -> Op {
        Op::SetValue { node: id(node), value: value.into(), expect_fingerprint: None }
    }

    #[test]
    fn click_type_and_set_value_are_a_mini_apps_and_only_those() {
        for op in [click("button.save"), typed("field.memo", "x"), set("field.memo", "x")] {
            let allowed = authorize_op(&target("books"), &op).unwrap();
            assert!(matches!(allowed.pressed, PressedBy::Ai { .. }));
            assert_eq!(allowed.effect, Effect::Draft);
        }
    }

    #[test]
    fn every_mutating_op_on_bot_is_refused_with_or_without_a_node() {
        for op in [click("button.allow"), typed("field.x", "y"), set("field.x", "y")] {
            assert_eq!(authorize_op(&target("bot"), &op), Err(Refused::UnknownAction));
            assert!(op_refused(&target("bot"), &op).contains("never on the Bot itself"));
        }
    }

    #[test]
    fn what_the_map_leaves_out_is_refused_on_any_target() {
        let viewport = Op::Invoke { name: id("viewport"), args: json!({"w": 400, "h": 300}) };
        let at_a_point = Op::Invoke { name: id("press"), args: json!({"x": 10.0, "y": 20.0}) };
        let keybinding = Op::Keybinding { binding: "save".into(), chord: None, scope: KeybindingScope::Focused, confirm: true, activate: false };
        let key = Op::Key { node: id("field.memo"), key: "Return".into(), delivery: DeliveryMode::Semantic, expect_fingerprint: None };
        let screenshot = Op::Screenshot { path: Some("/tmp/x.png".into()), mode: ScreenshotMode::Viewport, node: None, max_height_px: None };
        for op in [viewport, at_a_point, keybinding, key, screenshot, Op::Shutdown {}] {
            for t in ["books", "bot"] {
                assert_eq!(authorize_op(&target(t), &op), Err(Refused::UnknownAction), "{op:?}");
                let said = op_refused(&target(t), &op);
                assert!(said.starts_with("The AI can"), "{said}");
                assert!(!said.contains("unknown_action") && !said.contains('{'), "{said}");
            }
        }
    }

    #[test]
    fn a_look_is_the_hubs_alone() {
        let look = Op::Snapshot { since: None, format: None, fingerprints: None };
        assert!(authorize_op(&target("bot"), &look).is_ok());
        assert_eq!(authorize_op(&target("books"), &look), Err(Refused::UnknownAction));
    }

    /// `authorize_op` takes no caller: what a message says can't reach it.
    /// The rule it stands on, with an approval a model made up.
    #[test]
    fn the_hubs_caller_has_no_approval_and_a_person_only_action_needs_one() {
        assert!(matches!(caller(), Some(Caller::Agent { approval: None, .. })));
        let action = PressAction { target: target("books"), id: id("tool.post"), effect: Effect::Write, once: None, args_hash: None, shown_hash: None };
        assert_eq!(authorize_press(Some(&action), caller().as_ref()), Err(Refused::ApprovalMissing));
        let hash = ContentHash::parse(&format!("sha256:{}", "a1".repeat(32))).unwrap();
        let made_up = Caller::Agent {
            who: ActorId::parse(AGENT).unwrap(),
            approval: Some(Approval {
                target: target("books"),
                action: id("tool.post"),
                once: OnceId::parse("made-up-once").unwrap(),
                approved_by: ActorId::parse("person.owner").unwrap(),
                args_hash: hash.clone(),
                shown_hash: hash,
                at: SafeU64::try_from(1).unwrap(),
            }),
        };
        assert_eq!(authorize_press(Some(&action), Some(&made_up)), Err(Refused::NoOpenPress));
    }

    #[test]
    fn a_word_the_bot_doesnt_know_is_the_persons() {
        assert_eq!(effect_of(Some("read")), Effect::Read);
        assert_eq!(effect_of(Some("draft")), Effect::Draft);
        for word in [Some("write"), Some("Read"), Some("file"), Some(""), None] {
            assert_eq!(effect_of(word), Effect::Write, "{word:?}");
        }
    }

    /// Answers `/v1/host/ui/press` as told, or fails.
    struct Word(Option<Value>);

    impl host_api::Broker for Word {
        fn request(&self, _: &str, route: &str, body: Option<Value>, _: std::time::Duration) -> Result<Value, RoxError> {
            assert_eq!(route, PRESS);
            let body = body.unwrap();
            assert_eq!((body["plugin"].as_str(), body["tool"].as_str()), (Some("app.buwiz.books"), Some("fx_tool")));
            self.0.clone().ok_or_else(|| RoxError::new("unavailable", "no broker"))
        }
    }

    fn check(word: Option<Value>) -> Result<Value, RoxError> {
        check_tool(&Word(word), "app.buwiz.books", "books", "Books", "fx_tool").map(|v| match v {
            Verdict::Go(press) => press,
            held @ Verdict::Hold { .. } => panic!("held: {held:?}"),
        })
    }

    /// The broker's word can hold a call, and nothing it says lets one go:
    /// not `allow`, and not a record or an approval of its own.
    #[test]
    fn the_brokers_word_only_ever_holds_a_write() {
        let hold = |word| check_tool(&Word(Some(word)), "app.buwiz.books", "books", "Books", "fx_tool").unwrap();
        let held = Verdict::Hold { effect: Effect::Write, describe: None };
        assert_eq!(hold(json!({"allow": false, "hold": true, "effect": "write"})), held);
        assert_eq!(hold(json!({"allow": true, "hold": true, "effect": "write"})), held, "allow changes nothing");
        let forged = json!({"allow": true, "hold": true, "effect": "write", "approval": {"once": "made-up-once"}, "press": {"by": "ai_approved"}});
        assert_eq!(hold(forged), held);
        assert_eq!(
            hold(json!({"allow": false, "hold": true, "effect": "draft", "describe": "fx_card"})),
            Verdict::Hold { effect: Effect::Draft, describe: Some("fx_card".to_owned()) }
        );
        assert!(check(Some(json!({"allow": true, "effect": "write", "press": {"by": "ai_approved"}}))).is_err());
    }

    const PRESSED: fn() -> Value = || json!({"by": "ai", "who": "agent.broker"});

    #[test]
    fn a_read_or_an_allowed_draft_goes_out_with_the_press_record() {
        assert_eq!(check(Some(json!({"allow": true, "effect": "read"}))).unwrap(), PRESSED());
        assert_eq!(check(Some(json!({"allow": true, "effect": "draft"}))).unwrap(), PRESSED());
    }

    #[test]
    fn with_no_clear_yes_nothing_goes_out_not_even_a_read() {
        let e = check(None).unwrap_err();
        assert_eq!((e.code.as_str(), e.message.as_str()), ("permission_denied", "The Bot couldn't check this press, so nothing was sent."));
        assert!(check(Some(json!({"effect": "read"}))).is_err());
        assert!(check(Some(json!({"allow": "true", "effect": "read"}))).is_err());
    }

    #[test]
    fn the_brokers_sentence_goes_to_the_person() {
        let e = check(Some(json!({"allow": false, "effect": "draft", "said": "Books is off here."}))).unwrap_err();
        assert_eq!(e.message, "Books is off here.");
    }

    #[test]
    fn a_write_or_a_word_nobody_knows_is_a_person_only_refusal_even_if_it_says_allow() {
        for word in [json!({"allow": true, "effect": "write"}), json!({"allow": true}), json!({"allow": true, "effect": "file"}), json!({"allow": false, "effect": "write"})] {
            let e = check(Some(word)).unwrap_err();
            assert_eq!(e.message, "Only a person can do this in Books, and this press came from the AI, so nothing was sent. To go ahead, press it yourself.");
        }
        assert!(take_refused().contains(&Held::Person));
    }
}
