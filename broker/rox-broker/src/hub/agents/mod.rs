//! Agent messaging (plan 5.6, track E; agents.mjs): the user's agents
//! messaging each other and the world outside Rox. The v1 envelope,
//! agents' threads (a thread marked `agents: {conversation, participants,
//! subject, created_by, created_at}`, read-only and never a sidebar row),
//! one notice per conversation in each participant's chat, who may message
//! whom, the limits, and the agents an agent makes.
//!
//! - [`Agents`] (this file) decides and records, on the store's
//!   [`State`]; what it hands on for delivery comes back as
//!   [`Delivery`]s, which the caller routes once the store is written.
//! - `run` delivers (agents-run.mjs, agents-wire.mjs): the recipient's
//!   turn, one per agent at a time and four in all; a reply wakes the
//!   asker in its own chat once that chat is idle.
//! - `external` (agents-external.mjs): outside endpoints, a signed `ah_`
//!   webhook in, an outgoing webhook's outbox, the Claude Code bridge.
//!   Owns the tables `agent_endpoints`, `agent_hook_seen`, `agent_outbox`.
//! - `tools` (agents-tools.mjs): the six agent tools, by name.
//! - `hub/routes/agents.rs`: `/v1/host/agents/*`, `/v1/hooks/ah_*`,
//!   `/v1/bridge/*`.
//!
//! Turns run through `ctx.turns` (a reply's wake-up, Node's
//! `backgroundTurn`) and the [`run::AgentRequestTurn`] seam (a request's
//! answer, Node's `agentTurn` on a stand-in thread), which the tool loop
//! (5.4) fills; until it does, MOCK answers as Node's `chatReply` does.

pub mod external;
pub mod run;
#[cfg(test)]
mod tests;
pub mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Map, Value};

use super::ctx::HubCtx;
use super::store::json::js_slice;
use super::store::{Bot, State, Thread, Turn};
use super::traits::Clock;
use super::Registry;
use crate::js;

pub use run::AgentsHub;

/// agents.mjs `LIMITS`.
pub const RECIPIENTS: usize = 8;
pub const TEXT: usize = 16 * 1024;
pub const DATA: usize = 64 * 1024;
pub const SUBJECT: usize = 120;
pub const ATTACHMENTS: usize = 16;
pub const HOP: i64 = 6;
/// per agent per 10 minutes
pub const SENDS: usize = 30;
/// per conversation without a human, then paused
pub const PER_HOUR: usize = 20;
pub const MERGE_MS: f64 = 10.0 * 60_000.0;

/// A refusal (`AgentError`): its code and message. `internal` is the
/// store failing to write, Node's `persist()` throwing.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentError {
    pub code: String,
    pub message: String,
}

impl AgentError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

fn fail<T>(code: &str, message: impl Into<String>) -> Result<T, AgentError> {
    Err(AgentError::new(code, message))
}

/// A participant: a bot, or an outside endpoint `ext:<id>` with the bot it
/// belongs to (`owner`).
#[derive(Clone, Debug, PartialEq)]
pub struct Party {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
}

impl Party {
    pub fn of(b: &Bot) -> Self {
        Party { id: b.id.clone(), name: b.name.clone(), owner: None }
    }

    pub fn external(&self) -> bool {
        self.owner.is_some()
    }

    fn kind(&self) -> &'static str {
        if self.external() {
            "external"
        } else {
            "agent"
        }
    }

    fn wire(&self) -> Value {
        json!({"kind": self.kind(), "id": self.id, "name": self.name})
    }
}

/// An outside endpoint as a participant sees it (`externals.get(id)`).
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    pub bot: String,
}

/// Where the endpoints are looked up (agents-external.mjs's `get`).
pub trait Endpoints: Send + Sync {
    fn get(&self, id: &str) -> Option<Endpoint>;
}

/// A message handed on to a recipient: `deliver(b, msg, t)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub to: Party,
    pub msg: Value,
    /// the conversation's `t<n>` id
    pub thread: String,
}

// ————— JavaScript's ways with values —————

/// `String(v)`, `undefined` for a missing one (template literals).
pub(crate) fn js_str(v: Option<&Value>) -> String {
    v.map(js::string_of).unwrap_or_else(|| "undefined".into())
}

/// `String(v || "")`.
pub(crate) fn str_or_empty(v: Option<&Value>) -> String {
    v.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()
}

/// `s.trim()`.
pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(js::is_space)
}

/// `String(s ?? "").trim().slice(0, n)`.
pub(crate) fn clip(v: Option<&Value>, n: usize) -> String {
    let s = match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => js::string_of(v),
    };
    js_slice(trim(&s), n)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(js::truthy)
}

/// A time as JSON holds it: an integer when it is one.
pub(crate) fn ms(t: f64) -> Value {
    if t.fract() == 0.0 && t.abs() < 9e15 {
        json!(t as i64)
    } else {
        json!(t)
    }
}

/// `Date.parse(v)` for the ISO forms (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`,
/// then `THH:MM`, `:SS`, `.fff`, `Z` or `±HH:MM`; a date-time with no
/// offset is local time); NaN for anything else, where V8 would try its
/// legacy formats too.
pub fn date_parse(v: &Value) -> f64 {
    let Value::String(s) = v else {
        return match v {
            Value::Number(_) => date_parse(&json!(js::string_of(v))),
            _ => f64::NAN,
        };
    };
    parse_iso(s.trim_matches(js::is_space)).unwrap_or(f64::NAN)
}

