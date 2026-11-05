//! `ui_look`, the Agent Hub's read-only tool: what the model may see of the mini app screens
//! open in the Bot (`app/src/agent_hub/`).
//!
//! The screens are the Bot's, and the broker can't reach them by itself: a shipped Bot opens
//! no port. So the Bot keeps a connection here, `GET /v1/host/ui/stream`: a look goes down it
//! as a `buwiz-agent` envelope and the answer comes back on `POST /v1/host/ui/reply`, matched
//! by id. A channel of its own, because `/v1/host/events` is a broadcast and drops on lag.
//!
//! The Bot filters a screen before it answers (O1): the broker never holds what the model may
//! not read. The tool's switch, `screens`, starts off for every profile (`policy::OFF_BY_DEFAULT`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use buwiz_agent::{ActorId, Caller, Envelope, Op, Response, Target, UiNode, UiTree};
use bytes::Bytes;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::Out;
use crate::hub::ctx::HubCtx;
use crate::js;

pub const NAME: &str = "ui_look";

/// How long the Bot has to answer: it reads the tree on its main thread.
const WAIT: Duration = Duration::from_secs(10);
const PING: Duration = Duration::from_secs(25);

/// The model's definition.
pub fn def() -> Value {
    json!({
        "name": NAME,
        "description": "Look at the mini app screens open in the Bot: each screen's name, which one is on top, and its buttons and fields. It only reads. Page text and field values are hidden from you: a button with no name is a row of data. Use it when the user asks about what is on their screen.",
        "input_schema": {"type": "object", "properties": {}},
    })
}

/// The Bot's connection and the looks waiting for an answer.
#[derive(Default)]
pub struct UiLink {
    bot: Mutex<Option<mpsc::Sender<Bytes>>>,
    waiting: Mutex<HashMap<String, oneshot::Sender<Response>>>,
    next: AtomicU64,
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

    #[test]
    fn a_refusal_and_a_bad_answer_are_errors() {
        let e = request("ui-t".into()).unwrap();
        assert_eq!(render(&Response::err("ui-t", e.target.clone(), "no")).unwrap_err(), "no");
        let mut r = Response::ok("ui-t", e.target);
        r.result = Some(json!({"apps": [{"target": "not valid", "nodes": []}]}));
        assert!(render(&r).is_err());
    }
}
