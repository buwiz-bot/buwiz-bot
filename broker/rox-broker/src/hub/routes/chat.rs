//! The chat routes (5.2; routes/chat.mjs): threads, turns, `/chat` and
//! `/retry` (a job, run by `ctx.turns`), `/status`, reactions, copy and
//! share. [`install`] is track A's one line in `hub/mod.rs`: it loads the
//! store first (other tracks' starts read it), then registers the chat,
//! bot, group and chat-side MCP routes.
//!
//! `/thread/apps`' body is the Library's (5.7b): the [`LibraryOf`] seam in
//! `ctx.ext`; without it the route answers 501.
//!
//! `ctx.turns` defaults to [`MockTurns`], which the tool loop (5.4,
//! `agent::install`) replaces on the hub: a hub built without it (a test)
//! answers under MOCK what Node answers without a model or a pipeline
//! (`/upload` with no files, `/mock <word>` for an unknown word, a chat
//! reply when files ride the message, `backgroundTurn`'s mock reply);
//! every other `/chat` ends its job "generator error — not ported".

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use serde_json::{json, Map, Value};

use crate::helper::local::{command, exec, tools, MacTools};
use crate::hub::ctx::HubCtx;
use crate::hub::jobs::JobId;
use crate::hub::library::{botctx, groups};
use crate::hub::store::json::{has_len, js_number_str, js_slice, out, template, threads_json, turns_json, Lookups};
use crate::hub::store::Turn;
use crate::hub::traits::{ChatTurn, TurnDone, TurnRequest, TurnRunner};
use crate::hub::types::{not_ported, BoxFuture, HubRequest, HubResponse, Pattern::Exact as X, Pattern::Prefix as P, TurnCtx, Verb};
use crate::hub::Registry;
use crate::js;

// ————— answers as Node writes them —————

/// A body with or without `content-type: application/json` (Node sets it
/// on some answers only).
pub fn answer(status: u16, body: String, ct: bool) -> HubResponse {
    let mut res = Response::new(Full::new(Bytes::from(body)).map_err(|never| match never {}).boxed());
    *res.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if ct {
        res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    res
}

/// 200 JSON with the content type.
pub fn ok(v: &Value) -> HubResponse {
    answer(200, out(v), true)
}

/// `{ok: false, error}` (400), with or without the content type.
pub fn refused(e: &str, ct: bool) -> HubResponse {
    answer(400, out(&json!({"ok": false, "error": e})), ct)
}

/// Node's dispatcher answering a route that threw: 500 `{error}`.
pub fn thrown(e: &str) -> HubResponse {
    answer(500, out(&json!({"error": e})), false)
}

/// `JSON.parse(body || "{}")`, V8's error worded close to V8's (D8).
pub fn parse(body: &Bytes) -> Result<Value, String> {
    let text = String::from_utf8_lossy(body);
    let text = if text.is_empty() { "{}" } else { &text };
    serde_json::from_str::<Value>(text).map_err(|e| {
        let (line, col) = (e.line(), e.column());
        let pos: usize = text.split('\n').take(line.saturating_sub(1)).map(|l| l.encode_utf16().count() + 1).sum::<usize>() + col.saturating_sub(1);
        let msg = e.to_string();
        let msg = msg.split(" at line ").next().unwrap_or(&msg);
        format!("{msg} in JSON at position {pos} (line {line} column {col})")
    })
}

/// `MOCK ? "mock" : MODEL` (config.mjs).
pub fn mock() -> bool {
    std::env::var_os("MOCK").is_some_and(|v| !v.is_empty())
}

/// config.mjs `MODEL`.
pub fn model() -> String {
    let provider = std::env::var("GEN_PROVIDER").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "anthropic".into());
    std::env::var("GEN_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| if provider == "anthropic" { "claude-haiku-5-5".into() } else { "openai/gpt-4o-mini".into() })
}

/// appmeta.mjs `originOf(req)`.
pub fn origin_of(req: &HubRequest) -> String {
    match req.headers.get(hyper::header::HOST).and_then(|h| h.to_str().ok()).filter(|h| !h.is_empty()) {
        Some(h) => format!("http://{h}"),
        None => format!("http://127.0.0.1:{}", std::env::var("PORT").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "8787".into())),
    }
}

