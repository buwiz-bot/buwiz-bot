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
//! - A tool's effect comes from what the broker lists for it
//!   ([`tool_effect`]), the reverse of `for_agent` in the broker's
//!   `hub/tools/v1.rs`: anything not shown to be open to agents is the
//!   person's. A tool that says it only reads stays open to the screen that
//!   made the call, whatever its audience: its answer stays in the app.
//!
//! - A write open to agents still answers to the Bot's switches: the broker
//!   decides it, for the `ui_act` in flight (`POST /v1/host/ui/press`), as it
//!   does the model's direct call of that tool: off refuses, Ask asks. With
//!   no action in flight or no answer, it is refused.
//!
//! No approval exists yet (S4), so an agent's press on a person-only tool
//! is refused at the bridge, before anything is signed or sent.
//!
//! What a plugin lists is kept per generation ([`tool_list`]). A list gone
//! stale can't let anything through: the broker checks the live one again,
//! both for the switch and on the signed call.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use buwiz_agent::{ActorId, AgentId, Allowed, Caller, Effect, Op, PressAction, Refused, Target, authorize_press};
use serde_json::{Value, json};

use crate::mini_v1::{RoxError, host_api};

/// Who the Hub says is pressing.
const AGENT: &str = "agent.broker";

/// The most refusals kept for the answer to one action.
const KEPT: usize = 16;
/// Where the broker decides a press, and how long it may take: a person may
/// have to answer, and a bridge call lasts 30 s.
const PRESS: &str = "/v1/host/ui/press";
const PRESS_WAIT: Duration = Duration::from_secs(27);
/// How long a plugin's tools are kept, whatever its generation: a plugin
/// can list new tools without restarting.
const TOOLS_KEPT: Duration = Duration::from_secs(30);

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
    let action = op_effect(target, op).zip(op_id(op)).map(|(effect, id)| PressAction { target: target.clone(), id, effect, once: None });
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

/// What `tool` does, from `listed`: the tools of its plugin, as
/// `GET /v1/host/plugins` gives them. Unlisted, or no clear word that it is
/// open to agents, is [`Effect::Write`]: the person's.
pub fn tool_effect(listed: &[Value], tool: &str) -> Effect {
    let Some(def) = listed.iter().find(|t| t.get("name").and_then(Value::as_str) == Some(tool)) else {
        return Effect::Write;
    };
    if def.get("readOnly") == Some(&Value::Bool(true)) {
        return Effect::Read;
    }
    let open = def.get("audience").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x.as_str() == Some("agent")));
    if open { Effect::Draft } else { Effect::Write }
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

/// A plugin's tools as last listed.
struct Listed {
    generation: u64,
    at: Instant,
    tools: Vec<Value>,
}

static TOOLS: Mutex<Option<HashMap<String, Listed>>> = Mutex::new(None);

/// `plugin` is gone or stopped: its tools aren't kept.
pub fn forget_tools(plugin: &str) {
    if let Some(kept) = TOOLS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        kept.remove(plugin);
    }
}

/// `plugin`'s tools: kept ones if they are of `generation`, its current
/// one, and recent; else the broker's, kept when the broker names their
/// generation. `None` when the broker doesn't answer.
fn tool_list(broker: &dyn host_api::Broker, plugin: &str, generation: Option<u64>) -> Option<Vec<Value>> {
    let mut kept = TOOLS.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(g), Some(k)) = (generation, kept.as_ref().and_then(|m| m.get(plugin)))
        && k.generation == g
        && k.at.elapsed() < TOOLS_KEPT
    {
        return Some(k.tools.clone());
    }
    let rows = host_api::plugins(broker).ok()?;
    let row = rows.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(plugin))?;
    let tools = row.get("tools").and_then(Value::as_array)?.clone();
    let map = kept.get_or_insert_with(HashMap::new);
    match row.get("generation").and_then(Value::as_u64) {
        Some(generation) => map.insert(plugin.to_owned(), Listed { generation, at: Instant::now(), tools: tools.clone() }),
        None => map.remove(plugin),
    };
    Some(tools)
}

/// What the broker says of a write open to agents, for the `ui_act` in
/// flight: `Err` holds the sentence.
fn switch_allows(broker: &dyn host_api::Broker, plugin: &str, tool: &str) -> Result<(), String> {
    let said = "The Bot couldn't check this press, so nothing was sent.";
    match broker.request("POST", PRESS, Some(json!({"plugin": plugin, "tool": tool})), PRESS_WAIT) {
        Ok(v) if v.get("allow") == Some(&Value::Bool(true)) => Ok(()),
        Ok(v) => Err(v.get("said").and_then(Value::as_str).unwrap_or(said).to_owned()),
        Err(_) => Err(said.to_owned()),
    }
}

