//! The Agent Hub's tools. `ui_look` reads: what the model may see of the mini app screens open
//! in the Bot (`app/src/agent_hub/`). `ui_act` clicks, types and sets a value on the top one.
//!
//! The screens are the Bot's, and the broker can't reach them by itself: a shipped Bot opens
//! no port. So the Bot keeps a connection here, `GET /v1/host/ui/stream`: a look goes down it
//! as a `buwiz-agent` envelope and the answer comes back on `POST /v1/host/ui/reply`, matched
//! by id. A channel of its own, because `/v1/host/events` is a broadcast and drops on lag.
//!
//! The Bot filters a screen before it answers (O1): the broker never holds what the model may
//! not read. Each tool has its switch, `screens` and `screen_actions`, and both start off for
//! every profile (`policy::OFF_BY_DEFAULT`). Acting also needs `screens`: an action's answer is a
//! look.
//!
//! What may be pressed is decided in the Bot. All this side does is say what the model asked
//! for: the Bot takes nothing else from a message, not even its `caller`. What the Bot refuses
//! the model reads as a sentence, and a real refusal the person reads too, in the chat ([`act`]).
//!
//! What only this side can decide: each call of the agent's comes here first
//! (`POST /v1/host/ui/press`, [`press_word`]), against the live tool list. A read passes, so a
//! screen stays alive. A write open to agents is judged in the turn of the action in flight, on
//! the app that action is on, as the model's direct call of that tool is: off refuses, and Ask
//! says `hold`. So does a person-only tool whose plugin opted in (`_meta["buwiz/press"]`, O3).
//! Anything else is the person's. With no action in flight there is no turn to judge a write in,
//! and it is refused.
//!
//! `hold` only ever says a call may wait for the person. The card, and the yes, are the Bot's
//! alone: nothing this side says can let a held call go. The answer comes back with the action's,
//! as what happened to each held call and never its values: the card stays in the Bot.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use buwiz_agent::{ActorId, AgentId, Caller, DeliveryMode, Envelope, Op, Response, Target, UiNode, UiTree};
use buwiz_protocol::ui as ui_time;
use bytes::Bytes;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::{controls, v1, Out, Run};
use crate::helper::sessions;
use crate::hub::ctx::HubCtx;
use crate::hub::policy::{self, ask, keys};
use crate::hub::store::Turn;
use crate::js;

pub const NAME: &str = "ui_look";
pub const ACT: &str = "ui_act";
/// The switch of [`ACT`].
pub const ACT_KEY: &str = "screen_actions";

/// How long the Bot has to answer: it reads the tree on its main thread.
const WAIT: Duration = Duration::from_secs(10);
const ACT_WAIT: Duration = ui_time::ACT_WAIT;
const PRESS_WAIT: Duration = ui_time::PRESS_WAIT;
const NOBODY: &str = "Nobody answered in time, so this press was refused and nothing was sent.";
const PING: Duration = Duration::from_secs(25);
/// How long a full stream gets to take the word that the agent is opening a screen.
const OPENED_WAIT: Duration = Duration::from_secs(2);
const NOT_TOLD: &str = "The Bot couldn't be told the AI is opening this, so it wasn't opened.";
/// What Activity calls a press: its card's words stay in the Bot.
const PRESSED: &str = "A press on a screen";

/// The model's definition.
pub fn def() -> Value {
    json!({
        "name": NAME,
        "description": "Look at the mini app screens open in the Bot: each screen's name, which one is on top, and its buttons and fields. It only reads. Page text and field values are hidden from you: a button with no name is a row of data. Use it when the user asks about what is on their screen.",
        "input_schema": {"type": "object", "properties": {}},
    })
}

