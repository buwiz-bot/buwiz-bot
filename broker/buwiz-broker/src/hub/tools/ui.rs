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
//! One thing only this side can decide: whether the Bot's switches let a write through. While an
//! action runs, each write it leads to comes back here (`POST /v1/host/ui/press`, [`UiLink::press`])
//! and is judged in that action's turn, as the model's direct call of that tool is: off refuses,
//! Ask asks. With no action in flight there is no turn to judge it in, and it is refused.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use buwiz_agent::{ActorId, AgentId, Caller, DeliveryMode, Envelope, Op, Response, Target, UiNode, UiTree};
use bytes::Bytes;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::{controls, v1, Out, Run};
use crate::hub::ctx::HubCtx;
use crate::hub::policy::keys;
use crate::hub::store::Turn;
use crate::js;

pub const NAME: &str = "ui_look";
pub const ACT: &str = "ui_act";
/// The switch of [`ACT`].
pub const ACT_KEY: &str = "screen_actions";

/// How long the Bot has to answer: it reads the tree on its main thread.
const WAIT: Duration = Duration::from_secs(10);
/// How long an action has: its press, the app settling, and the calls it starts.
const ACT_WAIT: Duration = Duration::from_secs(45);
/// How long a press may wait for its answer, a person's included: a Bot's call lasts 30 s.
const PRESS_WAIT: Duration = Duration::from_secs(25);
const NOBODY: &str = "Nobody answered in time, so this press was refused and nothing was sent.";
const PING: Duration = Duration::from_secs(25);

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
        "description": "Act on the mini app screen on top in the Bot: press a button, or type in a field. Use the app and the node ids ui_look gave you, and look first: if the screen changed, nothing happens and you have to look again. It can fill in and save a draft. It can't do what only a person can do, such as post, void, close, confirm or send: that press is refused, nothing is sent, and you should say so and leave it to them. A node with no name can't be used. The answer is the screen after the action.",
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
    /// The action in flight: where its presses go.
    acting: Mutex<Option<mpsc::Sender<PressAsk>>>,
}

/// A write an action led to, waiting for its turn's word.
struct PressAsk {
    plugin: String,
    tool: String,
    answer: oneshot::Sender<Result<(), String>>,
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

    /// Claims the one action in flight; `None` while another runs.
    fn claim(&self) -> Option<(Claim<'_>, mpsc::Receiver<PressAsk>)> {
        let mut acting = self.acting.lock().unwrap_or_else(|e| e.into_inner());
        if acting.is_some() {
            return None;
        }
        let (tx, rx) = mpsc::channel(8);
        *acting = Some(tx);
        Some((Claim(self), rx))
    }