/// Decides a call of `tool` made under an agent's press: the press record
/// to sign with it, or the refusal. Nothing else may go out for a call of
/// the agent's: asked before signing, this is what holds it back.
/// `generation` is the plugin's current one.
pub fn check_tool(broker: &dyn host_api::Broker, plugin: &str, short: &str, title: &str, tool: &str, generation: Option<u64>) -> Result<Value, RoxError> {
    let effect = tool_list(broker, plugin, generation).map_or(Effect::Write, |t| tool_effect(&t, tool));
    let action = Target::parse(short).ok().zip(tool_id(tool)).map(|(target, id)| PressAction { target, id, effect, once: None });
    let Ok(allowed) = authorize_press(action.as_ref(), caller().as_ref()) else {
        note_refused(Held::Person);
        return Err(RoxError::new("permission_denied", tool_refused(title)));
    };
    if effect != Effect::Read
        && let Err(said) = switch_allows(broker, plugin, tool)
    {
        note_refused(Held::Said(said.clone()));
        return Err(RoxError::new("permission_denied", said));
    }
    Ok(json!(allowed.pressed))
}

/// Why a call under an agent's press was held back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// Its tool is the person's.
    Person,
    /// It would have reached the pasteboard.
    Pasteboard,
    /// The broker's sentence.
    Said(String),
}

static REFUSED: Mutex<Vec<Held>> = Mutex::new(Vec::new());

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