/// [`ACT`]'s definition.
pub fn act_def() -> Value {
    json!({
        "name": ACT,
        "description": "Act on the mini app screen on top in the Bot: press a button, or type in a field. Use the app and the node ids ui_look gave you, and look first: if the screen changed, nothing happens and you have to look again. It can fill in and save a draft. Pressing Post or Close month holds the press unsent while the person sees exactly what it will do on a card, and it goes out only if they allow it: press it when they asked for it, and wait for the answer. Everything else only a person can do, such as void, reopen, confirm or send, is refused: nothing is sent, and you should say so and leave it to them. Never press again after Not now. A node with no name can't be used. The answer is the screen after the action.",
        "input_schema": {"type": "object", "properties": {
            "app": {"type": "string", "description": "the screen's app, from ui_look"},
            "node": {"type": "string", "description": "a node id from ui_look"},
            "action": {"type": "string", "enum": ["click", "type", "set_value"], "description": "click a button, type after what a field holds, or replace what it holds"},
            "text": {"type": "string", "description": "the text, for type and set_value"},
        }, "required": ["app", "node", "action"]},
    })
}

/// The Bot's connection and the looks waiting for an answer.
#[derive(Default)]
pub struct UiLink {
    bot: Mutex<Option<mpsc::Sender<Bytes>>>,
    waiting: Mutex<HashMap<String, oneshot::Sender<Response>>>,
    next: AtomicU64,
    /// The action in flight.
    acting: Mutex<Option<Acting>>,
}

/// The action in flight: the plugin of the app it is on, and where its presses go.
#[derive(Clone)]
struct Acting {
    plugin: Option<String>,
    presses: mpsc::Sender<PressAsk>,
}

/// A write an action led to, waiting for its turn's word.
struct PressAsk {
    plugin: String,
    tool: String,
    answer: oneshot::Sender<Result<Word, String>>,
}

/// A turn's word on a write. Never a yes to a person-only call: that is the Bot's card's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    Go,
    /// It waits, unsent, for the person's answer on the Bot's card.
    Hold,
}

/// Ends an action's claim on [`UiLink::acting`], however it ends.
struct Claim<'a>(&'a UiLink);

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        *self.0.acting.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl UiLink {
    /// A Bot connected; → its stream. It takes the place of the one before, whose stream ends.
    pub fn attach(&self) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(16);
        let ping = tx.downgrade();
        *self.bot.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        tokio::spawn(async move {
            while let Some(tx) = ping.upgrade() {
                if tx.send(Bytes::from_static(b": ping\n\n")).await.is_err() {
                    return;
                }
                drop(tx);
                tokio::time::sleep(PING).await;
            }
        });
        rx
    }

    /// Sends `request` to the Bot; → its answer.
    async fn ask(&self, request: &Envelope, wait: Duration) -> Result<Response, String> {
        let bot = self.bot.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let bot = bot.ok_or("The Bot isn't connected to the broker.")?;
        let line = serde_json::to_string(request).map_err(|e| e.to_string())?;
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(request.id.clone(), tx);
        let sent = bot.send_timeout(Bytes::from(format!("event: ui\ndata: {line}\n\n")), wait).await.is_ok();
        let answer = if sent { tokio::time::timeout(wait, rx).await.ok().and_then(Result::ok) } else { None };
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&request.id);
        answer.ok_or_else(|| "The Bot didn't answer.".to_owned())
    }

    /// The broker is opening a screen for the model. The Bot takes that as the agent acting, so
    /// nothing that opens now goes out as the person's. With no Bot there is no one to tell, and
    /// no screen of the Bot's to mark. `Err` holds the sentence: a Bot that can't be told would
    /// read what opens as the person's, so nothing may open.
    pub async fn agent_opened(&self) -> Result<(), String> {
        let bot = self.bot.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(bot) = bot else { return Ok(()) };
        bot.send_timeout(Bytes::from_static(ui_time::AGENT_OPENED_EVENT), OPENED_WAIT).await.map_err(|_| NOT_TOLD.to_owned())
    }

    /// Claims the one action in flight; `None` while another runs.
    fn claim(&self, plugin: Option<String>) -> Option<(Claim<'_>, mpsc::Receiver<PressAsk>)> {
        let mut acting = self.acting.lock().unwrap_or_else(|e| e.into_inner());
        if acting.is_some() {
            return None;
        }
        let (presses, rx) = mpsc::channel(8);
        *acting = Some(Acting { plugin, presses });
        Some((Claim(self), rx))
    }

    /// `plugin`'s `tool`, which the Bot says the action in flight led to: `Err` holds the sentence.
    pub async fn press(&self, plugin: &str, tool: &str) -> Result<Word, String> {
        let acting = self.acting.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let acting = acting.ok_or("No action of the AI's is running, so this press was refused and nothing was sent.")?;
        if acting.plugin.as_deref() != Some(plugin) {
            return Err("That press isn't in the mini app the AI is acting on, so it was refused and nothing was sent.".to_owned());
        }
        let acting = acting.presses;
        let (answer, heard) = oneshot::channel();
        let ask = PressAsk { plugin: plugin.to_owned(), tool: tool.to_owned(), answer };
        let lost = "The Bot couldn't check this press, so nothing was sent.";
        acting.send(ask).await.map_err(|_| lost)?;
        match tokio::time::timeout(PRESS_WAIT + Duration::from_secs(1), heard).await {
            Ok(Ok(said)) => said,
            _ => Err(NOBODY.to_owned()),
        }
    }

    /// The Bot's answer `r`; false when no look is waiting for it.
    pub fn reply(&self, r: Response) -> bool {
        let waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&r.id);
        waiting.is_some_and(|tx| tx.send(r).is_ok())
    }
}

