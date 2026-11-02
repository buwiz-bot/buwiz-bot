//! v1 plugin routines (`v1/routines.mjs`, spec 19.1 "A run",
//! 19.2 "params"; `v1/broker.mjs`'s `routineTurn`).
//!
//! A routine is declared in a plugin's manifest; what runs is an instance
//! of it (`v1_instances`): plugin, routine, instance id, the chat it
//! reports to, params and trigger, each with its own thread, seen ids,
//! last run and on/off. The user's approval of what a routine reads is per
//! routine, stored as the exact `reads` it covered (`v1_routines`).
//!
//! An instance runs every `every_s` (a poll every 15 s finds the due ones)
//! or on its webhook. A run calls the routine's own query tool (origin
//! `routine`), hands the text to the first `reads` tool as `{query}` (or
//! uses its own result), and runs one agent turn per unseen item in the
//! instance's own thread; the chat it reports to gets one notice per run
//! that ran items, and one per failure streak. Seen ids are the last 1000.
//! Consent stays with the user: the agent never switches on an instance
//! whose routine writes or reads what is not approved; a card asks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use super::host::{RoutineHost, Thread};
use super::plugins::{self as pv, PluginView};
use super::text::{self, approved_text, len16, nonempty, num, num_or_null, params_problem, params_summary, tpl};
use super::threads;
use crate::hub::ctx::{Db, HubCtx};
use crate::hub::traits::TurnRequest;
use crate::hub::types::{ToolResult, TurnCtx};
use crate::js;
use crate::v1::V1Error;

pub const SEEN_MAX: usize = 1000;
pub const POLL_MS: u64 = 15_000;
const ITEM_MAX: usize = 40_000;
/// a webhook: at most HOOK_RATE calls a minute per instance, HOOK_ITEMS items
pub const HOOK_RATE: usize = 10;
const HOOK_ITEMS: usize = 100;
const EVERY_MIN: f64 = 60.0;
const EVERY_MAX: f64 = 86_400.0;

const SCHEMA: &str = "
  CREATE TABLE IF NOT EXISTS v1_routines(
    plugin TEXT NOT NULL, routine TEXT NOT NULL, thread TEXT, enabled INTEGER NOT NULL DEFAULT 0,
    approved_reads TEXT, seen TEXT NOT NULL DEFAULT '[]', last_run INTEGER, last_error TEXT,
    errors INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(plugin, routine));
  CREATE TABLE IF NOT EXISTS v1_instances(
    id TEXT PRIMARY KEY, plugin TEXT NOT NULL, routine TEXT NOT NULL, thread TEXT NOT NULL,
    label TEXT, params TEXT NOT NULL DEFAULT '{}', trigger TEXT NOT NULL DEFAULT 'schedule', every_s INTEGER,
    enabled INTEGER NOT NULL DEFAULT 0, pending INTEGER NOT NULL DEFAULT 0,
    seen TEXT NOT NULL DEFAULT '[]', last_run INTEGER, last_error TEXT, errors INTEGER NOT NULL DEFAULT 0,
    last_items INTEGER, hook_hash TEXT, created_at INTEGER NOT NULL);
";
/// how many new items the last run that went through ran (0: nothing new)
const LAST_ITEMS: &str = "ALTER TABLE v1_routines ADD COLUMN last_items INTEGER";

type Res<T = Value> = Result<T, V1Error>;

fn refuse<T>(code: &str, message: impl Into<String>) -> Res<T> {
    V1Error::err(code, message)
}

/// A `v1_instances` row.
#[derive(Clone, Debug)]
pub struct Inst {
    pub id: String,
    pub plugin: String,
    pub routine: String,
    pub thread: String,
    pub label: Option<String>,
    pub params: String,
    pub trigger: String,
    pub every_s: Option<f64>,
    pub enabled: bool,
    pub pending: bool,
    pub seen: String,
    pub last_run: Option<f64>,
    pub last_error: Option<String>,
    pub errors: f64,
    pub last_items: Option<f64>,
    pub hook_hash: Option<String>,
}

impl Inst {
    fn from_sql(r: &rusqlite::Row) -> rusqlite::Result<Inst> {
        Ok(Inst {
            id: r.get("id")?,
            plugin: r.get("plugin")?,
            routine: r.get("routine")?,
            thread: r.get("thread")?,
            label: r.get("label")?,
            params: r.get("params")?,
            trigger: r.get("trigger")?,
            every_s: r.get("every_s")?,
            enabled: r.get::<_, Option<f64>>("enabled")?.unwrap_or(0.0) != 0.0,
            pending: r.get::<_, Option<f64>>("pending")?.unwrap_or(0.0) != 0.0,
            seen: r.get("seen")?,
            last_run: r.get("last_run")?,
            last_error: r.get("last_error")?,
            errors: r.get::<_, Option<f64>>("errors")?.unwrap_or(0.0),
            last_items: r.get("last_items")?,
            hook_hash: r.get("hook_hash")?,
        })
    }

    fn params(&self) -> Value {
        pv::parse(&self.params, json!({}))
    }

    fn seen(&self) -> Vec<Value> {
        pv::parse(&self.seen, json!([])).as_array().cloned().unwrap_or_default()
    }
}

/// params, trigger, every_s and label as given and checked (`fields`).
#[derive(Default)]
struct Fields {
    params: Option<String>,
    trigger: Option<String>,
    every_s: Option<f64>,
    label: Option<Option<String>>,
}

/// One run in progress (`run` in `tick`).
struct Run {
    id: String,
    name: String,
    items: usize,
    home: Option<Thread>,
    seq: Option<usize>,
    turn: Option<String>,
}

/// An instance the agent named: one, or several when ambiguous.
pub enum Found {
    One(Value),
    Ambiguous(Vec<Value>),
}

// ── a routine's manifest entry ───────────────────────────────────────────

fn s<'a>(r: &'a Value, k: &str) -> &'a str {
    r.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn rid(r: &Value) -> &str {
    s(r, "id")
}

fn list<'a>(r: &'a Value, k: &str) -> Vec<&'a Value> {
    r.get(k).and_then(Value::as_array).map(|l| l.iter().collect()).unwrap_or_default()
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).into_iter().flatten().map(js::string_of).collect()
}

fn query<'a>(r: &'a Value, k: &str) -> Option<&'a Value> {
    r.get("query").and_then(|q| q.get(k))
}

fn query_tool(r: &Value) -> String {
    tpl(query(r, "tool"))
}

/// `JSON.stringify(r.reads || [])`.
fn reads_key(r: &Value) -> String {
    js::stringify(r.get("reads").filter(|v| js::truthy(v)).unwrap_or(&json!([])))
}

fn first_text(r: &Value) -> String {
    r.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|c| c.get("text"))
        .filter(|t| js::truthy(t))
        .map(js::string_of)
        .unwrap_or_default()
}

fn sha_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn new_id() -> String {
    format!("ri_{}", buwiz_protocol::keys::b64(&buwiz_protocol::keys::random::<9>()))
}

