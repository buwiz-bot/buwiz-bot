//! Agents and the world outside Rox (agents-external.mjs, drafts/agent-
//! messaging.md §10). An endpoint belongs to one agent and is a participant
//! `ext:<id>` in its conversations:
//!
//! - kind "hook": an incoming webhook, `POST /v1/hooks/<id>/<secret>` with
//!   `X-Rox-Signature: t=<unix s>,v1=<hex HMAC-SHA256(secret, "<t>.<body>")>`;
//!   what the agent sends back goes to its `url` when it has one (signed
//!   with `out_secret`, retried on 5xx for 24 h on the clock's timers,
//!   dropped on 4xx with a notice), else it is held for nobody to read.
//! - kind "bridge": a Claude Code session through rox-bridge, with the
//!   endpoint's secret as its bearer token; what the agent sends back is
//!   held until the bridge receives it (a long poll of at most 30 s).
//!
//! Owns `agent_endpoints`, `agent_hook_seen` and `agent_outbox`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{params, OptionalExtension, Row};
use serde_json::{json, Map, Value};
use tokio::sync::Notify;

use super::{AgentError, AgentsHub, Endpoint, Endpoints, Party};
use crate::hub::ctx::{Db, HubCtx};
use crate::hub::store::Turn;
use crate::hub::traits::Clock;
use crate::hub::types::BoxFuture;
use crate::js;

/// per hour per hook
pub const HOOK_RATE: usize = 60;
pub const SKEW_S: f64 = 300.0;
pub const BODY_MAX: usize = 512 * 1024;
const DAY: f64 = 24.0 * 3_600_000.0;

/// A refusal with its HTTP status (`ExternalError`).
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

fn refuse<T>(status: u16, code: &str, message: impl Into<String>) -> Result<T, ExternalError> {
    Err(ExternalError { status, code: code.into(), message: message.into() })
}

/// HMAC-SHA256 of `msg` under `key`, lowercase hex.
pub fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |x: u8| k.iter().map(|b| b ^ x).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    let outer = Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize();
    outer.iter().map(|b| format!("{b:02x}")).collect()
}

/// `X-Rox-Signature` for `body` at unix second `t` (as written).
pub fn signature(secret: &str, body: &str, t: &str) -> String {
    format!("t={t},v1={}", hmac_hex(secret.as_bytes(), format!("{t}.{body}").as_bytes()))
}

/// `timingSafeEqual` after a length check.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// An `agent_endpoints` row.
#[derive(Clone, Debug, PartialEq)]
pub struct EpRow {
    pub id: String,
    pub bot: String,
    pub kind: String,
    pub name: String,
    pub secret: String,
    pub url: Option<String>,
    pub out_secret: Option<String>,
    pub created_at: Value,
}

fn ep_row(r: &Row) -> rusqlite::Result<EpRow> {
    let num = |v: rusqlite::types::Value| match v {
        rusqlite::types::Value::Integer(i) => json!(i),
        rusqlite::types::Value::Real(f) => super::ms(f),
        rusqlite::types::Value::Text(t) => json!(t),
        _ => Value::Null,
    };
    Ok(EpRow {
        id: r.get("id")?,
        bot: r.get("bot")?,
        kind: r.get("kind")?,
        name: r.get("name")?,
        secret: r.get("secret")?,
        url: r.get("url")?,
        out_secret: r.get("out_secret")?,
        created_at: num(r.get("created_at")?),
    })
}

impl EpRow {
    /// `pub(r)`: what is shown of it, never its secrets.
    fn public(&self) -> Map<String, Value> {
        let mut o = Map::new();
        o.insert("id".into(), json!(self.id));
        o.insert("bot".into(), json!(self.bot));
        o.insert("kind".into(), json!(self.kind));
        o.insert("name".into(), json!(self.name));
        if let Some(u) = self.url.as_ref().filter(|u| !u.is_empty()) {
            o.insert("url".into(), json!(u));
        }
        o.insert("created_at".into(), self.created_at.clone());
        o
    }
}

/// An outbox row.
#[derive(Clone, Debug)]
struct Outbox {
    endpoint: String,
    msg: String,
    at: f64,
    state: String,
    attempts: i64,
}