fn parse_iso(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let digits = |from: usize, n: usize| -> Option<i64> { s.get(from..from + n).filter(|t| t.bytes().all(|c| c.is_ascii_digit())).and_then(|t| t.parse().ok()) };
    let (year, mut i) = match b.first() {
        Some(b'+') | Some(b'-') => {
            let y = digits(1, 6)?;
            (if b[0] == b'-' { -y } else { y }, 7)
        }
        _ => (digits(0, 4)?, 4),
    };
    let (mut month, mut day) = (1, 1);
    if b.get(i) == Some(&b'-') {
        month = digits(i + 1, 2)?;
        i += 3;
        if b.get(i) == Some(&b'-') {
            day = digits(i + 1, 2)?;
            i += 3;
        }
    }
    let (mut h, mut mi, mut sec, mut milli) = (0, 0, 0.0, 0.0);
    let mut offset: Option<i64> = Some(0);
    if matches!(b.get(i), Some(b'T') | Some(b't')) {
        h = digits(i + 1, 2)?;
        if b.get(i + 3) != Some(&b':') {
            return None;
        }
        mi = digits(i + 4, 2)?;
        i += 6;
        if b.get(i) == Some(&b':') {
            sec = digits(i + 1, 2)? as f64;
            i += 3;
            if b.get(i) == Some(&b'.') {
                let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                if n == 0 {
                    return None;
                }
                let frac: String = s[i + 1..i + 1 + n].chars().chain("00".chars()).take(3).collect();
                milli = frac.parse::<f64>().ok()?;
                i += 1 + n;
            }
        }
        offset = None;
        match b.get(i) {
            None => {}
            Some(b'Z') | Some(b'z') if i + 1 == b.len() => offset = Some(0),
            Some(c @ (b'+' | b'-')) if i + 6 == b.len() && b[i + 3] == b':' => {
                let m = digits(i + 1, 2)? * 60 + digits(i + 4, 2)?;
                offset = Some(if *c == b'-' { -m } else { m });
            }
            _ => return None,
        }
        if offset.is_some() || b.get(i).is_none() {
            i = b.len();
        }
    }
    if i != b.len() || !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 24 || mi > 59 || sec > 59.0 || (h == 24 && (mi > 0 || sec > 0.0 || milli > 0.0)) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let utc = ((days * 86400 + h * 3600 + mi * 60) as f64 + sec) * 1000.0 + milli;
    Some(match offset {
        Some(m) => utc - (m * 60_000) as f64,
        None => utc - local_offset_ms(utc),
    })
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// This Mac's offset from UTC at about `utc` (ms), as `localtime_r` has it.
fn local_offset_ms(utc: f64) -> f64 {
    let t = (utc / 1000.0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to live locals
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return 0.0;
    }
    tm.tm_gmtoff as f64 * 1000.0
}

/// `n.toString(36)` of a non-negative integer.
pub(crate) fn base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(D[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// `/^@[a-z0-9-]+:[a-z0-9_-]+:tkt_[A-Za-z0-9_-]+$/`: a v1 token naming a
/// ticket, what an attachment may be (spec 3).
fn ticket(s: &str) -> bool {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| regex_lite::Regex::new(r"^@[a-z0-9-]+:[a-z0-9_-]+:tkt_[A-Za-z0-9_-]+$").expect("the ticket pattern")).is_match(s)
}

/// A notice's one line (`agentsNoticeText`).
pub fn notice_text(n: &Value) -> String {
    let f = |k: &str| js_str(n.get(k));
    match n.get("kind").and_then(Value::as_str) {
        Some("agent_created") if truthy(n.get("temporary")) => format!("{} started a helper: {} · {}", f("parent"), f("name"), f("task")),
        Some("agent_created") => format!("{} made a new agent: {}", f("parent"), f("name")),
        Some("helper_finished") => format!("Helper {} finished", f("name")),
        Some("agents_paused") => format!("Paused: these agents have exchanged {PER_HOUR} messages in an hour"),
        _ => {
            let to = n.get("to").and_then(Value::as_array).cloned().unwrap_or_default();
            let to = if to.len() == 1 { js_str(to[0].get("name")) } else { format!("{} agents", to.len()) };
            let count = n.get("count").and_then(Value::as_f64).unwrap_or(f64::NAN);
            let more = if count > 1.0 { format!(" · {} messages", js_str(n.get("count"))) } else { String::new() };
            format!("{} → {to} · {}{more}", js_str(n.get("from").and_then(|x| x.get("name"))), f("subject"))
        }
    }
}

// ————— the bots and threads —————

/// `b.helper?.<key>`.
fn helper_of<'a>(b: &'a Bot, key: &str) -> Option<&'a Value> {
    b.helper.as_ref().and_then(|h| h.get(key))
}

/// `!!b && !b.helper?.ended_at`.
fn live(b: &Bot) -> bool {
    !truthy(helper_of(b, "ended_at"))
}

fn has_helper(b: &Bot) -> bool {
    truthy(b.helper.as_ref())
}

/// Whether `to` takes messages from `from` (spec 8).
fn accepts(to: &Bot, from: &str) -> bool {
    match to.accepts.as_ref() {
        Some(Value::String(a)) if a == "none" => false,
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(from)),
        _ => true,
    }
}

/// A thread's `agents` mark, as an object.
fn mark(t: &Thread) -> Option<&Map<String, Value>> {
    t.agents.as_ref().and_then(Value::as_object)
}