/// `/^ri_[A-Za-z0-9_-]{6,40}$/`.
pub fn is_instance_ref(s: &str) -> bool {
    s.strip_prefix("ri_").is_some_and(|t| (6..=40).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}

fn dedupe(items: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    items.iter().filter(|x| nonempty(x.get("id")).is_some() && x.is_object() && seen.insert(s(x, "id").to_owned())).cloned().collect()
}

/// the shown line of an item's turn: its subject or title when it has one
fn shown_of(item: &Value) -> String {
    let head = ["subject", "title", "name"].iter().filter_map(|k| item.get(*k).and_then(Value::as_str)).find(|v| !text::trim(v).is_empty());
    match head {
        Some(h) => {
            let squeezed: String = {
                let mut out = String::new();
                let mut space = false;
                for c in h.chars() {
                    if js::is_space(c) {
                        if !space {
                            out.push(' ');
                        }
                        space = true;
                    } else {
                        out.push(c);
                        space = false;
                    }
                }
                out
            };
            format!("New item · {}", approved_text(&squeezed, 120))
        }
        None => "New item".to_owned(),
    }
}

pub struct V1 {
    pub plugins: Arc<dyn PluginView>,
    pub host: Arc<dyn RoutineHost>,
    running: Mutex<HashSet<String>>,
    hook_calls: Mutex<HashMap<String, Vec<f64>>>,
    timer: Mutex<Option<u64>>,
    pub poll_ms: u64,
}

impl V1 {
    /// The runner on app.db: its tables, and the routines enabled before
    /// instances existed moved to one instance each (once).
    pub fn new(ctx: &HubCtx, plugins: Arc<dyn PluginView>, host: Arc<dyn RoutineHost>) -> Arc<V1> {
        let v1 = Arc::new(V1 { plugins, host, running: Mutex::new(HashSet::new()), hook_calls: Mutex::new(HashMap::new()), timer: Mutex::new(None), poll_ms: POLL_MS });
        let _ = ctx.db.with(|db| db.execute_batch(SCHEMA));
        let _ = ctx.db.with(|db| db.execute_batch(LAST_ITEMS));
        v1.move_singles(&ctx.db, ctx.clock.now());
        v1
    }

    fn move_singles(&self, db: &Db, now: f64) {
        type Single = (String, String, String, f64, String, Option<f64>, Option<String>, f64, Option<f64>);
        let rows: Vec<Single> = db
            .with(|db| {
                let mut st = db.prepare("SELECT * FROM v1_routines WHERE thread IS NOT NULL")?;
                let rows = st.query_map([], |r| {
                    Ok((
                        r.get("plugin")?,
                        r.get("routine")?,
                        r.get("thread")?,
                        r.get::<_, Option<f64>>("enabled")?.unwrap_or(0.0),
                        r.get::<_, Option<String>>("seen")?.filter(|s| !s.is_empty()).unwrap_or_else(|| "[]".to_owned()),
                        r.get("last_run")?,
                        r.get("last_error")?,
                        r.get::<_, Option<f64>>("errors")?.unwrap_or(0.0),
                        r.get("last_items")?,
                    ))
                })?;
                rows.collect()
            })
            .unwrap_or_default();
        for (plugin, routine, thread, enabled, seen, last_run, last_error, errors, last_items) in rows {
            let had = db.with(|db| db.query_row("SELECT 1 FROM v1_instances WHERE plugin=? AND routine=?", [&plugin, &routine], |_| Ok(())).optional()).ok().flatten().is_some();
            if had {
                continue;
            }
            let id = new_id();
            let made = db.with(|db| {
                db.execute(
                    "INSERT INTO v1_instances(id, plugin, routine, thread, enabled, seen, last_run, last_error, errors, last_items, created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                    params![id, plugin, routine, thread, i64::from(enabled != 0.0), seen, last_run.map(|v| v as i64), last_error, errors as i64, last_items.map(|v| v as i64), now as i64],
                )
            });
            if made.is_ok() {
                threads::adopt(self.host.as_ref(), &plugin, &routine, &id);
            }
        }
    }

    // ── reads ────────────────────────────────────────────────────────────

    fn sql<T>(db: &Db, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Res<T> {
        db.with(f).map_err(|e| V1Error::new("internal", format!("app.db: {e}")))
    }

    pub fn row_of(db: &Db, id: &str) -> Option<Inst> {
        db.with(|db| db.query_row("SELECT * FROM v1_instances WHERE id=?", [id], Inst::from_sql).optional()).ok().flatten()
    }

    fn instances_of(db: &Db, plugin: &str, routine: &str) -> Vec<Inst> {
        db.with(|db| {
            let mut st = db.prepare("SELECT * FROM v1_instances WHERE plugin=? AND routine=? ORDER BY created_at, rowid")?;
            let rows = st.query_map([plugin, routine], Inst::from_sql)?;
            rows.collect()
        })
        .unwrap_or_default()
    }

    fn all_instances(db: &Db, sql: &str) -> Vec<Inst> {
        db.with(|db| {
            let mut st = db.prepare(sql)?;
            let rows = st.query_map([], Inst::from_sql)?;
            rows.collect()
        })
        .unwrap_or_default()
    }

    /// `approvalOf`: the routine's `approved_reads`, `None` without a row.
    fn approval_of(db: &Db, plugin: &str, routine: &str) -> Option<Option<String>> {
        db.with(|db| db.query_row("SELECT approved_reads FROM v1_routines WHERE plugin=? AND routine=?", [plugin, routine], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten()
    }

    /// `routineOf(plugin, id)`: the manifest's routine.
    pub fn routine_of(&self, plugin: &str, id: &str) -> Option<Value> {
        let row = self.plugins.row(plugin)?;
        pv::routines_of(&row).into_iter().find(|r| r.get("id").and_then(Value::as_str) == Some(id))
    }

    fn title_of(&self, plugin: &str) -> String {
        pv::title_of(self.plugins.as_ref(), plugin)
    }

    fn must_routine(&self, body: &Value) -> Res<Value> {
        let (Some(plugin), Some(routine)) = (body.get("plugin").and_then(Value::as_str), body.get("routine").and_then(Value::as_str)) else {
            return refuse("invalid_args", "send {plugin, routine}");
        };
        if !body.is_object() {
            return refuse("invalid_args", "send {plugin, routine}");
        }
        match self.routine_of(plugin, routine) {
            Some(r) => Ok(r),
            None => refuse("not_found", format!("{} has no routine {}", js::slice_utf16(plugin, 80), js::slice_utf16(routine, 80))),
        }
    }

    /// a routine without reads has nothing to approve
    fn approved(r: &Value, ap: Option<&Option<String>>) -> bool {
        list(r, "reads").is_empty() || ap.and_then(|a| a.as_deref()) == Some(reads_key(r).as_str())
    }

    /// the user's consent an instance needs before it runs
    fn needs_consent(&self, db: &Db, plugin: &str, r: &Value) -> bool {
        !list(r, "writes").is_empty() || !Self::approved(r, Self::approval_of(db, plugin, rid(r)).as_ref())
    }

    /// "" when the routine may run now, else why not (`unavailable`).
    pub fn unavailable(&self, db: &Db, plugin: &str, r: &Value) -> String {
        let p = self.plugins.as_ref();
        let Some(own) = p.row(plugin).filter(|o| o.enabled) else { return format!("{plugin} is disabled") };
        let state = p.runtime(plugin).state;
        if state != "running" {
            return format!("{plugin} is {state}");
        }
        if pv::routine_prompt(p, plugin, rid(r)).is_none() {
            return format!("{plugin} is not an approved plugin");
        }
        let qt = query_tool(r);
        if pv::disabled_of(&own).contains(&qt) {
            return format!("{qt} is switched off");
        }
        for st in pv::reads_status(p, r) {
            if st.get("allowed") != Some(&Value::Bool(true)) {
                return js::string_of(st.get("reason").unwrap_or(&Value::Null));
            }
        }
        for x in list(r, "reads") {
            let xp = s(x, "plugin");
            let state = p.runtime(xp).state;
            if state != "running" {
                return format!("{xp} is {state}");
            }
            let off = p.row(xp).map(|row| pv::disabled_of(&row)).unwrap_or_default();
            if let Some(t) = strings(x.get("tools")).into_iter().find(|t| off.contains(t)) {
                return format!("{t} is switched off");
            }
        }
        let ap = Self::approval_of(db, plugin, rid(r));
        if !Self::approved(r, ap.as_ref()) {
            return if ap.flatten().is_some_and(|a| !a.is_empty()) { "what it reads changed in an update; approve it again".to_owned() } else { "you have not approved what it reads".to_owned() };
        }
        String::new()
    }

    fn every_of(r: &Value, row: &Inst) -> Value {
        match row.every_s.filter(|e| *e != 0.0) {
            Some(e) => num(e),
            None => r.get("every_s").cloned().unwrap_or(Value::Null),
        }
    }

    /// the wait after a run: the interval, doubled per failure past the third
    fn interval_ms(r: &Value, row: &Inst) -> f64 {
        let base = Self::every_of(r, row).as_f64().unwrap_or(f64::NAN) * 1000.0;
        if row.errors >= 3.0 {
            (base * 2f64.powf((row.errors - 2.0).min(6.0))).min(base.max(86_400_000.0))
        } else {
            base
        }
    }

    fn summary_of(r: &Value, row: &Inst) -> String {
        match row.label.as_deref().filter(|l| !l.is_empty()) {
            Some(l) => l.to_owned(),
            None => params_summary(r.get("params"), &row.params()),
        }
    }

    pub fn name_of(r: &Value, row: &Inst) -> String {
        let sum = Self::summary_of(r, row);
        let name = tpl(r.get("name"));
        approved_text(&if sum.is_empty() { name } else { format!("{name} · {sum}") }, 120)
    }

    fn is_running(&self, id: &str) -> bool {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).contains(id)
    }

    /// One instance as the user and the agent see it (`instanceInfo`).
    pub fn instance_info(&self, db: &Db, r: &Value, row: &Inst) -> Value {
        let reason = if self.plugins.row(&row.plugin).is_some() { self.unavailable(db, &row.plugin, r) } else { format!("{} is not installed", row.plugin) };
        let home = threads::find(self.host.as_ref(), &row.plugin, rid(r), Some(&row.id));
        let next = if row.enabled && row.trigger == "schedule" { num(row.last_run.filter(|v| *v != 0.0).unwrap_or(0.0) + Self::interval_ms(r, row)) } else { Value::Null };
        json!({
            "instance": row.id,
            "label": row.label.as_deref().filter(|l| !l.is_empty()),
            "params": row.params(),
            "summary": Self::summary_of(r, row),
            "trigger": row.trigger,
            "every_s": Self::every_of(r, row),
            "enabled": row.enabled,
            "pending": row.pending,
            "thread": row.thread,
            "home": home.as_ref().map(|h| h.id.clone()),
            "home_uid": home.as_ref().map(|h| h.uid.clone()),
            "available": reason.is_empty(),
            "reason": if reason.is_empty() { Value::Null } else { json!(reason) },
            "seen_count": row.seen().len(),
            "last_run": num_or_null(row.last_run),
            "last_error": row.last_error,
            "last_items": num_or_null(row.last_items),
            "next_run": next,
            "running": self.is_running(&row.id),
            "hook": row.hook_hash.as_deref().is_some_and(|h| !h.is_empty()),
        })
    }

    /// The user's side of a routine, for GET /v1/host/plugins: its
    /// instances, and the first one's state at the top (`info`).
    pub fn info(&self, db: &Db, plugin: &str, r: &Value) -> Value {
        let ap = Self::approval_of(db, plugin, rid(r));
        let reason = if self.plugins.row(plugin).is_some() { self.unavailable(db, plugin, r) } else { format!("{plugin} is not installed") };
        let instances: Vec<Value> = Self::instances_of(db, plugin, rid(r)).iter().map(|row| self.instance_info(db, r, row)).collect();
        let first = instances.iter().find(|x| x["enabled"] == json!(true)).or(instances.first());
        let f = |k: &str, d: Value| first.map(|x| x[k].clone()).filter(|v| !v.is_null()).unwrap_or(d);
        json!({
            "enabled": instances.iter().any(|x| x["enabled"] == json!(true)),
            "thread": f("thread", Value::Null),
            "home": f("home", Value::Null),
            "home_uid": f("home_uid", Value::Null),
            "approved": Self::approved(r, ap.as_ref()),
            "available": reason.is_empty(),
            "reason": if reason.is_empty() { Value::Null } else { json!(reason) },
            "seen_count": f("seen_count", json!(0)),
            "last_run": f("last_run", Value::Null),
            "last_error": f("last_error", Value::Null),
            "last_items": f("last_items", Value::Null),
            "next_run": f("next_run", Value::Null),
            "running": instances.iter().any(|x| x["running"] == json!(true)),
            "instances": instances,
        })
    }

    /// The plugin list (GET /v1/host/plugins): every record described,
    /// each routine's state merged.
    pub fn list(&self, db: &Db) -> Vec<Value> {
        let p = self.plugins.as_ref();
        p.rows().iter().map(|row| pv::describe(p, row, &|plugin, r| self.info(db, plugin, r))).collect()
    }

    /// the instance's own thread, made when it is enabled in a chat that exists
    fn home_of(&self, r: &Value, row: &Inst) -> Option<Thread> {
        if !row.enabled || row.thread.is_empty() || self.host.thread(&row.thread).is_none() {
            return None;
        }
        Some(threads::ensure(self.host.as_ref(), &row.plugin, rid(r), Some(&row.id), &Self::name_of(r, row), &row.thread))
    }

    fn upsert(db: &Db, plugin: &str, id: &str) -> Res<()> {
        Self::sql(db, |db| db.execute("INSERT OR IGNORE INTO v1_routines(plugin, routine) VALUES(?, ?)", [plugin, id])).map(|_| ())
    }

    fn set_approved(db: &Db, plugin: &str, r: &Value) -> Res<()> {
        Self::upsert(db, plugin, rid(r))?;
        Self::sql(db, |db| db.execute("UPDATE v1_routines SET approved_reads=? WHERE plugin=? AND routine=?", [&reads_key(r), plugin, rid(r)])).map(|_| ())
    }

    fn answer(&self, db: &Db, plugin: &str, r: &Value, row: Option<&Inst>) -> Value {
        let mut routine = Map::new();
        routine.insert("id".into(), json!(rid(r)));
        if let Value::Object(info) = self.info(db, plugin, r) {
            routine.extend(info);
        }
        let mut out = json!({"ok": true, "routine": routine});
        if let Some(row) = row {
            out["instance"] = match Self::row_of(db, &row.id) {
                Some(fresh) => self.instance_info(db, r, &fresh),
                None => Value::Null,
            };
        }
        out
    }

    /// {plugin, routine, instance?, thread?} → the instance meant (`resolve`).
    fn resolve(&self, db: &Db, body: &Value, r: &Value, thread: Option<&Value>) -> Res<Option<Inst>> {
        if let Some(inst) = body.get("instance") {
            let row = inst.as_str().and_then(|i| Self::row_of(db, i));
            return match row {
                Some(row) if body.get("plugin").and_then(Value::as_str) == Some(row.plugin.as_str()) && row.routine == rid(r) => Ok(Some(row)),
                _ => refuse("not_found", format!("no instance {} of {}", js::slice_utf16(&js::string_of(inst), 60), rid(r))),
            };
        }
        let all = Self::instances_of(db, s(body, "plugin"), rid(r));
        if let Some(t) = nonempty(thread) {
            return Ok(all.into_iter().find(|x| x.thread == t));
        }
        if all.len() > 1 {
            return refuse("invalid_args", format!("{} has {} instances; name one (instance)", rid(r), all.len()));
        }
        Ok(all.into_iter().next())
    }

    fn fields(r: &Value, b: &Value, was: bool) -> Res<Fields> {
        let mut out = Fields::default();
        if let Some(p) = b.get("params") {
            let why = params_problem(r.get("params"), Some(p));
            if !why.is_empty() {
                return refuse("invalid_args", why);
            }
            out.params = Some(js::stringify(p));
        } else if !was {
            let why = params_problem(r.get("params"), Some(&json!({})));
            if !why.is_empty() {
                return refuse("invalid_args", format!("{} needs params: {why}", rid(r)));
            }
        }
        if let Some(t) = b.get("trigger") {
            match t.as_str() {
                Some(t @ ("schedule" | "webhook")) => out.trigger = Some(t.to_owned()),
                _ => return refuse("invalid_args", "trigger is \"schedule\" or \"webhook\""),
            }
        }
        if let Some(e) = b.get("every_s").filter(|e| !e.is_null()) {
            let n = e.as_f64().filter(|n| n.is_finite() && n.fract() == 0.0);
            match n {
                Some(n) if (EVERY_MIN..=EVERY_MAX).contains(&n) => out.every_s = Some(n),
                _ => return refuse("invalid_args", format!("every_s is an integer from {EVERY_MIN} to {EVERY_MAX}")),
            }
        }
        if let Some(l) = b.get("label").filter(|l| !l.is_null()) {
            let Some(l) = l.as_str() else { return refuse("invalid_args", "label must be a string") };
            let l = approved_text(l, 60);
            out.label = Some(if l.is_empty() { None } else { Some(l) });
        }
        Ok(out)
    }

    fn create(db: &Db, now: f64, plugin: &str, r: &Value, thread: &str, f: &Fields, enabled: bool, pending: bool) -> Res<Inst> {
        let id = new_id();
        Self::sql(db, |db| {
            db.execute(
                "INSERT INTO v1_instances(id, plugin, routine, thread, label, params, trigger, every_s, enabled, pending, created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    id,
                    plugin,
                    rid(r),
                    thread,
                    f.label.clone().flatten(),
                    f.params.clone().unwrap_or_else(|| "{}".to_owned()),
                    f.trigger.clone().unwrap_or_else(|| "schedule".to_owned()),
                    f.every_s.map(|e| e as i64),
                    i64::from(enabled),
                    i64::from(pending),
                    now as i64
                ],
            )
        })?;
        Self::row_of(db, &id).ok_or_else(|| V1Error::new("internal", "the instance vanished"))
    }

    fn patch(db: &Db, id: &str, f: &Fields) -> Res<()> {
        Self::sql(db, |db| {
            if let Some(p) = &f.params {
                db.execute("UPDATE v1_instances SET params=? WHERE id=?", params![p, id])?;
            }
            if let Some(t) = &f.trigger {
                db.execute("UPDATE v1_instances SET trigger=? WHERE id=?", params![t, id])?;
            }
            if let Some(e) = f.every_s {
                db.execute("UPDATE v1_instances SET every_s=? WHERE id=?", params![e as i64, id])?;
            }
            if let Some(l) = &f.label {
                db.execute("UPDATE v1_instances SET label=? WHERE id=?", params![l, id])?;
            }
            Ok(())
        })
    }

    fn set_cols(db: &Db, id: &str, sets: &str, vals: &[&dyn rusqlite::ToSql]) -> Res<()> {
        let sql = format!("UPDATE v1_instances SET {sets} WHERE id=?");
        let mut all: Vec<&dyn rusqlite::ToSql> = vals.to_vec();
        all.push(&id);
        Self::sql(db, |db| db.execute(&sql, all.as_slice())).map(|_| ())
    }

    fn settle(&self, row: &Inst, state: &str) {
        self.host.settle_consent(&row.thread, &row.id, state);
    }

    // ── the host routes ──────────────────────────────────────────────────

    /// POST /v1/host/routines/approve {plugin, routine}.
    pub fn approve(&self, ctx: &HubCtx, body: &Value) -> Res {
        let r = self.must_routine(body)?;
        Self::set_approved(&ctx.db, s(body, "plugin"), &r)?;
        Ok(self.answer(&ctx.db, s(body, "plugin"), &r, None))
    }

    /// POST /v1/host/routines/enable {plugin, routine, instance?, thread?,
    /// params?, every_s?, trigger?, label?, approve?}.
    pub fn enable(&self, ctx: &HubCtx, body: &Value) -> Res {
        let r = self.must_routine(body)?;
        let db = &ctx.db;
        let plugin = s(body, "plugin");
        let named = body.get("instance").is_some_and(js::truthy);
        let row = self.resolve(db, body, &r, if named { None } else { body.get("thread") })?;
        let thread = nonempty(body.get("thread")).map(str::to_owned).or_else(|| row.as_ref().map(|x| x.thread.clone())).unwrap_or_default();
        if thread.is_empty() || self.host.thread(&thread).is_none() {
            return refuse("invalid_args", "thread must name an existing chat");
        }
        let f = Self::fields(&r, body, row.is_some())?;
        if body.get("approve") == Some(&Value::Bool(true)) {
            Self::set_approved(db, plugin, &r)?;
        }
        Self::upsert(db, plugin, rid(&r))?;
        let row = match row {
            None => Self::create(db, ctx.clock.now(), plugin, &r, &thread, &f, true, false)?,
            Some(row) => {
                Self::patch(db, &row.id, &f)?;
                Self::set_cols(db, &row.id, "thread=?, enabled=1, pending=0, errors=0", &[&thread])?;
                Self::row_of(db, &row.id).unwrap_or(row)
            }
        };
        self.settle(&row, "enabled");
        self.home_of(&r, &row);
        Ok(self.answer(db, plugin, &r, Some(&row)))
    }

    /// POST /v1/host/routines/disable {plugin, routine, instance?}.
    pub fn disable(&self, ctx: &HubCtx, body: &Value) -> Res {
        let r = self.must_routine(body)?;
        let db = &ctx.db;
        let one = body.get("instance").is_some();
        let rows: Vec<Inst> = if one { self.resolve(db, body, &r, body.get("thread"))?.into_iter().collect() } else { Self::instances_of(db, s(body, "plugin"), rid(&r)) };
        for row in &rows {
            Self::set_cols(db, &row.id, "enabled=0", &[])?;
        }
        Ok(self.answer(db, s(body, "plugin"), &r, if one { rows.first() } else { None }))
    }

    /// POST /v1/host/routines/update {plugin, routine, instance, params?,
    /// every_s?, trigger?, label?}: it stays on or off.
    pub fn update(&self, ctx: &HubCtx, body: &Value) -> Res {
        let r = self.must_routine(body)?;
        let db = &ctx.db;
        let Some(row) = self.resolve(db, body, &r, body.get("thread"))? else { return refuse("not_found", format!("{} has no instance", rid(&r))) };
        Self::patch(db, &row.id, &Self::fields(&r, body, true)?)?;
        if row.enabled {
            if let Some(fresh) = Self::row_of(db, &row.id) {
                self.home_of(&r, &fresh);
            }
        }
        Ok(self.answer(db, s(body, "plugin"), &r, Some(&row)))
    }

    /// POST /v1/host/routines/delete {plugin, routine, instance}: its
    /// thread stays; a pending one's card reads as declined.
    pub fn remove(&self, ctx: &HubCtx, body: &Value) -> Res {
        let r = self.must_routine(body)?;
        let db = &ctx.db;
        let Some(row) = self.resolve(db, body, &r, body.get("thread"))? else { return refuse("not_found", format!("{} has no instance", rid(&r))) };
        Self::sql(db, |db| db.execute("DELETE FROM v1_instances WHERE id=?", [&row.id]))?;
        if row.pending {
            self.settle(&row, "declined");
        }
        Ok(self.answer(db, s(body, "plugin"), &r, None))
    }

    /// POST /v1/host/routines/hook {plugin, routine, instance}: a new
    /// secret for the instance's webhook; only its hash is kept.
    pub fn hook(&self, ctx: &HubCtx, body: &Value) -> Res {
        let r = self.must_routine(body)?;
        let db = &ctx.db;
        let Some(row) = self.resolve(db, body, &r, body.get("thread"))? else { return refuse("not_found", format!("{} has no instance", rid(&r))) };
        if row.trigger != "webhook" {
            return refuse("invalid_args", "its trigger is a schedule; set trigger to webhook first");
        }
        let secret = buwiz_protocol::keys::b64(&buwiz_protocol::keys::random::<24>());
        Self::set_cols(db, &row.id, "hook_hash=?", &[&sha_hex(&secret)])?;
        let mut out = self.answer(db, s(body, "plugin"), &r, Some(&row));
        out["path"] = json!(format!("/v1/hooks/{}/{secret}", row.id));
        Ok(out)
    }

    /// POST /v1/host/routines/run {plugin, routine, instance?, reread?,
    /// wait?}: one run now, out of schedule.
    pub async fn run(self: &Arc<Self>, ctx: &Arc<HubCtx>, body: &Value) -> Res {
        let r = self.must_routine(body)?;
        let Some(row) = self.resolve(&ctx.db, body, &r, body.get("thread"))? else {
            return refuse("disabled", format!("{} is not enabled in any chat", approved_text(&tpl(r.get("name")), 80)));
        };
        let done = self.launch(ctx, &r, &row, body.get("reread") == Some(&Value::Bool(true)), None)?;
        let ran = match body.get("wait") == Some(&Value::Bool(true)) {
            true => Some(done.await.unwrap_or_else(|_| json!({"ran": false, "error": "the run was dropped"}))),
            false => None,
        };
        let mut out = self.answer(&ctx.db, s(body, "plugin"), &r, Some(&row));
        if let Some(ran) = ran {
            out["run"] = ran;
        }
        Ok(out)
    }

    /// `runInstance`: refused when paused, running, its chat is gone or
    /// it can't run; else its tick starts (claimed at once), its result on
    /// the channel.
    fn launch(self: &Arc<Self>, ctx: &Arc<HubCtx>, r: &Value, row: &Inst, reread: bool, items: Option<Vec<Value>>) -> Res<oneshot::Receiver<Value>> {
        let name = Self::name_of(r, row);
        if !row.enabled {
            return refuse("disabled", if row.pending { format!("{name} waits for the user's Enable") } else { format!("{name} is paused; resume it first") });
        }
        if self.is_running(&row.id) {
            return refuse("rate_limited", format!("{name} is already running"));
        }
        if self.host.thread(&row.thread).is_none() {
            return refuse("not_found", format!("{name}'s chat was deleted"));
        }
        let why = self.unavailable(&ctx.db, &row.plugin, r);
        if !why.is_empty() {
            return refuse("plugin_unavailable", format!("{name} can't run: {why}"));
        }
        if reread {
            Self::set_cols(&ctx.db, &row.id, "seen='[]'", &[])?;
        }
        let (tx, rx) = oneshot::channel();
        if !self.claim(&row.id) {
            let _ = tx.send(json!({"ran": false, "reason": "already running"}));
            return Ok(rx);
        }
        let (me, ctx, id) = (self.clone(), ctx.clone(), row.id.clone());
        tokio::spawn(async move {
            let out = me.tick_claimed(&ctx, &id, items).await;
            me.release(&id);
            let _ = tx.send(out);
        });
        Ok(rx)
    }

    // ── webhooks ─────────────────────────────────────────────────────────

    /// POST /v1/hooks/<instance>/<secret>: one run of a webhook instance;
    /// → {ok, instance, items} once it started.
    pub fn hook_call(self: &Arc<Self>, ctx: &Arc<HubCtx>, id: &str, secret: &str, body: &Value) -> Res {
        let db = &ctx.db;
        let row = if is_instance_ref(id) { Self::row_of(db, id) } else { None };
        let want = row.as_ref().and_then(|r| r.hook_hash.clone()).filter(|h| !h.is_empty());
        let got = sha_hex(secret);
        let matches = want.as_deref().is_some_and(|w| w.len() == got.len() && w.bytes().zip(got.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0);
        let Some(row) = row.filter(|r| r.trigger == "webhook" && matches) else { return refuse("not_found", "no such hook") };
        let Some(r) = self.routine_of(&row.plugin, &row.routine) else { return refuse("not_found", "no such hook") };
        let now = ctx.clock.now();
        {
            let mut calls = self.hook_calls.lock().unwrap_or_else(|e| e.into_inner());
            let mine = calls.entry(id.to_owned()).or_default();
            mine.retain(|t| *t > now - 60_000.0);
            if mine.len() >= HOOK_RATE {
                return refuse("rate_limited", format!("at most {HOOK_RATE} calls a minute"));
            }
            mine.push(now);
        }
        let mut items = None;
        if let Some(given) = body.as_object().and_then(|b| b.get("items")) {
            let ok = given.as_array().is_some_and(|l| l.len() <= HOOK_ITEMS && l.iter().all(|x| x.is_object() && nonempty(x.get("id")).is_some_and(|i| len16(i) <= 200)));
            if !ok {
                return refuse("invalid_args", format!("items is a list of at most {HOOK_ITEMS} objects with a string id"));
            }
            items = given.as_array().cloned();
        }
        let n = items.as_ref().map(Vec::len);
        self.launch(ctx, &r, &row, false, items)?;
        Ok(json!({"ok": true, "instance": row.id, "items": n}))
    }

    // ── the agent's routine tools ────────────────────────────────────────

    /// The instances of the chats `chats`, with their routine's name and
    /// plugin (`ofChats`).
    pub fn of_chats(&self, db: &Db, chats: &[String]) -> Vec<Value> {
        Self::all_instances(db, "SELECT * FROM v1_instances ORDER BY created_at, rowid")
            .iter()
            .filter(|row| chats.contains(&row.thread))
            .filter_map(|row| {
                let r = self.routine_of(&row.plugin, &row.routine)?;
                let mut o = Map::new();
                o.insert("plugin".into(), json!(row.plugin));
                o.insert("title".into(), json!(self.title_of(&row.plugin)));
                o.insert("id".into(), json!(rid(&r)));
                o.insert("name".into(), json!(approved_text(&tpl(r.get("name")), 80)));
                o.insert("writes".into(), r.get("writes").filter(|w| js::truthy(w)).cloned().unwrap_or(json!([])));
                if let Value::Object(info) = self.instance_info(db, &r, row) {
                    o.extend(info);
                }
                Some(Value::Object(o))
            })
            .collect()
    }

    /// An instance id, or `<plugin>/<routine>` when that routine has one
    /// instance among `chats` (`agentFind`).
    pub fn agent_find(&self, db: &Db, reference: &str, chats: &[String]) -> Option<Found> {
        let mine = self.of_chats(db, chats);
        if is_instance_ref(reference) {
            return mine.into_iter().find(|x| x["instance"] == json!(reference)).map(Found::One);
        }
        let at = reference.find('/').filter(|at| *at >= 1)?;
        let (plugin, id) = (&reference[..at], &reference[at + 1..]);
        let mut hits: Vec<Value> = mine.into_iter().filter(|x| x["plugin"] == json!(plugin) && x["id"] == json!(id)).collect();
        match hits.len() {
            0 => None,
            1 => hits.pop().map(Found::One),
            _ => Some(Found::Ambiguous(hits)),
        }
    }

    fn not_mine(reference: &str) -> ToolResult {
        err(format!("no plugin routine {} in this chat (routine_list names them)", js::slice_utf16(reference, 120)))
    }

    fn ambiguous(hits: &[Value]) -> ToolResult {
        err(format!("that routine has {} instances here; name one: {}", hits.len(), hits.iter().map(|a| js::string_of(&a["instance"])).collect::<Vec<_>>().join(", ")))
    }

    /// the card that asks the user to switch an instance on
    fn ask_consent(&self, r: &Value, row: &Inst) {
        let reads: Vec<String> = list(r, "reads").iter().map(|x| format!("{}: {}", self.title_of(s(x, "plugin")), strings(x.get("tools")).join(", "))).collect();
        let n = json!({
            "kind": "routine_consent",
            "plugin": row.plugin,
            "routine": rid(r),
            "instance": row.id,
            "name": approved_text(&tpl(r.get("name")), 80),
            "title": self.title_of(&row.plugin),
            "summary": Self::summary_of(r, row),
            "writes": r.get("writes").filter(|w| js::truthy(w)).cloned().unwrap_or(json!([])),
            "reads": reads,
            "state": "pending",
        });
        threads::notice(self.host.as_ref(), &row.thread, &n);
    }

    fn consent_line(&self, r: &Value, row: &Inst) -> String {
        let writes = strings(r.get("writes"));
        let tail = if writes.is_empty() { " (they approve what it reads)".to_owned() } else { format!(" (it may change {} data: {})", self.title_of(&row.plugin), writes.join(", ")) };
        format!("it waits for the user: a card in this chat asks them to Enable it{tail}")
    }

    /// routine_create for a plugin routine: an instance in `chats[0]`, on
    /// at once unless it needs the user's consent (`agentCreate`).
    pub fn agent_create(&self, ctx: &HubCtx, input: &Value, chats: &[String]) -> ToolResult {
        let made = (|| -> Res<String> {
            let r = self.must_routine(input)?;
            let plugin = s(input, "plugin");
            if pv::routine_prompt(self.plugins.as_ref(), plugin, rid(&r)).is_none() {
                return refuse("permission_denied", format!("{plugin} is not an approved plugin"));
            }
            let chat = chats.first().filter(|c| self.host.thread(c).is_some()).ok_or_else(|| V1Error::new("invalid_args", "no chat to report to"))?;
            let f = Self::fields(&r, input, false)?;
            let ask = self.needs_consent(&ctx.db, plugin, &r);
            let row = Self::create(&ctx.db, ctx.clock.now(), plugin, &r, chat, &f, !ask, ask)?;
            let name = Self::name_of(&r, &row);
            if ask {
                self.ask_consent(&r, &row);
                return Ok(format!("made {} — {name}; {}", row.id, self.consent_line(&r, &row)));
            }
            Self::upsert(&ctx.db, plugin, rid(&r))?;
            self.home_of(&r, &row);
            let how =
                if row.trigger == "webhook" { "on its webhook (the user gets its URL in the routine's panel)".to_owned() } else { format!("every {}s", js::string_of(&Self::every_of(&r, &row))) };
            Ok(format!("made and enabled {} — {name} · {how}", row.id))
        })();
        said(made)
    }

    /// routine_update for a plugin routine (`agentUpdate`).
    pub fn agent_update(&self, ctx: &HubCtx, reference: &str, input: &Value, chats: &[String]) -> ToolResult {
        let db = &ctx.db;
        let x = match self.agent_find(db, reference, chats) {
            None => return Self::not_mine(reference),
            Some(Found::Ambiguous(h)) => return Self::ambiguous(&h),
            Some(Found::One(x)) => x,
        };
        let done = (|| -> Res<String> {
            let r = self.routine_of(s(&x, "plugin"), s(&x, "id")).ok_or_else(|| V1Error::new("not_found", "no such routine"))?;
            let row = Self::row_of(db, s(&x, "instance")).ok_or_else(|| V1Error::new("not_found", "no such instance"))?;
            let f = Self::fields(&r, input, true)?;
            let on = input.get("enabled");
            let wants_on = on == Some(&Value::Bool(true)) || (on.is_none() && row.enabled);
            let new_params = f.params.as_ref().is_some_and(|p| *p != row.params);
            let ask = self.needs_consent(db, s(&x, "plugin"), &r) && ((on == Some(&Value::Bool(true)) && !row.enabled) || new_params);
            Self::patch(db, &row.id, &f)?;
            if on == Some(&Value::Bool(false)) {
                Self::set_cols(db, &row.id, "enabled=0", &[])?;
            } else if ask {
                Self::set_cols(db, &row.id, "enabled=0, pending=1", &[])?;
                if let Some(fresh) = Self::row_of(db, &row.id) {
                    self.ask_consent(&r, &fresh);
                }
                return Ok(format!("updated {}; {}", row.id, self.consent_line(&r, &row)));
            } else if wants_on {
                let errors = if on == Some(&Value::Bool(true)) { 0 } else { row.errors as i64 };
                Self::set_cols(db, &row.id, "enabled=1, pending=0, errors=?", &[&errors])?;
            }
            let now = Self::row_of(db, &row.id).unwrap_or(row);
            if now.enabled {
                self.home_of(&r, &now);
            }
            let state = if now.enabled {
                "on"
            } else if now.pending {
                "waiting for the user"
            } else {
                "paused"
            };
            Ok(format!("updated {} — {} · {state}", now.id, Self::name_of(&r, &now)))
        })();
        said(done)
    }

    /// routine_delete for a plugin routine's instance (`agentDelete`).
    pub fn agent_delete(&self, ctx: &HubCtx, reference: &str, chats: &[String]) -> ToolResult {
        let db = &ctx.db;
        let x = match self.agent_find(db, reference, chats) {
            None => return Self::not_mine(reference),
            Some(Found::Ambiguous(h)) => return Self::ambiguous(&h),
            Some(Found::One(x)) => x,
        };
        let Some(row) = Self::row_of(db, s(&x, "instance")) else { return Self::not_mine(reference) };
        if let Err(e) = Self::sql(db, |db| db.execute("DELETE FROM v1_instances WHERE id=?", [&row.id])) {
            return err(e.message);
        }
        if row.pending {
            self.settle(&row, "declined");
        }
        ToolResult { content: json!(format!("deleted {} — {}; its thread stays", row.id, s(&x, "name"))), is_error: false }
    }

    /// routine_run for a plugin routine: waits up to `wait` for the result
    /// (`agentRun`).
    pub async fn agent_run(self: &Arc<Self>, ctx: &Arc<HubCtx>, reference: &str, chats: &[String], wait: std::time::Duration) -> ToolResult {
        let x = match self.agent_find(&ctx.db, reference, chats) {
            None => return Self::not_mine(reference),
            Some(Found::Ambiguous(h)) => return Self::ambiguous(&h),
            Some(Found::One(x)) => x,
        };
        let (Some(r), Some(row)) = (self.routine_of(s(&x, "plugin"), s(&x, "id")), Self::row_of(&ctx.db, s(&x, "instance"))) else { return Self::not_mine(reference) };
        let name = Self::name_of(&r, &row);
        let rx = match self.launch(ctx, &r, &row, false, None) {
            Ok(rx) => rx,
            Err(e) => return err(e.message),
        };
        let out = match tokio::time::timeout(wait, rx).await {
            Ok(Ok(out)) => out,
            _ => {
                let home = x.get("home").filter(|h| js::truthy(h)).map(js::string_of).unwrap_or_else(|| "its own".to_owned());
                return ToolResult { content: json!(format!("{name} is running; what it finds lands in its thread ({home}) and this chat gets a notice")), is_error: false };
            }
        };
        if let Some(e) = out.get("error").filter(|e| js::truthy(e)) {
            return err(format!("{name} ran and failed: {}", js::string_of(e)));
        }
        if out.get("ran") != Some(&Value::Bool(true)) {
            return err(format!("{name} didn't run: {}", tpl(out.get("reason"))));
        }
        let home = threads::find(self.host.as_ref(), s(&x, "plugin"), s(&x, "id"), Some(s(&x, "instance")));
        let items = out["items"].as_f64().unwrap_or(0.0);
        let prompts = out["prompts"].as_f64().unwrap_or(0.0);
        let where_ = if prompts != 0.0 { format!(" (each ran in its thread {})", home.map_or_else(|| "undefined".to_owned(), |h| h.id)) } else { String::new() };
        ToolResult { content: json!(format!("{name} ran: {} item{} found, {} new{where_}", js::number(items), if items == 1.0 { "" } else { "s" }, js::number(prompts))), is_error: false }
    }

    // ── one run ──────────────────────────────────────────────────────────

    fn claim(&self, id: &str) -> bool {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_owned())
    }

    fn release(&self, id: &str) {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    /// A call to a plugin from a run: `buwiz/caller` names the routine on a
    /// call to its own plugin (16.1); the result's structuredContent.
    async fn call(&self, plugin: &str, tool: &str, args: Value, mark: Option<Value>) -> Result<Value, String> {
        let mut caller = Map::new();
        caller.insert("origin".into(), json!("routine"));
        caller.insert("plugin".into(), json!(plugin));
        if let Some(Value::Object(m)) = mark {
            caller.extend(m);
        }
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
        caller.insert("ts".into(), json!(ts));
        let r = self.plugins.call_tool(plugin, tool, args, Some(json!({"buwiz/caller": caller}))).await.map_err(|e| e.message)?;
        if r.get("isError").is_some_and(js::truthy) {
            let t = js::slice_utf16(&first_text(&r), 300);
            return Err(format!("{tool}: {}", if t.is_empty() { "failed".to_owned() } else { t }));
        }
        match r.get("structuredContent") {
            Some(sc) if sc.is_object() => Ok(sc.clone()),
            _ => Err(format!("{tool} answered without structuredContent")),
        }
    }

    /// steps 1-2: the search text from the own tool, then the items
    async fn items_of(&self, r: &Value, row: &Inst) -> Result<Vec<Value>, String> {
        let params = row.params();
        let mut args = query(r, "args").and_then(Value::as_object).cloned().unwrap_or_default();
        if let Some(p) = params.as_object() {
            args.extend(p.clone());
        }
        let qt = query_tool(r);
        let own = self.call(&row.plugin, &qt, Value::Object(args), Some(json!({"routine": rid(r), "instance": row.id, "params": params}))).await?;
        let field = tpl(query(r, "field"));
        let Some(text) = own.get(&field).and_then(Value::as_str) else { return Err(format!("{qt} gave no string {field}")) };
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let read = list(r, "reads").first().cloned().cloned();
        let (res, from) = match &read {
            Some(x) => {
                let tool = strings(x.get("tools")).into_iter().next().unwrap_or_else(|| "undefined".to_owned());
                (self.call(s(x, "plugin"), &tool, json!({"query": text}), None).await?, tool)
            }
            None => (own.clone(), qt.clone()),
        };
        match res.get("items").and_then(Value::as_array) {
            Some(items) => Ok(dedupe(items)),
            None => Err(format!("{from} gave no items")),
        }
    }

    fn turn_prompt(r: &Value, row: &Inst, label: &str, item: &Value, hooked: bool) -> String {
        let json = js::stringify_indent(item, 1);
        let reads = list(r, "reads");
        let from = if hooked {
            "its webhook".to_owned()
        } else if let Some(x) = reads.first() {
            format!("{} {}", tpl(x.get("plugin")), strings(x.get("tools")).into_iter().next().unwrap_or_else(|| "undefined".to_owned()))
        } else {
            query_tool(r)
        };
        let params = row.params();
        let how = if row.trigger == "webhook" { "on its webhook".to_owned() } else { format!("every {}s", js::string_of(&Self::every_of(r, row))) };
        let mut parts = vec![format!("(This routine runs on its own {how}; the user isn't waiting on this message. Do what it asks and report briefly.)"), label.to_owned()];
        if params.as_object().is_some_and(|p| !p.is_empty()) {
            let named = row.label.as_deref().filter(|l| !l.is_empty()).map(|l| format!(", \"{l}\"")).unwrap_or_default();
            parts.push(format!("This instance ({}{named}) runs with these params; keep to them:\n{}", row.id, js::stringify(&params)));
        }
        let shown = if len16(&json) > ITEM_MAX { format!("{}…[truncated]", js::slice_utf16(&json, ITEM_MAX)) } else { json };
        parts.push(format!("New item from {from}:\n{shown}"));
        parts.join("\n\n")
    }

    /// the run's notice in the chat it reports to
    fn notify(&self, r: &Value, row: &Inst, run: &Run, error: Option<&str>) {
        let home = run.home.clone().or_else(|| threads::find(self.host.as_ref(), &row.plugin, rid(r), Some(&row.id)));
        let mut n = json!({
            "kind": "routine",
            "plugin": row.plugin,
            "routine": rid(r),
            "instance": row.id,
            "name": run.name,
            "thread": home.as_ref().map(|h| h.id.clone()),
            "thread_uid": home.as_ref().map(|h| h.uid.clone()),
            "run": run.id,
            "seq": run.seq,
            "turn": run.turn,
            "items": run.items,
        });
        if let Some(e) = error {
            n["error"] = json!(js::slice_utf16(e, 300));
        }
        threads::notice(self.host.as_ref(), &row.thread, &n);
    }

    /// `tick(instance, items?)`: one run, unless it is running.
    pub async fn tick(self: &Arc<Self>, ctx: &Arc<HubCtx>, id: &str, items: Option<Vec<Value>>) -> Value {
        if !self.claim(id) {
            return json!({"ran": false, "reason": "already running"});
        }
        let out = self.tick_claimed(ctx, id, items).await;
        self.release(id);
        out
    }

    /// `tick(plugin, routine)`: the routine's only instance's run.
    pub async fn tick_routine(self: &Arc<Self>, ctx: &Arc<HubCtx>, plugin: &str, routine: &str) -> Value {
        let all = Self::instances_of(&ctx.db, plugin, routine);
        match all.as_slice() {
            [one] => self.tick(ctx, &one.id.clone(), None).await,
            _ => json!({"ran": false, "reason": "not enabled"}),
        }
    }

    /// One run of an instance: → {ran, items?, prompts?, reason?, error?}.
    async fn tick_claimed(self: &Arc<Self>, ctx: &Arc<HubCtx>, id: &str, given: Option<Vec<Value>>) -> Value {
        let db = &ctx.db;
        let started = ctx.clock.now();
        let row = Self::row_of(db, id);
        let r = row.as_ref().and_then(|row| self.routine_of(&row.plugin, &row.routine));
        let (Some(row), Some(r)) = (row.filter(|row| row.enabled), r) else { return json!({"ran": false, "reason": "not enabled"}) };
        if self.host.thread(&row.thread).is_none() {
            let _ = Self::set_cols(db, id, "enabled=0, last_run=?, last_error=?", &[&(started as i64), &"its chat was deleted"]);
            return json!({"ran": false, "reason": "its chat was deleted"});
        }
        // made here too for an instance enabled before threads were made early
        let home = self.home_of(&r, &row);
        let why = self.unavailable(db, &row.plugin, &r);
        if !why.is_empty() {
            let _ = Self::set_cols(db, id, "last_run=?", &[&(started as i64)]);
            return json!({"ran": false, "reason": why});
        }
        let mut run = Run { id: format!("run_{}", js::number(started)), name: Self::name_of(&r, &row), items: 0, home, seq: None, turn: None };
        match self.run_items(ctx, &r, &row, started, &mut run, given).await {
            Ok(res) => {
                if run.items > 0 {
                    self.notify(&r, &row, &run, None);
                }
                res
            }
            Err(e) => {
                let msg = js::slice_utf16(&e, 500);
                let _ = Self::set_cols(db, id, "last_run=?, last_error=?, errors=errors+1", &[&(started as i64), &msg]);
                eprintln!("  v1 routine {}/{} {id}: {}", row.plugin, row.routine, js::slice_utf16(&msg, 200));
                // once per failure streak, and whenever the failed run had
                // already posted items
                if run.items > 0 || row.errors == 0.0 {
                    self.notify(&r, &row, &run, Some(&msg));
                }
                json!({"ran": false, "error": msg})
            }
        }
    }

    /// step 3: one turn per unseen item, in the instance's own thread
    async fn run_items(self: &Arc<Self>, ctx: &Arc<HubCtx>, r: &Value, row: &Inst, started: f64, run: &mut Run, given: Option<Vec<Value>>) -> Result<Value, String> {
        let hooked = given.is_some();
        let items = match given {
            Some(g) => dedupe(&g),
            None => self.items_of(r, row).await?,
        };
        let mut seen = row.seen();
        let known: HashSet<String> = seen.iter().map(js::string_of).collect();
        let label = pv::routine_prompt(self.plugins.as_ref(), &row.plugin, rid(r)).unwrap_or_default();
        let surface = json!({
            "plugin": row.plugin, "routine": rid(r), "instance": row.id, "params": row.params(),
            "reads": r.get("reads").filter(|v| js::truthy(v)).cloned().unwrap_or(json!([])),
            "writes": r.get("writes").filter(|v| js::truthy(v)).cloned().unwrap_or(json!([])),
        });
        for item in &items {
            let item_id = s(item, "id").to_owned();
            if known.contains(&item_id) {
                continue;
            }
            if run.home.is_none() {
                run.home = Some(threads::ensure(self.host.as_ref(), &row.plugin, rid(r), Some(&row.id), &run.name, &row.thread));
            }
            let home = run.home.clone().expect("made above");
            let mark = json!({"plugin": row.plugin, "id": rid(r), "instance": row.id, "name": run.name, "run": run.id});
            let turn = TurnRequest {
                turn: TurnCtx { thread: home.id.clone(), routine: Some(format!("{}/{}", row.plugin, rid(r))), surface: Some(surface.clone()), ..TurnCtx::default() },
                shown: shown_of(item),
                prompt: Self::turn_prompt(r, row, &label, item, hooked),
                // backgroundTurn's `{routine: mark}` on the shown turn
                mark: Some(json!({"routine": mark})),
            };
            let done = routine_turn(ctx, self.host.as_ref(), turn).await?;
            if run.items == 0 {
                run.seq = Some(done.seq);
                run.turn = Some(done.turn);
            }
            seen.push(json!(item_id));
            run.items += 1;
            let kept = &seen[seen.len().saturating_sub(SEEN_MAX)..];
            Self::set_cols(&ctx.db, &row.id, "seen=?", &[&js::stringify(&json!(kept))]).map_err(|e| e.message)?;
        }
        Self::set_cols(&ctx.db, &row.id, "last_run=?, last_error=NULL, errors=0, last_items=?", &[&(started as i64), &(run.items as i64)]).map_err(|e| e.message)?;
        Ok(json!({"ran": true, "items": items.len(), "prompts": run.items}))
    }

    /// Every enabled scheduled instance whose interval has passed, one
    /// after another (`runDue`).
    pub async fn run_due(self: &Arc<Self>, ctx: &Arc<HubCtx>) {
        let now = ctx.clock.now();
        for row in Self::all_instances(&ctx.db, "SELECT * FROM v1_instances WHERE enabled=1 AND trigger='schedule' ORDER BY created_at, rowid") {
            let Some(r) = self.routine_of(&row.plugin, &row.routine) else { continue };
            if now >= row.last_run.unwrap_or(0.0) + Self::interval_ms(&r, &row) {
                self.tick(ctx, &row.id, None).await;
            }
        }
    }

    fn poll(self: &Arc<Self>, ctx: &Arc<HubCtx>) {
        let (me, c) = (self.clone(), ctx.clone());
        let id = ctx.clock.set_timeout(
            self.poll_ms,
            Box::new(move || {
                tokio::spawn(async move {
                    me.run_due(&c).await;
                    if me.timer.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                        me.poll(&c);
                    }
                });
            }),
        );
        *self.timer.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
    }

    /// 9.6: an uninstalled plugin's routines go with it.
    pub fn forget(db: &Db, plugin: &str) {
        let _ = db.with(|db| {
            db.execute("DELETE FROM v1_routines WHERE plugin=?", [plugin])?;
            db.execute("DELETE FROM v1_instances WHERE plugin=?", [plugin])
        });
    }

    /// Polls for due instances every `poll_ms` (the hub's start); the
    /// enabled instances' threads are made first.
    pub fn start(self: &Arc<Self>, ctx: &Arc<HubCtx>) {
        if self.timer.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return;
        }
        for row in Self::all_instances(&ctx.db, "SELECT * FROM v1_instances WHERE enabled=1") {
            if let Some(r) = self.routine_of(&row.plugin, &row.routine) {
                self.home_of(&r, &row);
            }
        }
        self.poll(ctx);
    }

    /// 9.6: an uninstalled plugin's routines go with it, however it was
    /// uninstalled (the plugin host's events).
    pub fn listen(&self, ctx: &Arc<HubCtx>) {
        let Some(mut events) = self.plugins.events() else { return };
        let c = ctx.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(e) if e.get("state").and_then(Value::as_str) == Some("uninstalled") => Self::forget(&c.db, s(&e, "id")),
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return,
                }
            }
        });
    }

    pub fn stop(&self, ctx: &HubCtx) {
        if let Some(t) = self.timer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            ctx.clock.clear(t);
        }
    }
}

fn err(content: String) -> ToolResult {
    ToolResult { content: json!(content), is_error: true }
}

fn said(r: Res<String>) -> ToolResult {
    match r {
        Ok(c) => ToolResult { content: json!(c), is_error: false },
        Err(e) => err(e.message),
    }
}

/// A routine's agent turn in its own thread (`routineTurn`, v1/broker.mjs):
/// the turn runner's, with the job naming the routine and its surface.
async fn routine_turn(ctx: &Arc<HubCtx>, host: &dyn RoutineHost, turn: TurnRequest) -> Result<crate::hub::traits::TurnDone, String> {
    if host.thread(&turn.turn.thread).is_none() {
        return Err("its thread was deleted".to_owned());
    }
    let Some(runner) = ctx.turns.clone() else { return Err("no turn runner".to_owned()) };
    runner.run(ctx.clone(), turn).await
}