    /// `plugin`'s `tool`, which the Bot says the action in flight led to: `Err` holds the sentence.
    pub async fn press(&self, plugin: &str, tool: &str) -> Result<(), String> {
        let acting = self.acting.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let acting = acting.ok_or("No action of the AI's is running, so this press was refused and nothing was sent.")?;
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

fn request(id: String) -> Result<Envelope, String> {
    let mut e = Envelope::new(id, bot()?, Op::Snapshot { since: None, format: None, fingerprints: None });
    let who = ActorId::parse("agent.broker").map_err(|e| e.to_string())?;
    e.caller = Some(Caller::Agent { who, approval: None });
    Ok(e)
}

/// The action the model asked for. Its input gives the app, the node, what to do and the text;
/// whatever else it holds stays here, and who is asking is this side's to say.
fn act_request(id: String, input: &Value) -> Result<Envelope, String> {
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
    Ok(e)
}

/// Whether a write the action led to may go out, as this turn's direct call of the tool would.
async fn judge(run: &Run, plugin: &str, tool: &str) -> Result<(), String> {
    let (ctx, title) = (&run.ctx, keys::label_of(&run.ctx, &format!("v1:{plugin}")).map_or_else(|| plugin.to_owned(), |l| l.0));
    let open = v1::tool_def(ctx, plugin, tool).is_some_and(|t| crate::helper::sessions::open_to_agents(&t));
    if !open || keys::key_of(ctx, tool, run.turn.surface.as_ref()).as_deref() != Some(&format!("v1:{plugin}")) {
        return Err(format!("Only a person can do this in {title}, so the AI's press was refused and nothing was sent."));
    }
    if controls::refusal(run, tool, &json!({})).is_some() {
        return Err(format!("{title} is off here, so the AI's press was refused and nothing was sent. Only a person can turn it on."));
    }
    if controls::ask_press(run, tool).await.is_some() {
        return Err(format!("The AI's press in {title} wasn't allowed, so nothing was sent."));
    }
    Ok(())
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
pub async fn look(ctx: &HubCtx) -> Out {
    let id = format!("ui-{}", ctx.ui.next.fetch_add(1, Ordering::Relaxed));
    let answer = match request(id) {
        Ok(e) => ctx.ui.ask(&e, WAIT).await,
        Err(e) => Err(e),
    };
    match answer.and_then(|r| render(&r)) {
        Ok(v) => Out::ok(js::stringify(&v)),
        Err(why) => Out::err(why),
    }
}

/// What an action's answer comes to: the screen after it, and what was refused.
fn render_act(r: &Response) -> Result<(Value, Vec<String>), String> {
    let mut view = render(r)?;
    let list = r.result.as_ref().and_then(|v| v.get("refused")).and_then(Value::as_array);
    let refused: Vec<String> = list.into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    view["done"] = json!(refused.is_empty());
    if !refused.is_empty() {
        view["refused"] = json!(refused);
    }
    Ok((view, refused))
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
    let e = match act_request(id, input) {
        Ok(e) => e,
        Err(why) => return Out::err(why),
    };
    let Some((_claim, mut presses)) = ui.claim() else {
        return Out::err("The AI's last action is still running, so nothing was pressed.".to_owned());
    };
    let asked = ui.ask(&e, ACT_WAIT);
    tokio::pin!(asked);
    let answer = loop {
        tokio::select! {
            r = &mut asked => break r,
            Some(p) = presses.recv() => {
                // the wait ending takes the card with it
                let said = tokio::time::timeout(PRESS_WAIT, judge(run, &p.plugin, &p.tool)).await;
                let _ = p.answer.send(said.unwrap_or_else(|_| Err(NOBODY.to_owned())));
            }
        }
    };
    let r = match answer {
        Ok(r) => r,
        Err(why) => return Out::err(why),
    };
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
        link.ask(&request("ui-t".into()).unwrap(), Duration::from_secs(2)).await
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
        let e = request("ui-t".into()).unwrap();
        let got = render(&page_answer(&e)).unwrap();
        assert!(!js::stringify(&got).contains("not for the model"));
        assert_eq!(got["screens"][1]["nodes"][1], json!({"id": "field.tin", "role": "textfield", "name": "TIN", "enabled": false, "focused": true}));
    }

    #[tokio::test]
    async fn without_a_bot_or_its_answer_the_look_fails_and_leaves_nothing_waiting() {
        let link = std::sync::Arc::new(UiLink::default());
        assert_eq!(asked(&link).await.unwrap_err(), "The Bot isn't connected to the broker.");
        let _stream = link.attach();
        let e = request("ui-t".into()).unwrap();
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

    fn asked_of(input: Value) -> Result<Envelope, String> {
        act_request("ui-t".into(), &input)
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
        let (claim, _rx) = link.claim().unwrap();
        assert!(link.claim().is_none(), "one at a time");
        drop(claim);
        assert!(link.claim().is_some(), "the claim ends with its action");
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
        let e = request("ui-t".into()).unwrap();
        assert_eq!(render(&Response::err("ui-t", e.target.clone(), "no")).unwrap_err(), "no");
        let mut r = Response::ok("ui-t", e.target);
        r.result = Some(json!({"apps": [{"target": "not valid", "nodes": []}]}));
        assert!(render(&r).is_err());
    }
}