fn mark_mut(t: &mut Thread) -> Option<&mut Map<String, Value>> {
    t.agents.as_mut().and_then(Value::as_object_mut)
}

fn participants(t: &Thread) -> Vec<Value> {
    mark(t).and_then(|m| m.get("participants")).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn includes(t: &Thread, id: &str) -> bool {
    participants(t).iter().any(|p| p.as_str() == Some(id))
}

/// `u.message?.<key>`.
fn msg_field<'a>(u: &'a Turn, key: &str) -> Option<&'a Value> {
    u.message.as_ref().and_then(|m| m.get(key))
}

/// The conversation's thread index (`conversation(conv)`).
pub fn conversation_index(s: &State, conv: &str) -> Option<usize> {
    s.list.iter().position(|t| mark(t).and_then(|m| m.get("conversation")).and_then(Value::as_str) == Some(conv))
}

/// `findThread(key)` as agents.mjs's `thread`.
fn thread_index(s: &State, key: Option<&str>) -> Option<usize> {
    let key = key?;
    s.list.iter().position(|t| t.id == key || t.uid.as_deref() == Some(key))
}

// ————— the broker —————

pub type Uid = Arc<dyn Fn() -> String + Send + Sync>;

/// agents.mjs's `createAgents`: the send limit's memory, the clock, the
/// uid minter and the endpoints; everything else is the store's.
pub struct Agents {
    clock: Arc<Clock>,
    uid: Uid,
    eps: Arc<dyn Endpoints>,
    /// sender id → send times, for the rate limit
    sent: Mutex<HashMap<String, Vec<f64>>>,
}

impl Agents {
    pub fn new(clock: Arc<Clock>, uid: Uid, eps: Arc<dyn Endpoints>) -> Self {
        Self { clock, uid, eps, sent: Mutex::new(HashMap::new()) }
    }

    pub fn now(&self) -> f64 {
        self.clock.now()
    }

    fn uid(&self) -> String {
        (self.uid)()
    }

    /// A participant: a bot, or an outside endpoint `ext:<id>`.
    pub fn party(&self, s: &State, id: &str) -> Option<Party> {
        match id.strip_prefix("ext:") {
            Some(ep) => self.eps.get(ep).map(|e| Party { id: format!("ext:{}", e.id), name: e.name, owner: Some(e.bot) }),
            None => s.bot(id).map(Party::of),
        }
    }