/// `x?.trim()` of a body field: `Ok(None)` for null/undefined, the V8
/// TypeError for a value that is not a string.
pub fn trimmed(b: &Value, k: &str) -> Result<Option<String>, String> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')).to_owned())),
        Some(_) => Err(format!("{k}?.trim is not a function")),
    }
}

/// The store must have loaded; else Node would not be up.
pub(crate) fn unready(ctx: &HubCtx) -> Option<HubResponse> {
    ctx.store.read(|s| s.list.is_empty()).then(|| thrown("the store did not load"))
}

macro_rules! ready {
    ($ctx:expr) => {
        if let Some(r) = $crate::hub::routes::chat::unready(&$ctx) {
            return r;
        }
    };
}
pub(crate) use ready;

/// A write's failure as Node's dispatcher answers a throw.
macro_rules! wrote {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return $crate::hub::routes::chat::thrown(&e),
        }
    };
}
pub(crate) use wrote;

// ————— the seams —————

/// artifacts.mjs `libraryOf(thread, origin)`: `/thread/apps`' list for a
/// thread (its `t<n>` id). Track C fills it in `ctx.ext`.
#[derive(Clone)]
pub struct LibraryOf(pub Arc<dyn Fn(Arc<HubCtx>, String, String) -> BoxFuture<'static, Value> + Send + Sync>);

// ————— install —————

/// Track A: the store's start, then its routes.
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    match ctx.store.load() {
        Ok(()) => {
            match groups::migrate(ctx) {
                Ok(true) => eprintln!("bot sections + pinned Mini Apps → sidebar groups"),
                Ok(false) => {}
                Err(e) => eprintln!("rox-broker: the groups' migration: {e}"),
            }
            if let Err(e) = ctx.store.write_shelf() {
                eprintln!("rox-broker: widgets/index.tsx: {e}");
            }
            ctx.store.write_index();
        }
        Err(e) => eprintln!("rox-broker: the store did not load: {e}"),
    }
    if ctx.local.is_none() {
        ctx.local = Some(Arc::new(MacTools));
    }
    if ctx.turns.is_none() {
        ctx.turns = Some(Arc::new(MockTurns));
    }
    botctx::register(ctx);
    routes.route(Verb::Post, X("/chat"), chat);
    routes.route(Verb::Get, X("/threads"), threads);
    routes.route(Verb::Post, X("/thread/new"), thread_new);
    routes.route(Verb::Get, X("/turns"), turns);
    routes.route(Verb::Post, X("/thread/update"), thread_update);
    routes.route(Verb::Post, X("/thread/delete"), thread_delete);
    routes.route(Verb::Post, X("/thread/select"), thread_select);
    routes.route(Verb::Get, P("/status"), status);
    routes.route(Verb::Post, X("/copy"), copy);
    routes.route(Verb::Post, X("/react"), react);
    routes.route(Verb::Post, X("/turn/delete"), turn_delete);
    routes.route(Verb::Post, X("/retry"), retry);
    routes.route(Verb::Post, X("/share"), share);
    routes.route(Verb::Post, X("/share/thread"), share_thread);
    routes.route(Verb::Get, X("/thread/apps"), thread_apps);
    super::bots::install(routes);
    super::groups::install(routes);
    super::mcp_chat::install(routes);
}

// ————— the routes —————

/// Runs a queued job's turn; a failure ends the job as Node's crash does.
fn start(ctx: Arc<HubCtx>, id: JobId, chat_turn: ChatTurn) {
    tokio::spawn(async move {
        let runner = ctx.turns.clone();
        let r = match runner {
            Some(r) => r.generate(ctx.clone(), chat_turn).await,
            None => Err("not ported".to_owned()),
        };
        if let Err(e) = r {
            eprintln!("✗ job {id} crashed: {e}");
            ctx.jobs.with(id, |j| j.done = true);
            ctx.jobs.set(&ctx.store, id, "done", &format!("generator error — {e}"));
        }
    });
}

