//! v0 routines (routines.mjs): work the agent does on a timer, in the chat
//! it was asked in. A routine ticks every `every_s` seconds (60 at
//! least); with a `check` a tick first calls one MCP tool directly (no
//! model) and wakes the agent only for items it hasn't seen. The turn runs
//! in the routine's own chat, never the active one. A timer per routine
//! through the hub's clock, recomputed from the table at start and on
//! every change; three failures in a row back off.
//!
//!   routines(id, name, thread, every_s, enabled, check, prompt, seen,
//!            last_run, last_check, last_error, errors, created_at, template)

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};

use super::host::RoutineHost;
use super::mcp::{call_tool_json, McpView};
use super::templates::{self, Source};
use super::text::{is_email, is_obj, len16, num, num_or_null, number, round, tpl, trim};
use crate::hub::ctx::{Db, HubCtx};
use crate::hub::traits::TurnRequest;
use crate::hub::types::TurnCtx;
use crate::js;

const MIN_EVERY: f64 = 60.0;
const MAX_EVERY: f64 = 7.0 * 86400.0;
pub const SEEN_MAX: usize = 5000;
/// the longest a timer waits at once (`2 ** 31 - 1` ms)
const MAX_WAIT: f64 = 2_147_483_647.0;

const SCHEMA: &str = r#"
  CREATE TABLE IF NOT EXISTS routines(
    id TEXT PRIMARY KEY, name TEXT NOT NULL, thread TEXT NOT NULL,
    every_s INTEGER NOT NULL, enabled INTEGER DEFAULT 1, "check" TEXT, prompt TEXT,
    seen TEXT, last_run INTEGER, last_check INTEGER, last_error TEXT,
    errors INTEGER DEFAULT 0, created_at INTEGER);
"#;

/// `clampEvery(s)`: 60 s to a week, rounded.
pub fn clamp_every(s: Option<&Value>) -> f64 {
    let n = number(s);
    MIN_EVERY.max(MAX_EVERY.min(round(if n.is_nan() || n == 0.0 { 0.0 } else { n })))
}

/// A refusal: Node's thrown Error, and `need` when Rox has to ask for the
/// user's Google address.
#[derive(Debug, Clone, PartialEq)]
pub struct Fail {
    pub error: String,
    pub need: Option<&'static str>,
}

impl From<String> for Fail {
    fn from(error: String) -> Self {
        Fail { error, need: None }
    }
}

impl From<&str> for Fail {
    fn from(error: &str) -> Self {
        Fail { error: error.to_owned(), need: None }
    }
}

type Res<T> = Result<T, Fail>;

/// A `routines` row.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub thread: String,
    pub every_s: f64,
    pub enabled: bool,
    pub check: Option<String>,
    pub prompt: Option<String>,
    pub seen: Option<String>,
    pub last_run: Option<f64>,
    pub last_check: Option<f64>,
    pub last_error: Option<String>,
    pub errors: Option<f64>,
    pub created_at: Option<f64>,
    pub template: Option<String>,
}

impl Row {
    fn from_sql(r: &rusqlite::Row) -> rusqlite::Result<Row> {
        Ok(Row {
            id: r.get("id")?,
            name: r.get("name")?,
            thread: r.get("thread")?,
            every_s: r.get::<_, Option<f64>>("every_s")?.unwrap_or(0.0),
            enabled: r.get::<_, Option<f64>>("enabled")?.is_some_and(|e| e != 0.0),
            check: r.get("check")?,
            prompt: r.get("prompt")?,
            seen: r.get("seen")?,
            last_run: r.get("last_run")?,
            last_check: r.get("last_check")?,
            last_error: r.get("last_error")?,
            errors: r.get("errors")?,
            created_at: r.get("created_at")?,
            template: r.get::<_, Option<String>>("template").ok().flatten(),
        })
    }