    /// The agents `caller` may message: live, not itself, taking messages
    /// from it; a temporary helper only to its parent. An outside sender
    /// reaches the one agent its endpoint belongs to.
    pub fn reachable<'a>(&self, s: &'a State, caller: &Party) -> Vec<&'a Bot> {
        match &caller.owner {
            Some(owner) => s.bots.iter().filter(|b| &b.id == owner && live(b) && accepts(b, &caller.id)).collect(),
            None => {
                s.bots.iter().filter(|b| b.id != caller.id && live(b) && accepts(b, &caller.id) && (!has_helper(b) || helper_of(b, "parent").and_then(Value::as_str) == Some(&caller.id))).collect()
            }
        }
    }

    /// `agents_list {}`.
    pub fn list(&self, s: &State, caller: &Party) -> Value {
        Value::Array(
            self.reachable(s, caller)
                .into_iter()
                .map(|b| {
                    let mut o = Map::new();
                    o.insert("id".into(), json!(b.id));
                    o.insert("name".into(), json!(b.name));
                    o.insert("description".into(), description(b));
                    o.insert("accepts_messages".into(), json!(true));
                    if has_helper(b) {
                        o.insert("helper".into(), json!(true));
                    }
                    if truthy(b.made_by.as_ref()) {
                        o.insert("made_by".into(), b.made_by.clone().unwrap_or(Value::Null));
                    }
                    Value::Object(o)
                })
                .collect(),
        )
    }

    /// One recipient as `to` names it: an id, a name, or `{id}`.
    fn recipient(&self, s: &State, caller: &Party, r: &Value) -> Result<Party, AgentError> {
        let key = match r {
            Value::String(k) => Some(k.clone()),
            _ => {
                let id = r.get("id");
                if r.get("kind").and_then(Value::as_str) == Some("external") && truthy(id) && !js_str(id).starts_with("ext:") {
                    Some(format!("ext:{}", js_str(id)))
                } else {
                    id.and_then(Value::as_str).map(str::to_owned)
                }
            }
        };
        let Some(key) = key.filter(|k| !trim(k).is_empty()) else { return fail("invalid_args", "each of `to` is an agent id or name") };
        if key.starts_with("ext:") {
            let Some(ep) = self.party(s, &key) else { return fail("invalid_args", format!("no outside endpoint {}", clip(Some(&json!(key)), 40))) };
            if ep.owner.as_deref() != Some(caller.id.as_str()) {
                return fail("not_allowed", format!("{} is not one of your outside endpoints", ep.name));
            }
            return Ok(ep);
        }
        let k = trim(&key).to_lowercase();
        let Some(b) = s.bots.iter().find(|x| x.id == key).or_else(|| s.bots.iter().find(|x| live(x) && x.name.to_lowercase() == k)) else {
            return fail("invalid_args", format!("no agent {}", clip(Some(&json!(key)), 40)));
        };
        if b.id == caller.id {
            return fail("invalid_args", "an agent cannot message itself");
        }
        if !self.reachable(s, caller).iter().any(|x| x.id == b.id) {
            return fail("not_allowed", format!("{} does not take messages from you", b.name));
        }
        Ok(Party::of(b))
    }

    /// The conversation's notice in `p`'s chat: a new line, or the last one
    /// updated in place within the merge window (spec 6).
    fn notice_in(&self, s: &mut State, p: &Party, ti: usize, msg: &Value) {
        if p.external() {
            return;
        }
        let Some(ci) = thread_index(s, s.bot(&p.id).and_then(|b| b.thread_id.as_deref())) else { return };
        let (conv, tid, tuid, subject) = {
            let t = &s.list[ti];
            let m = mark(t).cloned().unwrap_or_default();
            (m.get("conversation").cloned().unwrap_or(Value::Null), t.id.clone(), t.uid.clone(), m.get("subject").cloned().unwrap_or(Value::Null))
        };
        let at = self.now();
        let chat = &mut s.list[ci];
        let last = chat.turns.iter_mut().rev().find(|u| u.notice.as_ref().is_some_and(|n| n.get("kind").and_then(Value::as_str) == Some("agents") && n.get("conversation") == Some(&conv)));
        if let Some(last) = last {
            let n = last.notice.as_mut().and_then(Value::as_object_mut).expect("found above");
            let prev = n.get("at").and_then(Value::as_f64).unwrap_or(f64::NAN);
            if at - prev <= MERGE_MS {
                let count = n.get("count").and_then(Value::as_f64).unwrap_or(f64::NAN) + 1.0;
                n.insert("from".into(), msg["from"].clone());
                n.insert("to".into(), msg["to"].clone());
                n.insert("count".into(), ms(count));
                n.insert("last_id".into(), msg["id"].clone());
                n.insert("at".into(), ms(at));
                let text = notice_text(last.notice.as_ref().expect("set above"));
                last.text = Some(text);
                return;
            }
        }
        let n = json!({
            "kind": "agents", "conversation": conv, "thread": tid, "thread_uid": tuid,
            "from": msg["from"], "to": msg["to"], "subject": subject, "count": 1, "last_id": msg["id"], "at": ms(at),
        });
        let text = notice_text(&n);
        let id = self.uid();
        s.list[ci].turns.push(Turn { id: Some(id), ..Turn::notice(&text, n) });
    }

    /// `{…n, at: now()}` as a notice in bot `id`'s chat.
    fn notice(&self, s: &mut State, id: &str, n: &Value) {
        let Some(ci) = thread_index(s, s.bot(id).and_then(|b| b.thread_id.as_deref())) else { return };
        let mut n = n.clone();
        n["at"] = ms(self.now());
        let text = notice_text(&n);
        let uid = self.uid();
        s.list[ci].turns.push(Turn { id: Some(uid), ..Turn::notice(&text, n) });
    }

    fn limited(&self, caller: &str) -> Result<(), AgentError> {
        let now = self.now();
        let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        let times = sent.entry(caller.to_owned()).or_default();
        times.retain(|x| *x > now - 10.0 * 60_000.0);
        if times.len() >= SENDS {
            return fail("rate_limited", format!("at most {SENDS} messages per 10 minutes"));
        }
        times.push(now);
        Ok(())
    }

    /// `agent_send`: validates, records the message in its conversation's
    /// thread, posts the notices and hands it on for each recipient
    /// (`out`). `hop` is the broker's: 0 from a human-driven turn, +1 for
    /// each relay while handling another agent's message; `id` the sender's
    /// own (an outside sender's idempotency key).
    pub fn send(&self, s: &mut State, caller: &Party, input: &Value, hop: i64, id: Option<Value>, out: &mut Vec<Delivery>) -> Result<Value, AgentError> {
        let get = |k: &str| input.get(k);
        let text = get("text").and_then(Value::as_str).unwrap_or("").to_owned();
        if trim(&text).is_empty() {
            return fail("invalid_args", "`text` is required");
        }
        if text.len() > TEXT {
            return fail("invalid_args", "`text` is over 16 KB");
        }
        let data = get("data");
        if data.is_some_and(|d| !d.is_object()) {
            return fail("invalid_args", "`data` must be an object");
        }
        if data.is_some_and(|d| js::stringify(d).len() > DATA) {
            return fail("invalid_args", "`data` is over 64 KB");
        }
        let attachments = match get("attachments") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) if a.len() <= ATTACHMENTS => a.clone(),
            _ => return fail("invalid_args", format!("`attachments` is a list of at most {ATTACHMENTS} tokens")),
        };
        if attachments.iter().any(|a| !a.as_str().is_some_and(ticket)) {
            return fail("invalid_args", "an attachment is a ticket token (@<plugin>:<scope>:tkt_…), never a raw identifier");
        }
        let mut deadline = None;
        if let Some(d) = get("deadline") {
            let t = date_parse(d);
            if !t.is_finite() {
                return fail("invalid_args", "`deadline` is an RFC 3339 time");
            }
            if t <= self.now() {
                return fail("invalid_args", "`deadline` has passed");
            }
            deadline = Some(t);
        }
        let mut ti = None;
        if let Some(c) = get("conversation") {
            ti = conversation_index(s, &js::string_of(c));
            let Some(i) = ti else { return fail("invalid_args", "no such conversation") };
            if !includes(&s.list[i], &caller.id) {
                return fail("not_allowed", "you are not in this conversation");
            }
        }
        if hop > HOP {
            if let Some(i) = ti {
                let id = self.uid();
                let n = json!({"kind": "agents_hop_limit", "at": ms(self.now())});
                s.list[i].turns.push(Turn { id: Some(id), ..Turn::notice(&format!("Stopped: a message was relayed more than {HOP} times"), n) });
            }
            return fail("hop_limit", format!("a message relayed more than {HOP} times between agents is refused"));
        }
        // one agent may come as its id or name alone
        let to_given = match get("to") {
            Some(Value::String(x)) if x != "*" => Some(json!([x])),
            other => other.cloned(),
        };
        let to: Vec<Party> = match &to_given {
            Some(Value::String(_)) => self.reachable(s, caller).into_iter().filter(|b| !has_helper(b)).map(Party::of).collect(),
            Some(Value::Array(a)) if !a.is_empty() => {
                let mut to: Vec<Party> = Vec::new();
                for r in a {
                    let p = self.recipient(s, caller, r)?;
                    // a Set of the objects: a bot is one object, an endpoint a new one each time
                    if p.external() || !to.iter().any(|x| !x.external() && x.id == p.id) {
                        to.push(p);
                    }
                }
                to
            }
            None if ti.is_some() => participants(&s.list[ti.expect("checked")])
                .iter()
                .filter(|p| p.as_str() != Some(caller.id.as_str()))
                .filter_map(|p| self.party(s, &js::string_of(p)))
                .filter(|p| p.external() || s.bot(&p.id).is_some_and(live))
                .collect(),
            None if caller.external() => self.reachable(s, caller).into_iter().map(Party::of).collect(),
            _ => return fail("invalid_args", "give `to`: a list of agents, or \"*\" for all of them"),
        };
        if to.is_empty() {
            return fail("invalid_args", "no agent to send to");
        }
        if to.len() > RECIPIENTS {
            return fail("too_many_recipients", format!("at most {RECIPIENTS} recipients"));
        }
        if let Some(r) = get("in_reply_to") {
            if !ti.is_some_and(|i| s.list[i].turns.iter().any(|u| msg_field(u, "id") == Some(r))) {
                return fail("invalid_args", "`in_reply_to` is not a message of this conversation");
            }
        }
        let ti = match ti {
            Some(i) => i,
            None => {
                let subject = clip(get("subject"), SUBJECT);
                if subject.is_empty() {
                    return fail("invalid_args", "`subject` is required for a new conversation");
                }
                let id = s.new_thread_id();
                let uid = self.uid();
                let conv = format!("conv_{}", self.uid());
                let agents = json!({"conversation": conv, "participants": [caller.id], "subject": subject, "created_by": caller.id, "created_at": ms(self.now())});
                s.list.push(Thread { id, uid: Some(uid), title: subject, agents: Some(agents), ..Default::default() });
                s.list.len() - 1
            }
        };
        let m = mark(&s.list[ti]).cloned().unwrap_or_default();
        if truthy(m.get("paused")) {
            return fail("paused", "this conversation is paused until the user resumes it");
        }
        // the last hour's messages, those after the user last resumed it
        let now = self.now();
        let resumed = m.get("resumed_at").and_then(Value::as_f64);
        let recent = |u: &Turn| {
            let at = msg_field(u, "sent_at").map(date_parse).unwrap_or(f64::NAN);
            at >= now - 3_600_000.0 && !resumed.is_some_and(|r| at <= r)
        };
        if s.list[ti].turns.iter().filter(|u| u.role == "agent_message" && recent(u)).count() >= PER_HOUR {
            let t = &mut s.list[ti];
            mark_mut(t).expect("a conversation").insert("paused".into(), ms(now));
            let n = json!({"kind": "agents_paused", "conversation": m.get("conversation"), "thread": t.id, "thread_uid": t.uid});
            let mut first = n.clone();
            first["at"] = ms(now);
            let id = self.uid();
            t.turns.push(Turn { id: Some(id), ..Turn::notice(&notice_text(&n), first) });
            for p in participants(&s.list[ti]) {
                self.notice(s, &js::string_of(&p), &n);
            }
            return fail("paused", format!("these agents exchanged {PER_HOUR} messages in an hour; the user resumes the conversation"));
        }
        // an outside sender has its endpoint's own rate (external.rs)
        if !caller.external() {
            self.limited(&caller.id)?;
        }
        {
            let ps = mark_mut(&mut s.list[ti]).expect("a conversation").entry("participants").or_insert_with(|| json!([]));
            if let Value::Array(ps) = ps {
                for b in &to {
                    if !ps.iter().any(|p| p.as_str() == Some(b.id.as_str())) {
                        ps.push(json!(b.id));
                    }
                }
            }
        }
        let mut msg = Map::new();
        msg.insert("v".into(), json!(1));
        msg.insert("id".into(), id.filter(js::truthy).unwrap_or_else(|| json!(format!("msg_{}", self.uid()))));
        msg.insert("conversation".into(), m.get("conversation").cloned().unwrap_or(Value::Null));
        if let Some(r) = get("in_reply_to") {
            msg.insert("in_reply_to".into(), r.clone());
        }
        msg.insert("from".into(), caller.wire());
        msg.insert("to".into(), Value::Array(to.iter().map(Party::wire).collect()));
        msg.insert("subject".into(), m.get("subject").cloned().unwrap_or(Value::Null));
        msg.insert("text".into(), json!(text));
        if let Some(d) = data {
            msg.insert("data".into(), d.clone());
        }
        if !attachments.is_empty() {
            msg.insert("attachments".into(), Value::Array(attachments));
        }
        msg.insert("expects_reply".into(), json!(truthy(get("expects_reply"))));
        if let Some(d) = deadline {
            msg.insert("deadline".into(), json!(js::iso(d)));
        }
        msg.insert("hop".into(), json!(hop));
        msg.insert("sent_at".into(), json!(js::iso(now)));
        let msg = Value::Object(msg);
        let uid = self.uid();
        let t = &mut s.list[ti];
        t.turns.push(Turn {
            role: "agent_message".into(),
            id: Some(uid),
            text: Some(text),
            message: Some(msg.clone()),
            pending: Some(Value::Array(to.iter().map(|b| json!(b.id)).collect())),
            ..Default::default()
        });
        // a new message reopens an archived conversation (spec 6)
        mark_mut(t).expect("a conversation").shift_remove("archived");
        let tid = t.id.clone();
        for p in std::iter::once(caller).chain(to.iter()) {
            self.notice_in(s, p, ti, &msg);
        }
        for b in &to {
            out.push(Delivery { to: b.clone(), msg: msg.clone(), thread: tid.clone() });
        }
        Ok(json!({"id": msg["id"], "conversation": msg["conversation"], "delivered_to": [], "queued_for": to.iter().map(|b| json!(b.id)).collect::<Vec<_>>()}))
    }

    /// The message `msg_id` reached `bot` (its turn ran, or it needed none).
    pub fn delivered(&self, s: &mut State, thread: &str, msg_id: &Value, bot: &str) {
        let Some(t) = s.list.iter_mut().find(|t| t.id == thread) else { return };
        let Some(u) = t.turns.iter_mut().find(|x| msg_field(x, "id") == Some(msg_id)) else { return };
        let Some(Value::Array(pending)) = u.pending.as_mut() else { return };
        pending.retain(|p| p.as_str() != Some(bot));
        if pending.is_empty() {
            u.pending = None;
        }
    }

    /// Every message still waiting for a recipient, oldest first: what a
    /// restart hands on again (spec 5.5).
    pub fn undelivered(&self, s: &State) -> Vec<Delivery> {
        let mut out = Vec::new();
        for t in s.list.iter().filter(|t| t.agents.is_some()) {
            for u in &t.turns {
                for id in u.pending.as_ref().and_then(Value::as_array).into_iter().flatten() {
                    let Some(p) = self.party(s, &js::string_of(id)) else { continue };
                    if p.external() || s.bot(&p.id).is_some_and(live) {
                        out.push(Delivery { to: p, msg: u.message.clone().unwrap_or(Value::Null), thread: t.id.clone() });
                    }
                }
            }
        }
        out
    }

    /// `agent_conversations {with?, limit?}`.
    pub fn conversations(&self, s: &State, caller: &str, input: &Value) -> Value {
        let other = input.get("with").filter(|v| js::truthy(v));
        let limit = match input.get("limit") {
            None => 20.0,
            Some(v) => super::store::json::js_number(Some(v)),
        };
        let limit = if limit.is_nan() || limit == 0.0 { 20.0 } else { limit };
        let n = limit.min(50.0).max(1.0);
        let mut rows: Vec<(String, Value)> = s
            .list
            .iter()
            .filter(|t| t.agents.is_some() && includes(t, caller) && other.map_or(true, |o| participants(t).contains(o)))
            .map(|t| {
                let m = mark(t).cloned().unwrap_or_default();
                let msgs: Vec<&Turn> = t.turns.iter().filter(|u| u.role == "agent_message").collect();
                let last_at = msgs.last().and_then(|u| msg_field(u, "sent_at")).filter(|v| js::truthy(v)).cloned().unwrap_or(Value::Null);
                let mut o = Map::new();
                o.insert("conversation".into(), m.get("conversation").cloned().unwrap_or(Value::Null));
                o.insert("subject".into(), m.get("subject").cloned().unwrap_or(Value::Null));
                o.insert(
                    "participants".into(),
                    Value::Array(
                        participants(t)
                            .iter()
                            .map(|id| {
                                let name = self.party(s, &js::string_of(id)).map(|p| json!(p.name)).filter(js::truthy).unwrap_or_else(|| id.clone());
                                json!({"id": id, "name": name})
                            })
                            .collect(),
                    ),
                );
                o.insert("last_at".into(), last_at.clone());
                o.insert("count".into(), json!(msgs.len()));
                if truthy(m.get("archived")) {
                    o.insert("archived".into(), json!(true));
                }
                if truthy(m.get("paused")) {
                    o.insert("paused".into(), json!(true));
                }
                (js::string_of(&last_at), Value::Object(o))
            })
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        Value::Array(rows.into_iter().take(n as usize).map(|(_, v)| v).collect())
    }

    /// `agent_read {conversation, since?}`: the messages, oldest first;
    /// `since` is a message id or a time. Marks them read by the caller.
    pub fn read(&self, s: &mut State, caller: &str, input: &Value) -> Result<Value, AgentError> {
        let Some(ti) = conversation_index(s, &str_or_empty(input.get("conversation"))) else { return fail("invalid_args", "no such conversation") };
        let t = &mut s.list[ti];
        if !includes(t, caller) {
            return fail("not_allowed", "you are not in this conversation");
        }
        let mut msgs: Vec<Value> = t.turns.iter().filter(|u| u.role == "agent_message").map(|u| u.message.clone().unwrap_or(Value::Null)).collect();
        if let Some(since) = input.get("since").filter(|v| js::truthy(v)) {
            msgs = match msgs.iter().position(|m| m.get("id") == Some(since)) {
                Some(i) => msgs.split_off(i + 1),
                None => {
                    let since = js::string_of(since);
                    msgs.into_iter().filter(|m| m.get("sent_at").and_then(Value::as_str).is_some_and(|at| utf16_gt(at, &since))).collect()
                }
            };
        }
        // what the reader has seen: a reply it read needs no wake-up (run.rs)
        let ids: Vec<&Value> = msgs.iter().filter_map(|m| m.get("id")).collect();
        for u in t.turns.iter_mut() {
            let Some(id) = msg_field(u, "id") else { continue };
            if !ids.contains(&id) {
                continue;
            }
            let mut by = u.read_by.as_ref().and_then(Value::as_array).cloned().unwrap_or_default();
            if !by.iter().any(|x| x.as_str() == Some(caller)) {
                by.push(json!(caller));
                u.read_by = Some(Value::Array(by));
            }
        }
        let m = mark(t).cloned().unwrap_or_default();
        Ok(json!({"conversation": m.get("conversation"), "subject": m.get("subject"), "messages": msgs}))
    }

    /// Whether `bot` has read message `msg_id` with agent_read.
    pub fn read_by(&self, s: &State, msg_id: &Value, bot: &str) -> bool {
        s.list.iter().any(|t| {
            t.agents.is_some() && t.turns.iter().any(|u| msg_field(u, "id") == Some(msg_id) && u.read_by.as_ref().and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x.as_str() == Some(bot))))
        })
    }

    /// `agent_create`: a permanent agent the user asked for, or the caller's
    /// one temporary helper (spec 4.1). It starts with at most the caller's
    /// reach and the model the user named, else the caller's.
    pub fn create(&self, s: &mut State, caller: &str, input: &Value, hop: i64, out: &mut Vec<Delivery>) -> Result<Value, AgentError> {
        let Some(parent) = s.bot(caller).cloned() else { return fail("invalid_args", "no such agent") };
        if has_helper(&parent) {
            return fail("not_allowed", "a helper cannot create agents");
        }
        let name = clip(input.get("name"), 40);
        let purpose = clip(input.get("purpose"), 300);
        if name.is_empty() {
            return fail("invalid_args", "`name` is required");
        }
        if purpose.is_empty() {
            return fail("invalid_args", "`purpose` is required: what the agent is for");
        }
        if s.bots.iter().any(|b| live(b) && b.name.to_lowercase() == name.to_lowercase()) {
            return fail("invalid_args", format!("an agent named {name} exists"));
        }
        let temporary = truthy(input.get("temporary"));
        if temporary {
            if let Some(h) = s.bots.iter().find(|b| live(b) && helper_of(b, "parent").and_then(Value::as_str) == Some(caller)) {
                return fail("not_allowed", format!("you already have a helper ({}); remove it with agent_remove first", h.name));
            }
        }
        let wanted: Option<Vec<String>> = input.get("plugins").and_then(Value::as_array).map(|a| a.iter().map(js::string_of).collect());
        let tid = s.new_thread_id();
        let now = self.now();
        let id = format!("b{}{}", base36(now.max(0.0) as u64), base36(u64::from(u16::from_le_bytes(rox_v1::keys::random::<2>())) % 46656));
        let color = Some(clip(input.get("avatar"), 9)).filter(|c| !c.is_empty()).or_else(|| Some(parent.color.clone()).filter(|c| !c.is_empty())).unwrap_or_else(|| "#1084FE".into());
        let mut b = Bot::new(&id, &name, "blob", &color, Some(tid.clone()));
        b.description = Some(purpose.clone());
        let instructions = clip(input.get("instructions"), 4000);
        if !instructions.is_empty() {
            b.instructions = Some(instructions);
        }
        // the model the user named, else the parent's; its own picker after
        b.model = Some(clip(input.get("model"), 80)).filter(|m| !m.is_empty()).unwrap_or_else(|| parent.model.clone());
        b.effort = parent.effort.clone();
        b.fast = parent.fast;
        // never more than the parent can use
        b.integrations = match &wanted {
            Some(w) => parent.integrations.iter().filter(|x| x.as_str().or_else(|| x.get("id").and_then(Value::as_str)).is_some_and(|k| w.iter().any(|w| w == k))).cloned().collect(),
            None => parent.integrations.clone(),
        };
        b.made_by = Some(json!(caller));
        b.purpose = Some(json!(purpose));
        if temporary {
            b.helper = Some(json!({"parent": caller, "task": purpose, "created_at": ms(now)}));
        }
        let uid = self.uid();
        s.list.push(Thread { id: tid.clone(), uid: Some(uid.clone()), title: name.clone(), ..Default::default() });
        s.bots.push(b);
        self.notice(s, caller, &json!({"kind": "agent_created", "agent": id, "name": name, "parent": parent.name, "temporary": temporary, "task": purpose, "thread": tid, "thread_uid": uid}));
        let mut made = json!({"id": id, "name": name, "temporary": temporary});
        if !clip(input.get("first_message"), TEXT).is_empty() {
            let first = json!({"to": [id], "subject": clip(Some(&json!(purpose)), SUBJECT), "text": js::string_of(&input["first_message"]), "expects_reply": true});
            let sent = self.send(s, &Party::of(&parent), &first, hop, None, out)?;
            made["conversation"] = sent["conversation"].clone();
        }
        Ok(made)
    }

    /// `agent_remove {id}`: ends a temporary helper the caller made; its
    /// chat and conversations stay, archived ("Past helpers").
    pub fn remove(&self, s: &mut State, caller: &str, input: &Value) -> Result<Value, AgentError> {
        let id = str_or_empty(input.get("id"));
        let Some(b) = s.bot(&id).filter(|b| has_helper(b) && helper_of(b, "parent").and_then(Value::as_str) == Some(caller)).cloned() else {
            return fail("not_allowed", "you can remove only a temporary helper you made");
        };
        if truthy(helper_of(&b, "ended_at")) {
            return fail("invalid_args", format!("{} has finished already", b.name));
        }
        let now = self.now();
        if let Some(Value::Object(h)) = s.bot_mut(&id).and_then(|x| x.helper.as_mut()) {
            h.insert("ended_at".into(), ms(now));
        }
        for t in s.list.iter_mut() {
            if t.agents.is_some() && includes(t, &id) {
                mark_mut(t).expect("a conversation").insert("archived".into(), json!(true));
            }
        }
        let chat_uid = thread_index(s, b.thread_id.as_deref()).and_then(|i| s.list[i].uid.clone());
        let mut n = json!({"kind": "helper_finished", "agent": b.id, "name": b.name, "thread": b.thread_id});
        if let Some(u) = chat_uid {
            n["thread_uid"] = json!(u);
        }
        self.notice(s, caller, &n);
        Ok(json!({"removed": true}))
    }

    /// The user archives a conversation, or brings it back.
    pub fn archive(&self, s: &mut State, conv: Option<&Value>, archived: bool) -> Result<(), AgentError> {
        let Some(ti) = conversation_index(s, &str_or_empty(conv)) else { return fail("invalid_args", "no such conversation") };
        let m = mark_mut(&mut s.list[ti]).expect("a conversation");
        if archived {
            m.insert("archived".into(), json!(true));
        } else {
            m.shift_remove("archived");
        }
        Ok(())
    }

    /// The user resumes a paused conversation: the hourly count starts over.
    pub fn resume(&self, s: &mut State, conv: Option<&Value>) -> Result<(), AgentError> {
        let Some(ti) = conversation_index(s, &str_or_empty(conv)) else { return fail("invalid_args", "no such conversation") };
        let now = self.now();
        let m = mark_mut(&mut s.list[ti]).expect("a conversation");
        m.shift_remove("paused");
        m.insert("resumed_at".into(), ms(now));
        Ok(())
    }

    /// "Accept messages from": "all", "none", or a list of agent ids.
    pub fn set_accepts(&self, s: &mut State, bot: Option<&Value>, accepts: Option<&Value>) -> Result<(), AgentError> {
        let Some(b) = s.bot_mut(&str_or_empty(bot)) else { return fail("invalid_args", "no such agent") };
        let ok = match accepts {
            Some(Value::String(a)) => a == "all" || a == "none",
            Some(Value::Array(a)) => a.iter().all(Value::is_string),
            _ => false,
        };
        if !ok {
            return fail("invalid_args", "accepts is \"all\", \"none\" or a list of agent ids");
        }
        b.accepts = accepts.cloned();
        Ok(())
    }

    /// A message from outside endpoint `ep`: to its agent unless `to` says
    /// otherwise; `id` is the sender's idempotency key.
    pub fn send_from(&self, s: &mut State, ep: &str, input: &Value, id: Option<Value>, out: &mut Vec<Delivery>) -> Result<Value, AgentError> {
        let Some(caller) = self.party(s, &format!("ext:{ep}")) else { return fail("invalid_args", "no such outside endpoint") };
        self.send(s, &caller, input, 0, id, out)
    }
}