async fn chat(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let attachments = b.get("attachments");
    let prompt = match b.get("prompt").filter(|p| js::truthy(p)) {
        Some(p) => js::string_of(p),
        None if has_len(attachments) => String::new(),
        None => return refused("missing prompt", false),
    };
    let mut extra = Map::new();
    if let Some(reply) = b.get("reply").filter(|r| r.get("text").is_some_and(js::truthy)) {
        let mut r = Map::new();
        if let Some(i) = reply.get("i") {
            r.insert("i".into(), i.clone());
        }
        r.insert("text".into(), json!(js_slice(&js::string_of(&reply["text"]), 400)));
        match reply.get("files") {
            Some(Value::Array(fs)) if !fs.is_empty() => {
                r.insert("files".into(), Value::Array(fs.iter().take(8).map(|f| json!(js::string_of(f))).collect()));
            }
            Some(Value::String(s)) if !s.is_empty() => return refused("reply.files.slice(...).map is not a function", false),
            _ => {}
        }
        extra.insert("reply".into(), Value::Object(r));
    }
    if has_len(attachments) {
        extra.insert("attachments".into(), attachments.cloned().unwrap_or(Value::Null));
    }
    if let Some(Value::Array(v1)) = b.get("v1_attachments") {
        if !v1.is_empty() {
            extra.insert("v1Attachments".into(), Value::Array(v1.iter().take(16).cloned().collect()));
        }
    }
    let named = b.get("thread").filter(|t| !t.is_null());
    let target = match named {
        Some(t) => {
            let key = js::string_of(t);
            match ctx.store.read(|s| s.thread(&key).map(|t| t.id.clone())) {
                Some(id) => Some(id),
                None => return refused(&format!("unknown thread {}", js_slice(&key, 40)), false),
            }
        }
        None => None,
    };
    let (thread, agents) = ctx.store.read(|s| {
        let thread = target.clone().unwrap_or_else(|| s.active.clone());
        let t = target.as_deref().and_then(|t| s.thread(t)).or_else(|| s.thread(&s.active));
        (thread, t.is_some_and(|t| t.agents.as_ref().is_some_and(js::truthy)))
    });
    if agents {
        return answer(409, out(&json!({"ok": false, "code": "read_only", "error": "an agents' conversation is read-only: message an agent in its own chat"})), true);
    }
    let extra = Value::Object(extra);
    let id = ctx.jobs.create(&thread, extra.clone(), None, true);
    start(ctx.clone(), id, ChatTurn { turn: TurnCtx { thread, job: Some(id), ..Default::default() }, prompt, extra, retry: None });
    ok(&json!({"ok": true, "id": id}))
}

async fn threads(ctx: Arc<HubCtx>, _req: HubRequest) -> HubResponse {
    ready!(ctx);
    let bots = ctx.store.read(|s| s.bots.clone());
    let integrations: std::collections::HashMap<String, Value> = wrote!(ctx.db.with(|c| Ok(bots.iter().map(|b| (b.id.clone(), botctx::integrations_of(&ctx, c, b))).collect())));
    let m = if mock() { "mock".to_owned() } else { model() };
    let body = ctx.store.read(|s| threads_json(s, &m, |b| integrations.get(&b.id).cloned().unwrap_or(json!([]))));
    ok(&body)
}

async fn thread_new(ctx: Arc<HubCtx>, _req: HubRequest) -> HubResponse {
    ready!(ctx);
    let id = wrote!(ctx.store.write(|s| {
        let t = s.fresh_thread("new chat");
        let id = t.id.clone();
        s.list.insert(0, t);
        s.active = id.clone();
        id
    }));
    ctx.store.write_index();
    ok(&json!({"ok": true, "id": id}))
}

async fn turns(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let key = req.query("thread").filter(|k| !k.is_empty());
    let l = wrote!(ctx.db.with(Lookups::read));
    let gen = ctx.store.gen_dir();
    let body = ctx.store.touch(|s| {
        let i = key.as_deref().and_then(|k| s.list.iter().position(|t| t.id == k || t.uid.as_deref() == Some(k))).unwrap_or_else(|| s.active_index());
        let turns = turns_json(&mut s.list[i], &l, &gen);
        let a = s.active_thread();
        let mut o = Map::new();
        o.insert("active".into(), json!(a.id));
        if let Some(u) = &a.uid {
            o.insert("activeUid".into(), json!(u));
        }
        o.insert("thread".into(), json!(s.list[i].id));
        o.insert("turns".into(), turns);
        Value::Object(o)
    });
    ok(&body)
}