    fn seen(&self) -> Vec<Value> {
        self.seen.as_deref().map(|s| parse(s, json!([]))).and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    fn check(&self) -> Option<Value> {
        self.check.as_deref().map(|c| parse(c, Value::Null)).filter(|c| !c.is_null())
    }
}

/// `parseJson(s, d)`: `JSON.parse(s)`, `d` when it throws.
fn parse(s: &str, d: Value) -> Value {
    serde_json::from_str(s).unwrap_or(d)
}

/// The wait before the next tick: the interval, doubled per failure past
/// the third (at most 64×, a day at most unless the interval is longer).
pub fn interval_ms(r: &Row) -> f64 {
    let base = r.every_s * 1000.0;
    match r.errors {
        Some(e) if e >= 3.0 => (base * 2f64.powf((e - 2.0).min(6.0))).min(base.max(86_400_000.0)),
        _ => base,
    }
}

fn next_run(r: &Row) -> Option<f64> {
    r.enabled.then(|| r.last_run.filter(|v| *v != 0.0).unwrap_or(0.0) + interval_ms(r))
}

// ── picking items out of a check's result ────────────────────────────────

fn hash_of(v: &Value) -> String {
    Sha1::digest(js::stringify(v).as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_owned()
}

/// `x?.[name]` of a JSON value.
fn prop(x: Option<&Value>, name: &str) -> Option<Value> {
    match x? {
        Value::Object(o) => o.get(name).cloned(),
        Value::Array(a) if name == "length" => Some(json!(a.len())),
        Value::Array(a) => name.parse::<usize>().ok().filter(|i| i.to_string() == name).and_then(|i| a.get(i).cloned()),
        Value::String(s) if name == "length" => Some(json!(len16(s))),
        Value::String(s) => {
            let i = name.parse::<usize>().ok().filter(|i| i.to_string() == name)?;
            s.encode_utf16().nth(i).map(|u| json!(String::from_utf16_lossy(&[u])))
        }
        _ => None,
    }
}

/// `walk(v, path)`: the values at a path like `messages[].id`.
fn walk(v: &Value, path: &str) -> Vec<Value> {
    let mut vals: Vec<Option<Value>> = vec![Some(v.clone())];
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        let name = seg.strip_suffix("[]").unwrap_or(seg);
        if !name.is_empty() {
            vals = vals.iter().map(|x| prop(x.as_ref(), name)).collect();
        }
        if seg.ends_with("[]") {
            vals = vals
                .into_iter()
                .flat_map(|x| match x {
                    Some(Value::Array(a)) => a.into_iter().map(Some).collect(),
                    _ => Vec::new(),
                })
                .collect();
        }
    }
    vals.into_iter().flatten().filter(|x| !x.is_null()).collect()
}

/// A JavaScript regular expression's source and flags as a fancy-regex.
fn js_regex(source: &str, flags: &str) -> Result<fancy_regex::Regex, String> {
    let inline: String = flags.chars().filter(|f| matches!(f, 'i' | 'm' | 's')).collect();
    let pattern = if inline.is_empty() { source.to_owned() } else { format!("(?{inline}){source}") };
    fancy_regex::Regex::new(&pattern).map_err(|e| format!("Invalid regular expression: /{source}/{flags}: {e}"))
}

/// `itemsOf(result, ids)`: `[(id, item)]`. `ids` is a path into the
/// result's JSON (`messages[].id`; a path ending in `[]` identifies an
/// item by its whole JSON), or a `/regex/` over a text result whose
/// matches' first group is an id.
pub fn items_of(result: &Value, ids: &str) -> Result<Vec<(String, Value)>, String> {
    let ids = trim(ids);
    if let Some(body) = ids.strip_prefix('/') {
        if let Some(at) = body.rfind('/') {
            let (source, flags) = (&body[..at], &body[at + 1..]);
            if !source.is_empty() && flags.bytes().all(|b| b.is_ascii_lowercase()) {
                let text = match result {
                    Value::String(s) => s.clone(),
                    v => js::stringify(v),
                };
                let re = js_regex(source, flags)?;
                let mut out = Vec::new();
                for caps in re.captures_iter(&text) {
                    let caps = caps.map_err(|e| e.to_string())?;
                    let id = caps.get(1).or_else(|| caps.get(0)).map(|m| m.as_str().to_owned()).unwrap_or_default();
                    out.push((id.clone(), json!({"id": id})));
                }
                return Ok(out);
            }
        }
    }
    let parsed;
    let result = match result {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s).map_err(|_| "the check's result is text, not JSON — give ids as a /regex/".to_owned())?;
            &parsed
        }
        v => v,
    };
    let (item_path, id_path) = match ids.rfind("[]") {
        None => (format!("{ids}[]"), String::new()),
        Some(cut) => (ids[..cut + 2].to_owned(), ids[cut + 2..].strip_prefix('.').unwrap_or(&ids[cut + 2..]).to_owned()),
    };
    Ok(walk(result, &item_path)
        .into_iter()
        .map(|item| {
            let id = if !id_path.is_empty() {
                walk(&item, &id_path).into_iter().next()
            } else if item.is_object() || item.is_array() {
                Some(json!(hash_of(&item)))
            } else {
                Some(item.clone())
            };
            let id = match id {
                None | Some(Value::Null) => hash_of(&item),
                Some(v) => js::string_of(&v),
            };
            (id, item)
        })
        .collect())
}