/// One outgoing POST: the url, the body, the headers → the status.
pub type Post = Arc<dyn Fn(String, String, Vec<(String, String)>) -> BoxFuture<'static, Result<u16, String>> + Send + Sync>;

/// `fetch(url, {method: "POST", body, headers, signal: AbortSignal.timeout(15_000)})`.
pub fn http_post() -> Post {
    Arc::new(|url, body, headers| {
        Box::pin(async move {
            let url = url::Url::parse(&url).map_err(|e| e.to_string())?;
            let mut req = crate::helper::http::Req::post(url, body.into_bytes());
            for (k, v) in &headers {
                req.set(k, v);
            }
            match tokio::time::timeout(Duration::from_secs(15), crate::helper::http::fetch(req)).await {
                Ok(Ok(res)) => Ok(res.status),
                Ok(Err(e)) => Err(e),
                Err(_) => Err("The operation was aborted due to timeout".into()),
            }
        })
    })
}

pub struct Externals {
    db: Db,
    clock: Arc<Clock>,
    post: Post,
    /// hook id → call times, for the rate limit
    hits: Mutex<HashMap<String, Vec<f64>>>,
    /// bridge long polls: the endpoint and its waker
    waiting: Mutex<Vec<(String, Arc<Notify>)>>,
}

impl Endpoints for Externals {
    fn get(&self, id: &str) -> Option<Endpoint> {
        self.row(id).map(|r| Endpoint { id: r.id, name: r.name, bot: r.bot })
    }
}

impl Externals {
    /// The endpoints on app.db at `path`; its tables made when missing.
    pub fn new(path: std::path::PathBuf, clock: Arc<Clock>, post: Post) -> Self {
        let x = Self { db: Db::new(path), clock, post, hits: Mutex::new(HashMap::new()), waiting: Mutex::new(Vec::new()) };
        let made = x.db.with(|c| {
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS agent_endpoints(
      id TEXT PRIMARY KEY, bot TEXT NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL,
      secret TEXT NOT NULL, url TEXT, out_secret TEXT, created_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS agent_hook_seen(
      endpoint TEXT NOT NULL, id TEXT NOT NULL, hash TEXT NOT NULL, answer TEXT NOT NULL, at INTEGER NOT NULL,
      PRIMARY KEY(endpoint, id));
    CREATE TABLE IF NOT EXISTS agent_outbox(
      seq INTEGER PRIMARY KEY AUTOINCREMENT, endpoint TEXT NOT NULL, msg TEXT NOT NULL, at INTEGER NOT NULL,
      state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER);",
            )
        });
        if let Err(e) = made {
            eprintln!("rox-broker: the agents' endpoint tables: {e}");
        }
        x
    }

    fn now(&self) -> f64 {
        self.clock.now()
    }

    fn row(&self, id: &str) -> Option<EpRow> {
        self.db.with(|c| c.query_row("SELECT * FROM agent_endpoints WHERE id=?", [id], ep_row).optional()).ok().flatten()
    }

    fn internal(e: String) -> ExternalError {
        ExternalError { status: 500, code: "internal".into(), message: e }
    }

    /// A new endpoint for agent `bot`; its secrets are answered only here.
    /// `exists` is `agents.party(bot)`.
    pub fn create(&self, body: &Value, exists: impl Fn(&str) -> bool) -> Result<Map<String, Value>, ExternalError> {
        let kind = body.get("kind").and_then(Value::as_str).filter(|k| *k == "hook" || *k == "bridge");
        let Some(kind) = kind else { return refuse(400, "invalid_args", "kind is \"hook\" or \"bridge\"") };
        let label = js::slice_utf16(super::trim(&super::str_or_empty(body.get("name"))), 60);
        if label.is_empty() {
            return refuse(400, "invalid_args", "name is required");
        }
        let bot = super::str_or_empty(body.get("bot"));
        if !exists(&bot) {
            return refuse(400, "invalid_args", "no such agent");
        }
        let url = body.get("url");
        if let Some(u) = url {
            let u = js::string_of(u);
            let ok = ["http://", "https://"].iter().any(|p| u.strip_prefix(p).is_some_and(|rest| !rest.is_empty() && !rest.chars().any(is_js_space)));
            if !ok {
                return refuse(400, "invalid_args", "url is an http(s) URL");
            }
        }
        let url = url.filter(|u| js::truthy(u)).map(js::string_of);
        let key = |n: usize| rox_v1::keys::b64(&rox_v1::keys::random::<32>()[..n]);
        let id = format!("{}_{}", if kind == "hook" { "ah" } else { "ab" }, key(9));
        let secret = key(24);
        let out_secret = url.as_ref().map(|_| key(24));
        let now = self.now() as i64;
        self.db
            .with(|c| {
                c.execute("INSERT INTO agent_endpoints(id, bot, kind, name, secret, url, out_secret, created_at) VALUES(?,?,?,?,?,?,?,?)", params![id, bot, kind, label, secret, url, out_secret, now])
            })
            .map_err(Self::internal)?;
        let r = self.row(&id).ok_or_else(|| Self::internal("the endpoint's row is gone".into()))?;
        let mut o = r.public();
        o.insert("secret".into(), json!(secret));
        if let Some(s) = out_secret {
            o.insert("out_secret".into(), json!(s));
        }
        if kind == "hook" {
            o.insert("path".into(), json!(format!("/v1/hooks/{id}/{secret}")));
        }
        Ok(o)
    }