async fn thread_update(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let id = b.get("id");
    let found = ctx.store.read(|s| id.and_then(Value::as_str).is_some_and(|id| s.list.iter().any(|t| t.id == id)));
    if !found {
        return refused(&format!("unknown thread {}", template(id)), false);
    }
    let title = match trimmed(&b, "title") {
        Ok(t) => t,
        Err(e) => return refused(&e, false),
    };
    let id = id.and_then(Value::as_str).unwrap_or_default().to_owned();
    wrote!(ctx.store.write(|s| {
        if let Some(t) = s.list.iter_mut().find(|t| t.id == id) {
            if let Some(title) = title.filter(|t| !t.is_empty()) {
                t.title = title;
            }
            if let Some(h) = b.get("hidden") {
                t.hidden = js::truthy(h);
            }
            if let Some(a) = b.get("archived") {
                t.archived = js::truthy(a);
            }
        }
    }));
    ctx.store.write_index();
    ok(&json!({"ok": true}))
}

/// Deletes a chat's (or a bot's chat's) unpublished modules, their state
/// and their app rows (`/thread/delete`, `/bot/delete`).
pub fn sweep_modules(ctx: &HubCtx, names: &[String]) -> Result<(), String> {
    let published: Vec<String> = ctx.db.with(|c| {
        let mut st = c.prepare("SELECT name FROM widgets")?;
        let v = st.query_map([], |r| Ok(js::string_of(&crate::hub::store::write::to_json(r.get(0)?))))?.collect();
        v
    })?;
    for name in names {
        let card = name.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        if !card || published.contains(name) {
            continue;
        }
        let _ = std::fs::remove_file(ctx.store.gen_dir().join(format!("{name}.tsx")));
        ctx.db.with(|c| {
            c.execute("DELETE FROM kv WHERE name=?", [name])?;
            c.execute("DELETE FROM apps WHERE name=?", [name])
        })?;
    }
    Ok(())
}

/// The card names of a thread's AI turns.
pub fn card_names(t: Option<&crate::hub::store::Thread>) -> Vec<String> {
    t.map(|t| t.turns.iter().filter(|u| u.role == "ai").filter_map(|u| u.name.clone()).collect()).unwrap_or_default()
}

async fn thread_delete(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let id = b.get("id");
    let Some(names) = ctx.store.read(|s| id.and_then(Value::as_str).and_then(|id| s.list.iter().find(|t| t.id == id)).map(|t| card_names(Some(t)))) else {
        return refused(&format!("unknown thread {}", template(id)), false);
    };
    if let Err(e) = sweep_modules(&ctx, &names) {
        return refused(&e, false);
    }
    let id = id.and_then(Value::as_str).unwrap_or_default().to_owned();
    let active = wrote!(ctx.store.write(|s| {
        for b in s.bots.iter_mut().filter(|b| b.thread_id.as_deref() == Some(id.as_str())) {
            b.thread_id = None;
        }
        s.list.retain(|t| t.id != id);
        if s.list.is_empty() {
            let t = s.fresh_thread("new chat");
            s.list.push(t);
        }
        if s.active == id {
            s.active = s.list[0].id.clone();
        }
        s.active.clone()
    }));
    ctx.store.write_index();
    ok(&json!({"ok": true, "active": active}))
}

async fn thread_select(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let key = |k: &str| b.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned);
    let (id, uid) = (key("id"), key("uid"));
    let l = wrote!(ctx.db.with(Lookups::read));
    let gen = ctx.store.gen_dir();
    let found = ctx.store.touch(|s| {
        let i = id
            .as_deref()
            .and_then(|k| s.list.iter().position(|t| t.id == k || t.uid.as_deref() == Some(k)))
            .or_else(|| uid.as_deref().and_then(|k| s.list.iter().position(|t| t.id == k || t.uid.as_deref() == Some(k))))?;
        s.active = s.list[i].id.clone();
        let turns = turns_json(&mut s.list[i], &l, &gen);
        Some(json!({"ok": true, "id": s.list[i].id, "uid": s.list[i].uid, "turns": turns}))
    });
    let Some(body) = found else {
        let shown = match b.get("id").filter(|v| !v.is_null()) {
            Some(v) => js::string_of(v),
            None => template(b.get("uid").filter(|v| !v.is_null())),
        };
        return refused(&format!("unknown thread {shown}"), false);
    };
    wrote!(ctx.store.write(|_| ()));
    ctx.store.write_index();
    ok(&body)
}