/// `b.purpose || b.description || ""`.
pub(crate) fn description(b: &Bot) -> Value {
    if truthy(b.purpose.as_ref()) {
        return b.purpose.clone().unwrap_or(Value::Null);
    }
    json!(b.description.clone().filter(|d| !d.is_empty()).unwrap_or_default())
}

/// `a > b` as JavaScript compares strings (UTF-16 code units).
fn utf16_gt(a: &str, b: &str) -> bool {
    a.encode_utf16().cmp(b.encode_utf16()) == std::cmp::Ordering::Greater
}

/// The agents' broker in `ctx.ext`, for the tool loop (5.4): its delivery
/// turns and its tools reach the same queue and limits.
#[derive(Clone)]
pub struct AgentsSeam(pub Arc<AgentsHub>);

/// Track E's line in `hub/mod.rs`: the endpoints' tables, the broker,
/// its tools, routes and start (what a restart left undelivered).
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    let ext = Arc::new(external::Externals::new(ctx.data.join("generated").join("app.db"), ctx.clock.clone(), external::http_post()));
    let agents = Agents::new(ctx.clock.clone(), Arc::new(super::store::uuidv7), ext.clone());
    let hub = AgentsHub::new(agents, ext, Arc::new(run::Wired));
    ctx.ext.insert(AgentsSeam(hub.clone()));
    tools::register(ctx, hub.clone());
    super::routes::agents::install(routes, hub.clone());
    routes.on_start(move |ctx| hub.start(&ctx));
}