    /// The endpoints of `bot` (all of them when it is not given).
    pub fn list(&self, bot: Option<&Value>) -> Result<Value, ExternalError> {
        let bot = bot.filter(|b| js::truthy(b)).map(js::string_of);
        let rows = self
            .db
            .with(|c| {
                let sql = format!("SELECT * FROM agent_endpoints{} ORDER BY created_at", if bot.is_some() { " WHERE bot=?" } else { "" });
                let mut st = c.prepare(&sql)?;
                let rows = match &bot {
                    Some(b) => st.query_map([b], ep_row)?.collect::<rusqlite::Result<Vec<_>>>()?,
                    None => st.query_map([], ep_row)?.collect::<rusqlite::Result<Vec<_>>>()?,
                };
                Ok(rows)
            })
            .map_err(Self::internal)?;
        Ok(Value::Array(rows.iter().map(|r| Value::Object(r.public())).collect()))
    }

    /// A new secret for endpoint `id`.
    pub fn rotate(&self, id: Option<&Value>) -> Result<Map<String, Value>, ExternalError> {
        let key = super::str_or_empty(id);
        if self.row(&key).is_none() {
            return refuse(400, "invalid_args", "no such endpoint");
        }
        let secret = rox_v1::keys::b64(&rox_v1::keys::random::<24>());
        self.db.with(|c| c.execute("UPDATE agent_endpoints SET secret=? WHERE id=?", params![secret, key])).map_err(Self::internal)?;
        let mut o = Map::new();
        o.insert("id".into(), id.cloned().unwrap_or(Value::Null));
        o.insert("secret".into(), json!(secret));
        Ok(o)
    }

    /// Endpoint `id` and its outbox, gone.
    pub fn remove(&self, id: Option<&Value>) -> Result<(), ExternalError> {
        let id = super::str_or_empty(id);
        self.db
            .with(|c| {
                c.execute("DELETE FROM agent_endpoints WHERE id=?", [&id])?;
                c.execute("DELETE FROM agent_outbox WHERE endpoint=?", [&id])
            })
            .map(|_| ())
            .map_err(Self::internal)
    }