async fn status(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let id = match req.query("id") {
        Some(v) => js_number_str(&v),
        None => 0.0,
    };
    ok(&ctx.jobs.json(id))
}

async fn copy(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let text = b.get("text").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_default();
    let mut cmd = command("pbcopy", &[]);
    cmd.stdin = Some(text.into_bytes());
    match exec(&ctx, cmd).await {
        Ok(_) => ok(&json!({"ok": true})),
        Err(e) => refused(&e, false),
    }
}

async fn react(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let r = b.get("r").filter(|r| js::truthy(r)).map(js::string_of);
    let key = b.get("key").and_then(Value::as_str).filter(|k| k.starts_with("idx:")).map(str::to_owned);
    let name = b.get("name").cloned();
    wrote!(ctx.store.write(|s| match &key {
        Some(k) => {
            let n = js_number_str(&k[4..]);
            let i = s.active_index();
            if n.fract() == 0.0 && n >= 0.0 {
                if let Some(t) = s.list[i].turns.get_mut(n as usize) {
                    t.reaction = r.clone();
                }
            }
        }
        None => {
            for t in s.list.iter_mut() {
                for u in t.turns.iter_mut().filter(|u| u.role == "ai") {
                    let same = match &name {
                        None => u.name.is_none() && !u.null_name,
                        Some(Value::Null) => u.null_name,
                        Some(Value::String(n)) => u.name.as_deref() == Some(n.as_str()),
                        Some(_) => false,
                    };
                    if same {
                        u.reaction = r.clone();
                    }
                }
            }
        }
    }));
    ctx.store.write_index();
    ok(&json!({"ok": true}))
}

async fn turn_delete(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let key = b.get("key").and_then(Value::as_str).unwrap_or_default().to_owned();
    let dropped = wrote!(ctx.store.write(|s| {
        let i = s.active_index();
        let turns = &mut s.list[i].turns;
        let mut at = None;
        if let Some(name) = key.strip_prefix("name:") {
            at = turns.iter().position(|t| t.role == "ai" && t.name.as_deref() == Some(name));
        } else if let Some(n) = key.strip_prefix("idx:") {
            let n = js_number_str(n);
            if n.fract() == 0.0 && n >= 0.0 && (n as usize) < turns.len() {
                at = Some(n as usize);
            }
        }
        let at = at?;
        let mut drop = vec![at];
        if key.starts_with("name:") && at > 0 && turns[at - 1].role == "user" {
            drop.insert(0, at - 1);
        }
        let mut j = 0;
        turns.retain(|_| {
            let keep = !drop.contains(&j);
            j += 1;
            keep
        });
        Some(drop.len())
    }));
    let Some(dropped) = dropped else { return refused("no such turn", false) };
    ctx.store.write_index();
    ok(&json!({"ok": true, "dropped": dropped}))
}

async fn retry(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let name = b.get("name").cloned();
    let matches = |u: &Turn| {
        u.role == "ai"
            && match &name {
                None => u.name.is_none() && !u.null_name,
                Some(Value::Null) => u.null_name,
                Some(Value::String(n)) => u.name.as_deref() == Some(n.as_str()),
                Some(_) => false,
            }
    };
    let (thread, prompt) = ctx.store.read(|s| {
        let (mut thread, mut prompt): (Option<String>, Option<String>) = (None, None);
        for t in &s.list {
            let idx = t.turns.iter().position(|x| matches(x) && x.widget).or_else(|| t.turns.iter().position(&matches));
            let Some(idx) = idx else { continue };
            thread = Some(t.id.clone());
            if let Some(u) = t.turns[..idx].iter().rev().find(|u| u.role == "user") {
                prompt = u.text.clone();
            }
        }
        (thread, prompt)
    });
    let (Some(thread), Some(prompt)) = (thread, prompt.filter(|p| !p.is_empty())) else {
        return refused(&format!("no prompt found for {}", template(name.as_ref())), false);
    };
    let retry = name.as_ref().map(js::string_of);
    let id = ctx.jobs.create(&thread, Value::Null, retry.clone(), false);
    start(ctx.clone(), id, ChatTurn { turn: TurnCtx { thread, job: Some(id), ..Default::default() }, prompt, extra: json!({}), retry });
    ok(&json!({"ok": true, "id": id}))
}

