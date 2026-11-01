//! Rust ports of the module-level routine tests, by ID, on a manual
//! clock, a stub turn runner (the JS tests' recorded `runTurn`), the
//! in-memory store and fake plugin and MCP hosts standing in for the
//! fixture plugin, the google-workspace plugin over its fake upstream, and
//! mcp-agent's apps server:
//! RTN-01…16 (v1-routines.test.mjs; RTN-06 and RTN-14 test the tool
//! loop's surface filter, 5.4, and are checked here only as far as the
//! surface the runner hands it), INS-01…05 (v1-instances.test.mjs),
//! routine-threads.test.mjs (threads.rs), mcp-agent.test.mjs's v0 check
//! (MAGT-03), and the v0 HTTP cases (routines-v0.test.mjs RV0-*) at module
//! level.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::header::{HeaderMap, HeaderValue};
use hyper::Method;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::host::{MemoryHost, RoutineHost};
use super::mcp::{Client, McpView, Server};
use super::plugins::PluginView;
use super::v0::V0;
use super::v1::V1;
use super::{mount, Routines};
use crate::helper::plugins::{Clock, Row, RuntimeView};
use crate::hub::ctx::HubCtx;
use crate::hub::traits::{TurnDone, TurnRequest, TurnRunner};
use crate::hub::types::{BoxFuture, HubRequest};
use crate::hub::Hub;
use crate::v1::trust::Trust;
use crate::v1::V1Error;

const FX: &str = "app.buwiz.fixture";
const GWS: &str = "app.buwiz.google-workspace";
const START: f64 = 1_800_000_000_000.0;

// ── the fixture and google-workspace plugins ─────────────────────────────

fn tool(name: &str, ro: bool) -> Value {
    if ro {
        json!({"name": name, "description": name, "annotations": {"readOnlyHint": true}})
    } else {
        json!({"name": name, "description": name})
    }
}

struct Fake {
    rows: Mutex<Vec<Row>>,
    stopped: Mutex<Vec<String>>,
    mail: Mutex<Vec<Value>>,
    /// (plugin, tool, args, meta)
    calls: Mutex<Vec<(String, String, Value, Option<Value>)>>,
}

impl Fake {
    fn new(fx_routines: Value) -> Arc<Fake> {
        let row = |id: &str, short: &str, title: &str, routines: Value, tools: Vec<Value>| Row {
            id: id.into(),
            short: short.into(),
            command: "node".into(),
            args: "[]".into(),
            env: "{}".into(),
            version: "1.0.0".into(),
            protocol: "1.0.0".into(),
            publisher_key_id: "k1".into(),
            plugin_pub: "pub".into(),
            manifest: json!({"id": id, "short": short, "title": title, "publisher": {"id": "app.buwiz"}, "routines": routines}).to_string(),
            manifest_sha256: "0".repeat(64),
            tools: json!(tools).to_string(),
            enabled: true,
            disabled_tools: "[]".into(),
            created_at: 1,
            package: None,
            skills: "[]".into(),
        };
        let gws = row(GWS, "gws", "Google Workspace", json!([]), vec![tool("gws_search_mail", true), tool("gws_read_mail", true)]);
        let fx = row(FX, "fx", "Fixture", fx_routines, vec![tool("fx_echo", false), tool("fx_search", true), tool("fx_query", false)]);
        Arc::new(Fake { rows: Mutex::new(vec![gws, fx]), stopped: Mutex::new(Vec::new()), mail: Mutex::new(Vec::new()), calls: Mutex::new(Vec::new()) })
    }

    fn mail(&self, m: Value) {
        *self.mail.lock().unwrap() = m.as_array().cloned().unwrap();
    }

    fn calls_to(&self, tool: &str) -> Vec<Value> {
        self.calls.lock().unwrap().iter().filter(|c| c.1 == tool).map(|c| c.2.clone()).collect()
    }

    fn set_tool(&self, id: &str, tool: &str, off: bool) {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.iter_mut().find(|r| r.id == id).unwrap();
        let mut list: Vec<String> = serde_json::from_str(&row.disabled_tools).unwrap();
        list.retain(|t| t != tool);
        if off {
            list.push(tool.into());
        }
        row.disabled_tools = json!(list).to_string();
    }

    fn set_enabled(&self, id: &str, on: bool) {
        self.rows.lock().unwrap().iter_mut().find(|r| r.id == id).unwrap().enabled = on;
        let mut stopped = self.stopped.lock().unwrap();
        stopped.retain(|s| s != id);
        if !on {
            stopped.push(id.into());
        }
    }

    fn edit_routines(&self, f: impl FnOnce(&mut Value)) {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.iter_mut().find(|r| r.id == FX).unwrap();
        let mut m: Value = serde_json::from_str(&row.manifest).unwrap();
        f(&mut m["routines"]);
        row.manifest = m.to_string();
    }

    fn uninstall(&self, id: &str) {
        self.rows.lock().unwrap().retain(|r| r.id != id);
    }
}

fn ok(v: Value) -> Value {
    json!({"content": [{"type": "text", "text": v.to_string()}], "structuredContent": v})
}

impl PluginView for Arc<Fake> {
    fn row(&self, id: &str) -> Option<Row> {
        self.rows.lock().unwrap().iter().find(|r| r.id == id).cloned()
    }
    fn rows(&self) -> Vec<Row> {
        self.rows.lock().unwrap().clone()
    }
    fn runtime(&self, id: &str) -> RuntimeView {
        let installed = self.row(id).is_some();
        let running = installed && !self.stopped.lock().unwrap().iter().any(|s| s == id);
        let tools = self.row(id).map(|r| serde_json::from_str::<Vec<Value>>(&r.tools).unwrap()).unwrap_or_default();
        RuntimeView { state: if running { "running" } else { "stopped" }, conn: running, tools: if running { tools } else { Vec::new() }, generation: 1, ..RuntimeView::default() }
    }
    fn trust(&self) -> Trust {
        Trust { approved: vec![(FX.into(), "k1".into()), (GWS.into(), "k1".into())], ..Trust::default() }
    }
    fn call_tool(&self, id: &str, tool: &str, args: Value, meta: Option<Value>) -> BoxFuture<'static, Result<Value, V1Error>> {
        self.calls.lock().unwrap().push((id.into(), tool.into(), args.clone(), meta));
        let out = if self.runtime(id).state != "running" {
            Err(V1Error::new("plugin_unavailable", format!("{id} is not running")))
        } else {
            Ok(match tool {
                "fx_query" => ok(json!({"query": args.get("q").and_then(Value::as_str).unwrap_or("from:fixture")})),
                "fx_search" => ok(json!({"items": [{"id": args.get("query").and_then(Value::as_str).filter(|q| !q.is_empty()).unwrap_or("all")}]})),
                "gws_search_mail" => {
                    let items: Vec<Value> = self
                        .mail
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|m| {
                            let mut o = serde_json::Map::new();
                            for k in ["id", "from", "subject", "date"] {
                                if let Some(v) = m.get(k) {
                                    o.insert(k.into(), v.clone());
                                }
                            }
                            if let Some(b) = m.get("body") {
                                o.insert("snippet".into(), b.clone());
                            }
                            Value::Object(o)
                        })
                        .collect();
                    ok(json!({"items": items}))
                }
                _ => ok(json!({"tool": tool, "args": args})),
            })
        };
        Box::pin(async move { out })
    }
    fn events(&self) -> Option<broadcast::Receiver<Value>> {
        None
    }
}

// ── the recorded agent turn ──────────────────────────────────────────────

struct Stub {
    host: Arc<MemoryHost>,
    seen: Mutex<Vec<TurnRequest>>,
    fail: AtomicBool,
    n: AtomicU64,
}

impl TurnRunner for Stub {
    fn run(&self, _: Arc<HubCtx>, t: TurnRequest) -> BoxFuture<'static, Result<TurnDone, String>> {
        if self.fail.load(Ordering::SeqCst) {
            return Box::pin(async { Err("the model is unreachable".to_owned()) });
        }
        let turn = format!("turn-{}", self.n.fetch_add(2, Ordering::SeqCst));
        let mut s = self.host.lock();
        let th = s.list.iter_mut().find(|x| x.id == t.turn.thread).expect("the runner's thread");
        let seq = th.turns.len();
        let mut user = json!({"id": turn, "role": "user", "text": t.shown});
        if let Some(m) = t.mark.as_ref().and_then(|m| m.get("routine")) {
            user["routine"] = m.clone();
        }
        th.turns.push(user);
        th.turns.push(json!({"id": format!("{turn}r"), "role": "ai", "text": "Recorded."}));
        drop(s);
        self.seen.lock().unwrap().push(t);
        Box::pin(async move { Ok(TurnDone { seq, turn }) })
    }
}