    /// `POST /v1/hooks/<id>/<secret>`: a signed envelope from outside
    /// (§10.1) → `{id, conversation}`. `raw` is the body as text.
    pub fn incoming(&self, hub: &Arc<AgentsHub>, ctx: &Arc<HubCtx>, id: &str, secret: &str, raw: &str, header: Option<&str>) -> Result<Value, ExternalError> {
        let r = self.row(id).filter(|r| r.kind == "hook" && same(&r.secret, secret));
        let Some(r) = r else { return refuse(404, "not_found", "no such hook") };
        if raw.len() > BODY_MAX {
            return refuse(413, "too_large", "the body is over 512 KB");
        }
        let now = self.now();
        let signed = parse_signature(header.unwrap_or(""))
            .is_some_and(|(t, v1)| (now / 1000.0 - t.parse::<f64>().unwrap_or(f64::NAN)).abs() <= SKEW_S && same(&signature(&r.secret, raw, t), &format!("t={t},v1={v1}")));
        if !signed {
            return refuse(401, "bad_signature", "X-Rox-Signature is missing, wrong or older than 5 minutes");
        }
        {
            let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
            let times = hits.entry(id.to_owned()).or_default();
            times.retain(|x| *x > now - 3_600_000.0);
            if times.len() >= HOOK_RATE {
                return refuse(429, "rate_limited", format!("at most {HOOK_RATE} calls an hour"));
            }
            times.push(now);
        }
        let Ok(env) = serde_json::from_str::<Value>(raw) else { return refuse(400, "invalid_args", "the body is not JSON") };
        if !env.is_object() {
            return refuse(400, "invalid_args", "the body is an envelope object");
        }
        if env.get("v").and_then(Value::as_f64) != Some(1.0) {
            return refuse(400, "unsupported_version", "v must be 1");
        }
        if let Some(i) = env.get("id") {
            if !msg_id(&js::string_of(i)) {
                return refuse(400, "invalid_args", "id is msg_ and 8 to 80 letters, digits, _ or -");
            }
        }
        let env_id = env.get("id").filter(|v| js::truthy(v)).map(js::string_of);
        self.db.with(|c| c.execute("DELETE FROM agent_hook_seen WHERE at < ?", [(now - DAY) as i64])).map_err(Self::internal)?;
        let hash = rox_v1::keys::sha256_hex(raw.as_bytes());
        if let Some(eid) = &env_id {
            let seen = self
                .db
                .with(|c| c.query_row("SELECT hash, answer FROM agent_hook_seen WHERE endpoint=? AND id=?", [id, eid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).optional())
                .map_err(Self::internal)?;
            if let Some((h, answer)) = seen {
                if h != hash {
                    return refuse(409, "replayed", "this id came with another body in the last 24 hours");
                }
                return serde_json::from_str(&answer).map_err(|e| Self::internal(e.to_string()));
            }
        }
        let mut input = Map::new();
        for k in ["to", "conversation", "in_reply_to", "subject", "text", "data", "attachments", "expects_reply", "deadline"] {
            if let Some(v) = env.get(k) {
                input.insert(k.into(), v.clone());
            }
        }
        let sent = hub.write(ctx, |s, out| hub.agents.send_from(s, &r.id, &Value::Object(input), env.get("id").cloned(), out));
        let answer = match sent {
            Ok(sent) => json!({"id": sent["id"], "conversation": sent["conversation"]}),
            Err(e) if e.code == "internal" => return Err(Self::internal(e.message)),
            Err(e) => return refuse(if e.code == "rate_limited" { 429 } else { 400 }, &e.code, e.message),
        };
        if let Some(eid) = &env_id {
            self.db
                .with(|c| c.execute("INSERT INTO agent_hook_seen(endpoint, id, hash, answer, at) VALUES(?,?,?,?,?)", params![id, eid, hash, js::stringify(&answer), now as i64]))
                .map_err(Self::internal)?;
        }
        Ok(answer)
    }

    /// A message an agent sent to endpoint `p`: to its url, or held for
    /// the bridge (§10.2, §10.3).
    pub fn deliver(self: &Arc<Self>, hub: &Arc<AgentsHub>, ctx: &Arc<HubCtx>, p: &Party, msg: &Value) {
        let msg_id = js_id(msg);
        let Some(r) = self.row(p.id.strip_prefix("ext:").unwrap_or(&p.id)) else {
            eprintln!("agents: {msg_id} to a removed endpoint {}", p.id);
            return;
        };
        let now = self.now() as i64;
        let url = r.url.as_ref().filter(|u| !u.is_empty());
        let seq = self.db.with(|c| {
            c.execute(
                "INSERT INTO agent_outbox(endpoint, msg, at, state, next_at) VALUES(?,?,?,?,?)",
                params![r.id, js::stringify(msg), now, if url.is_some() { "queued" } else { "held" }, url.map(|_| now)],
            )?;
            Ok(c.last_insert_rowid())
        });
        let seq = match seq {
            Ok(seq) => seq,
            Err(e) => {
                eprintln!("agents: {msg_id} to {}: {e}", r.id);
                return;
            }
        };
        if url.is_some() {
            let (x, hub, ctx) = (self.clone(), hub.clone(), ctx.clone());
            tokio::spawn(async move { x.send(&hub, &ctx, seq).await });
        } else {
            let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
            waiting.retain(|(ep, w)| {
                if *ep == r.id {
                    w.notify_one();
                    false
                } else {
                    true
                }
            });
        }
    }

    /// One outgoing attempt: 2xx delivered, 4xx dropped (its sender told),
    /// 5xx or no answer retried with backoff until a day has passed.
    pub fn send<'a>(self: &'a Arc<Self>, hub: &'a Arc<AgentsHub>, ctx: &'a Arc<HubCtx>, seq: i64) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let o = self
                .db
                .with(|c| {
                    c.query_row("SELECT endpoint, msg, at, state, attempts FROM agent_outbox WHERE seq=?", [seq], |r| {
                        Ok(Outbox { endpoint: r.get(0)?, msg: r.get(1)?, at: r.get::<_, f64>(2)?, state: r.get(3)?, attempts: r.get(4)? })
                    })
                    .optional()
                })
                .ok()
                .flatten();
            let Some(o) = o else { return };
            let r = self.row(&o.endpoint);
            let Some((r, url)) = r.and_then(|r| r.url.clone().filter(|u| !u.is_empty()).map(|u| (r, u))) else { return };
            if o.state != "queued" {
                return;
            }
            let t = ((self.now() / 1000.0).floor() as i64).to_string();
            let headers = vec![("content-type".to_owned(), "application/json".to_owned()), ("x-rox-signature".to_owned(), signature(r.out_secret.as_deref().unwrap_or(""), &o.msg, &t))];
            let status = (self.post)(url, o.msg.clone(), headers).await.unwrap_or(0);
            let attempts = o.attempts + 1;
            if (200..300).contains(&status) {
                let _ = self.db.with(|c| c.execute("UPDATE agent_outbox SET state='delivered', attempts=? WHERE seq=?", params![attempts, seq]));
                return;
            }
            if (400..500).contains(&status) || self.now() - o.at >= DAY {
                let _ = self.db.with(|c| c.execute("UPDATE agent_outbox SET state='dropped', attempts=? WHERE seq=?", params![attempts, seq]));
                let msg = serde_json::from_str(&o.msg).unwrap_or(Value::Null);
                self.dropped(hub, ctx, &r, &msg, status);
                return;
            }
            let wait = 3_600_000u64.min(30_000 * 2u64.saturating_pow((attempts - 1).clamp(0, 30) as u32));
            let next = self.now() as i64 + wait as i64;
            let _ = self.db.with(|c| c.execute("UPDATE agent_outbox SET attempts=?, next_at=? WHERE seq=?", params![attempts, next, seq]));
            let (x, hub, ctx2) = (self.clone(), hub.clone(), ctx.clone());
            ctx.clock.set_timeout(
                wait,
                Box::new(move || {
                    tokio::spawn(async move { x.send(&hub, &ctx2, seq).await });
                }),
            );
        })
    }

    /// The agent that sent it learns, in a notice in the conversation.
    fn dropped(&self, hub: &Arc<AgentsHub>, ctx: &Arc<HubCtx>, r: &EpRow, msg: &Value, status: u16) {
        let id = js_id(msg);
        let conv = msg.get("conversation").and_then(Value::as_str).unwrap_or("").to_owned();
        let now = self.now();
        let text = format!("Not delivered to {}{}", r.name, if status != 0 { format!(" (HTTP {status})") } else { String::new() });
        let notice = json!({"kind": "agents_undelivered", "endpoint": r.id, "message": msg.get("id"), "at": super::ms(now)});
        let _ = hub.write(ctx, |s, _| {
            if let Some(i) = super::conversation_index(s, &conv) {
                s.list[i].turns.push(Turn { id: Some(format!("drop-{id}")), ..Turn::notice(&text, notice) });
            }
            Ok(())
        });
        eprintln!("agents: {id} to {} dropped ({})", r.id, if status != 0 { status.to_string() } else { "no answer".into() });
    }

    /// Outgoing messages a restart left queued, tried again.
    pub fn resume(self: &Arc<Self>, hub: &Arc<AgentsHub>, ctx: &Arc<HubCtx>) {
        let seqs = self
            .db
            .with(|c| {
                let mut st = c.prepare("SELECT seq FROM agent_outbox WHERE state='queued'")?;
                let rows = st.query_map([], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>();
                rows
            })
            .unwrap_or_default();
        for seq in seqs {
            let (x, hub, ctx) = (self.clone(), hub.clone(), ctx.clone());
            tokio::spawn(async move { x.send(&hub, &ctx, seq).await });
        }
    }

    /// The bridge with token `token`, or a 401.
    pub fn bridge(&self, token: &str) -> Result<EpRow, ExternalError> {
        let rows = if token.is_empty() {
            Vec::new()
        } else {
            self.db
                .with(|c| {
                    let mut st = c.prepare("SELECT * FROM agent_endpoints WHERE kind='bridge'")?;
                    let rows = st.query_map([], ep_row)?.collect::<rusqlite::Result<Vec<_>>>();
                    rows
                })
                .map_err(Self::internal)?
        };
        match rows.into_iter().find(|x| same(&x.secret, token)) {
            Some(r) => Ok(r),
            None => refuse(401, "unauthorized", "the bridge token is missing or wrong"),
        }
    }

    /// What is held for endpoint `ep` (of conversation `conv`), oldest
    /// first, taken.
    fn take(&self, ep: &str, conv: Option<&Value>) -> Vec<Value> {
        let rows = self
            .db
            .with(|c| {
                let mut st = c.prepare("SELECT seq, msg FROM agent_outbox WHERE endpoint=? AND state='held' ORDER BY seq")?;
                let rows = st.query_map([ep], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>();
                rows
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        for (seq, msg) in rows {
            let m: Value = serde_json::from_str(&msg).unwrap_or(Value::Null);
            if conv.is_some_and(|c| m.get("conversation") != Some(c)) {
                continue;
            }
            let _ = self.db.with(|c| c.execute("UPDATE agent_outbox SET state='delivered' WHERE seq=?", [seq]));
            out.push(m);
        }
        out
    }

    /// `rox_receive`: what the bridge's agent sent it, oldest first,
    /// waiting up to `wait_s` (at most 30) when there is nothing yet.
    pub async fn receive(&self, r: &EpRow, body: &Value) -> Value {
        let conv = body.get("conversation").filter(|c| js::truthy(c));
        let mut got = self.take(&r.id, conv);
        let wait_s = match body.get("wait_s") {
            None => 0.0,
            Some(v) => crate::hub::store::json::js_number(Some(v)),
        };
        let wait_s = if wait_s.is_nan() { 0.0 } else { wait_s };
        let wait = wait_s.min(30.0).max(0.0) * 1000.0;
        if got.is_empty() && wait > 0.0 {
            let w = Arc::new(Notify::new());
            self.waiting.lock().unwrap_or_else(|e| e.into_inner()).push((r.id.clone(), w.clone()));
            // what came between the first look and the waiter's start
            got = self.take(&r.id, conv);
            if got.is_empty() {
                let _ = tokio::time::timeout(Duration::from_millis(wait as u64), w.notified()).await;
                got = self.take(&r.id, conv);
            }
            self.waiting.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, x)| !Arc::ptr_eq(x, &w));
        }
        json!({"messages": got})
    }

    /// The hook calls counted for `id` (tests).
    #[cfg(test)]
    pub fn hits(&self, id: &str) -> usize {
        self.hits.lock().unwrap().get(id).map_or(0, Vec::len)
    }
}

/// A message's id as a template literal prints it.
fn js_id(msg: &Value) -> String {
    super::js_str(msg.get("id"))
}

/// `/^\s/` for a URL's `[^\s]`.
fn is_js_space(c: char) -> bool {
    js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `/^t=(\d+),v1=([0-9a-f]{64})$/`.
fn parse_signature(h: &str) -> Option<(&str, &str)> {
    let rest = h.strip_prefix("t=")?;
    let (t, v1) = rest.split_once(",v1=")?;
    (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && v1.len() == 64 && v1.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then_some((t, v1))
}

/// `/^msg_[A-Za-z0-9_-]{8,80}$/`.
fn msg_id(s: &str) -> bool {
    s.strip_prefix("msg_").is_some_and(|r| (8..=80).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}

/// An agent's refusal as an external one (`sendFrom`'s callers).
pub fn from_agent(e: AgentError, status: u16) -> ExternalError {
    ExternalError { status, code: e.code, message: e.message }
}