/// "$last_check" anywhere in a check's args → when the last check that
/// was fully handled started (ISO; the epoch before the first).
fn fill_args(v: &Value, last_check: Option<f64>) -> Value {
    match v {
        Value::String(s) if s == "$last_check" => json!(js::iso(last_check.filter(|v| *v != 0.0).unwrap_or(0.0))),
        Value::Array(a) => Value::Array(a.iter().map(|x| fill_args(x, last_check)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), fill_args(x, last_check))).collect()),
        v => v.clone(),
    }
}

fn or_empty(v: Option<&Value>) -> Value {
    v.filter(|v| js::truthy(v)).cloned().unwrap_or(json!({}))
}

pub struct V0 {
    pub host: Arc<dyn RoutineHost>,
    pub mcp: Arc<dyn McpView>,
    timers: Mutex<HashMap<String, u64>>,
    running: Mutex<HashSet<String>>,
}

impl V0 {
    pub fn new(db: &Db, host: Arc<dyn RoutineHost>, mcp: Arc<dyn McpView>) -> Arc<V0> {
        let _ = db.with(|db| db.execute_batch(SCHEMA));
        let _ = db.with(|db| db.execute_batch("ALTER TABLE routines ADD COLUMN template TEXT"));
        Arc::new(V0 { host, mcp, timers: Mutex::new(HashMap::new()), running: Mutex::new(HashSet::new()) })
    }

    pub fn row_of(db: &Db, id: &str) -> Option<Row> {
        db.with(|db| db.query_row("SELECT * FROM routines WHERE id=?", [id], Row::from_sql).optional()).ok().flatten()
    }