impl Stub {
    fn all(&self) -> Vec<TurnRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

/// The instance a recorded turn ran for, and its item's id (read back from
/// the prompt: the runner gets no item of its own).
fn instance_of(t: &TurnRequest) -> String {
    t.mark.as_ref().map(|m| m["routine"]["instance"].as_str().unwrap().to_owned()).unwrap_or_default()
}

fn item_of(t: &TurnRequest) -> String {
    let at = t.prompt.find("\"id\": \"").expect("an item") + 7;
    t.prompt[at..].split('"').next().unwrap().to_owned()
}

// ── a world ──────────────────────────────────────────────────────────────

struct World {
    ctx: Arc<HubCtx>,
    host: Arc<MemoryHost>,
    fake: Arc<Fake>,
    stub: Arc<Stub>,
    v1: Arc<V1>,
    dir: PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp(label: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("rox-routines-{label}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(dir.join("generated")).unwrap();
    dir
}

fn context(dir: &std::path::Path, stub: Option<Arc<Stub>>) -> HubCtx {
    let mut ctx = HubCtx::new(dir.to_path_buf(), Clock::manual());
    ctx.clock.manual_step(Some(START), false);
    if let Some(s) = stub {
        ctx.turns = Some(s);
    }
    ctx
}

impl World {
    /// `before(db)` runs on app.db before the runner is made.
    fn new(label: &str, routines: Value, chats: &[(&str, &str, &str)], before: impl FnOnce(&HubCtx, &MemoryHost)) -> World {
        let dir = temp(label);
        let host = Arc::new(MemoryHost::new(0));
        for (id, uid, title) in chats {
            host.add_chat(id, uid, title);
        }
        let stub = Arc::new(Stub { host: host.clone(), seen: Mutex::new(Vec::new()), fail: AtomicBool::new(false), n: AtomicU64::new(1) });
        let ctx = Arc::new(context(&dir, Some(stub.clone())));
        before(&ctx, &host);
        let fake = Fake::new(routines);
        let v1 = V1::new(&ctx, Arc::new(fake.clone()), host.clone());
        World { ctx, host, fake, stub, v1, dir }
    }

    fn step(&self, s: f64) {
        let now = self.ctx.clock.now();
        self.ctx.clock.manual_step(Some(now + s * 1000.0), false);
    }

    fn now(&self) -> f64 {
        self.ctx.clock.now()
    }

    fn listed(&self, id: &str) -> Value {
        let list = self.v1.list(&self.ctx.db);
        let p = list.iter().find(|p| p["id"] == FX).unwrap().clone();
        p["routines"].as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone()
    }

    fn home(&self, routine: &str) -> Option<super::host::MemThread> {
        self.host.lock().list.iter().find(|t| t.routine.as_ref().is_some_and(|r| r["id"] == routine)).cloned()
    }

    fn home_of(&self, instance: &str) -> Option<super::host::MemThread> {
        self.host.lock().list.iter().find(|t| t.routine.as_ref().is_some_and(|r| r["instance"] == instance)).cloned()
    }

    fn notices(&self, chat: &str) -> Vec<Value> {
        self.host.turns(chat).into_iter().filter(|t| t["role"] == "notice").collect()
    }

    fn inst(&self, routine: &str) -> String {
        self.ctx.db.with(|db| db.query_row("SELECT id FROM v1_instances WHERE plugin=? AND routine=?", [FX, routine], |r| r.get::<_, String>(0))).unwrap()
    }

    fn col(&self, id: &str, col: &str) -> Value {
        let sql = format!("SELECT {col} FROM v1_instances WHERE id=?");
        self.ctx
            .db
            .with(|db| {
                db.query_row(&sql, [id], |r| {
                    Ok(match r.get_ref(0)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(i) => json!(i),
                        rusqlite::types::ValueRef::Real(f) => json!(f),
                        rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        rusqlite::types::ValueRef::Blob(_) => Value::Null,
                    })
                })
            })
            .unwrap_or(Value::Null)
    }

    async fn idle(&self, chat: &str, instance: &str) {
        for _ in 0..400 {
            let running = self.v1.of_chats(&self.ctx.db, &[chat.to_owned()]).iter().any(|x| x["instance"] == instance && x["running"] == true);
            if !running {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("{instance} kept running");
    }
}

fn code<T: std::fmt::Debug>(r: Result<T, V1Error>) -> String {
    match r {
        Ok(_) => "resolved".to_owned(),
        Err(e) => e.code,
    }
}

// ── RTN (v1-routines.test.mjs) ───────────────────────────────────────────

fn rtn_routines() -> Value {
    let reads = json!([{"plugin": GWS, "tools": ["gws_search_mail", "gws_read_mail"]}]);
    json!([
        {"id": "bir-mail", "name": "BIR mail", "description": "Files BIR confirmations.", "every_s": 300, "query": {"tool": "fx_query", "field": "query", "args": {"q": "from:bir.gov.ph"}}, "prompt": "Record each BIR confirmation.", "reads": reads},
        {"id": "bad-reads", "name": "Bad reads", "description": "Reads a tool that writes.", "every_s": 300, "query": {"tool": "fx_query", "field": "query"}, "prompt": "Echo.", "reads": [{"plugin": FX, "tools": ["fx_echo"]}]},
        {"id": "empty", "name": "Empty", "description": "Has nothing to search for.", "every_s": 300, "query": {"tool": "fx_query", "field": "query", "args": {"q": ""}}, "prompt": "Nothing.", "reads": reads},
    ])
}

fn mail1() -> Value {
    json!({"id": "m1", "from": "BIR eFPS <efps@bir.gov.ph>", "to": "owner@example.com", "subject": "Tax Return Receipt Confirmation", "date": "Mon, 26 Oct 2026 09:12:00 +0800", "body": "1701Q received. Reference 2026-ABC-123."})
}

fn mail2() -> Value {
    json!({"id": "m2", "from": "BIR eFPS <efps@bir.gov.ph>", "subject": "Tax Return Receipt Confirmation", "date": "Tue, 27 Oct 2026 10:00:00 +0800", "body": "2550Q received."})
}

fn b(v: Value) -> Value {
    let mut out = json!({"plugin": FX});
    for (k, x) in v.as_object().unwrap() {
        out[k] = x.clone();
    }
    out
}

/// The JS file's cases in its order (each builds on the last).
#[tokio::test]
async fn rtn_v1_routines_in_order() {
    let w = World::new("rtn", rtn_routines(), &[("chat-1", "uid-chat-1", "Piccolo")], |_, _| {});
    w.fake.mail(json!([mail1()]));
    let (ctx, v1) = (&w.ctx, &w.v1);
    let searches = || w.fake.calls_to("gws_search_mail");

    // RTN-01 an enabled routine the user has not approved does not run
    v1.enable(ctx, &b(json!({"routine": "bir-mail", "thread": "chat-1"}))).unwrap();
    let r = w.listed("bir-mail");
    assert_eq!((r["enabled"].clone(), r["approved"].clone(), r["available"].clone()), (json!(true), json!(false), json!(false)), "RTN-01");
    assert!(r["reason"].as_str().unwrap().contains("not approved"), "RTN-01");
    let home = w.home("bir-mail").expect("RTN-01: its thread exists before any run");
    assert_eq!(home.title, "BIR mail");
    assert!(home.turns.is_empty());
    assert_eq!((r["home"].clone(), r["home_uid"].clone()), (json!(home.id), json!(home.uid)), "RTN-01");
    assert_eq!(r["reads_status"][0]["allowed"], true, "RTN-01: the reads themselves are allowed");
    v1.run_due(ctx).await;
    assert_eq!(w.stub.count(), 0, "RTN-01");
    assert!(w.fake.calls_to("fx_query").is_empty(), "RTN-01: the query tool was not called");
    assert!(searches().is_empty(), "RTN-01: nothing was searched");

    // RTN-02 a tick: own query with origin routine, the reads search with
    // its text, one prompt per new item
    let a = v1.approve(ctx, &b(json!({"routine": "bir-mail"}))).unwrap();
    assert_eq!((a["routine"]["approved"].clone(), a["routine"]["available"].clone()), (json!(true), json!(true)), "RTN-02");
    w.step(300.0);
    v1.run_due(ctx).await;
    let q = w.fake.calls.lock().unwrap().iter().find(|c| c.1 == "fx_query").cloned().unwrap();
    assert_eq!(w.fake.calls_to("fx_query").len(), 1, "RTN-02");
    let caller = &q.3.unwrap()["buwiz/caller"];
    assert_eq!((caller["origin"].clone(), caller["plugin"].clone(), caller["routine"].clone()), (json!("routine"), json!(FX), json!("bir-mail")), "RTN-02 8.4: origin routine");
    assert_eq!(searches().iter().map(|a| a["query"].clone()).collect::<Vec<_>>(), [json!("from:bir.gov.ph")], "RTN-02: the search got the query string");
    assert_eq!(w.stub.count(), 1, "RTN-02: exactly one prompt run");
    let t = w.stub.all()[0].clone();
    assert_eq!(t.turn.thread, home.id, "RTN-02: the routine's own thread");
    assert_eq!(t.turn.routine.as_deref(), Some("app.buwiz.fixture/bir-mail"));
    assert_eq!(t.shown, "New item · Tax Return Receipt Confirmation");
    assert!(t.prompt.starts_with("(This routine runs on its own every 300s"), "{}", t.prompt);
    assert!(t.prompt.contains("## Routine: BIR mail (approved plugin app.buwiz.fixture)\nRecord each BIR confirmation."), "RTN-02 10.2 label");
    assert!(t.prompt.contains("New item from app.buwiz.google-workspace gws_search_mail:\n{\n \"id\": \"m1\",\n \"from\": \"BIR eFPS <efps@bir.gov.ph>\",\n \"subject\": \"Tax Return Receipt Confirmation\",\n \"date\": \"Mon, 26 Oct 2026 09:12:00 +0800\",\n \"snippet\": \"1701Q received. Reference 2026-ABC-123.\"\n}"), "RTN-02: {}", t.prompt);
    let reads = json!([{"plugin": GWS, "tools": ["gws_search_mail", "gws_read_mail"]}]);
    assert_eq!(t.turn.surface, Some(json!({"plugin": FX, "routine": "bir-mail", "instance": w.inst("bir-mail"), "params": {}, "reads": reads, "writes": []})), "RTN-02");
    let r = w.listed("bir-mail");
    assert_eq!((r["seen_count"].clone(), r["last_error"].clone(), r["last_items"].clone()), (json!(1), Value::Null, json!(1)), "RTN-02");

    // RTN-03 a seen item does not run again; a new one runs once; nothing
    // runs before it is due
    w.step(300.0);
    v1.run_due(ctx).await;
    assert_eq!((searches().len(), w.stub.count()), (2, 1), "RTN-03: m1 was seen");
    w.fake.mail(json!([mail2(), mail1()]));
    w.step(299.0);
    v1.run_due(ctx).await;
    assert_eq!(searches().len(), 2, "RTN-03: not due yet");
    w.step(1.0);
    v1.run_due(ctx).await;
    assert_eq!(w.stub.count(), 2, "RTN-03");
    assert_eq!(item_of(&w.stub.all()[1]), "m2");
    // the seen ids survive a new runner on the same db
    let again = V1::new(ctx, Arc::new(w.fake.clone()), w.host.clone());
    assert_eq!(again.tick_routine(ctx, FX, "bir-mail").await, json!({"ran": true, "items": 2, "prompts": 0}), "RTN-03");

    // RTN-04 a reads tool without readOnlyHint makes the routine unavailable
    v1.enable(ctx, &b(json!({"routine": "bad-reads", "thread": "chat-1"}))).unwrap();
    v1.approve(ctx, &b(json!({"routine": "bad-reads"}))).unwrap();
    let r = w.listed("bad-reads");
    assert_eq!((r["approved"].clone(), r["available"].clone()), (json!(true), json!(false)), "RTN-04");
    assert!(r["reason"].as_str().unwrap().contains("read-only"), "RTN-04");
    let before = w.stub.count();
    let res = v1.tick_routine(ctx, FX, "bad-reads").await;
    assert_eq!(res["ran"], false, "RTN-04");
    assert!(res["reason"].as_str().unwrap().contains("fx_echo"), "RTN-04");
    assert!(w.fake.calls_to("fx_echo").is_empty(), "RTN-04: fx_echo was never called");
    assert_eq!(w.stub.count(), before);
    v1.disable(ctx, &b(json!({"routine": "bad-reads"}))).unwrap();

    // RTN-05 an empty search text is nothing to do; a switched-off tool or
    // a stopped reads plugin makes it unavailable
    v1.enable(ctx, &b(json!({"routine": "empty", "thread": "chat-1"}))).unwrap();
    v1.approve(ctx, &b(json!({"routine": "empty"}))).unwrap();
    let n = searches().len();
    assert_eq!(v1.tick_routine(ctx, FX, "empty").await, json!({"ran": true, "items": 0, "prompts": 0}), "RTN-05");
    assert_eq!(searches().len(), n, "RTN-05: no search for an empty text");
    assert!(w.home("empty").unwrap().turns.is_empty(), "RTN-05: nothing new adds no turn");
    assert_eq!(w.listed("empty")["last_items"], 0);
    v1.disable(ctx, &b(json!({"routine": "empty"}))).unwrap();
    w.fake.set_tool(GWS, "gws_search_mail", true);
    assert!(v1.tick_routine(ctx, FX, "bir-mail").await["reason"].as_str().unwrap().contains("gws_search_mail is switched off"), "RTN-05");
    w.fake.set_tool(GWS, "gws_search_mail", false);
    w.fake.set_tool(FX, "fx_query", true);
    assert!(v1.tick_routine(ctx, FX, "bir-mail").await["reason"].as_str().unwrap().contains("fx_query is switched off"), "RTN-05");
    w.fake.set_tool(FX, "fx_query", false);
    w.fake.set_enabled(GWS, false);
    assert!(w.listed("bir-mail")["reason"].as_str().unwrap().contains("google-workspace is disabled"), "RTN-05");
    assert_eq!(v1.tick_routine(ctx, FX, "bir-mail").await["ran"], false);
    w.fake.set_enabled(GWS, true);
    assert_eq!(v1.tick_routine(ctx, FX, "bir-mail").await["ran"], true, "RTN-05");

    // RTN-13 an enabled routine's thread is made at the server's start or
    // its next tick, empty
    let notes = w.notices("chat-1").len();
    let empty_id = w.home("empty").unwrap().id;
    w.host.remove(&empty_id);
    ctx.db.with(|db| db.execute("UPDATE v1_instances SET enabled=1 WHERE plugin=? AND routine=?", [FX, "empty"])).unwrap();
    let again = V1::new(ctx, Arc::new(w.fake.clone()), w.host.clone());
    again.start(ctx);
    again.stop(ctx);
    let made = w.home("empty").expect("RTN-13: made at start");
    assert_eq!(made.routine, Some(json!({"plugin": FX, "id": "empty", "instance": w.inst("empty"), "name": "Empty", "thread": "chat-1"})), "RTN-13");
    assert_eq!(made.title, "Empty");
    assert!(made.turns.is_empty());
    w.host.remove(&made.id);
    assert_eq!(v1.tick_routine(ctx, FX, "empty").await, json!({"ran": true, "items": 0, "prompts": 0}), "RTN-13");
    let made = w.home("empty").expect("RTN-13: made by the tick");
    assert!(made.turns.is_empty(), "RTN-13: nothing new adds no turn");
    assert_eq!(w.listed("empty")["home"], json!(made.id));
    v1.tick_routine(ctx, FX, "empty").await;
    v1.enable(ctx, &b(json!({"routine": "empty", "thread": "chat-1"}))).unwrap();
    assert_eq!(w.host.lock().list.iter().filter(|t| t.routine.as_ref().is_some_and(|r| r["id"] == "empty")).count(), 1, "RTN-13: made once");
    assert_eq!(w.home("empty").unwrap().id, made.id);
    assert_eq!(w.notices("chat-1").len(), notes, "RTN-13: the chat it reports to got no notice");
    v1.disable(ctx, &b(json!({"routine": "empty"}))).unwrap();

    // RTN-06 (the tool loop's filter, 5.4): the surface the turn runs
    // under names the plugin and exactly the routine's reads
    let s = w.stub.all()[0].turn.surface.clone().unwrap();
    assert_eq!((s["plugin"].clone(), s["reads"].clone()), (json!(FX), reads.clone()), "RTN-06");
    // RTN-14 (5.4): the run's surface carries the writes the tool loop allows
    assert_eq!(s["writes"], json!([]), "RTN-14");

    // RTN-07 an update that changes what a routine reads needs a new approval
    w.fake.edit_routines(|rs| rs[0]["reads"] = json!([{"plugin": GWS, "tools": ["gws_search_mail"]}]));
    let r = w.listed("bir-mail");
    assert_eq!(r["approved"], false, "RTN-07");
    assert!(r["reason"].as_str().unwrap().contains("changed in an update"), "RTN-07");
    assert_eq!(v1.tick_routine(ctx, FX, "bir-mail").await["ran"], false);
    assert_eq!(v1.approve(ctx, &b(json!({"routine": "bir-mail"}))).unwrap()["routine"]["available"], true, "RTN-07");

    // RTN-10 a run's turns go to the routine's own thread, made on its
    // first run and reused after
    let h = w.home("bir-mail").unwrap();
    assert_eq!(w.host.lock().list.iter().filter(|t| t.routine.as_ref().is_some_and(|r| r["id"] == "bir-mail")).count(), 1, "RTN-10: one thread");
    assert_eq!(h.title, "BIR mail");
    assert_eq!(h.routine, Some(json!({"plugin": FX, "id": "bir-mail", "instance": w.inst("bir-mail"), "name": "BIR mail", "thread": "chat-1"})), "RTN-10");
    assert_eq!(w.stub.all().iter().map(|t| t.turn.thread.clone()).collect::<Vec<_>>(), [h.id.clone(), h.id.clone()], "RTN-10: the second run reused it");
    let shown: Vec<Value> = h.turns.iter().filter(|t| t["role"] == "user").cloned().collect();
    assert_eq!(shown.iter().map(|t| t["routine"]["name"].clone()).collect::<Vec<_>>(), [json!("BIR mail"), json!("BIR mail")], "RTN-10");
    assert_ne!(shown[0]["routine"]["run"], shown[1]["routine"]["run"], "RTN-10: one run id per run");
    assert!(w.host.turns("chat-1").iter().all(|t| t["role"] == "notice"), "RTN-10: the chat holds no item turn");

    // RTN-11 each run with new items leaves one notice pointing at the run
    let ns = w.notices("chat-1");
    assert_eq!(ns.len(), 2, "RTN-11");
    assert_eq!(ns[0]["text"], "BIR mail ran · 1 new item");
    assert_eq!(
        ns[0]["notice"],
        json!({"kind": "routine", "plugin": FX, "routine": "bir-mail", "instance": w.inst("bir-mail"), "name": "BIR mail", "thread": h.id, "thread_uid": h.uid, "run": h.turns[0]["routine"]["run"], "seq": 0, "turn": h.turns[0]["id"], "items": 1}),
        "RTN-11"
    );
    assert_eq!(crate::js::stringify(&ns[0]["notice"]).find("\"kind\""), Some(1), "RTN-11: the notice's key order");
    assert_eq!(ns[1]["notice"]["seq"], 2, "RTN-11: after the first's two turns");
    assert_eq!(ns[1]["notice"]["turn"], h.turns[2]["id"]);
    assert!(ns[1]["notice"]["run"].as_str().unwrap().strip_prefix("run_").is_some_and(|d| d.bytes().all(|c| c.is_ascii_digit())));

    // RTN-12 a failure leaves one notice per failure streak
    w.fake.mail(json!([{"id": "m3", "subject": "Payment confirmation", "body": "Paid."}, mail2(), mail1()]));
    w.stub.fail.store(true, Ordering::SeqCst);
    let n = w.notices("chat-1").len();
    let r1 = v1.tick_routine(ctx, FX, "bir-mail").await;
    assert!(r1["error"].as_str().unwrap().contains("model is unreachable"), "RTN-12");
    let f = w.notices("chat-1").pop().unwrap();
    assert_eq!(w.notices("chat-1").len(), n + 1);
    assert_eq!(f["text"], "BIR mail failed: the model is unreachable", "RTN-12");
    assert_eq!((f["notice"]["error"].clone(), f["notice"]["items"].clone()), (json!("the model is unreachable"), json!(0)));
    assert_eq!(f["notice"]["thread"], json!(w.home("bir-mail").unwrap().id), "RTN-12: the thread from earlier runs");
    w.step(3600.0);
    v1.tick_routine(ctx, FX, "bir-mail").await;
    v1.tick_routine(ctx, FX, "bir-mail").await;
    assert_eq!(w.notices("chat-1").len(), n + 1, "RTN-12: no notice for the same streak");
    w.stub.fail.store(false, Ordering::SeqCst);
    v1.tick_routine(ctx, FX, "bir-mail").await;
    assert_eq!(w.notices("chat-1").pop().unwrap()["text"], "BIR mail ran · 1 new item", "RTN-12: it recovered");
    w.stub.fail.store(true, Ordering::SeqCst);
    w.fake.mail(json!([{"id": "m4", "subject": "Another", "body": "x"}]));
    v1.tick_routine(ctx, FX, "bir-mail").await;
    assert_eq!(w.notices("chat-1").len(), n + 3, "RTN-12: a new streak notices again");
    w.stub.fail.store(false, Ordering::SeqCst);

    // RTN-15 Run now ticks once out of schedule and counts like a run;
    // paused, running or unavailable is refused
    let before = w.listed("bir-mail");
    let n = w.stub.count();
    ctx.db.with(|db| db.execute("UPDATE v1_routines SET errors=5 WHERE plugin=? AND routine=?", [FX, "bir-mail"])).unwrap();
    v1.run_due(ctx).await;
    assert_eq!(w.stub.count(), n, "RTN-15: the schedule doesn't run it");
    w.step(1.0);
    w.fake.mail(json!([{"id": "m5", "subject": "Run now mail", "body": "x"}]));
    let r = v1.run(ctx, &b(json!({"routine": "bir-mail", "wait": true}))).await.unwrap();
    assert_eq!(r["run"], json!({"ran": true, "items": 1, "prompts": 1}), "RTN-15");
    assert_eq!(w.stub.count(), n + 1);
    assert_eq!(r["routine"]["last_run"], super::text::num(w.now()), "RTN-15: it counts as the last run");
    assert!(r["routine"]["last_run"].as_f64() > before["last_run"].as_f64());
    assert_eq!((r["routine"]["last_items"].clone(), r["routine"]["running"].clone()), (json!(1), json!(false)));
    assert_eq!(w.notices("chat-1").pop().unwrap()["text"], "BIR mail ran · 1 new item");
    assert_eq!(v1.run(ctx, &b(json!({"routine": "bir-mail", "wait": true}))).await.unwrap()["run"], json!({"ran": true, "items": 1, "prompts": 0}));
    assert_eq!(v1.run(ctx, &b(json!({"routine": "bir-mail", "wait": true, "reread": true}))).await.unwrap()["run"], json!({"ran": true, "items": 1, "prompts": 1}), "RTN-15: reread");
    let first = v1.run(ctx, &b(json!({"routine": "bir-mail"}))).await.unwrap();
    assert_eq!(first["routine"]["running"], true, "RTN-15: without wait it answers at once");
    assert_eq!(code(v1.run(ctx, &b(json!({"routine": "bir-mail"}))).await), "rate_limited");
    w.idle("chat-1", &w.inst("bir-mail")).await;
    assert_eq!(code(v1.run(ctx, &b(json!({"routine": "empty"}))).await), "disabled", "RTN-15: paused");
    w.fake.set_tool(FX, "fx_query", true);
    let last = w.listed("bir-mail")["last_run"].clone();
    w.step(5.0);
    assert_eq!(code(v1.run(ctx, &b(json!({"routine": "bir-mail"}))).await), "plugin_unavailable", "RTN-15");
    assert_eq!(w.listed("bir-mail")["last_run"], last, "RTN-15: the refusal is no run");
    w.fake.set_tool(FX, "fx_query", false);
    assert_eq!(code(v1.run(ctx, &b(json!({"routine": "nope"}))).await), "not_found");

    // RTN-16 the agent lists and runs the plugin routines of its chats
    // only, and cannot enable one
    let mine = v1.of_chats(&ctx.db, &["chat-1".into()]);
    let mut pairs: Vec<(String, bool)> = mine.iter().map(|x| (x["id"].as_str().unwrap().to_owned(), x["enabled"] == true)).collect();
    pairs.sort();
    assert_eq!(pairs, [("bad-reads".to_owned(), false), ("bir-mail".to_owned(), true), ("empty".to_owned(), false)], "RTN-16");
    let x = mine.iter().find(|m| m["id"] == "bir-mail").unwrap();
    assert_eq!((x["plugin"].clone(), x["name"].clone(), x["every_s"].clone(), x["home"].clone()), (json!(FX), json!("BIR mail"), json!(300), json!(w.home("bir-mail").unwrap().id)));
    assert!(v1.of_chats(&ctx.db, &["chat-2".into()]).is_empty(), "RTN-16: another chat's bot sees none");
    w.fake.mail(json!([{"id": "m6", "subject": "Agent run", "body": "x"}]));
    let ran = v1.agent_run(ctx, &format!("{FX}/bir-mail"), &["chat-1".into()], std::time::Duration::from_secs(60)).await;
    assert!(!ran.is_error, "{:?}", ran.content);
    let said = ran.content.as_str().unwrap();
    assert!(said.starts_with("BIR mail ran: 1 item found, 1 new (each ran in its thread t") && said.ends_with(')'), "RTN-16: {said}");
    assert!(v1.agent_run(ctx, &format!("{FX}/bir-mail"), &["chat-2".into()], std::time::Duration::from_secs(60)).await.is_error, "RTN-16: not its chat");
    let paused = v1.agent_run(ctx, &format!("{FX}/empty"), &["chat-1".into()], std::time::Duration::from_secs(60)).await;
    assert!(paused.is_error && paused.content.as_str().unwrap().contains("Empty is paused; resume it first"), "RTN-16");
    assert_eq!(w.listed("empty")["enabled"], false, "RTN-16: running it did not enable it");

    // RTN-08 bad requests are refused; a deleted chat stops the routine;
    // uninstall forgets it
    assert_eq!(code(v1.approve(ctx, &b(json!({"routine": "nope"})))), "not_found", "RTN-08");
    assert_eq!(code(v1.enable(ctx, &b(json!({"routine": "bir-mail", "thread": "no-such-chat"})))), "invalid_args");
    assert_eq!(code(v1.approve(ctx, &json!({"plugin": FX}))), "invalid_args");
    w.host.remove("chat-1");
    assert_eq!(v1.tick_routine(ctx, FX, "bir-mail").await["reason"], "its chat was deleted", "RTN-08");
    assert_eq!(w.listed("bir-mail")["enabled"], false);
    w.fake.uninstall(FX);
    V1::forget(&ctx.db, FX);
    let n: i64 = ctx.db.with(|db| db.query_row("SELECT COUNT(*) FROM v1_routines WHERE plugin=?", [FX], |r| r.get(0))).unwrap();
    assert_eq!(n, 0, "RTN-08");
}

// ── INS (v1-instances.test.mjs) ──────────────────────────────────────────

fn ins_routines() -> Value {
    let reads = json!([{"plugin": GWS, "tools": ["gws_search_mail", "gws_read_mail"]}]);
    let params = json!({"type": "object", "properties": {"q": {"type": "string", "maxLength": 200}, "forms": {"type": "array", "items": {"type": "string"}, "maxItems": 4}}, "required": ["q"]});
    let mut loose = params.clone();
    loose["required"] = json!([]);
    json!([
        {"id": "bir-mail", "name": "BIR mail", "description": "Files BIR confirmations.", "every_s": 300, "query": {"tool": "fx_query", "field": "query"}, "prompt": "Record each BIR confirmation.", "reads": reads, "params": loose},
        {"id": "payments", "name": "Payments", "description": "Records payments.", "every_s": 300, "query": {"tool": "fx_query", "field": "query"}, "prompt": "Record each payment.", "reads": reads, "writes": ["fx_echo"], "params": params},
        {"id": "hooked", "name": "Hooked", "description": "Runs on a webhook.", "every_s": 300, "query": {"tool": "fx_query", "field": "query"}, "prompt": "Handle it.", "reads": reads},
    ])
}

#[tokio::test]
async fn ins_v1_instances_in_order() {
    let reads = json!([{"plugin": GWS, "tools": ["gws_search_mail", "gws_read_mail"]}]);
    let old_run = START - 1000.0;
    let w = World::new("ins", ins_routines(), &[("chat-1", "uid-chat-1", "Piccolo"), ("chat-2", "uid-chat-2", "Vegeta")], |ctx, host| {
        // the thread of a routine from before instances
        let mut s = host.lock();
        s.list.push(super::host::MemThread {
            id: "t50".into(),
            uid: "uid-t50".into(),
            title: "BIR mail".into(),
            routine: Some(json!({"plugin": FX, "id": "bir-mail", "name": "BIR mail", "thread": "chat-1"})),
            turns: vec![json!({"id": "old", "role": "user", "text": "New item"})],
        });
        drop(s);
        // a routine enabled and run before instances existed
        ctx.db
            .with(|db| {
                db.execute_batch(
                    "CREATE TABLE v1_routines(plugin TEXT NOT NULL, routine TEXT NOT NULL, thread TEXT, enabled INTEGER NOT NULL DEFAULT 0,
                     approved_reads TEXT, seen TEXT NOT NULL DEFAULT '[]', last_run INTEGER, last_error TEXT, errors INTEGER NOT NULL DEFAULT 0, last_items INTEGER, PRIMARY KEY(plugin, routine))",
                )?;
                db.execute("INSERT INTO v1_routines VALUES(?,?,?,?,?,?,?,?,?,?)", rusqlite::params![FX, "bir-mail", "chat-1", 1, reads.to_string(), "[\"m1\"]", old_run as i64, None::<String>, 0, 1])
            })
            .unwrap();
    });
    w.fake.mail(json!([{"id": "m1", "from": "BIR eFPS <efps@bir.gov.ph>", "subject": "Tax Return Receipt Confirmation", "date": "Mon, 26 Oct 2026 09:12:00 +0800", "body": "1701Q received."}]));
    let (ctx, v1) = (&w.ctx, &w.v1);
    let rows = |routine: &str| -> Vec<String> {
        ctx.db
            .with(|db| {
                let mut st = db.prepare("SELECT id FROM v1_instances WHERE routine=? ORDER BY created_at, rowid")?;
                let r = st.query_map([routine], |r| r.get::<_, String>(0))?;
                r.collect()
            })
            .unwrap()
    };

    // INS-01 a routine enabled before instances becomes one instance with
    // its chat, seen ids, last run and thread
    let ids = rows("bir-mail");
    assert_eq!(ids.len(), 1, "INS-01");
    let x = ids[0].clone();
    assert!(x.starts_with("ri_"));
    let cols: Vec<Value> = ["thread", "enabled", "seen", "last_run", "last_items", "trigger", "params"].iter().map(|c| w.col(&x, c)).collect();
    assert_eq!(cols, [json!("chat-1"), json!(1), json!("[\"m1\"]"), json!(old_run as i64), json!(1), json!("schedule"), json!("{}")], "INS-01");
    assert_eq!(w.host.thread("t50").unwrap().routine.unwrap()["instance"], json!(x), "INS-01: its thread is the instance's");
    let _second = V1::new(ctx, Arc::new(w.fake.clone()), w.host.clone());
    assert_eq!(rows("bir-mail").len(), 1, "INS-01: once");
    let listed = w.listed("bir-mail");
    assert_eq!(listed["instances"].as_array().unwrap().len(), 1);
    assert_eq!((listed["instances"][0]["home"].clone(), listed["home"].clone(), listed["seen_count"].clone()), (json!("t50"), json!("t50"), json!(1)), "INS-01");

    // INS-03 params go to the query tool, the prompt and the surface; ones
    // that don't fit are refused
    let e = v1.enable(ctx, &b(json!({"routine": "bir-mail", "thread": "chat-2", "params": {"q": "from:taxpayer-a", "forms": ["2550Q"]}, "label": "Taxpayer A"}))).unwrap();
    let ei = e["instance"]["instance"].as_str().unwrap().to_owned();
    assert_ne!(ei, x, "INS-03: a new instance in chat-2; chat-1's stays");
    assert_eq!(e["instance"]["summary"], "Taxpayer A");
    assert_eq!(e["instance"]["params"], json!({"q": "from:taxpayer-a", "forms": ["2550Q"]}));
    let r = v1.run(ctx, &b(json!({"instance": ei, "routine": "bir-mail", "wait": true}))).await.unwrap();
    assert_eq!(r["run"], json!({"ran": true, "items": 1, "prompts": 1}), "INS-03");
    assert_eq!(w.fake.calls_to("fx_query").last().unwrap()["q"], "from:taxpayer-a", "INS-03: the query tool got params.q");
    let t = w.stub.all().pop().unwrap();
    assert!(t.prompt.contains("runs with these params; keep to them:\n{\"q\":\"from:taxpayer-a\",\"forms\":[\"2550Q\"]}"), "INS-03: {}", t.prompt);
    assert_eq!(t.turn.surface.as_ref().unwrap()["params"], json!({"q": "from:taxpayer-a", "forms": ["2550Q"]}));
    assert_eq!(t.turn.surface.as_ref().unwrap()["instance"], json!(ei));
    assert_eq!(t.turn.thread, w.home_of(&ei).unwrap().id);
    assert_eq!(w.home_of(&ei).unwrap().title, "BIR mail · Taxpayer A", "INS-03");
    for params in [json!({"nope": 1}), json!({"q": 5}), json!({"forms": "2550Q"}), json!({"forms": ["a", "b", "c", "d", "e"]})] {
        assert_eq!(code(v1.update(ctx, &b(json!({"routine": "bir-mail", "instance": ei, "params": params.clone()})))), "invalid_args", "INS-03 {params}");
    }
    assert_eq!(code(v1.enable(ctx, &b(json!({"routine": "payments", "thread": "chat-1"})))), "invalid_args", "INS-03: payments needs q");

    // INS-02 two instances of one routine keep separate seen ids, threads
    // and notices
    let both = rows("bir-mail");
    let (a, c) = (both[0].clone(), both[1].clone());
    assert_eq!((w.col(&a, "thread"), w.col(&c, "thread")), (json!("chat-1"), json!("chat-2")));
    w.fake.mail(
        json!([{"id": "m2", "from": "BIR eFPS <efps@bir.gov.ph>", "subject": "Payment Confirmation", "date": "Tue, 27 Oct 2026 10:00:00 +0800", "body": "2550Q paid."}, {"id": "m1", "subject": "x"}]),
    );
    let n = w.stub.count();
    w.step(600.0);
    v1.run_due(ctx).await;
    assert_eq!(w.stub.count(), n + 2, "INS-02");
    let last2: Vec<(String, String)> = w.stub.all()[n..].iter().map(|t| (instance_of(t), item_of(t))).collect();
    assert_eq!(last2, [(a.clone(), "m2".to_owned()), (c.clone(), "m2".to_owned())], "INS-02");
    assert_ne!(w.home_of(&a).unwrap().id, w.home_of(&c).unwrap().id, "INS-02: a thread each");
    assert_eq!((w.col(&a, "seen"), w.col(&c, "seen")), (json!("[\"m1\",\"m2\"]"), json!("[\"m1\",\"m2\"]")));
    assert_eq!(w.notices("chat-1").pop().unwrap()["notice"]["instance"], json!(a));
    assert_eq!(w.notices("chat-2").pop().unwrap()["notice"]["instance"], json!(c));
    v1.disable(ctx, &b(json!({"routine": "bir-mail", "instance": a}))).unwrap();
    w.fake.mail(json!([{"id": "m3", "subject": "Third"}]));
    w.step(600.0);
    v1.run_due(ctx).await;
    assert_eq!(instance_of(&w.stub.all().pop().unwrap()), c, "INS-02: pausing one leaves the other running");
    assert_eq!(w.col(&a, "seen"), json!("[\"m1\",\"m2\"]"), "INS-02: the paused one saw nothing");
    assert_eq!(code(v1.run(ctx, &b(json!({"routine": "bir-mail"}))).await), "invalid_args", "INS-02: ambiguous");

    // INS-04 the agent's instance of a routine that changes data waits
    // behind a consent card
    let chat1 = ["chat-1".to_owned()];
    let before = w.notices("chat-1").len();
    let made = v1.agent_create(ctx, &b(json!({"routine": "payments", "params": {"q": "from:payments"}, "label": "Taxpayer B"})), &chat1);
    let said = made.content.as_str().unwrap().to_owned();
    assert!(!made.is_error, "{said}");
    assert!(said.contains("waits for the user: a card in this chat asks them to Enable it (it may change Fixture data: fx_echo)"), "INS-04: {said}");
    let id = said.split_whitespace().nth(1).unwrap().to_owned();
    assert!(id.starts_with("ri_"));
    assert_eq!((w.col(&id, "enabled"), w.col(&id, "pending")), (json!(0), json!(1)), "INS-04");
    let card = w.notices("chat-1").pop().unwrap();
    assert_eq!(w.notices("chat-1").len(), before + 1);
    assert_eq!(card["notice"]["kind"], "routine_consent");
    assert_eq!(
        (card["notice"]["instance"].clone(), card["notice"]["state"].clone(), card["notice"]["writes"].clone(), card["notice"]["summary"].clone()),
        (json!(id), json!("pending"), json!(["fx_echo"]), json!("Taxpayer B"))
    );
    assert_eq!(card["text"], "Enable Payments (Taxpayer B) here? It may change Fixture data without asking: fx_echo. It reads Google Workspace: gws_search_mail, gws_read_mail.", "INS-04");
    let n = w.stub.count();
    w.step(600.0);
    v1.run_due(ctx).await;
    assert_eq!(w.stub.all().iter().filter(|t| instance_of(t) == id).count(), 0, "INS-04: it doesn't run");
    assert_eq!(w.stub.count(), n);
    assert!(v1.agent_run(ctx, &id, &chat1, std::time::Duration::from_secs(60)).await.content.as_str().unwrap().contains("waits for the user's Enable"));
    let tried = v1.agent_update(ctx, &id, &json!({"enabled": true}), &chat1);
    assert!(tried.content.as_str().unwrap().contains("waits for the user"), "INS-04: the agent can't switch it on");
    assert_eq!(w.col(&id, "enabled"), json!(0));
    assert_eq!(w.notices("chat-1").len(), before + 2, "INS-04: it asked again");
    let on = v1.enable(ctx, &b(json!({"routine": "payments", "instance": id, "approve": true}))).unwrap();
    assert_eq!((on["instance"]["enabled"].clone(), on["instance"]["pending"].clone()), (json!(true), json!(false)), "INS-04: the user's Enable");
    assert!(w.notices("chat-1").iter().filter(|t| t["notice"]["instance"] == json!(id)).all(|t| t["notice"]["state"] == "enabled"), "INS-04: its cards read as enabled");
    let changed = v1.agent_update(ctx, &id, &json!({"params": {"q": "from:other"}}), &chat1);
    assert!(changed.content.as_str().unwrap().contains("waits for the user"), "INS-04: new params ask again");
    assert_eq!((w.col(&id, "enabled"), w.col(&id, "pending")), (json!(0), json!(1)));
    assert_eq!(serde_json::from_str::<Value>(w.col(&id, "params").as_str().unwrap()).unwrap()["q"], "from:other");
    v1.enable(ctx, &b(json!({"routine": "payments", "instance": id}))).unwrap();
    assert!(v1.agent_update(ctx, &id, &json!({"label": "Taxpayer B2"}), &chat1).content.as_str().unwrap().ends_with("· on"), "INS-04: a label alone needs no consent");
    let plain = v1.agent_create(ctx, &b(json!({"routine": "bir-mail", "params": {"q": "from:c"}})), &chat1);
    let said = plain.content.as_str().unwrap();
    assert!(said.starts_with("made and enabled ri_") && said.ends_with(" — BIR mail · q: from:c · every 300s"), "INS-04: {said}");
    assert!(v1.agent_delete(ctx, &id, &["chat-2".to_owned()]).is_error, "INS-04: not its chat");
    assert!(v1.agent_delete(ctx, &id, &chat1).content.as_str().unwrap().starts_with("deleted"));
    assert_eq!(w.col(&id, "id"), Value::Null);

    // INS-05 a webhook instance runs on its secret URL only, with the
    // items it is handed, rate limited
    let e = v1.enable(ctx, &b(json!({"routine": "hooked", "thread": "chat-1", "trigger": "webhook", "approve": true}))).unwrap();
    let id = e["instance"]["instance"].as_str().unwrap().to_owned();
    assert_eq!((e["instance"]["trigger"].clone(), e["instance"]["hook"].clone()), (json!("webhook"), json!(false)), "INS-05");
    assert_eq!(code(v1.hook_call(ctx, &id, "guess", &json!({}))), "not_found", "INS-05: no secret yet");
    let path = v1.hook(ctx, &b(json!({"routine": "hooked", "instance": id}))).unwrap()["path"].as_str().unwrap().to_owned();
    let parts: Vec<&str> = path.split('/').collect();
    let secret = parts[4].to_owned();
    assert_eq!(parts[3], id);
    assert!(secret.len() == 32 && secret.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'), "INS-05");
    let hash = w.col(&id, "hook_hash").as_str().unwrap().to_owned();
    assert!(hash.len() == 64 && !hash.contains(&secret), "INS-05: only its hash is kept");
    assert_eq!(code(v1.hook_call(ctx, &id, &format!("{secret}x"), &json!({}))), "not_found", "INS-05: a wrong secret");
    assert_eq!(code(v1.hook_call(ctx, "ri_nosuchinstance", &secret, &json!({}))), "not_found");
    let queries = w.fake.calls_to("gws_search_mail").len();
    let res = v1.hook_call(ctx, &id, &secret, &json!({"items": [{"id": "h1", "subject": "Hooked one"}, {"id": "h1"}]})).unwrap();
    assert_eq!(res, json!({"ok": true, "instance": id, "items": 2}), "INS-05");
    w.idle("chat-1", &id).await;
    assert_eq!(w.fake.calls_to("gws_search_mail").len(), queries, "INS-05: no search");
    let hooked: Vec<String> = w.stub.all().iter().filter(|t| instance_of(t) == id).map(item_of).collect();
    assert_eq!(hooked, ["h1"], "INS-05");
    assert!(w.stub.all().pop().unwrap().prompt.contains("on its webhook"));
    assert_eq!(code(v1.hook_call(ctx, &id, &secret, &json!({"items": [{"nope": 1}]}))), "invalid_args");
    w.step(3600.0);
    v1.run_due(ctx).await;
    assert_eq!(w.stub.all().iter().filter(|t| instance_of(t) == id).count(), 1, "INS-05: the schedule leaves it alone");
    let mut limited = None;
    for i in 0..=super::v1::HOOK_RATE {
        w.idle("chat-1", &id).await;
        if code(v1.hook_call(ctx, &id, &secret, &json!({"items": []}))) == "rate_limited" {
            limited = Some(i);
            break;
        }
    }
    assert_eq!(limited, Some(super::v1::HOOK_RATE), "INS-05: the call after HOOK_RATE in a minute");
    w.step(61.0);
    w.idle("chat-1", &id).await;
    let next = v1.hook(ctx, &b(json!({"routine": "hooked", "instance": id}))).unwrap()["path"].as_str().unwrap().to_owned();
    assert_eq!(code(v1.hook_call(ctx, &id, &secret, &json!({}))), "not_found", "INS-05: a new secret retires the old");
    assert_eq!(code(v1.hook_call(ctx, &id, next.split('/').nth(4).unwrap(), &json!({}))), "resolved");
    let sched = rows("bir-mail")[0].clone();
    assert_eq!(code(v1.hook(ctx, &b(json!({"routine": "bir-mail", "instance": sched})))), "invalid_args", "INS-05: a schedule instance has no webhook");
}

// ── the host routes in process (RTN-09, the token, the status map) ──────

fn req(method: Method, url: &str, token: Option<&str>, body: Option<Value>) -> HubRequest {
    let mut headers = HeaderMap::new();
    if let Some(t) = token {
        headers.insert("x-buwiz-host", HeaderValue::from_str(t).unwrap());
    }
    if body.is_some() {
        headers.insert("content-type", HeaderValue::from_static("application/json"));
    }
    HubRequest { method, url: url.to_owned(), headers, body: body.map(|b| Bytes::from(b.to_string())).unwrap_or_default(), params: Vec::new() }
}

async fn text(res: crate::hub::types::HubResponse) -> (u16, String) {
    let status = res.status().as_u16();
    (status, String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned())
}

#[tokio::test]
async fn rtn_09_and_the_host_routes_token_refusals_and_status_map() {
    let dir = temp("host");
    let host = Arc::new(MemoryHost::new(0));
    host.add_chat("chat-1", "uid-chat-1", "Piccolo");
    let routines = json!([{"id": "fx-digest", "name": "Fixture digest", "description": "d", "every_s": 300, "query": {"tool": "fx_search", "field": "query"}, "prompt": "p", "reads": [{"plugin": FX, "tools": ["fx_search"]}]}]);
    let fake = Fake::new(routines);
    let ctx = context(&dir, None);
    let (h, f) = (host.clone(), fake.clone());
    let hub = Hub::with(ctx, vec![], move |routes, ctx| {
        let rs = Routines::new(ctx, h, Arc::new(f), Arc::new(super::mcp::NoMcp), false);
        mount(routes, ctx, rs);
    });
    hub.start();
    let token = std::fs::read_to_string(dir.join("host-token")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(dir.join("host-token")).unwrap().permissions().mode() & 0o777, 0o600, "host-token is 0600");
    assert!(token.len() == 43 && token.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'));
    let tok = Some(token.as_str());
    let refusal = |code: &str, message: &str, retryable: bool| json!({"ok": false, "code": code, "message": message, "retryable": retryable}).to_string();

    // RTH-01: 401, 405, 404, bodies
    assert_eq!(text(hub.answer(req(Method::GET, "/v1/host/plugins", None, None)).await).await, (401, refusal("unauthorized", "X-Buwiz-Host is missing or wrong", false)));
    assert_eq!(text(hub.answer(req(Method::GET, "/v1/host/plugins", Some(&"x".repeat(43)), None)).await).await.0, 401);
    assert_eq!(text(hub.answer(req(Method::GET, "/v1/host/call", tok, None)).await).await, (405, refusal("not_supported", "GET /v1/host/call", false)));
    assert_eq!(text(hub.answer(req(Method::PUT, "/v1/host/plugins", tok, None)).await).await, (405, refusal("not_supported", "PUT /v1/host/plugins", false)));
    assert_eq!(text(hub.answer(req(Method::POST, "/v1/host/nope", tok, Some(json!({})))).await).await, (404, refusal("not_found", "no route /v1/host/nope", false)));
    assert_eq!(text(hub.answer(req(Method::GET, "/v1/host/plugins?x=1", tok, None)).await).await.0, 200);
    for raw in ["[]", "1", "null", "{", "\"x\""] {
        let mut r = req(Method::POST, "/v1/host/routines/approve", tok, Some(json!({})));
        r.body = Bytes::from(raw);
        assert_eq!(text(hub.answer(r).await).await, (400, refusal("invalid_args", "the body must be a JSON object (at most 2 MiB)", false)), "{raw}");
    }
    let mut big = req(Method::POST, "/v1/host/routines/approve", tok, Some(json!({})));
    big.body = Bytes::from(json!({"pad": "x".repeat(2 * 1024 * 1024)}).to_string());
    assert_eq!(text(hub.answer(big).await).await.0, 400);
    let mut empty = req(Method::POST, "/v1/host/routines/approve", tok, Some(json!({})));
    empty.body = Bytes::new();
    assert_eq!(text(hub.answer(empty).await).await, (400, refusal("invalid_args", "send {plugin, routine}", false)));
    assert_eq!(text(hub.answer(req(Method::POST, "/v1/host/agents/resume", tok, Some(json!({})))).await).await.0, 501, "track E's");
    assert_eq!(
        text(hub.answer(req(Method::POST, "/v1/host/routines/approve", tok, Some(json!({"plugin": FX, "routine": "nope"})))).await).await,
        (404, refusal("not_found", &format!("{FX} has no routine nope"), false))
    );

    // RTN-09 approve stores the approval; the plugin list shows it
    assert_eq!(text(hub.answer(req(Method::POST, "/v1/host/routines/approve", None, Some(json!({"plugin": FX, "routine": "fx-digest"})))).await).await.0, 401, "RTN-09: host token");
    let digest = || async {
        let (_, t) = text(hub.answer(req(Method::GET, "/v1/host/plugins", tok, None)).await).await;
        let v: Value = serde_json::from_str(&t).unwrap();
        v["plugins"].as_array().unwrap().iter().find(|p| p["id"] == FX).unwrap()["routines"][0].clone()
    };
    let was = digest().await;
    assert_eq!((was["approved"].clone(), was["enabled"].clone(), was["available"].clone()), (json!(false), json!(false), json!(false)), "RTN-09");
    let (s, t) = text(hub.answer(req(Method::POST, "/v1/host/routines/approve", tok, Some(json!({"plugin": FX, "routine": "fx-digest"})))).await).await;
    assert_eq!(s, 200);
    assert_eq!(serde_json::from_str::<Value>(&t).unwrap()["routine"]["approved"], true);
    let now = digest().await;
    assert_eq!((now["approved"].clone(), now["available"].clone()), (json!(true), json!(true)), "RTN-09");
    assert_eq!(now["reads_status"][0]["allowed"], true);
    let (s, t) = text(hub.answer(req(Method::POST, "/v1/host/routines/enable", tok, Some(json!({"plugin": FX, "routine": "fx-digest", "thread": "no-such-chat"})))).await).await;
    assert_eq!((s, serde_json::from_str::<Value>(&t).unwrap()["code"].clone()), (400, json!("invalid_args")));
    let (s, t) = text(hub.answer(req(Method::POST, "/v1/host/routines/run", tok, Some(json!({"plugin": FX, "routine": "fx-digest"})))).await).await;
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!((s, v["code"].clone()), (409, json!("disabled")), "RTN-09: Run now on a paused routine");
    assert!(v["message"].as_str().unwrap().contains("not enabled in any chat"));
    assert_eq!(digest().await["instances"], json!([]));
    let mut hook = req(Method::POST, "/v1/hooks/ri_nosuchinstance/secret", None, Some(json!({})));
    hook.params = Vec::new();
    let (s, t) = text(hub.answer(hook).await).await;
    assert_eq!((s, serde_json::from_str::<Value>(&t).unwrap()["code"].clone()), (404, json!("not_found")), "RTN-09: a webhook takes no host token");

    // the plugin list's shape, byte for byte (RTH-02's), on the fake
    let (_, t) = text(hub.answer(req(Method::GET, "/v1/host/plugins", tok, None)).await).await;
    let fx = serde_json::from_str::<Value>(&t).unwrap()["plugins"][1].clone();
    let keys: Vec<&str> = fx.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "id",
            "short",
            "title",
            "version",
            "protocol",
            "publisher",
            "state",
            "code",
            "message",
            "enabled",
            "generation",
            "plugin_pub",
            "first_party",
            "manifest_sha256",
            "apps",
            "commands",
            "scopes",
            "routines",
            "tools"
        ]
    );
    let rkeys: Vec<&str> = fx["routines"][0].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        rkeys,
        [
            "id",
            "name",
            "description",
            "every_s",
            "query",
            "prompt",
            "reads",
            "reads_status",
            "enabled",
            "thread",
            "home",
            "home_uid",
            "approved",
            "available",
            "reason",
            "seen_count",
            "last_run",
            "last_error",
            "last_items",
            "next_run",
            "running",
            "instances"
        ]
    );
    assert_eq!(fx["tools"][1], json!({"name": "fx_search", "description": "fx_search", "disabled": false, "readOnly": true}));
    assert_eq!(fx["first_party"], true);

    // a restart writes a new token; the old one is refused (RTH-14)
    hub.ctx().clock.manual_step(None, false);
    let rs_token = token.clone();
    let hub2 = {
        let (h, f) = (host.clone(), fake.clone());
        Hub::with(context(&dir, None), vec![], move |routes, ctx| {
            let rs = Routines::new(ctx, h, Arc::new(f), Arc::new(super::mcp::NoMcp), false);
            mount(routes, ctx, rs);
        })
    };
    hub2.start();
    let fresh = std::fs::read_to_string(dir.join("host-token")).unwrap();
    assert_ne!(fresh, rs_token, "RTH-14: a new token");
    assert_eq!(text(hub2.answer(req(Method::GET, "/v1/host/plugins", Some(&rs_token), None)).await).await.0, 401, "RTH-14: the old one is refused");
    assert_eq!(text(hub2.answer(req(Method::GET, "/v1/host/plugins", Some(&fresh), None)).await).await.0, 200);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── v0 (routines.mjs; mcp-agent.test.mjs; routines-v0.test.mjs) ─────────

/// A stdio MCP server's answers: tools and their results, a manifest.
struct FakeMcp {
    servers: Mutex<Vec<Server>>,
    tools: Mutex<HashMap<String, Vec<Value>>>,
    /// tool name → its result (raw tools/call)
    results: Mutex<HashMap<String, Value>>,
    manifests: Mutex<HashMap<String, Value>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl FakeMcp {
    fn new() -> Arc<FakeMcp> {
        Arc::new(FakeMcp {
            servers: Mutex::new(Vec::new()),
            tools: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            manifests: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn add(&self, id: &str, tools: Vec<Value>, manifest: Value) {
        self.servers.lock().unwrap().push(Server { id: id.into(), name: id.into(), env: "{}".into(), account: None });
        self.tools.lock().unwrap().insert(id.into(), tools);
        self.manifests.lock().unwrap().insert(id.into(), manifest);
    }

    fn answer(&self, tool: &str, result: Value) {
        self.results.lock().unwrap().insert(tool.into(), result);
    }
}

impl McpView for Arc<FakeMcp> {
    fn servers(&self) -> Vec<Server> {
        self.servers.lock().unwrap().clone()
    }
    fn ensure(&self, id: &str) -> BoxFuture<'static, Client> {
        let t = self.tools.lock().unwrap().get(id).cloned().ok_or_else(|| "no server".to_owned());
        Box::pin(async move { t })
    }
    fn call(&self, _: &str, tool: &str, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        self.calls.lock().unwrap().push((tool.into(), args.clone()));
        let r = self.results.lock().unwrap().get(tool).cloned().unwrap_or_else(|| json!({"isError": true, "content": [{"type": "text", "text": "no tool"}]}));
        Box::pin(async move { Ok(r) })
    }
    fn manifest(&self, id: &str) -> BoxFuture<'static, Value> {
        let m = self.manifests.lock().unwrap().get(id).cloned().unwrap_or(json!({}));
        Box::pin(async move { m })
    }
    fn status(&self, id: &str) -> Value {
        json!({"status": if self.tools.lock().unwrap().contains_key(id) { "connected" } else { "idle" }, "tools": 0})
    }
}

struct V0World {
    ctx: Arc<HubCtx>,
    host: Arc<MemoryHost>,
    mcp: Arc<FakeMcp>,
    stub: Arc<Stub>,
    v0: Arc<V0>,
    dir: PathBuf,
}

impl Drop for V0World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn v0_world(label: &str) -> V0World {
    let dir = temp(label);
    let host = Arc::new(MemoryHost::new(0));
    host.add_chat("t1", "uid-t1", "new chat");
    let stub = Arc::new(Stub { host: host.clone(), seen: Mutex::new(Vec::new()), fail: AtomicBool::new(false), n: AtomicU64::new(1) });
    let ctx = Arc::new(context(&dir, Some(stub.clone())));
    ctx.db.with(|db| db.execute_batch("CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT)")).unwrap();
    let mcp = FakeMcp::new();
    let v0 = V0::new(&ctx.db, host.clone(), Arc::new(mcp.clone()));
    V0World { ctx, host, mcp, stub, v0, dir }
}

fn text_result(t: &str) -> Value {
    json!({"content": [{"type": "text", "text": t}]})
}

/// mcp-agent.test.mjs "a routine's check calls the server; nothing new
/// means no turn" (MAGT-03).
#[tokio::test]
async fn magt_03_a_routines_check_calls_the_server_and_nothing_new_is_no_turn() {
    let w = v0_world("magt");
    w.mcp.add("apps", vec![json!({"name": "echo"})], json!({}));
    w.mcp.answer("echo", json!({"content": [{"type": "text", "text": "{\"tool\":\"echo\",\"args\":{\"n\":1}}"}]}));
    let r =
        w.v0.create(
            &w.ctx,
            &json!({"name": "Echo check", "thread": "t1", "every_s": 3600, "prompt": "Report new items.", "check": {"server": "apps", "tool": "echo", "args": {"n": 1}, "ids": "nothing[].id"}}),
            false,
            None,
        )
        .await
        .unwrap();
    w.v0.run(&w.ctx, Some(&r["id"])).await.unwrap();
    let row = V0::row_of(&w.ctx.db, r["id"].as_str().unwrap()).unwrap();
    assert_eq!(row.last_error, None);
    assert!(row.last_check.is_some() && row.last_check == row.last_run, "the check ran");
    assert_eq!(w.mcp.calls.lock().unwrap().clone(), [("echo".to_owned(), json!({"n": 1}))]);
    assert_eq!(w.stub.count(), 0, "no turn");
}

fn mail_text(ids: &[&str]) -> String {
    format!("Found {} messages:\n{}", ids.len(), ids.iter().map(|id| format!("Message ID: {id}\nSubject: BIR {id}\n")).collect::<String>())
}

fn mailfx(w: &V0World) {
    let gmail_schema =
        json!({"type": "object", "properties": {"query": {"type": "string"}, "page_size": {"type": "integer"}, "user_google_email": {"type": "string"}}, "required": ["query", "user_google_email"]});
    let manifest = json!({"title": "Mail fixture", "routines": [
        {"id": "bir-mail", "name": "BIR mail", "description": "Files BIR confirmations.", "every_s": 120, "needs": "gmail", "query": {"tool": "bir_query", "field": "query"}, "prompt": "Record each confirmation."},
        {"id": "daily", "name": "Daily note", "every_s": 30, "prompt": "Say good morning."},
        {"id": "cal", "name": "Calendar sync", "needs": "calendar", "prompt": "Sync."},
        {"id": "noquery", "name": "No query", "needs": "gmail", "prompt": "x"},
        {"id": "idle", "name": "Idle", "description": 5},
        {"id": "", "name": "dropped: empty id"},
        {"name": "dropped: no id"},
        "dropped: not an object"
    ]});
    w.mcp.add(
        "mailfx",
        vec![
            json!({"name": "bir_query", "inputSchema": {"type": "object"}}),
            json!({"name": "search_gmail_messages", "inputSchema": gmail_schema}),
            json!({"name": "get_gmail_messages_content_batch", "inputSchema": {"type": "object"}}),
        ],
        manifest,
    );
    w.mcp.answer("bir_query", json!({"content": [{"type": "text", "text": "{\"query\":\"from:bir.gov.ph\"}"}], "structuredContent": {"query": "from:bir.gov.ph"}}));
    w.mcp.answer("search_gmail_messages", text_result(&mail_text(&["m1", "m2"])));
}

/// RV0-01…09 at module level (the HTTP file needs the store): templates,
/// from-template refusals and its Gmail routine, ticks, update clamps and
/// the backoff.
#[tokio::test]
async fn rv0_templates_ticks_updates_and_the_backoff() {
    let w = v0_world("rv0");
    let (ctx, v0, host) = (&w.ctx, &w.v0, w.host.as_ref());
    // RV0-01 a fresh server: no routines, no templates
    assert_eq!(crate::js::stringify(&json!({"routines": v0.list(&ctx.db)})), "{\"routines\":[]}");
    assert!(super::templates::list(&ctx.db, host, &w.mcp.clone()).await.is_empty(), "RV0-01");

    // RV0-02 a connected server's templates: clamped, available or why
    // not, needs_email
    mailfx(&w);
    let t = super::templates::list(&ctx.db, host, &w.mcp.clone()).await;
    assert_eq!(
        crate::js::stringify(&json!(t)),
        crate::js::stringify(&json!([
            {"server": "mailfx", "id": "bir-mail", "name": "BIR mail", "description": "Files BIR confirmations.", "every_s": 120, "needs": "gmail", "available": true, "needs_email": true},
            {"server": "mailfx", "id": "daily", "name": "Daily note", "description": "", "every_s": 60, "needs": null, "available": true},
            {"server": "mailfx", "id": "cal", "name": "Calendar sync", "description": "", "every_s": 3600, "needs": "calendar", "available": false, "reason": "Buwiz Bot can't provide \"calendar\" yet"},
            {"server": "mailfx", "id": "noquery", "name": "No query", "description": "", "every_s": 3600, "needs": "gmail", "available": false, "reason": "The template names no query tool", "needs_email": true},
            {"server": "mailfx", "id": "idle", "name": "Idle", "description": "", "every_s": 3600, "needs": null, "available": false, "reason": "The template has nothing to do"},
        ])),
        "RV0-02"
    );

    // RV0-03 from-template refusals; the Gmail one asks for the address
    let refused = |j: Value| async move { v0.from_template(ctx, &j).await.unwrap_err() };
    assert_eq!(refused(json!({"server": "mailfx", "id": "daily", "thread": "t999"})).await.error, "unknown chat t999");
    assert_eq!(refused(json!({"server": "mailfx", "id": "daily"})).await.error, "unknown chat ");
    assert_eq!(refused(json!({"server": "nope", "id": "daily", "thread": "t1"})).await.error, "no connected server nope");
    assert_eq!(refused(json!({"server": "mailfx", "id": "zzz", "thread": "t1"})).await.error, "mailfx has no routine template zzz");
    assert_eq!(refused(json!({"server": "mailfx", "id": "cal", "thread": "t1"})).await.error, "Buwiz Bot can't provide \"calendar\" yet");
    assert_eq!(refused(json!({"server": "mailfx", "id": "bir-mail", "thread": "t1", "user_google_email": "bob"})).await.error, "bob isn't an email address");
    let need = refused(json!({"server": "mailfx", "id": "bir-mail", "thread": "t1"})).await;
    assert_eq!((need.error.as_str(), need.need), ("Buwiz Bot needs your Google address for Gmail searches", Some("user_google_email")), "RV0-03");
    assert!(v0.list(&ctx.db).is_empty(), "RV0-03: nothing was made");
    assert_eq!(super::templates::meta_get(&ctx.db, "google_email"), None, "RV0-03: a bad address is not kept");

    // RV0-04 a Gmail template becomes a routine with a check; the address
    // is kept; the same template again is the same routine
    let mail = v0.from_template(ctx, &json!({"server": "mailfx", "id": "bir-mail", "thread": "t1", "user_google_email": " owner@example.com "})).await.unwrap();
    let id = mail["id"].as_str().unwrap().to_owned();
    assert_eq!(&id[14..15], "7", "RV0-04: a UUID v7");
    let mut shown = mail.clone();
    for (k, v) in [("created_at", json!(0)), ("next_run", json!(0)), ("running", json!(false))] {
        shown[k] = v;
    }
    let prompt = "Each new item is a Gmail message from mailfx's search_gmail_messages. Read them with mcp_mailfx_get_gmail_messages_content_batch (user_google_email: owner@example.com).\n\nRecord each confirmation.";
    assert_eq!(
        crate::js::stringify(&shown),
        crate::js::stringify(&json!({
            "id": id, "name": "BIR mail", "thread": "t1", "threadTitle": "new chat", "botName": "", "every_s": 120, "enabled": true,
            "check": {"server": "mailfx", "tool": "search_gmail_messages", "args": {"page_size": 25, "user_google_email": "owner@example.com"}, "ids": "/Message ID: (\\S+)/",
                      "args_from": {"server": "mailfx", "tool": "bir_query", "args": {}, "field": "query", "into": "query"}},
            "prompt": prompt, "seen_count": 0, "last_run": null, "last_check": null, "last_error": null, "errors": 0, "next_run": 0, "running": false, "created_at": 0, "template": "mailfx/bir-mail",
        })),
        "RV0-04"
    );
    assert_eq!(mail["next_run"], 120_000, "RV0-04: never run, due at once");
    assert_eq!(super::templates::meta_get(&ctx.db, "google_email").as_deref(), Some("owner@example.com"));
    let t = super::templates::list(&ctx.db, host, &w.mcp.clone()).await;
    assert_eq!(t[0].get("needs_email"), None, "RV0-04: the address is known now");
    let again = v0.from_template(ctx, &json!({"server": "mailfx", "id": "bir-mail", "thread": "t1"})).await.unwrap();
    assert_eq!(again["id"], json!(id), "RV0-04: the routine it already has");

    // RV0-05 its first tick comes on its own: the timer, armed for an
    // overdue routine, fires; the check runs, then one turn
    assert!(w.ctx.clock.manual_step(None, true), "RV0-05: a timer was armed");
    for _ in 0..200 {
        if V0::row_of(&ctx.db, &id).unwrap().last_run.is_some() && !v0.is_running(&id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let done = v0.json(&V0::row_of(&ctx.db, &id).unwrap());
    assert_eq!((done["errors"].clone(), done["seen_count"].clone()), (json!(0), json!(2)), "RV0-05 {done}");
    assert_eq!(done["last_check"], done["last_run"]);
    assert_eq!(done["next_run"].as_f64().unwrap(), done["last_run"].as_f64().unwrap() + 120_000.0);
    let calls = w.mcp.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["bir_query", "search_gmail_messages"]);
    assert_eq!(crate::js::stringify(&calls[1].1), "{\"page_size\":25,\"user_google_email\":\"owner@example.com\",\"query\":\"from:bir.gov.ph\"}", "RV0-05");
    assert_eq!(w.stub.all()[0].shown, "Routine · BIR mail · 2 new");
    assert_eq!(w.stub.all()[0].turn.routine.as_deref(), Some(id.as_str()));
    assert_eq!(V0::row_of(&ctx.db, &id).unwrap().seen.as_deref(), Some("[\"m1\",\"m2\"]"));

    // RV0-06 Run now: nothing new is no turn; a new item is one turn
    let quiet = v0.run(ctx, Some(&json!(id))).await.unwrap();
    assert_eq!((quiet["seen_count"].clone(), quiet["running"].clone(), quiet["last_error"].clone()), (json!(2), json!(false), Value::Null), "RV0-06");
    assert_eq!(w.stub.count(), 1, "RV0-06: no new turn");
    w.mcp.answer("search_gmail_messages", text_result(&mail_text(&["m1", "m2", "m3"])));
    assert_eq!(v0.run(ctx, Some(&json!(id))).await.unwrap()["seen_count"], 3);
    assert_eq!(w.stub.all()[1].shown, "Routine · BIR mail · 1 new");
    w.mcp.answer("search_gmail_messages", text_result("No messages found."));
    let none = v0.run(ctx, Some(&json!(id))).await.unwrap();
    assert_eq!((none["errors"].clone(), none["last_error"].clone(), none["seen_count"].clone()), (json!(0), Value::Null, json!(3)));
    let q = w.mcp.results.lock().unwrap().remove("bir_query").unwrap();
    let failed = v0.run(ctx, Some(&json!(id))).await.unwrap();
    assert_eq!((failed["errors"].clone(), failed["last_error"].clone()), (json!(1), json!("bir_query: no tool")), "RV0-06");
    w.mcp.answer("bir_query", q);
    w.mcp.answer("search_gmail_messages", text_result(&mail_text(&["m1", "m2", "m3"])));
    let good = v0.run(ctx, Some(&json!(id))).await.unwrap();
    assert_eq!((good["errors"].clone(), good["last_error"].clone()), (json!(0), Value::Null), "RV0-06: a good run clears it");
    assert_eq!(w.stub.count(), 2);

    // RV0-07 a prompt-only template: its run posts the shown line in its chat
    let daily = v0.from_template(ctx, &json!({"server": "mailfx", "id": "daily", "thread": "t1"})).await.unwrap();
    assert_eq!((daily["check"].clone(), daily["prompt"].clone(), daily["every_s"].clone(), daily["template"].clone()), (Value::Null, json!("Say good morning."), json!(60), json!("mailfx/daily")));
    v0.run(ctx, Some(&daily["id"])).await.unwrap();
    let last = w.stub.all().pop().unwrap();
    assert_eq!((last.shown.as_str(), last.turn.thread.as_str()), ("Routine · Daily note", "t1"), "RV0-07");
    assert_eq!(
        last.prompt,
        "(Routine \"Daily note\" (".to_owned()
            + daily["id"].as_str().unwrap()
            + ") is running on its own, every 60s — the user isn't waiting on this message. Do what it asks and report briefly.)\n\nSay good morning."
    );
    assert_eq!(v0.list(&ctx.db).iter().map(|x| x["name"].clone()).collect::<Vec<_>>(), [json!("BIR mail"), json!("Daily note")], "RV0-07: oldest first");

    // RV0-08 update: on/off, clamped interval, prompt, name; refusals
    let up = |j: Value| {
        let mut j = j;
        j["id"] = daily["id"].clone();
        v0.update(ctx, &j).unwrap()
    };
    let off = up(json!({"enabled": false}));
    assert_eq!((off["enabled"].clone(), off["next_run"].clone()), (json!(false), Value::Null), "RV0-08");
    assert_eq!(up(json!({"every_s": 10}))["every_s"], 60);
    assert_eq!(up(json!({"every_s": 1e9}))["every_s"], 604800);
    assert_eq!(up(json!({"every_s": "abc"}))["every_s"], 60, "RV0-08: not a number is 0, clamped to 60");
    assert_eq!(up(json!({"every_s": 90.6}))["every_s"], 91);
    assert_eq!(up(json!({"prompt": "x".repeat(9000)}))["prompt"].as_str().unwrap().len(), 8000);
    assert_eq!(up(json!({"prompt": null}))["prompt"], "");
    assert_eq!(up(json!({"name": "  Renamed  "}))["name"], "Renamed");
    assert_eq!(up(json!({"name": "   "}))["name"], "Renamed", "RV0-08: a blank name is ignored");
    assert_eq!(up(json!({"name": "n".repeat(100)}))["name"].as_str().unwrap().len(), 80);
    assert_eq!(v0.update(ctx, &json!({"id": "nope", "enabled": true})).unwrap_err().error, "no routine nope");
    ctx.db.with(|db| db.execute("UPDATE routines SET last_run=NULL, errors=5 WHERE id=?", [daily["id"].as_str().unwrap()])).unwrap();
    let on = up(json!({"enabled": true}));
    assert_eq!((on["enabled"].clone(), on["errors"].clone(), on["next_run"].clone()), (json!(true), json!(0), json!(91_000)), "RV0-08: re-enabling clears the count");

    // RV0-09 three failures in a row back off: x2, x4 … x64, a day at most
    v0.update(ctx, &json!({"id": daily["id"], "enabled": false})).unwrap();
    let broken = v0.create(ctx, &json!({"name": "Broken", "thread": "t1", "every_s": 60, "prompt": "x"}), true, None).await.unwrap();
    let bid = broken["id"].as_str().unwrap().to_owned();
    ctx.db.with(|db| db.execute(r#"UPDATE routines SET "check"=? WHERE id=?"#, [r#"{"server":"gone","tool":"x","args":{},"ids":"items[].id"}"#, &bid])).unwrap();
    let step = || async { v0.run(ctx, Some(&json!(bid))).await.unwrap() };
    for n in [1, 2] {
        let r = step().await;
        assert_eq!((r["errors"].clone(), r["last_error"].clone(), r["enabled"].clone()), (json!(n), json!("gone isn't connected"), json!(true)));
        assert_eq!(r["next_run"].as_f64().unwrap() - r["last_run"].as_f64().unwrap(), 60_000.0, "RV0-09: no backoff before the third");
    }
    let gap = |r: &Value| r["next_run"].as_f64().unwrap() - r["last_run"].as_f64().unwrap();
    assert_eq!(gap(&step().await), 120_000.0, "RV0-09: x2 at the third");
    assert_eq!(gap(&step().await), 240_000.0, "RV0-09: x4 at the fourth");
    ctx.db.with(|db| db.execute("UPDATE routines SET errors=8 WHERE id=?", [&bid])).unwrap();
    assert_eq!(gap(&step().await), 60_000.0 * 64.0, "RV0-09: x64 at most");
    ctx.db.with(|db| db.execute("UPDATE routines SET errors=8, every_s=3600 WHERE id=?", [&bid])).unwrap();
    assert_eq!(gap(&step().await), 86_400_000.0, "RV0-09: a day at most");
    ctx.db.with(|db| db.execute("UPDATE routines SET errors=8, every_s=604800 WHERE id=?", [&bid])).unwrap();
    assert_eq!(gap(&step().await), 604_800_000.0, "RV0-09: an interval over a day is never stretched");
    let on = v0.update(ctx, &json!({"id": bid, "enabled": true})).unwrap();
    assert_eq!((on["errors"].clone(), gap(&on)), (json!(0), 604_800_000.0));

    // RV0-10 a deleted chat disables its routine on its next tick
    w.host.add_chat("t2", "uid-t2", "doomed");
    let orphan = v0.create(ctx, &json!({"name": "Orphan", "thread": "t2", "every_s": 60, "prompt": "hi"}), true, None).await.unwrap();
    w.host.remove("t2");
    let r = v0.run(ctx, Some(&orphan["id"])).await.unwrap();
    assert_eq!(
        (r["enabled"].clone(), r["last_error"].clone(), r["errors"].clone(), r["next_run"].clone(), r["threadTitle"].clone()),
        (json!(false), json!("its chat was deleted"), json!(0), Value::Null, json!("")),
        "RV0-10"
    );

    // RV0-12 delete; unknown ids
    v0.delete(ctx, Some(&orphan["id"])).unwrap();
    assert_eq!(v0.delete(ctx, Some(&orphan["id"])).unwrap_err().error, format!("no routine {}", orphan["id"].as_str().unwrap()));
    assert_eq!(v0.run(ctx, Some(&orphan["id"])).await.unwrap_err().error, format!("no routine {}", orphan["id"].as_str().unwrap()), "RV0-12");
}

/// RV0-13: at start an overdue enabled routine runs; a disabled one does
/// not. Seen ids keep the last 5000.
#[tokio::test]
async fn rv0_13_overdue_routines_tick_at_start_and_seen_ids_are_capped() {
    let w = v0_world("rv0-13");
    let ctx = &w.ctx;
    let due = w.v0.create(ctx, &json!({"name": "Due at start", "thread": "t1", "every_s": 60, "prompt": "x"}), true, None).await.unwrap();
    let off = w.v0.create(ctx, &json!({"name": "Off at start", "thread": "t1", "every_s": 60, "prompt": "x"}), false, None).await.unwrap();
    // a restart: a new runner on the same db, on a new clock
    let fresh = V0::new(&ctx.db, w.host.clone(), Arc::new(w.mcp.clone()));
    let ctx2 = Arc::new(context(&w.dir, Some(w.stub.clone())));
    ctx2.clock.manual_step(Some(START + 5_000.0), false);
    fresh.start(&ctx2);
    assert!(ctx2.clock.manual_step(None, true), "an overdue routine's timer");
    for _ in 0..200 {
        if V0::row_of(&ctx2.db, due["id"].as_str().unwrap()).unwrap().last_run.is_some() && !fresh.is_running(due["id"].as_str().unwrap()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(V0::row_of(&ctx2.db, due["id"].as_str().unwrap()).unwrap().last_run.is_some(), "RV0-13: it ran after the start");
    assert_eq!(V0::row_of(&ctx2.db, off["id"].as_str().unwrap()).unwrap().last_run, None, "RV0-13: the disabled one did not");
    assert_eq!(w.stub.all().iter().map(|t| t.shown.clone()).collect::<Vec<_>>(), ["Routine · Due at start"]);

    // seen ids: the last 5000
    w.mcp.add("big", vec![json!({"name": "list"})], json!({}));
    let ids: Vec<Value> = (0..5003).map(|i| json!({"id": format!("x{i}")})).collect();
    w.mcp.answer("list", json!({"content": [], "structuredContent": {"items": ids}}));
    let r = w.v0.create(ctx, &json!({"name": "Big", "thread": "t1", "every_s": 60, "check": {"server": "big", "tool": "list", "ids": "items[].id"}}), true, None).await.unwrap();
    let ran = w.v0.run(ctx, Some(&r["id"])).await.unwrap();
    assert_eq!(ran["seen_count"], 5000, "the seen ids are capped at 5000");
    let seen: Vec<String> = serde_json::from_str(V0::row_of(&ctx.db, r["id"].as_str().unwrap()).unwrap().seen.as_deref().unwrap()).unwrap();
    assert_eq!((seen[0].as_str(), seen[4999].as_str()), ("x3", "x5002"), "the newest kept");
}

#[test]
fn items_of_paths_regexes_and_hashes_as_routines_mjs() {
    use super::v0::items_of;
    let ids = |r: Value, p: &str| items_of(&r, p).unwrap().into_iter().map(|(k, _)| k).collect::<Vec<_>>();
    assert_eq!(ids(json!({"messages": [{"id": "a"}, {"id": 2}, {"x": 1}]}), "messages[].id").len(), 3);
    assert_eq!(ids(json!({"messages": [{"id": "a"}, {"id": 2}]}), "messages[].id"), ["a", "2"]);
    assert_eq!(ids(json!("Message ID: m1\nMessage ID: m2"), r"/Message ID: (\S+)/"), ["m1", "m2"]);
    assert_eq!(ids(json!({"threads": [{"threadId": "t9"}]}), r#"/"(?:id|threadId)"\s*:\s*"([^"]+)"/"#), ["t9"]);
    assert_eq!(ids(json!(["a", "b"]), ""), ["a", "b"], "the result itself is the list");
    let changes = ids(json!({"changes": [{"k": 1}, {"k": 1}, {"k": 2}]}), "changes[]");
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0], changes[1], "a whole item's hash");
    assert_eq!(changes[0].len(), 16);
    assert_eq!(items_of(&json!("not json"), "items[].id").unwrap_err(), "the check's result is text, not JSON — give ids as a /regex/");
    assert_eq!(ids(json!("{\"items\":[{\"id\":\"z\"}]}"), "items[].id"), ["z"], "a JSON text result is parsed");
}

#[test]
fn the_backoff_doubles_after_the_third_failure_and_is_capped() {
    use super::v0::{interval_ms, Row};
    let row = |every: f64, errors: f64| Row {
        id: "r".into(),
        name: "n".into(),
        thread: "t".into(),
        every_s: every,
        enabled: true,
        check: None,
        prompt: None,
        seen: None,
        last_run: None,
        last_check: None,
        last_error: None,
        errors: Some(errors),
        created_at: None,
        template: None,
    };
    let base = 60_000.0;
    assert_eq!([0.0, 1.0, 2.0, 3.0, 4.0, 8.0, 20.0].map(|e| interval_ms(&row(60.0, e)) / base), [1.0, 1.0, 1.0, 2.0, 4.0, 64.0, 64.0]);
    assert_eq!(interval_ms(&row(3600.0, 8.0)), 86_400_000.0, "min(base·2^k, max(base, day))");
    assert_eq!(interval_ms(&row(604_800.0, 8.0)), 604_800_000.0);
}