#[cfg(test)]
pub(crate) mod tests {
    use buwiz_agent::{Approval, DeliveryMode, KeybindingScope, OnceId, PressedBy, ScreenshotMode};

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
        let action = PressAction { target: target("books"), id: id("tool.post"), effect: Effect::Write, once: None };
        assert_eq!(authorize_press(Some(&action), caller().as_ref()), Err(Refused::ApprovalMissing));
        let made_up = Caller::Agent {
            who: ActorId::parse(AGENT).unwrap(),
            approval: Some(Approval { target: target("books"), action: id("tool.post"), once: OnceId::parse("made-up-once").unwrap(), approved_by: ActorId::parse("person.owner").unwrap() }),
        };
        assert_eq!(authorize_press(Some(&action), Some(&made_up)), Err(Refused::NoOpenPress));
    }

    #[test]
    fn a_tool_is_the_persons_unless_it_says_it_is_open_to_agents_or_only_reads() {
        let listed = [
            json!({"name": "fx_draft", "audience": ["app", "agent"]}),
            json!({"name": "fx_read", "readOnly": true}),
            json!({"name": "fx_read_for_apps", "readOnly": true, "audience": ["app"]}),
            json!({"name": "fx_app", "audience": ["app"]}),
            json!({"name": "fx_bare"}),
            json!({"name": "fx_empty", "audience": []}),
            json!({"name": "fx_word", "audience": "agent"}),
            json!({"name": "fx_vague", "readOnly": "true"}),
        ];
        let effect = |name| tool_effect(&listed, name);
        assert_eq!(effect("fx_draft"), Effect::Draft);
        assert_eq!(effect("fx_read"), Effect::Read);
        assert_eq!(effect("fx_read_for_apps"), Effect::Read);
        for name in ["fx_app", "fx_bare", "fx_empty", "fx_word", "fx_vague", "fx_not_listed"] {
            assert_eq!(effect(name), Effect::Write, "{name}");
        }
    }

    /// Answers `/v1/host/plugins`, or fails, and the switch as told.
    struct Listing {
        list: Option<Value>,
        press: Result<Value, ()>,
        lists: std::sync::atomic::AtomicUsize,
        presses: std::sync::atomic::AtomicUsize,
    }

    #[allow(non_snake_case)]
    fn Listing(list: Option<Value>) -> Listing {
        Listing { list, press: Ok(json!({"allow": true})), lists: Default::default(), presses: Default::default() }
    }

    impl host_api::Broker for Listing {
        fn request(&self, _: &str, route: &str, _: Option<Value>, _: std::time::Duration) -> Result<Value, RoxError> {
            use std::sync::atomic::Ordering::SeqCst;
            let down = || RoxError::new("unavailable", "no broker");
            if route == PRESS {
                self.presses.fetch_add(1, SeqCst);
                return self.press.clone().map_err(|()| down());
            }
            assert_eq!(route, "/v1/host/plugins");
            self.lists.fetch_add(1, SeqCst);
            self.list.clone().ok_or_else(down)
        }
    }

    fn check(b: &Listing, tool: &str) -> Result<Value, RoxError> {
        check_tool(b, "app.buwiz.books", "books", "Books", tool, None)
    }

    fn draft_list(plugin: &str, generation: u64, audience: Value) -> Option<Value> {
        Some(json!({"plugins": [{"id": plugin, "generation": generation, "tools": [{"name": "fx_draft", "audience": audience}, {"name": "fx_read", "readOnly": true}]}]}))
    }

    #[test]
    fn a_read_goes_out_with_the_press_record_and_asks_no_switch() {
        let b = Listing(Some(json!({"plugins": [{"id": "app.buwiz.books", "tools": [{"name": "books_app_month_pack", "readOnly": true, "audience": ["app"]}]}]})));
        assert_eq!(check(&b, "books_app_month_pack").unwrap(), json!({"by": "ai", "who": "agent.broker"}));
        assert_eq!(b.presses.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_write_open_to_agents_answers_to_the_brokers_switch() {
        let list = || Some(json!({"plugins": [{"id": "app.buwiz.books", "tools": [{"name": "books_entry_draft", "audience": ["app", "agent"]}]}]}));
        let mut off = Listing(list());
        off.press = Ok(json!({"allow": false, "said": "Books is off for this Bot, so nothing was sent."}));
        assert_eq!(check(&off, "books_entry_draft").unwrap_err().message, "Books is off for this Bot, so nothing was sent.");
        let mut gone = Listing(list());
        gone.press = Err(());
        assert_eq!(check(&gone, "books_entry_draft").unwrap_err().message, "The Bot couldn't check this press, so nothing was sent.");
        let mut bare = Listing(list());
        bare.press = Ok(json!({}));
        assert!(check(&bare, "books_entry_draft").is_err(), "no clear yes is a no");
    }

    #[test]
    fn a_plugins_tools_are_fetched_once_a_generation() {
        use std::sync::atomic::Ordering::SeqCst;
        let plugin = "app.buwiz.cachefx";
        let check = |b: &Listing, g| check_tool(b, plugin, "cachefx", "Fx", "fx_draft", Some(g));
        let b = Listing(draft_list(plugin, 4, json!(["agent"])));
        for _ in 0..3 {
            check(&b, 4).unwrap();
        }
        assert_eq!(b.lists.load(SeqCst), 1);
        assert_eq!(b.presses.load(SeqCst), 3, "the switch is the broker's, every time");
        // a new generation reads again, and its answer holds
        let b = Listing(draft_list(plugin, 5, json!(["app"])));
        assert!(check(&b, 5).is_err());
        assert_eq!(b.lists.load(SeqCst), 1);
    }

    #[test]
    fn kept_tools_go_when_their_plugin_stops_and_a_stale_list_stops_at_the_broker() {
        use std::sync::atomic::Ordering::SeqCst;
        let plugin = "app.buwiz.stalefx";
        let check = |b: &Listing| check_tool(b, plugin, "stalefx", "Fx", "fx_draft", Some(1));
        check(&Listing(draft_list(plugin, 1, json!(["agent"]))), ).unwrap();
        // the plugin narrowed the tool in the same generation: the kept list says open, the live
        // check doesn't
        let mut stale = Listing(draft_list(plugin, 1, json!(["app"])));
        stale.press = Ok(json!({"allow": false, "said": "no"}));
        assert!(check(&stale).is_err());
        assert_eq!(stale.lists.load(SeqCst), 0, "it was the kept list");
        forget_tools(plugin);
        let fresh = Listing(draft_list(plugin, 1, json!(["app"])));
        assert!(check(&fresh).is_err());
        assert_eq!(fresh.lists.load(SeqCst), 1);
    }

    #[test]
    fn a_tool_open_to_agents_goes_out_with_the_press_record() {
        let b = Listing(Some(json!({"plugins": [{"id": "app.buwiz.books", "tools": [{"name": "books_entry_draft", "audience": ["app", "agent"]}]}]})));
        assert_eq!(check(&b, "books_entry_draft").unwrap(), json!({"by": "ai", "who": "agent.broker"}));
    }

    #[test]
    fn with_no_audience_or_no_answer_the_press_is_refused_in_a_plain_sentence() {
        let bare = Listing(Some(json!({"plugins": [{"id": "app.buwiz.books", "tools": [{"name": "books_app_post_entry"}]}]})));
        let other = Listing(Some(json!({"plugins": [{"id": "app.buwiz.other", "tools": [{"name": "books_app_post_entry", "audience": ["agent"]}]}]})));
        for b in [bare, other, Listing(None)] {
            let e = check(&b, "books_app_post_entry").unwrap_err();
            assert_eq!(e.code, "permission_denied");
            assert_eq!(e.message, "Only a person can do this in Books, and this press came from the AI, so nothing was sent. To go ahead, press it yourself.");
        }
        assert!(take_refused().contains(&Held::Person));
    }
}