/// A look for the turn of `bot`: the Bot answers with what that chat opened and nothing else.
fn request(id: String, bot_id: Option<String>) -> Result<Envelope, String> {
    let mut e = Envelope::new(id, bot()?, Op::Snapshot { since: None, format: None, fingerprints: None });
    let who = ActorId::parse("agent.broker").map_err(|e| e.to_string())?;
    e.caller = Some(Caller::Agent { who, approval: None });
    e.bot = bot_id;
    Ok(e)
}

/// The action the model asked for. Its input gives the app, the node, what to do and the text;
/// whatever else it holds stays here, and who is asking, and for which Bot, is this side's to say.
fn act_request(id: String, input: &Value, bot: Option<String>) -> Result<Envelope, String> {
    let text = |k: &str| input.get(k).and_then(Value::as_str);
    let target = text("app").and_then(|a| Target::parse(a).ok()).ok_or("That isn't an app from the last look, so nothing was pressed.")?;
    let node = text("node").and_then(|n| AgentId::parse(n).ok()).ok_or("That isn't a button or field from the last look, so nothing was pressed.")?;
    let typed = || text("text").map(str::to_owned).ok_or("Say what to type, so nothing was typed.");
    let op = match text("action") {
        Some("click") => Op::Click { node, delivery: DeliveryMode::Semantic, expect_fingerprint: None },
        Some("type") => Op::Type { node, text: typed()?, delivery: DeliveryMode::Semantic, expect_fingerprint: None },
        Some("set_value") => Op::SetValue { node, value: typed()?, expect_fingerprint: None },
        _ => return Err("The AI can click, type or set a value, so nothing happened.".to_owned()),
    };
    let mut e = Envelope::new(id, target, op);
    let who = ActorId::parse("agent.broker").map_err(|e| e.to_string())?;
    e.caller = Some(Caller::Agent { who, approval: None });
    e.bot = bot;
    Ok(e)
}

/// Whether `plugin` and `tool` have the shape of a plugin's id and a tool's name.
fn well_formed(plugin: &str, tool: &str) -> bool {
    let ok = |s: &str, more: &[u8]| !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || more.contains(&b));
    ok(plugin, b".-") && ok(tool, b"._-")
}