async fn share(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let name = template(b.get("name"));
    let card = name.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    let file = [ctx.store.gen_dir(), ctx.store.widgets_dir()].into_iter().map(|d| d.join(format!("{name}.tsx"))).find(|p| p.exists());
    let (true, Some(file)) = (card, file) else { return refused(&format!("no such module {name}"), false) };
    let local = tools(&ctx);
    let path = file.to_string_lossy().into_owned();
    tokio::spawn(async move {
        let _ = local.run(command("open", &["-R", &path])).await;
    });
    ok(&json!({"ok": true}))
}

async fn share_thread(ctx: Arc<HubCtx>, _req: HubRequest) -> HubResponse {
    ready!(ctx);
    let gen = ctx.store.gen_dir();
    let text = ctx.store.read(|s| {
        s.active_thread()
            .turns
            .iter()
            .map(|t| {
                // BUG: a text reply (no module) is shared as "Gen UI
                // (undefined.tsx via <label>):" with an empty body (CHAT-11)
                if t.role == "user" {
                    format!("You: {}", t.text.clone().unwrap_or_else(|| "undefined".into()))
                } else {
                    let name = t.name.clone().unwrap_or_else(|| if t.null_name { "null".into() } else { "undefined".into() });
                    let code = std::fs::read_to_string(gen.join(format!("{name}.tsx"))).unwrap_or_default();
                    format!("Gen UI ({name}.tsx via {}):\n{code}", t.label.clone().unwrap_or_else(|| "undefined".into()))
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    });
    let mut cmd = command("pbcopy", &[]);
    cmd.stdin = Some(crate::hub::store::json::sql_text(&text).into_bytes());
    match exec(&ctx, cmd).await {
        Ok(_) => ok(&json!({"ok": true})),
        Err(e) => thrown(&e),
    }
}

async fn thread_apps(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let Some(lib) = ctx.ext.get::<LibraryOf>().cloned() else { return not_ported() };
    let key = req.query("thread").filter(|k| !k.is_empty());
    let id = ctx.store.read(|s| key.as_deref().and_then(|k| s.list.iter().find(|t| t.id == k)).unwrap_or_else(|| s.active_thread()).id.clone());
    let apps = (lib.0)(ctx.clone(), id, origin_of(&req)).await;
    ok(&json!({"apps": apps}))
}

// ————— the MOCK turns of a hub without the tool loop —————

/// What MOCK Node answers with no model and no pipeline (see the module's
/// docs); everything else is not ported.
pub struct MockTurns;

const MOCK_REPLY: &str = "_(mock reply — no model configured)_";

/// `labelOf(choiceFor(thread))`.
fn label_for(ctx: &HubCtx, thread: &str) -> String {
    let meta = |k: &str| ctx.db.with(|c| c.query_row("SELECT v FROM meta WHERE k=?", [k], |r| r.get::<_, Option<String>>(0))).ok().flatten().filter(|v| !v.is_empty());
    let bot = ctx.store.read(|s| s.thread_bot(thread).map(|b| (b.name.clone(), b.model.clone(), b.fast)));
    match bot {
        Some((name, _, _)) if !name.is_empty() => name,
        Some((_, m, fast)) if !m.is_empty() => format!("{m}{}", if fast { "--fast" } else { "" }),
        Some((_, _, fast)) => match meta("model") {
            Some(m) => format!("{m}{}", if fast { "--fast" } else { "" }),
            None => model(),
        },
        None => match meta("model") {
            Some(m) => format!("{m}{}", if meta("fast").as_deref() == Some("1") { "--fast" } else { "" }),
            None => model(),
        },
    }
}

fn first_word(prompt: &str) -> Option<String> {
    // /^\/mock\s+([a-z0-9]+)\s*$/i
    let rest = prompt.get(5..)?;
    if !prompt[..5].eq_ignore_ascii_case("/mock") {
        return None;
    }
    let after = rest.trim_start_matches(|c: char| c.is_whitespace());
    if after.len() == rest.len() {
        return None;
    }
    let word = after.trim_end_matches(|c: char| c.is_whitespace());
    (!word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| word.to_ascii_lowercase())
}

/// `/^\/mock\s+images?\s+(\d+)/i`
fn images_count(prompt: &str) -> bool {
    let Some(rest) = prompt.get(5..) else { return false };
    let after = rest.trim_start_matches(char::is_whitespace);
    if after.len() == rest.len() || after.len() < 5 || !after.is_char_boundary(5) || !after[..5].eq_ignore_ascii_case("image") {
        return false;
    }
    let after = &after[5..];
    let after = after.strip_prefix(['s', 'S']).unwrap_or(after);
    let n = after.trim_start_matches(char::is_whitespace);
    n.len() != after.len() && n.starts_with(|c: char| c.is_ascii_digit())
}

/// `/^\/<cmd>\b/i` on the trimmed prompt.
fn slash(prompt: &str, cmd: &str) -> bool {
    let n = cmd.len() + 1;
    prompt.len() >= n && prompt.is_char_boundary(n) && prompt[..n].eq_ignore_ascii_case(&format!("/{cmd}")) && !prompt[n..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl MockTurns {
    /// Two turns, a user's then the reply, and the job done "replied".
    fn reply(ctx: &HubCtx, job: JobId, thread: &str, title: Option<&str>, user: Turn, ai: Turn) -> Result<(), String> {
        ctx.store.write(|s| {
            let i = s.list.iter().position(|t| t.id == thread).unwrap_or_else(|| s.active_index());
            let t = &mut s.list[i];
            if let Some(title) = title {
                if t.turns.is_empty() {
                    t.title = title.into();
                }
            }
            t.turns.push(user);
            t.turns.push(ai);
        })?;
        ctx.store.write_index();
        ctx.jobs.finish(&ctx.store, job, "replied", Some("reply"));
        Ok(())
    }
}

impl TurnRunner for MockTurns {
    fn run(&self, ctx: Arc<HubCtx>, turn: TurnRequest) -> BoxFuture<'static, Result<TurnDone, String>> {
        Box::pin(async move {
            if !mock() {
                return Err("not ported (the tool loop, 5.4)".into());
            }
            let thread = turn.turn.thread.clone();
            let user = Turn::user(&turn.shown).with_extra(turn.mark.as_ref().unwrap_or(&Value::Null));
            // where the turn landed is the shown turn (routineTurn's `at`)
            let shown = ctx.store.append_turn(&thread, user)?.ok_or_else(|| format!("unknown thread {thread}"))?;
            ctx.store.write_index();
            let label = match &turn.turn.model {
                Some(m) if !m.model.is_empty() => format!("{}{}", m.model, if m.fast { "--fast" } else { "" }),
                _ => model(),
            };
            ctx.store.append_turn(&thread, Turn::ai(MOCK_REPLY, &label))?.ok_or_else(|| format!("unknown thread {thread}"))?;
            ctx.store.write_index();
            if let Some(job) = turn.turn.job {
                ctx.jobs.finish(&ctx.store, job, "replied", Some("reply"));
            }
            Ok(shown)
        })
    }

    fn generate(&self, ctx: Arc<HubCtx>, chat: ChatTurn) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            if !mock() {
                return Err("not ported (the tool loop, 5.4)".into());
            }
            let job = chat.turn.job.ok_or("no job")?;
            let thread = chat.turn.thread.clone();
            let p = chat.prompt.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')).to_owned();
            let user = || Turn::user(&chat.prompt).with_extra(&chat.extra);
            if chat.retry.is_none() && slash(&p, "mock") {
                let word = first_word(&p).filter(|w| w != "files" && w != "media");
                let alias = |w: &str| -> String {
                    let a = [
                        ("markdown", "md"),
                        ("text", "txt"),
                        ("rust", "rs"),
                        ("vcard", "vcf"),
                        ("doc", "docx"),
                        ("xls", "xlsx"),
                        ("ppt", "pptx"),
                        ("sheet", "xlsx"),
                        ("deck", "pptx"),
                        ("video", "mov"),
                        ("image", "png"),
                        ("images", "png"),
                        ("document", "docx"),
                        ("word", "docx"),
                        ("excel", "xlsx"),
                        ("spreadsheet", "xlsx"),
                        ("powerpoint", "pptx"),
                        ("presentation", "pptx"),
                        ("slides", "pptx"),
                    ];
                    a.iter().find(|(k, _)| *k == w).map(|(_, v)| (*v).to_owned()).unwrap_or_else(|| w.to_owned())
                };
                let known = ["pdf", "md", "html", "txt", "json", "csv", "rs", "tsx", "png", "svg", "gif", "webp", "vcf", "mov", "m4a", "wav", "aiff", "docx", "xlsx", "pptx"];
                if images_count(&p) {
                    return Err("not ported: /mock images (the tool loop, 5.4)".into());
                }
                if let Some(word) = word {
                    let exts: Vec<String> = if word == "audio" { vec!["m4a".into(), "wav".into(), "aiff".into()] } else { vec![alias(&word)] };
                    if exts.iter().any(|e| !known.contains(&e.as_str())) {
                        let mut sorted = known.to_vec();
                        sorted.sort();
                        let text = format!("No mock for `{word}` — try one of: {} (or `files`, `media`, `images n`, `audio`).", sorted.join(", "));
                        let ai = Turn { files: Some(json!([])), ..Turn::ai(&text, "mock") };
                        // `Mock <word>` first, then reply()'s own "Mock files"
                        return Self::reply(&ctx, job, &thread, Some("Mock files"), user(), ai);
                    }
                }
                return Err("not ported: /mock's files (the tool loop, 5.4)".into());
            }
            if chat.retry.is_none() && (slash(&p, "upload") || slash(&p, "uploadx")) {
                let files = chat.extra.get("attachments").and_then(Value::as_array).is_some_and(|a| {
                    a.iter().any(|x| {
                        x.get("uri").and_then(Value::as_str).and_then(|u| u.strip_prefix("file://")).is_some_and(|p| js::decode_uri_component(p).is_ok_and(|p| std::path::Path::new(&p).exists()))
                    })
                });
                if files {
                    return Err("not ported: /upload's file cards (the tool loop, 5.4)".into());
                }
                let rel = format!("m{}", groups::now36());
                let _ = std::fs::create_dir_all(ctx.store.gen_dir().join("files").join(rel));
                let text = "Attach a file and /upload echoes it back as a viewable card — nothing goes to the model.";
                let ai = Turn { files: Some(json!([])), ..Turn::ai(text, "upload") };
                return Self::reply(&ctx, job, &thread, Some("Upload echo"), user(), ai);
            }
            let attachments = chat.extra.get("attachments").and_then(Value::as_array).cloned().unwrap_or_default();
            let revise = attachments.iter().any(|a| a.get("uri").and_then(Value::as_str).is_some_and(|u| u.starts_with("module://")));
            let non_module = attachments.iter().any(|a| !a.get("uri").and_then(Value::as_str).is_some_and(|u| u.starts_with("module://")));
            if chat.retry.is_none() && !revise && non_module {
                let phrases = ["Thinking it through", "Mulling it over", "Putting it together"];
                let phrase = phrases[(rox_v1::keys::random::<1>()[0] as usize) % phrases.len()];
                ctx.jobs.set(&ctx.store, job, "thinking", phrase);
                ctx.jobs.with(job, |j| {
                    j.fields.insert("_phraseKey".into(), json!("thinking"));
                    j.fields.insert("_phrase".into(), json!(phrase));
                });
                let title = js_slice(&chat.prompt, 42);
                let label = label_for(&ctx, &thread);
                return Self::reply(&ctx, job, &thread, Some(&title), user(), Turn::ai(MOCK_REPLY, &label));
            }
            Err("not ported: MOCK's generate path (the pipeline, 5.7a)".into())
        })
    }
}