    fn sql(db: &Db, q: &str, p: &[&dyn rusqlite::ToSql]) -> Res<()> {
        db.with(|db| db.execute(q, p)).map(|_| ()).map_err(|e| Fail::from(format!("app.db: {e}")))
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).contains(id)
    }

    /// A routine as the endpoints and tools hand it out (`routineJson`).
    pub fn json(&self, r: &Row) -> Value {
        let title = self.host.thread(&r.thread).filter(|t| t.id == r.thread).map(|t| t.title).unwrap_or_default();
        json!({
            "id": r.id,
            "name": r.name,
            "thread": r.thread,
            "threadTitle": title,
            "botName": self.host.bot_name(&r.thread).unwrap_or_default(),
            "every_s": num(r.every_s),
            "enabled": r.enabled,
            "check": r.check(),
            "prompt": r.prompt.clone().unwrap_or_default(),
            "seen_count": r.seen().len(),
            "last_run": num_or_null(r.last_run.filter(|v| *v != 0.0)),
            "last_check": num_or_null(r.last_check.filter(|v| *v != 0.0)),
            "last_error": r.last_error.as_deref().filter(|e| !e.is_empty()),
            "errors": num(r.errors.unwrap_or(0.0)),
            "next_run": num_or_null(next_run(r)),
            "running": self.is_running(&r.id),
            "created_at": num_or_null(r.created_at),
            "template": r.template.as_deref().filter(|t| !t.is_empty()),
        })
    }

    pub fn list(&self, db: &Db) -> Vec<Value> {
        db.with(|db| {
            let mut st = db.prepare("SELECT * FROM routines ORDER BY created_at")?;
            let rows = st.query_map([], Row::from_sql)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .iter()
        .map(|r| self.json(r))
        .collect()
    }

    // ── the scheduler ────────────────────────────────────────────────────

    /// (Re)arms the routine's timer from its row (`arm`).
    pub fn arm(self: &Arc<Self>, ctx: &Arc<HubCtx>, id: &str) {
        if let Some(t) = self.timers.lock().unwrap_or_else(|e| e.into_inner()).remove(id) {
            ctx.clock.clear(t);
        }
        let Some(r) = Self::row_of(&ctx.db, id).filter(|r| r.enabled) else { return };
        let at = next_run(&r).unwrap_or(0.0);
        let wait = 1000f64.max((at - ctx.clock.now()).min(MAX_WAIT));
        let (me, c, key) = (self.clone(), ctx.clone(), id.to_owned());
        let timer = ctx.clock.set_timeout(
            wait as u64,
            Box::new(move || {
                me.timers.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                let Some(cur) = Self::row_of(&c.db, &key).filter(|r| r.enabled) else { return };
                // a timer capped short of a far-off tick just re-arms
                if next_run(&cur).unwrap_or(0.0) > c.clock.now() + 1000.0 {
                    return me.arm(&c, &key);
                }
                tokio::spawn(async move {
                    me.tick(&c, &key).await;
                });
            }),
        );
        self.timers.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_owned(), timer);
    }

    /// On start: a timer per enabled routine; overdue ones tick at once.
    pub fn start(self: &Arc<Self>, ctx: &Arc<HubCtx>) {
        let ids: Vec<String> = ctx
            .db
            .with(|db| {
                let mut st = db.prepare("SELECT id FROM routines WHERE enabled=1")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect()
            })
            .unwrap_or_default();
        for id in ids {
            self.arm(ctx, &id);
        }
    }

    /// A check's args for this tick: its own, with `args[into]` from
    /// `args_from`'s tool's result (`checkArgs`).
    async fn check_args(&self, check: &Value, last_check: Option<f64>) -> Result<Value, String> {
        let mut args = fill_args(&or_empty(check.get("args")), last_check);
        let Some(f) = check.get("args_from").filter(|f| js::truthy(f)) else { return Ok(args) };
        let server = f.get("server").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_default();
        if self.mcp.server(&server).is_none() {
            return Err(format!("{} isn't connected", tpl(f.get("server"))));
        }
        let tool = f.get("tool").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_default();
        let res = call_tool_json(self.mcp.as_ref(), &server, &tool, fill_args(&or_empty(f.get("args")), last_check)).await?;
        if res["ok"] != json!(true) {
            return Err(format!("{}: {}", tpl(f.get("tool")), js::slice_utf16(&res.get("error").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_else(|| "failed".to_owned()), 300)));
        }
        let result = match &res["result"] {
            Value::String(s) => parse(s, json!({})),
            v => v.clone(),
        };
        let field = f.get("field").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_default();
        let v = walk(&result, &field).into_iter().next();
        let Some(v) = v.filter(|v| *v != json!("")) else { return Err(format!("{} gave no {}", tpl(f.get("tool")), tpl(f.get("field")))) };
        let into = f.get("into").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_else(|| "query".to_owned());
        match args.as_object_mut() {
            Some(o) => {
                o.insert(into, v);
            }
            None => {
                let mut o = Map::new();
                o.insert(into, v);
                args = Value::Object(o);
            }
        }
        Ok(args)
    }

    /// One tick: the check (when there is one), then at most one agent turn.
    pub async fn tick(self: &Arc<Self>, ctx: &Arc<HubCtx>, id: &str) {
        if !self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_owned()) {
            return;
        }
        let r = Self::row_of(&ctx.db, id);
        let started = ctx.clock.now();
        if let Some(r) = r {
            if let Err(e) = self.tick_row(ctx, &r, started).await {
                let msg = js::slice_utf16(&e, 500);
                let _ = Self::sql(&ctx.db, "UPDATE routines SET last_run=?, last_error=?, errors=COALESCE(errors,0)+1 WHERE id=?", &[&(started as i64), &msg, &id]);
                eprintln!("  routine {}: {}", r.name, js::slice_utf16(&e, 200));
            }
        }
        self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        self.arm(ctx, id);
    }

    async fn tick_row(&self, ctx: &Arc<HubCtx>, r: &Row, started: f64) -> Result<(), String> {
        let db = &ctx.db;
        let started_i = started as i64;
        let Some(t) = self.host.thread(&r.thread) else {
            let _ = Self::sql(db, "UPDATE routines SET enabled=0, last_run=?, last_error=? WHERE id=?", &[&started_i, &"its chat was deleted", &r.id]);
            return Ok(());
        };
        let check = r.check();
        let mut fresh: Option<Vec<(String, Value)>> = None;
        if let Some(check) = &check {
            let server = check.get("server").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_default();
            if self.mcp.server(&server).is_none() {
                return Err(format!("{} isn't connected", tpl(check.get("server"))));
            }
            let tool = check.get("tool").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_default();
            let args = self.check_args(check, r.last_check).await?;
            let res = call_tool_json(self.mcp.as_ref(), &server, &tool, args).await?;
            if res["ok"] != json!(true) {
                return Err(format!("{}: {}", tpl(check.get("tool")), js::slice_utf16(&res.get("error").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_else(|| "failed".to_owned()), 300)));
            }
            let seen = r.seen();
            let mut by_id: Vec<(String, Value)> = Vec::new();
            let ids = check.get("ids").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_default();
            for (k, item) in items_of(&res["result"], &ids)? {
                if !seen.contains(&json!(k)) && !by_id.iter().any(|(x, _)| *x == k) {
                    by_id.push((k, item));
                }
            }
            // nothing new: no turn, no model
            if by_id.is_empty() {
                let _ = Self::sql(db, "UPDATE routines SET last_run=?, last_check=?, last_error=NULL, errors=0 WHERE id=?", &[&started_i, &started_i, &r.id]);
                return Ok(());
            }
            fresh = Some(by_id);
        }
        let items: Option<Vec<Value>> = fresh.as_ref().map(|f| f.iter().map(|(_, item)| item.clone()).collect());
        self.run_turn(ctx, r, &t.id, check.as_ref(), items.as_deref()).await?;
        let mut seen = r.seen();
        seen.extend(fresh.unwrap_or_default().into_iter().map(|(k, _)| json!(k)));
        let kept = &seen[seen.len().saturating_sub(SEEN_MAX)..];
        let last_check: Option<i64> = if check.is_some() { Some(started_i) } else { r.last_check.map(|v| v as i64) };
        Self::sql(db, "UPDATE routines SET seen=?, last_run=?, last_check=?, last_error=NULL, errors=0 WHERE id=?", &[&js::stringify(&json!(kept)), &started_i, &last_check, &r.id])
            .map_err(|e| e.error)
    }

    /// The agent turn, posted into the routine's own chat: the chat shows a
    /// one-line note; the model reads the prompt plus the new items.
    async fn run_turn(&self, ctx: &Arc<HubCtx>, r: &Row, thread: &str, check: Option<&Value>, items: Option<&[Value]>) -> Result<(), String> {
        let shown = match items {
            Some(i) => format!("Routine · {} · {} new", r.name, i.len()),
            None => format!("Routine · {}", r.name),
        };
        let mut parts = vec![
            format!("(Routine \"{}\" ({}) is running on its own, every {}s — the user isn't waiting on this message. Do what it asks and report briefly.)", r.name, r.id, js::number(r.every_s)),
            r.prompt.clone().unwrap_or_default(),
        ];
        if let (Some(items), Some(check)) = (items, check) {
            let listed = js::slice_utf16(&js::stringify_indent(&json!(items), 1), 40_000);
            parts.push(format!("New items from {} {} ({}):\n{listed}", tpl(check.get("server")), tpl(check.get("tool")), items.len()));
        }
        let prompt = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");
        let Some(runner) = ctx.turns.clone() else { return Err("no turn runner".to_owned()) };
        let turn = TurnRequest { turn: TurnCtx { thread: thread.to_owned(), routine: Some(r.id.clone()), ..TurnCtx::default() }, shown, prompt, mark: None };
        runner.run(ctx.clone(), turn).await?;
        println!("✓ routine {} → {thread}{}", r.name, items.map(|i| format!(" ({} new)", i.len())).unwrap_or_default());
        Ok(())
    }

    // ── changes (endpoints and agent tools) ──────────────────────────────

    async fn tool_of(&self, server: &str, tool: &str) -> Res<()> {
        if self.mcp.server(server).is_none() {
            return Err(format!("no connected server {server}").into());
        }
        if let Ok(tools) = self.mcp.ensure(server).await {
            if !tools.iter().any(|t| t.get("name").and_then(Value::as_str) == Some(tool)) {
                return Err(format!("{server} has no tool {tool}").into());
            }
        }
        Ok(())
    }

    async fn check_of(&self, c: Option<&Value>) -> Res<Option<Value>> {
        let Some(c) = c.filter(|c| !c.is_null()) else { return Ok(None) };
        if !(c.is_object() || c.is_array()) {
            return Err("check must be {server, tool, args, ids}".into());
        }
        let str_of = |v: Option<&Value>| v.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let (server, tool) = (str_of(c.get("server")), str_of(c.get("tool")));
        let ids = trim(&str_of(c.get("ids"))).to_owned();
        self.tool_of(&server, &tool).await?;
        if ids.is_empty() {
            return Err("check.ids is required — a path like messages[].id, or a /regex/ for a text result".into());
        }
        let args = if is_obj(c.get("args")) { c["args"].clone() } else { json!({}) };
        let Some(f) = c.get("args_from").filter(|f| !f.is_null()) else { return Ok(Some(json!({"server": server, "tool": tool, "args": args, "ids": ids}))) };
        if !f.is_object() || !js::truthy(&f["server"]) || !js::truthy(&f["tool"]) || !js::truthy(&f["field"]) {
            return Err("check.args_from must be {server, tool, args, field, into}".into());
        }
        self.tool_of(&js::string_of(&f["server"]), &js::string_of(&f["tool"])).await?;
        let into = f.get("into").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_else(|| "query".to_owned());
        let args_from = json!({
            "server": js::string_of(&f["server"]), "tool": js::string_of(&f["tool"]), "args": if is_obj(f.get("args")) { f["args"].clone() } else { json!({}) },
            "field": js::string_of(&f["field"]), "into": into,
        });
        Ok(Some(json!({"server": server, "tool": tool, "args": args, "ids": ids, "args_from": args_from})))
    }

    /// `createRoutine({name, thread, every_s, prompt, check, enabled, template})`.
    pub async fn create(self: &Arc<Self>, ctx: &Arc<HubCtx>, input: &Value, enabled: bool, template: Option<&str>) -> Res<Value> {
        let name = js::slice_utf16(trim(&input.get("name").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()), 80);
        if name.is_empty() {
            return Err("a routine needs a name".into());
        }
        let thread = input.get("thread").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let Some(t) = self.host.thread(&thread) else { return Err(format!("unknown chat {}", tpl(input.get("thread"))).into()) };
        let prompt = input.get("prompt").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let check = input.get("check").filter(|c| !c.is_null());
        if trim(&prompt).is_empty() && !check.is_some_and(js::truthy) {
            return Err("a routine needs a prompt or a check".into());
        }
        let c = self.check_of(check).await?;
        let id = uuidv7(ctx.clock.now());
        let check_text = c.as_ref().map(js::stringify);
        Self::sql(
            &ctx.db,
            r#"INSERT INTO routines(id,name,thread,every_s,enabled,"check",prompt,seen,errors,created_at,template) VALUES(?,?,?,?,?,?,?,?,0,?,?)"#,
            params![id, name, t.id, clamp_every(input.get("every_s")) as i64, i64::from(enabled), check_text, js::slice_utf16(&prompt, 8000), "[]", ctx.clock.now() as i64, template],
        )?;
        self.arm(ctx, &id);
        Self::row_of(&ctx.db, &id).map(|r| self.json(&r)).ok_or_else(|| "the routine vanished".into())
    }

    /// enabled, every_s, prompt, name: omitted ones keep their values;
    /// re-enabling clears the failure count (`updateRoutine`).
    pub fn update(self: &Arc<Self>, ctx: &Arc<HubCtx>, j: &Value) -> Res<Value> {
        let id = j.get("id").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let Some(r) = Self::row_of(&ctx.db, &id) else { return Err(format!("no routine {}", tpl(j.get("id"))).into()) };
        let db = &ctx.db;
        if let Some(e) = j.get("enabled") {
            let on = i64::from(js::truthy(e));
            Self::sql(db, "UPDATE routines SET enabled=?, errors=CASE WHEN ? THEN 0 ELSE errors END WHERE id=?", &[&on, &on, &r.id])?;
        }
        if let Some(e) = j.get("every_s") {
            Self::sql(db, "UPDATE routines SET every_s=? WHERE id=?", &[&(clamp_every(Some(e)) as i64), &r.id])?;
        }
        if let Some(p) = j.get("prompt") {
            let p = if p.is_null() { String::new() } else { js::string_of(p) };
            Self::sql(db, "UPDATE routines SET prompt=? WHERE id=?", &[&js::slice_utf16(&p, 8000), &r.id])?;
        }
        if let Some(n) = j.get("name") {
            let n = js::string_of(n);
            if !trim(&n).is_empty() {
                Self::sql(db, "UPDATE routines SET name=? WHERE id=?", &[&js::slice_utf16(trim(&n), 80), &r.id])?;
            }
        }
        self.arm(ctx, &r.id);
        Self::row_of(db, &r.id).map(|r| self.json(&r)).ok_or_else(|| "the routine vanished".into())
    }

    pub fn delete(&self, ctx: &HubCtx, id: Option<&Value>) -> Res<()> {
        let key = id.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let Some(r) = Self::row_of(&ctx.db, &key) else { return Err(format!("no routine {}", tpl(id)).into()) };
        if let Some(t) = self.timers.lock().unwrap_or_else(|e| e.into_inner()).remove(&r.id) {
            ctx.clock.clear(t);
        }
        Self::sql(&ctx.db, "DELETE FROM routines WHERE id=?", &[&r.id])
    }

    /// POST /routine/run {id}: tick now, out of schedule; done when it is.
    /// One already running is refused.
    pub async fn run(self: &Arc<Self>, ctx: &Arc<HubCtx>, id: Option<&Value>) -> Res<Value> {
        let key = id.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let Some(r) = Self::row_of(&ctx.db, &key) else { return Err(format!("no routine {}", tpl(id)).into()) };
        if self.is_running(&r.id) {
            return Err(format!("{} is already running", r.name).into());
        }
        self.tick(ctx, &r.id).await;
        Self::row_of(&ctx.db, &r.id).map(|r| self.json(&r)).ok_or_else(|| "the routine vanished".into())
    }

    /// POST /routine/from-template {server, id, thread, user_google_email?}
    /// → the routine (`routineFromTemplate`): the template's check and
    /// prompt, in that chat; the same template in the same chat again is
    /// the routine it already has.
    pub async fn from_template(self: &Arc<Self>, ctx: &Arc<HubCtx>, j: &Value) -> Res<Value> {
        let db = &ctx.db;
        let host = self.host.as_ref();
        let thread = j.get("thread").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let Some(t) = host.thread(&thread) else {
            let shown = j.get("thread").filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
            return Err(format!("unknown chat {shown}").into());
        };
        let key = format!("{}/{}", tpl(j.get("server")), tpl(j.get("id")));
        let had = db.with(|db| db.query_row("SELECT * FROM routines WHERE template=? AND thread=?", [&key, &t.id], Row::from_sql).optional()).ok().flatten();
        if let Some(had) = had {
            return Ok(self.json(&had));
        }
        let server = j.get("server").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let Some(row) = self.mcp.server(&server) else { return Err(format!("no connected server {}", tpl(j.get("server"))).into()) };
        let manifest = self.mcp.manifest(&row.id).await;
        let Some(tpl_) = templates::templates_of(&manifest).into_iter().find(|x| j.get("id").and_then(Value::as_str) == Some(x.id.as_str())) else {
            return Err(format!("{} has no routine template {}", tpl(j.get("server")), tpl(j.get("id"))).into());
        };
        if let Some(e) = j.get("user_google_email") {
            if !is_email(Some(e)) {
                return Err(format!("{} isn't an email address", js::string_of(e)).into());
            }
            templates::meta_set(db, "google_email", trim(&js::string_of(e)));
        }
        let source = if tpl_.needs.as_deref() == Some("gmail") { Some(templates::gmail_source(self.mcp.as_ref()).await) } else { None };
        let reason = templates::unavailable(&tpl_, source.as_ref());
        if !reason.is_empty() {
            return Err(reason.into());
        }
        let (mut check, mut prompt) = (Value::Null, tpl_.prompt.clone());
        if let Some(Source::Found(grow, a)) = &source {
            if templates::needs_email(db, host, source.as_ref()) {
                return Err(Fail { error: "Buwiz Bot needs your Google address for Gmail searches".to_owned(), need: Some("user_google_email") });
            }
            (check, prompt) = templates::gmail_check(db, host, self.mcp.as_ref(), &row.id, &tpl_, grow, a).await;
        }
        let input = json!({"name": tpl_.name, "thread": t.id, "every_s": tpl_.every_s, "prompt": prompt, "check": check});
        self.create(ctx, &input, true, Some(&key)).await
    }
}

/// A UUID v7 (state.mjs's `uuidv7`): the time in ms, then random bits,
/// counting up within one millisecond.
pub fn uuidv7(now: f64) -> String {
    static LAST: Mutex<(u64, u16)> = Mutex::new((0, 0));
    let mut b = buwiz_protocol::keys::random::<16>();
    let mut ms = now.max(0.0).floor() as u64;
    let ctr = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let mut ctr = (u16::from(b[6] & 0x0f) << 8) | u16::from(b[7]);
        if ms > last.0 {
            ctr &= 0x7ff;
            *last = (ms, ctr);
        } else {
            if last.1 >= 0xfff {
                last.0 += 1;
                last.1 = 0;
            } else {
                last.1 += 1;
            }
            ms = last.0;
            ctr = last.1;
        }
        ctr
    };
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = 0x70 | (ctr >> 8) as u8;
    b[7] = (ctr & 0xff) as u8;
    b[8] = 0x80 | (b[8] & 0x3f);
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}