/// `POST /v1/host/ui/press {plugin, tool}`: the answer to a call of the agent's, from the live
/// tool list. `None` for what isn't a plugin and a tool.
pub async fn press_word(ctx: &HubCtx, plugin: &str, tool: &str) -> Option<Value> {
    if !well_formed(plugin, tool) {
        return None;
    }
    let def = v1::tool_def(ctx, plugin, tool);
    if def.as_ref().is_some_and(keys::declared_read) {
        return Some(json!({"allow": true, "effect": "read"}));
    }
    let open = def.as_ref().is_some_and(sessions::open_to_agents);
    if !open && !def.as_ref().is_some_and(sessions::press_opted_in) {
        return Some(json!({"allow": false, "effect": "write"}));
    }
    let effect = if open { "draft" } else { "write" };
    Some(match ctx.ui.press(plugin, tool).await {
        Ok(Word::Go) => json!({"allow": true, "effect": effect}),
        Ok(Word::Hold) => {
            let mut word = json!({"allow": false, "hold": true, "effect": effect});
            // a card is read with a read of the plugin's own, or not at all
            let describe = def.as_ref().and_then(sessions::press_describe).filter(|d| v1::tool_def(ctx, plugin, d).as_ref().is_some_and(keys::declared_read));
            if let Some(d) = describe {
                word["describe"] = json!(d);
            }
            word
        }
        Err(said) => json!({"allow": false, "effect": effect, "said": said}),
    })
}

/// The plugin of the app `short` names, if one is installed.
fn plugin_of(ctx: &HubCtx, short: &str) -> Option<String> {
    v1::view(ctx).rows().into_iter().find(|r| r.short == short).map(|r| r.id)
}

/// Whether a write the action led to may go out, as this turn's direct call of the tool would.
fn judge(run: &Run, plugin: &str, tool: &str) -> Result<Word, String> {
    let key = format!("v1:{plugin}");
    let (ctx, title) = (&run.ctx, keys::label_of(&run.ctx, &key).map_or_else(|| plugin.to_owned(), |l| l.0));
    let def = v1::tool_def(ctx, plugin, tool);
    let open = def.as_ref().is_some_and(sessions::open_to_agents) && keys::key_of(ctx, tool, run.turn.surface.as_ref()).as_deref() == Some(&key);
    if !open && !def.as_ref().is_some_and(sessions::press_opted_in) {
        return Err(format!("Only a person can do this in {title}, so the AI's press was refused and nothing was sent."));
    }
    if controls::press_off(run, &key) {
        return Err(format!("{title} is off here, so the AI's press was refused and nothing was sent. Only a person can turn it on."));
    }
    if open && !controls::press_asks(run, &key) {
        return Ok(Word::Go);
    }
    if !ask::present(&run.turn) {
        policy::log_ask(ctx, controls::screen_bot(run).as_deref(), PRESSED, tool, "needs_ok");
        return Err(ask::NOBODY.to_owned());
    }
    run.set_job("waiting", ask::WAITING);
    Ok(Word::Hold)
}

fn bot() -> Result<Target, String> {
    Target::parse("bot").map_err(|e| e.to_string())
}

/// One node as the model reads it. Only these fields: whatever else an answer carries stays
/// out.
fn node_view(n: &UiNode) -> Value {
    let mut v = json!({"id": n.id.as_str(), "role": n.role, "name": n.name});
    if !n.enabled {
        v["enabled"] = json!(false);
    }
    if n.focused {
        v["focused"] = json!(true);
    }
    v
}

fn state(n: &UiNode) -> &str {
    n.states.first().map_or("card", String::as_str)
}

/// The answer as the model reads it.
fn render(r: &Response) -> Result<Value, String> {
    if !r.ok {
        return Err(r.error.clone().unwrap_or_else(|| "The Bot refused.".into()));
    }
    let apps = r.result.as_ref().and_then(|v| v.get("apps")).cloned().unwrap_or(Value::Null);
    let trees: Vec<UiTree> = serde_json::from_value(apps).map_err(|_| "The Bot's answer can't be read.".to_owned())?;
    let pages = || trees.iter().flat_map(|t| t.nodes.iter().map(move |p| (t, p)));
    let screens: Vec<Value> = pages().map(|(t, p)| json!({"id": p.id.as_str(), "app": t.target.as_str(), "name": p.name, "place": state(p), "nodes": p.children.iter().map(node_view).collect::<Vec<_>>()})).collect();
    let top = pages().find(|(_, p)| state(p) == "top").map(|(_, p)| p.name.clone());
    Ok(json!({"top": top, "screens": screens}))
}

/// `ui_look`.
pub async fn look(run: &Run) -> Out {
    let ctx = &run.ctx;
    let id = format!("ui-{}", ctx.ui.next.fetch_add(1, Ordering::Relaxed));
    let answer = match request(id, controls::screen_bot(run)) {
        Ok(e) => ctx.ui.ask(&e, WAIT).await,
        Err(e) => Err(e),
    };
    match answer.and_then(|r| render(&r)) {
        Ok(v) => Out::ok(js::stringify(&v)),
        Err(why) => Out::err(why),
    }
}

/// The sentences under `key` in an action's answer.
fn sentences(r: &Response, key: &str) -> Vec<String> {
    let list = r.result.as_ref().and_then(|v| v.get(key)).and_then(Value::as_array);
    list.into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// What an action's answer comes to: the screen after it, and what was refused for the person
/// to read. `declined` is what the person turned down themselves: the model reads it, and they
/// don't need telling.
fn render_act(r: &Response) -> Result<(Value, Vec<String>), String> {
    let mut view = render(r)?;
    let refused = sentences(r, "refused");
    let all: Vec<String> = refused.iter().cloned().chain(sentences(r, "declined")).collect();
    view["done"] = json!(all.is_empty());
    if !all.is_empty() {
        view["refused"] = json!(all);
    }
    Ok((view, refused))
}

/// Keeps what happened to each held call in Activity: the tool and the outcome, as every
/// other answer on a card, and never its values.
fn log_held(run: &Run, r: &Response) {
    let list = r.result.as_ref().and_then(|v| v.get("held")).and_then(Value::as_array);
    for h in list.into_iter().flatten() {
        if let (Some(tool), Some(outcome)) = (h.get("tool").and_then(Value::as_str), h.get("outcome").and_then(Value::as_str)) {
            policy::log_ask(&run.ctx, controls::screen_bot(run).as_deref(), PRESSED, tool, outcome);
        }
    }
}

/// Whether the Bot marked its refusal as one the person should read.
fn tells_person(r: &Response) -> bool {
    !r.ok && r.result.as_ref().and_then(|v| v.get("tell")) == Some(&Value::Bool(true))
}

/// Tells the person what was refused, in the chat the model is working in. Its own line: what
/// the model says next is up to the model.
fn tell_person(run: &Run, said: &str) {
    let n = json!({"kind": "ui_refused", "text": said});
    match run.ctx.store.append_turn(&run.thread_id(), Turn::notice(said, n)) {
        Ok(_) => run.ctx.store.write_index(),
        Err(e) => eprintln!("buwiz-broker: ui_act's notice: the store's write: {e}"),
    }
}

/// `ui_act`.
pub async fn act(run: &Run, input: &Value) -> Out {
    let ui = &run.ctx.ui;
    let id = format!("ui-{}", ui.next.fetch_add(1, Ordering::Relaxed));
    let e = match act_request(id, input, controls::screen_bot(run)) {
        Ok(e) => e,
        Err(why) => return Out::err(why),
    };
    let Some((_claim, mut presses)) = ui.claim(plugin_of(&run.ctx, e.target.as_str())) else {
        return Out::err("The AI's last action is still running, so nothing was pressed.".to_owned());
    };
    let asked = ui.ask(&e, ACT_WAIT);
    tokio::pin!(asked);
    let answer = loop {
        tokio::select! {
            r = &mut asked => break r,
            Some(p) = presses.recv() => {
                let _ = p.answer.send(judge(run, &p.plugin, &p.tool));
            }
        }
    };
    let r = match answer {
        Ok(r) => r,
        Err(why) => return Out::err(why),
    };
    log_held(run, &r);
    match render_act(&r) {
        Ok((view, refused)) => {
            refused.iter().for_each(|said| tell_person(run, said));
            Out::ok(js::stringify(&view))
        }
        Err(why) => {
            if tells_person(&r) {
                tell_person(run, &why);
            }
            Out::err(why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Bot's side of a test: reads one request off `stream`, and answers with `answer(id)`.
    async fn bot_answers(link: std::sync::Arc<UiLink>, mut stream: mpsc::Receiver<Bytes>, answer: impl Fn(&Envelope) -> Response) {
        loop {
            let chunk = stream.recv().await.expect("a chunk");
            let text = String::from_utf8_lossy(&chunk).into_owned();
            if let Some(line) = text.strip_prefix("event: ui\ndata: ") {
                let e: Envelope = serde_json::from_str(line.trim()).unwrap();
                assert!(link.reply(answer(&e)));
                return;
            }
        }
    }

    fn page_answer(e: &Envelope) -> Response {
        let mut r = Response::ok(e.id.clone(), e.target.clone());
        r.result = Some(json!({"apps": [{"target": "ebirforms", "platform": "desktop", "ready": true, "nodes": [
            {"id": "screen.forms", "role": "page", "name": "eBIRForms: Forms", "enabled": true, "focused": false, "bounds": {"x": 0, "y": 0, "w": 0, "h": 0}, "states": ["covered"]},
            {"id": "screen.taxprofile", "role": "page", "name": "eBIRForms: Tax Profile", "enabled": true, "focused": false, "bounds": {"x": 0, "y": 0, "w": 0, "h": 0}, "states": ["top"], "children": [
                {"id": "button.new-return", "role": "button", "name": "New return", "enabled": true, "focused": false, "bounds": {"x": 0, "y": 0, "w": 0, "h": 0}},
                {"id": "field.tin", "role": "textfield", "name": "TIN", "value": "not for the model", "enabled": false, "focused": true, "bounds": {"x": 0, "y": 0, "w": 0, "h": 0}},
            ]},
        ]}]}));
        r
    }

    async fn asked(link: &std::sync::Arc<UiLink>) -> Result<Response, String> {
        link.ask(&request("ui-t".into(), None).unwrap(), Duration::from_secs(2)).await
    }

    #[tokio::test]
    async fn a_look_goes_down_the_stream_and_its_answer_comes_back_by_id() {
        let link = std::sync::Arc::new(UiLink::default());
        let stream = link.attach();
        tokio::spawn(bot_answers(link.clone(), stream, |e| {
            assert_eq!(e.target.as_str(), "bot");
            assert!(matches!(e.op, Op::Snapshot { .. }));
            assert!(matches!(e.caller, Some(Caller::Agent { approval: None, .. })));
            page_answer(e)
        }));
        let got = render(&asked(&link).await.unwrap()).unwrap();
        assert_eq!(got["top"], "eBIRForms: Tax Profile");
        assert_eq!(got["screens"][0]["place"], "covered");
        assert_eq!(got["screens"][1]["nodes"][0], json!({"id": "button.new-return", "role": "button", "name": "New return"}));
    }

    #[test]
    fn no_value_reaches_the_model_whatever_an_answer_carries() {
        let e = request("ui-t".into(), None).unwrap();
        let got = render(&page_answer(&e)).unwrap();
        assert!(!js::stringify(&got).contains("not for the model"));
        assert_eq!(got["screens"][1]["nodes"][1], json!({"id": "field.tin", "role": "textfield", "name": "TIN", "enabled": false, "focused": true}));
    }

    #[tokio::test]
    async fn without_a_bot_or_its_answer_the_look_fails_and_leaves_nothing_waiting() {
        let link = std::sync::Arc::new(UiLink::default());
        assert_eq!(asked(&link).await.unwrap_err(), "The Bot isn't connected to the broker.");
        let _stream = link.attach();
        let e = request("ui-t".into(), None).unwrap();
        assert_eq!(link.ask(&e, Duration::from_millis(30)).await.unwrap_err(), "The Bot didn't answer.");
        assert!(link.waiting.lock().unwrap().is_empty());
        assert!(!link.reply(Response::ok("ui-t", e.target)), "a late answer finds nothing");
    }

    #[tokio::test]
    async fn a_new_connection_ends_the_old_stream() {
        let link = UiLink::default();
        let mut old = link.attach();
        let _new = link.attach();
        while let Some(chunk) = old.recv().await {
            assert_eq!(&chunk[..], b": ping\n\n");
        }
    }

    #[test]
    fn a_look_says_whose_turn_it_is() {
        assert_eq!(request("ui-t".into(), Some("hex".into())).unwrap().bot.as_deref(), Some("hex"));
        assert_eq!(request("ui-t".into(), None).unwrap().bot, None, "a plain chat's");
    }

    #[tokio::test]
    async fn opening_a_screen_for_the_model_tells_the_bot() {
        let link = UiLink::default();
        assert_eq!(link.agent_opened().await, Ok(()), "no Bot, no screen of its to mark");
        let mut stream = link.attach();
        link.agent_opened().await.unwrap();
        let heard = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let chunk = stream.recv().await.expect("a chunk");
                if chunk[..] != b": ping\n\n"[..] {
                    return chunk;
                }
            }
        });
        assert_eq!(&heard.await.expect("the Bot hears it")[..], ui_time::AGENT_OPENED_EVENT);
    }

    /// A Bot that can't be told would take what opens for the person's.
    #[tokio::test]
    async fn a_bot_that_cant_be_told_stops_the_open() {
        let link = UiLink::default();
        let stream = link.attach();
        for _ in 0..16 {
            link.bot.lock().unwrap().as_ref().unwrap().try_send(Bytes::from_static(b": full\n\n")).ok();
        }
        assert_eq!(link.agent_opened().await, Err(NOT_TOLD.to_owned()), "a full stream");
        drop(stream);
        assert_eq!(link.agent_opened().await, Err(NOT_TOLD.to_owned()), "a stream that ended");
    }

    fn asked_of(input: Value) -> Result<Envelope, String> {
        act_request("ui-t".into(), &input, None)
    }

    #[test]
    fn an_action_says_what_the_model_asked_for_and_only_that() {
        let e = asked_of(json!({"app": "books", "node": "button.save-draft", "action": "click"})).unwrap();
        assert_eq!(e.target.as_str(), "books");
        assert!(matches!(&e.op, Op::Click { node, expect_fingerprint: None, .. } if node.as_str() == "button.save-draft"));
        let e = asked_of(json!({"app": "books", "node": "field.amount", "action": "set_value", "text": "1,350.00"})).unwrap();
        assert!(matches!(&e.op, Op::SetValue { value, .. } if value == "1,350.00"));
        let e = asked_of(json!({"app": "books", "node": "field.memo", "action": "type", "text": "Rent"})).unwrap();
        assert!(matches!(&e.op, Op::Type { text, .. } if text == "Rent"));
    }

    #[test]
    fn a_caller_or_an_approval_from_the_model_goes_nowhere() {
        let approval = json!({"target": "books", "action": "tool.post", "once": "made-up-once", "approved_by": "person.owner"});
        let e = asked_of(json!({
            "app": "books", "node": "button.post", "action": "click",
            "caller": {"kind": "person", "who": "person.owner"}, "approval": approval, "expect_fingerprint": "1",
        }))
        .unwrap();
        assert!(matches!(&e.caller, Some(Caller::Agent { approval: None, who }) if who.as_str() == "agent.broker"));
        assert!(matches!(&e.op, Op::Click { expect_fingerprint: None, .. }));
    }

    #[test]
    fn what_ui_act_cant_ask_for_is_a_sentence_and_goes_nowhere() {
        for input in [
            json!({"node": "button.x", "action": "click"}),
            json!({"app": "Not A Target", "node": "button.x", "action": "click"}),
            json!({"app": "books", "node": "not an id", "action": "click"}),
            json!({"app": "books", "node": "button.x", "action": "keybinding"}),
            json!({"app": "books", "node": "button.x"}),
            json!({"app": "books", "node": "field.x", "action": "type"}),
        ] {
            let why = asked_of(input).unwrap_err();
            assert!(why.ends_with('.') && why.contains("nothing"), "{why}");
        }
    }

    #[test]
    fn only_a_refusal_the_bot_marked_reaches_the_person() {
        let t = Target::parse("books").unwrap();
        let mut policy = Response::err("ui-t", t.clone(), "covered");
        policy.result = Some(json!({"tell": true}));
        assert!(tells_person(&policy));
        let mut model = Response::err("ui-t", t.clone(), "no such id");
        model.result = Some(json!({"tell": false}));
        assert!(!tells_person(&model));
        assert!(!tells_person(&Response::err("ui-t", t, "unmarked")));
    }

    #[tokio::test]
    async fn with_no_action_in_flight_a_press_is_refused() {
        let link = UiLink::default();
        assert!(link.press("app.buwiz.books", "books_entry_draft").await.unwrap_err().contains("No action"));
        let (claim, _rx) = link.claim(None).unwrap();
        assert!(link.claim(None).is_none(), "one at a time");
        drop(claim);
        assert!(link.claim(None).is_some(), "the claim ends with its action");
    }

    #[tokio::test]
    async fn a_press_naming_another_plugin_than_the_action_is_refused() {
        let link = UiLink::default();
        let (_claim, _rx) = link.claim(Some("app.buwiz.books".into())).unwrap();
        assert!(link.press("app.buwiz.ebirforms", "ebirforms_form_save_draft").await.unwrap_err().contains("isn't in the mini app"));
        let unknown = UiLink::default();
        let (_claim, _rx) = unknown.claim(None).unwrap();
        assert!(unknown.press("app.buwiz.books", "books_entry_draft").await.is_err(), "an action on no known plugin leads to no write");
    }

    #[test]
    fn a_press_has_to_name_a_plugin_and_a_tool() {
        assert!(well_formed("app.buwiz.books", "books_entry_draft"));
        for (p, t) in [("", "x"), ("x", ""), ("a b", "x"), ("x", "a/b"), ("x", "a;b"), (&*"p".repeat(129), "x"), ("x", &*"t".repeat(129))] {
            assert!(!well_formed(p, t), "{p} {t}");
        }
    }

    #[test]
    fn an_answer_with_a_refusal_says_so_and_a_clean_one_doesnt() {
        let e = asked_of(json!({"app": "books", "node": "button.x", "action": "click"})).unwrap();
        let mut r = page_answer(&e);
        let (view, refused) = render_act(&r).unwrap();
        assert_eq!((view["done"].clone(), refused.len()), (json!(true), 0));
        assert!(view.get("refused").is_none());
        r.result.as_mut().unwrap()["refused"] = json!(["\"Save & post\" needs a person."]);
        let (view, refused) = render_act(&r).unwrap();
        assert_eq!(view["done"], false);
        assert_eq!(view["refused"][0], "\"Save & post\" needs a person.");
        assert_eq!(refused.len(), 1);
        assert_eq!(view["top"], "eBIRForms: Tax Profile", "the screen after it");
    }

    #[test]
    fn a_refusal_and_a_bad_answer_are_errors() {
        let e = request("ui-t".into(), None).unwrap();
        assert_eq!(render(&Response::err("ui-t", e.target.clone(), "no")).unwrap_err(), "no");
        let mut r = Response::ok("ui-t", e.target);
        r.result = Some(json!({"apps": [{"target": "not valid", "nodes": []}]}));
        assert!(render(&r).is_err());
    }
}
