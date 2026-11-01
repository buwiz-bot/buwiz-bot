//! The v1 plugin host (spec 4.3, 5.2, 12-14; `v1/plugins.mjs`): install
//! records, processes, restarts.
//!
//! A v1 plugin is a stdio MCP child this host owns. Every spawn runs the
//! same gate: connect, `buwiz.provision` with the plugin key as the first
//! request (7.8), read `buwiz://manifest`, verify it (5.2), check tools/list
//! against it, and compare it with the install record (12.1). Only then is
//! the plugin `running`, under a new generation. A crash restarts it with
//! backoff 1-60 s; the seventh restart within 10 minutes leaves it `failed`
//! (14.4). A verification failure never loops. Records live in
//! `v1_plugins`; keys live in the keystore and reach the plugin only on its
//! stdin.
//!
//! Node keeps the views (the plugin list, routines) and reaches this
//! host and its sessions (`sessions.rs`) over the line (`helper::line`); each change of a
//! plugin's state goes out as a snapshot on [`Host::events`].

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex as AsyncMutex};

use super::keystore::Keystore;
use super::mcp::{self, Conn, Hooks, RpcError, Spec};
use crate::js;
use crate::v1::manifest::{self, MANIFEST_URI};
use crate::v1::trust::Trust;
use crate::v1::{get, kind_of, tpl, V1Error};

pub const BACKOFF_S: [u64; 7] = [1, 2, 4, 8, 16, 32, 60];
pub const RESTART_WINDOW_MS: f64 = 10.0 * 60.0 * 1000.0;
pub const MAX_RESTARTS: usize = 6;
const LOG_CAP: u64 = 5 * 1024 * 1024;

const SCHEMA: &str = "
  CREATE TABLE IF NOT EXISTS v1_plugins(
    id TEXT PRIMARY KEY, short TEXT NOT NULL, command TEXT NOT NULL, args TEXT NOT NULL, env TEXT NOT NULL,
    version TEXT NOT NULL, protocol TEXT NOT NULL, publisher_key_id TEXT NOT NULL, plugin_pub TEXT NOT NULL,
    manifest TEXT NOT NULL, manifest_sha256 TEXT NOT NULL, tools TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 1, disabled_tools TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
";

type Res<T = Value> = Result<T, V1Error>;

/// Ends a plugin's opens (sessions.rs): disable, uninstall and update run
/// it, as plugins.mjs ran its closers.
pub type Closer = Arc<dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// The broker's limits on a plugin's answers.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub call: Duration,
    pub resolve: Duration,
    pub reserved: Duration,
    pub cancel: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        let ms = |n| Duration::from_millis(n);
        Self { call: ms(120_000), resolve: ms(10_000), reserved: ms(5_000), cancel: ms(5_000) }
    }
}

// ── the clock ────────────────────────────────────────────────────────────

/// What a clock timer runs when it fires.
pub type Timer = Box<dyn FnOnce() + Send>;

/// Time for records and the restart policy. `Manual` (tests) keeps the
/// time it is told and fires timers only when asked, oldest first, as
/// v1-plugins.test.mjs's fake clock does.
pub enum Clock {
    System(Mutex<HashMap<u64, tokio::task::AbortHandle>>, AtomicU64),
    Manual(Mutex<Manual>),
}

pub struct Manual {
    now: f64,
    next: u64,
    timers: VecDeque<(u64, Timer)>,
}

impl Clock {
    pub fn system() -> Self {
        Clock::System(Mutex::new(HashMap::new()), AtomicU64::new(1))
    }

    pub fn manual() -> Self {
        Clock::Manual(Mutex::new(Manual { now: 0.0, next: 1, timers: VecDeque::new() }))
    }

    pub fn now(&self) -> f64 {
        match self {
            Clock::System(..) => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0),
            Clock::Manual(m) => m.lock().unwrap_or_else(|e| e.into_inner()).now,
        }
    }

    /// Runs `f` after `ms` (a manual clock: when told to fire); → its id.
    pub fn set_timeout(&self, ms: u64, f: Timer) -> u64 {
        match self {
            Clock::System(timers, next) => {
                let id = next.fetch_add(1, Ordering::SeqCst);
                let task = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    f();
                });
                timers.lock().unwrap_or_else(|e| e.into_inner()).insert(id, task.abort_handle());
                id
            }
            Clock::Manual(m) => {
                let mut m = m.lock().unwrap_or_else(|e| e.into_inner());
                let id = m.next;
                m.next += 1;
                m.timers.push_back((id, f));
                id
            }
        }
    }

    /// Cancels the timer `id` (none: nothing).
    pub fn clear(&self, id: u64) {
        match self {
            Clock::System(timers, _) => {
                if let Some(t) = timers.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
                    t.abort();
                }
            }
            Clock::Manual(m) => m.lock().unwrap_or_else(|e| e.into_inner()).timers.retain(|(t, _)| *t != id),
        }
    }

    /// Manual time: set it to `now`, and fire the oldest timer when `fire`.
    pub fn manual_step(&self, now: Option<f64>, fire: bool) -> bool {
        let Clock::Manual(m) = self else { return false };
        let f = {
            let mut m = m.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(now) = now {
                m.now = now;
            }
            if fire {
                m.timers.pop_front().map(|(_, f)| f)
            } else {
                None
            }
        };
        let fired = f.is_some();
        if let Some(f) = f {
            f();
        }
        fired
    }
}

// ── records ──────────────────────────────────────────────────────────────

/// A `v1_plugins` row.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: String,
    pub short: String,
    pub command: String,
    pub args: String,
    pub env: String,
    pub version: String,
    pub protocol: String,
    pub publisher_key_id: String,
    pub plugin_pub: String,
    pub manifest: String,
    pub manifest_sha256: String,
    pub tools: String,
    pub enabled: bool,
    pub disabled_tools: String,
    pub created_at: i64,
}

impl Row {
    fn from_sql(r: &rusqlite::Row) -> rusqlite::Result<Row> {
        Ok(Row {
            id: r.get("id")?,
            short: r.get("short")?,
            command: r.get("command")?,
            args: r.get("args")?,
            env: r.get("env")?,
            version: r.get("version")?,
            protocol: r.get("protocol")?,
            publisher_key_id: r.get("publisher_key_id")?,
            plugin_pub: r.get("plugin_pub")?,
            manifest: r.get("manifest")?,
            manifest_sha256: r.get("manifest_sha256")?,
            tools: r.get("tools")?,
            enabled: r.get::<_, f64>("enabled")? != 0.0,
            disabled_tools: r.get("disabled_tools")?,
            created_at: r.get::<_, f64>("created_at")? as i64,
        })
    }

    pub fn manifest(&self) -> Value {
        parse(&self.manifest, json!({}))
    }

    fn spec(&self) -> Spec {
        let args = parse(&self.args, json!([]));
        let env = parse(&self.env, json!({}));
        Spec {
            inherit_all: false,
            client: "rox-broker",
            command: self.command.clone(),
            args: args.as_array().into_iter().flatten().map(js::string_of).collect(),
            env: env.as_object().map(js::ordered).unwrap_or_default().into_iter().map(|(k, v)| (k.clone(), js::string_of(v))).collect(),
        }
    }
}

/// `JSON.parse(s) ?? d`, `d` when it throws.
pub(crate) fn parse(s: &str, d: Value) -> Value {
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Null) | Err(_) => d,
        Ok(v) => v,
    }
}

// ── runtimes ─────────────────────────────────────────────────────────────

struct Runtime {
    state: &'static str,
    code: Option<String>,
    message: String,
    generation: u64,
    conn: Option<Arc<Conn>>,
    tools: Vec<Value>,
    instructions: String,
    restarts: Vec<f64>,
    timer: Option<u64>,
    next_delay: Option<u64>,
}

impl Runtime {
    fn new() -> Self {
        Runtime { state: "stopped", code: None, message: String::new(), generation: 0, conn: None, tools: Vec::new(), instructions: String::new(), restarts: Vec::new(), timer: None, next_delay: None }
    }
}

/// A plugin's runtime as the views read it (`Host::runtime_view`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeView {
    pub state: &'static str,
    pub code: Option<String>,
    pub message: String,
    pub generation: u64,
    pub conn: bool,
    pub tools: Vec<Value>,
    pub instructions: String,
}

/// A spawn that passed the gate.
struct Gate {
    conn: Arc<Conn>,
    manifest: Value,
    sha256: String,
    tools: Vec<Value>,
    instructions: String,
}

/// Why a gate or call failed: a refusal, or what the connection said.
enum Fail {
    V1(V1Error),
    Rpc(RpcError),
}

impl Fail {
    fn message(&self) -> &str {
        match self {
            Fail::V1(e) => &e.message,
            Fail::Rpc(e) => &e.message,
        }
    }

    /// As the host API answers it: a refusal as is, anything else `internal`.
    fn into_v1(self) -> V1Error {
        match self {
            Fail::V1(e) => e,
            Fail::Rpc(e) => V1Error::new("internal", e.message),
        }
    }
}

impl From<V1Error> for Fail {
    fn from(e: V1Error) -> Self {
        Fail::V1(e)
    }
}

impl From<RpcError> for Fail {
    fn from(e: RpcError) -> Self {
        Fail::Rpc(e)
    }
}

fn refuse<T>(code: &str, message: impl Into<String>) -> Result<T, Fail> {
    Err(Fail::V1(V1Error::new(code, message)))
}

pub struct Options {
    pub db: PathBuf,
    pub keys: Keystore,
    pub trust_file: PathBuf,
    pub log_dir: Option<PathBuf>,
    pub clock: Clock,
    pub timeouts: Timeouts,
}

pub struct Host {
    db: Mutex<Connection>,
    keys: Keystore,
    trust_file: PathBuf,
    log_dir: Option<PathBuf>,
    pub clock: Clock,
    pub timeouts: Timeouts,
    runtimes: Mutex<HashMap<String, Runtime>>,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    events: broadcast::Sender<Value>,
    /// orders snapshots: a newer one replaces an older one in Node's view
    seq: AtomicU64,
    closers: Mutex<Vec<Closer>>,
    me: Weak<Host>,
}

impl Host {
    pub fn new(o: Options) -> Result<Arc<Host>, String> {
        if let Some(dir) = o.db.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let db = Connection::open(&o.db).map_err(|e| e.to_string())?;
        db.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        let (events, _) = broadcast::channel(1024);
        Ok(Arc::new_cyclic(|me| Host {
            db: Mutex::new(db),
            keys: o.keys,
            trust_file: o.trust_file,
            log_dir: o.log_dir,
            clock: o.clock,
            timeouts: o.timeouts,
            runtimes: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            events,
            seq: AtomicU64::new(1),
            closers: Mutex::new(Vec::new()),
            me: me.clone(),
        }))
    }

    /// Snapshots of every plugin's state as it changes.
    pub fn events(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    /// The trust store, read now.
    pub fn trust(&self) -> Trust {
        Trust::load(&self.trust_file)
    }

    fn sql<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, V1Error> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        f(&db).map_err(|e| V1Error::new("internal", format!("app.db: {e}")))
    }

    pub fn row(&self, id: &str) -> Option<Row> {
        self.sql(|db| db.query_row("SELECT * FROM v1_plugins WHERE id=?", [id], Row::from_sql).optional()).ok().flatten()
    }

    pub fn rows(&self) -> Vec<Row> {
        self.sql(|db| {
            let mut st = db.prepare("SELECT * FROM v1_plugins ORDER BY created_at")?;
            let rows = st.query_map([], Row::from_sql)?.collect::<rusqlite::Result<Vec<_>>>();
            rows
        })
        .unwrap_or_default()
    }

    fn lock_of(&self, id: &str) -> Arc<AsyncMutex<()>> {
        self.locks.lock().unwrap_or_else(|e| e.into_inner()).entry(id.to_owned()).or_default().clone()
    }

    fn with_rt<T>(&self, id: &str, f: impl FnOnce(&mut Runtime) -> T) -> T {
        let mut map = self.runtimes.lock().unwrap_or_else(|e| e.into_inner());
        f(map.entry(id.to_owned()).or_insert_with(Runtime::new))
    }

    fn conn_of(&self, id: &str) -> Option<Arc<Conn>> {
        self.with_rt(id, |rt| rt.conn.clone())
    }

    /// A plugin's state as the line sends it: what Node's runtime held,
    /// plus `seq` (later snapshots are higher), the process id and the
    /// pending restart (`timer`, `next_delay_ms`).
    pub fn snapshot(&self, id: &str) -> Value {
        let enabled = self.row(id).is_some_and(|r| r.enabled);
        self.with_rt(id, |rt| {
            json!({
                "id": id, "seq": self.seq.fetch_add(1, Ordering::SeqCst), "state": rt.state, "code": rt.code, "message": rt.message,
                "generation": rt.generation, "enabled": enabled, "tools": rt.tools, "instructions": rt.instructions,
                "pid": rt.conn.as_ref().and_then(|c| c.pid), "timer": rt.timer, "next_delay_ms": rt.next_delay,
            })
        })
    }

    /// The snapshot of a plugin this host knows (a runtime), else `None`.
    pub fn snapshot_if(&self, id: &str) -> Option<Value> {
        let known = self.runtimes.lock().unwrap_or_else(|e| e.into_inner()).contains_key(id);
        known.then(|| self.snapshot(id))
    }

    /// Registers what ends a plugin's opens.
    pub fn on_close(&self, f: Closer) {
        self.closers.lock().unwrap_or_else(|e| e.into_inner()).push(f);
    }

    async fn end_opens(&self, id: &str) {
        let closers = self.closers.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for f in closers {
            f(id.to_owned()).await;
        }
    }

    /// A plugin's live state as v1/plugins.mjs's views read `runtime(id)`:
    /// state, code, message, generation, whether it has a connection, its
    /// tools and its instructions.
    pub fn runtime_view(&self, id: &str) -> RuntimeView {
        self.with_rt(id, |rt| RuntimeView {
            state: rt.state,
            code: rt.code.clone(),
            message: rt.message.clone(),
            generation: rt.generation,
            conn: rt.conn.is_some(),
            tools: rt.tools.clone(),
            instructions: rt.instructions.clone(),
        })
    }

    pub fn runtime_of(&self, id: &str) -> (&'static str, u64, Vec<Value>) {
        self.with_rt(id, |rt| (rt.state, rt.generation, rt.tools.clone()))
    }

    /// Every plugin with a runtime or a record.
    pub fn snapshots(&self) -> Vec<Value> {
        let mut ids: Vec<String> = self.rows().into_iter().map(|r| r.id).collect();
        for id in self.runtimes.lock().unwrap_or_else(|e| e.into_inner()).keys() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids.iter().map(|id| self.snapshot(id)).collect()
    }

    fn emit(&self, id: &str) {
        let _ = self.events.send(self.snapshot(id));
    }

    fn set_state(&self, id: &str, state: &'static str, code: Option<&str>, message: &str) {
        self.with_rt(id, |rt| {
            rt.state = state;
            rt.code = code.map(str::to_owned);
            rt.message = js::slice_utf16(message, 500);
        });
        self.emit(id);
    }

    // ── the gate ─────────────────────────────────────────────────────────

    async fn connect(&self, spec: &Spec, seed: &str, log_id: &str, running: Option<&str>) -> Result<(Arc<Conn>, Arc<ConnHooks>), Fail> {
        let hooks = Arc::new(ConnHooks {
            host: self.me.clone(),
            running: running.map(str::to_owned),
            conn: OnceLock::new(),
            sink: Mutex::new(Sink::new(self.log_dir.as_ref().map(|d| d.join(format!("{log_id}.log"))), seed)),
            leaked: AtomicBool::new(false),
        });
        let conn = Conn::connect(spec, hooks.clone()).await?;
        let _ = hooks.conn.set(Arc::downgrade(&conn));
        Ok((conn, hooks))
    }

    async fn reserved(&self, conn: &Conn, name: &str, args: Value, timeout: Duration) -> Result<Value, Fail> {
        let r = conn.call_tool(name, &args, None, timeout).await?;
        let data = result_data(&r);
        if get(&r, "isError").is_some_and(js::truthy) {
            let code = get(&data, "code").and_then(Value::as_str).unwrap_or("invalid_args");
            let msg = get(&data, "message").filter(|m| js::truthy(m)).map(js::string_of).unwrap_or_else(|| "refused".to_owned());
            return refuse(code, format!("{name}: {}", js::slice_utf16(&msg, 200)));
        }
        Ok(data)
    }

    /// The gate every spawn passes: provision, read, verify (5.2 steps
    /// 1-5, 7). `running` names the plugin whose runtime the connection
    /// will be (its hooks restart it); `None` for a temporary one.
    async fn bootstrap(&self, spec: &Spec, seed: &str, running: Option<&str>, known: &str) -> Result<Gate, Fail> {
        let (conn, hooks) = self.connect(spec, seed, if known.is_empty() { "inspect" } else { known }, running).await?;
        match self.gate(&conn, seed, known).await {
            Ok((manifest, sha256, tools, instructions)) if !hooks.leaked.load(Ordering::SeqCst) => Ok(Gate { conn, manifest, sha256, tools, instructions }),
            Ok(_) => {
                conn.close().await;
                refuse("plugin_invalid", "the plugin wrote its key to stderr (4.3)")
            }
            Err(e) => {
                conn.close().await;
                Err(e)
            }
        }
    }

    async fn gate(&self, conn: &Conn, seed: &str, known: &str) -> Result<(Value, String, Vec<Value>, String), Fail> {
        // a first spawn learns the id buwiz.provision names from the manifest
        // (resources are readable before provision; only tools are gated)
        let id = if known.is_empty() { peek_id(conn).await } else { known.to_owned() };
        let public = rox_v1::keys::SessionKey::from_seed(seed).map(|k| k.public()).unwrap_or_default();
        let args = json!({"protocol": manifest::HOST_PROTOCOL, "plugin": id, "key": {"alg": "ed25519", "seed": seed}, "pub": public});
        let prov = match self.reserved(conn, "buwiz.provision", args, self.timeouts.reserved).await {
            Ok(p) => p,
            Err(e) => return refuse("plugin_invalid", format!("buwiz.provision failed: {}", js::slice_utf16(once_prefixed(e.message()), 200))),
        };
        let sha_ok = get(&prov, "manifest_sha256").map(js::string_of).is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        if get(&prov, "ok") != Some(&Value::Bool(true)) || !sha_ok {
            return refuse("plugin_invalid", "buwiz.provision did not answer {ok: true, manifest_sha256}");
        }
        let (manifest, sha256, tools, instructions) = self.read_verified(conn).await?;
        if get(&prov, "manifest_sha256").and_then(Value::as_str) != Some(sha256.as_str()) {
            return refuse("manifest_invalid", "buwiz.provision reports a manifest other than the one the plugin serves");
        }
        Ok((manifest, sha256, tools, instructions))
    }

    async fn read_verified(&self, conn: &Conn) -> Result<(Value, String, Vec<Value>, String), Fail> {
        let listed = conn.request("resources/list", json!({}), mcp::DEFAULT_TIMEOUT).await?;
        let resources = get(&listed, "resources").and_then(Value::as_array).cloned().unwrap_or_default();
        if !resources.iter().any(|r| get(r, "uri").and_then(Value::as_str) == Some(MANIFEST_URI)) {
            return refuse("manifest_invalid", "the plugin lists no buwiz://manifest");
        }
        let read = conn.request("resources/read", json!({"uri": MANIFEST_URI}), mcp::DEFAULT_TIMEOUT).await?;
        let text = first_text(&read).unwrap_or_default();
        let v = manifest::verify(&text, &self.trust(), manifest::HOST_PROTOCOL)?;
        let listed = conn.request("tools/list", json!({}), mcp::DEFAULT_TIMEOUT).await?;
        let tools = get(&listed, "tools").and_then(Value::as_array).cloned().unwrap_or_default();
        manifest::check_tools(&v.manifest, &tools)?;
        Ok((v.manifest, v.sha256, tools, conn.instructions()))
    }

    /// 5.2 step 6: one publisher per id; `short` unique among enabled plugins.
    fn check_installed(&self, m: &Value, key_id: &str) -> Res<()> {
        let id = tpl(get(m, "id"));
        if let Some(row) = self.row(&id) {
            if row.publisher_key_id != key_id {
                return V1Error::err("manifest_invalid", format!("{id} is installed under publisher key {}", row.publisher_key_id));
            }
        }
        let short = tpl(get(m, "short"));
        let clash: Option<String> = self.sql(|db| db.query_row("SELECT id FROM v1_plugins WHERE short=? AND id<>? AND enabled=1", [&short, &id], |r| r.get(0)).optional())?;
        match clash {
            Some(other) => V1Error::err("manifest_invalid", format!("short {short} is already used by {other}")),
            None => Ok(()),
        }
    }

    // ── running ──────────────────────────────────────────────────────────

    /// Stops the plugin's process; calls in flight fail at once.
    async fn stop_conn(&self, id: &str) {
        let conn = self.with_rt(id, |rt| rt.conn.take());
        if let Some(conn) = conn {
            conn.stopping.store(true, Ordering::SeqCst);
            conn.abandon();
            conn.close().await;
        }
    }

    fn clear_timer(&self, id: &str) {
        if let Some(t) = self.with_rt(id, |rt| rt.timer.take()) {
            self.clock.clear(t);
        }
    }

    fn schedule_restart(&self, id: &str) {
        let now = self.clock.now();
        let delay = self.with_rt(id, |rt| {
            rt.restarts.retain(|t| now - t < RESTART_WINDOW_MS);
            if rt.restarts.len() >= MAX_RESTARTS {
                return None;
            }
            let delay = BACKOFF_S[rt.restarts.len().min(BACKOFF_S.len() - 1)] * 1000;
            rt.restarts.push(now);
            rt.next_delay = Some(delay);
            Some(delay)
        });
        let Some(delay) = delay else {
            return self.set_state(id, "failed", Some("failed"), &format!("restarted {MAX_RESTARTS} times in 10 minutes"));
        };
        let (me, pid) = (self.me.clone(), id.to_owned());
        let timer = self.clock.set_timeout(
            delay,
            Box::new(move || {
                let Some(host) = me.upgrade() else { return };
                tokio::spawn(async move {
                    host.with_rt(&pid, |rt| rt.timer = None);
                    let lock = host.lock_of(&pid);
                    let _held = lock.lock().await;
                    host.spawn_locked(&pid).await;
                });
            }),
        );
        self.with_rt(id, |rt| rt.timer = Some(timer));
        self.set_state(id, "restarting", None, &format!("restarting in {} s", delay / 1000));
    }

    /// The install record against what the plugin now serves (12.1, 13.3).
    fn compare_installed(row: &Row, manifest: &Value, sha256: &str) -> Option<V1Error> {
        if sha256 == row.manifest_sha256 {
            return None;
        }
        if get(manifest, "version").and_then(Value::as_str) == Some(row.version.as_str()) {
            return Some(V1Error::new("manifest_changed", "the manifest changed without a version change"));
        }
        Some(V1Error::new("update_available", format!("the plugin now serves version {}; install it to update", tpl(get(manifest, "version")))))
    }

    fn adopt(&self, id: &str, g: Gate) {
        self.with_rt(id, |rt| {
            rt.conn = Some(g.conn);
            rt.tools = g.tools;
            rt.instructions = g.instructions;
            rt.generation += 1;
        });
        self.set_state(id, "running", None, "");
    }

    fn fail_state(&self, id: &str, e: &V1Error) {
        let state = if e.code == "update_available" { "update_available" } else { "failed" };
        self.set_state(id, state, Some(&e.code), &e.message);
    }

    /// Spawns an installed, enabled plugin through the gate (the caller
    /// holds its lock); a verification failure leaves it `failed`.
    async fn spawn_locked(&self, id: &str) -> &'static str {
        let Some(row) = self.row(id).filter(|r| r.enabled) else { return self.with_rt(id, |rt| rt.state) };
        if self.conn_of(id).is_some() {
            return self.with_rt(id, |rt| rt.state);
        }
        // a record from before a protocol bump: say so, don't spawn a plugin
        // that can only answer the old reserved tools
        if let Some(why) = older_protocol(&row) {
            self.set_state(id, "failed", Some("protocol_unsupported"), &why);
            return "failed";
        }
        if self.with_rt(id, |rt| rt.state) != "restarting" {
            self.set_state(id, "starting", None, "");
        }
        let seed = match self.key(id).await {
            Some(seed) => seed,
            None => {
                self.set_state(id, "failed", Some("plugin_invalid"), "the plugin key is missing from the keystore; reinstall");
                return "failed";
            }
        };
        let g = match self.bootstrap(&row.spec(), &seed, Some(id), id).await {
            Ok(g) => g,
            // a process that would not start or answer counts as a crash; a
            // plugin that answered with something invalid does not loop (14.4)
            Err(Fail::V1(e)) => {
                self.set_state(id, "failed", Some(&e.code), &e.message);
                return "failed";
            }
            Err(Fail::Rpc(_)) => {
                self.schedule_restart(id);
                return self.with_rt(id, |rt| rt.state);
            }
        };
        if !self.row(id).is_some_and(|r| r.enabled) {
            g.conn.close().await;
            return self.with_rt(id, |rt| rt.state);
        }
        let why = Self::compare_installed(&row, &g.manifest, &g.sha256);
        if why.is_none() {
            if let Err(e) = self.check_installed(&g.manifest, &row.publisher_key_id) {
                g.conn.close().await;
                self.set_state(id, "failed", Some(&e.code), &e.message);
                return "failed";
            }
        }
        if let Some(e) = why {
            g.conn.close().await;
            self.fail_state(id, &e);
            return self.with_rt(id, |rt| rt.state);
        }
        self.adopt(id, g);
        "running"
    }

    /// resources/list_changed: read and verify again (5.1, 12.1).
    async fn reread(&self, id: &str, conn: Arc<Conn>) {
        let current = |h: &Host| h.conn_of(id).is_some_and(|c| Arc::ptr_eq(&c, &conn));
        if !current(self) {
            return;
        }
        let Some(row) = self.row(id) else { return };
        let why = match self.read_verified(&conn).await {
            Ok((manifest, sha256, tools, _)) => {
                let why = Self::compare_installed(&row, &manifest, &sha256);
                if why.is_none() {
                    self.with_rt(id, |rt| rt.tools = tools);
                }
                why
            }
            Err(Fail::V1(e)) => Some(e),
            Err(Fail::Rpc(e)) => Some(V1Error::new("manifest_invalid", e.message)),
        };
        let Some(why) = why else { return };
        if !current(self) {
            return;
        }
        self.stop_conn(id).await;
        self.fail_state(id, &why);
    }

    async fn key(&self, id: &str) -> Option<String> {
        let (keys, id) = (self.keys.clone(), id.to_owned());
        tokio::task::spawn_blocking(move || keys.get(&id)).await.ok().and_then(Result::ok).flatten()
    }

    // ── host API ─────────────────────────────────────────────────────────

    fn update_info(&self, row: Option<&Row>, m: &Value) -> Value {
        let Some(row) = row else { return Value::Null };
        let version = tpl(get(m, "version"));
        let order = manifest::compare_semver(&version, &row.version).unwrap_or(0);
        json!({
            "from_version": row.version, "to_version": get(m, "version"),
            "downgrade": order < 0, "same_version": order == 0,
            "added_permissions": manifest::added_permissions(&row.manifest(), m),
        })
    }

    /// 3.1 rule 5: a command another enabled plugin also declares.
    fn ambiguity(&self, m: &Value) -> Vec<String> {
        let mine = get(m, "commands").and_then(Value::as_array).cloned().unwrap_or_default();
        let (id, short) = (get(m, "id").cloned(), tpl(get(m, "short")));
        let mut out = Vec::new();
        for other in self.rows() {
            if !other.enabled || Some(&Value::String(other.id.clone())) == id.as_ref() {
                continue;
            }
            let om = other.manifest();
            for c in get(&om, "commands").and_then(Value::as_array).into_iter().flatten() {
                if mine.iter().any(|x| crate::v1::same(get(x, "name"), get(c, "name"))) {
                    let n = tpl(get(c, "name"));
                    out.push(format!("/{n} is also a command of {}; it will need /{short}:{n}", other.id));
                }
            }
        }
        out
    }

    fn title_of(&self, id: Option<&Value>) -> String {
        let Some(key) = id.and_then(Value::as_str) else { return tpl(id) };
        match self.row(key) {
            Some(row) => get(&row.manifest(), "title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| key.to_owned()),
            None => key.to_owned(),
        }
    }

    /// POST /v1/host/plugins/inspect: a temporary spawn, verified, with the
    /// review of 13.1 step 3. Nothing is installed or stored.
    pub async fn inspect(&self, body: &Value) -> Res {
        let (spec, _) = spawn_spec(body)?;
        let fresh = rox_v1::keys::b64(&rox_v1::keys::random::<32>());
        let g = self.bootstrap(&spec, &fresh, None, "").await.map_err(Fail::into_v1)?;
        g.conn.close().await;
        let m = g.manifest;
        let key_id = get(&m, "publisher").and_then(|p| get(p, "key_id")).map(js::string_of).unwrap_or_default();
        self.check_installed(&m, &key_id)?;
        let row = self.row(&tpl(get(&m, "id")));
        Ok(json!({
            "ok": true, "manifest": m, "manifest_sha256": g.sha256,
            "review": manifest::review(&m, &|id| self.title_of(id)),
            "update": self.update_info(row.as_ref(), &m),
            "warnings": self.ambiguity(&m),
        }))
    }

    fn record(&self, m: &Value, sha256: &str, raw: &(Value, Value), spec: &Spec, public: &str, tools: &[Value], prev: Option<&Row>) -> Res<()> {
        let now = self.clock.now() as i64;
        let tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                let read_only = get(t, "annotations").and_then(|a| get(a, "readOnlyHint")) == Some(&Value::Bool(true));
                json!({"name": get(t, "name"), "description": get(t, "description").filter(|d| js::truthy(d)).cloned().unwrap_or(json!("")), "readOnly": read_only})
            })
            .collect();
        let s = |k: &str| tpl(get(m, k));
        let key_id = get(m, "publisher").and_then(|p| get(p, "key_id")).map(js::string_of).unwrap_or_default();
        self.sql(|db| {
            db.execute(
                "INSERT OR REPLACE INTO v1_plugins(id,short,command,args,env,version,protocol,publisher_key_id,plugin_pub,manifest,manifest_sha256,tools,enabled,disabled_tools,created_at,updated_at)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,1,?,?,?)",
                params![
                    s("id"), s("short"), spec.command, js::stringify(&raw.0), js::stringify(&raw.1), s("version"), s("protocol"), key_id, public,
                    js::stringify(m), sha256, js::stringify(&Value::Array(tools)),
                    prev.map_or("[]", |p| p.disabled_tools.as_str()), prev.map_or(now, |p| if p.created_at != 0 { p.created_at } else { now }), now,
                ],
            )
        })?;
        Ok(())
    }

    /// POST /v1/host/plugins/install: installs what was reviewed (13.1 step
    /// 4) or updates an installed plugin (13.3). `manifest_sha256` must be
    /// the reviewed hash; an update that adds permissions needs
    /// `approve_permissions: true`, an older version `allow_downgrade: true`.
    pub async fn install(&self, body: &Value) -> Res {
        let (spec, raw) = spawn_spec(body)?;
        let wanted = get(body, "manifest_sha256").map(js::string_of).unwrap_or_else(|| "undefined".to_owned());
        if !(wanted.len() == 64 && wanted.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) {
            return V1Error::err("invalid_args", "manifest_sha256 (the reviewed manifest's hash) is required");
        }
        // an update keeps the plugin key; a new install generates one (4.3)
        let fresh = rox_v1::keys::b64(&rox_v1::keys::random::<32>());
        let g0 = self.bootstrap(&spec, &fresh, None, "").await.map_err(Fail::into_v1)?;
        let m = g0.manifest.clone();
        let id = tpl(get(&m, "id"));
        let row = self.row(&id);
        // what comes after the checks: a new key, nothing, or the installed key
        enum Next {
            Fresh,
            Unchanged,
            Update(String),
        }
        let checked: Res<Next> = async {
            if g0.sha256 != wanted {
                return V1Error::err("manifest_invalid", "the plugin serves a manifest other than the one reviewed; inspect it again");
            }
            let key_id = get(&m, "publisher").and_then(|p| get(p, "key_id")).map(js::string_of).unwrap_or_default();
            self.check_installed(&m, &key_id)?;
            let Some(row) = &row else { return Ok(Next::Fresh) };
            if g0.sha256 == row.manifest_sha256 {
                return Ok(Next::Unchanged);
            }
            let u = self.update_info(Some(row), &m);
            if u["same_version"] == true {
                return V1Error::err("manifest_changed", "the manifest changed without a version change");
            }
            if u["downgrade"] == true && get(body, "allow_downgrade") != Some(&Value::Bool(true)) {
                return Err(V1Error::new("invalid_args", format!("{} is older than the installed {}", tpl(get(&m, "version")), row.version)).with_details(json!({"reason": "downgrade"})));
            }
            let added = u["added_permissions"].as_array().cloned().unwrap_or_default();
            if !added.is_empty() && get(body, "approve_permissions") != Some(&Value::Bool(true)) {
                return Err(V1Error::new("consent_declined", "this update adds permissions the user has not approved").with_details(json!({"reason": "permissions_added", "added": added})));
            }
            // the update runs under the installed key: spawn it again with that
            match self.key(&id).await {
                Some(seed) => Ok(Next::Update(seed)),
                None => V1Error::err("plugin_invalid", "the installed plugin key is missing; uninstall and install again"),
            }
        }
        .await;
        let seed = match checked {
            Err(e) => {
                g0.conn.close().await;
                return Err(e);
            }
            Ok(Next::Unchanged) => {
                g0.conn.close().await;
                return Ok(json!({"ok": true, "id": get(&m, "id"), "unchanged": true}));
            }
            Ok(Next::Update(seed)) => {
                g0.conn.close().await;
                seed
            }
            Ok(Next::Fresh) => fresh,
        };

        let lock = self.lock_of(&id);
        let _held = lock.lock().await;
        let g = if row.is_some() {
            // 13.3 step 5-6: stop the old version; its opens end with it
            self.clear_timer(&id);
            self.stop_conn(&id).await;
            self.end_opens(&id).await;
            let g = self.bootstrap(&spec, &seed, Some(&id), &id).await.map_err(Fail::into_v1)?;
            if g.sha256 != wanted {
                g.conn.close().await;
                return V1Error::err("manifest_invalid", "the plugin changed its manifest during the update");
            }
            g
        } else {
            self.put_key(&id, &seed).await?;
            // the reviewed process becomes the running one under the stored key
            g0.conn.stopping.store(true, Ordering::SeqCst);
            g0.conn.close().await;
            match self.bootstrap(&spec, &seed, Some(&id), &id).await {
                Ok(g) => g,
                Err(e) => {
                    self.remove_key(&id).await;
                    return Err(e.into_v1());
                }
            }
        };
        let public = rox_v1::keys::SessionKey::from_seed(&seed).map(|k| k.public()).unwrap_or_default();
        self.record(&m, &g.sha256, &raw, &spec, &public, &g.tools, row.as_ref())?;
        self.with_rt(&id, |rt| rt.restarts.clear());
        self.adopt(&id, g);
        let (state, generation) = self.with_rt(&id, |rt| (rt.state, rt.generation));
        Ok(json!({"ok": true, "id": get(&m, "id"), "state": state, "generation": generation, "plugin_pub": public, "updated": row.is_some()}))
    }

    async fn put_key(&self, id: &str, seed: &str) -> Res<()> {
        let (keys, id, seed) = (self.keys.clone(), id.to_owned(), seed.to_owned());
        tokio::task::spawn_blocking(move || keys.put(&id, &seed)).await.map_err(|e| V1Error::new("internal", e.to_string()))?.map_err(|e| V1Error::new("internal", e))
    }

    async fn remove_key(&self, id: &str) {
        let (keys, id) = (self.keys.clone(), id.to_owned());
        let _ = tokio::task::spawn_blocking(move || keys.remove(&id)).await;
    }

    fn must_row(&self, id: &str) -> Res<Row> {
        self.row(id).ok_or_else(|| V1Error::new("not_found", format!("{} is not installed", js::slice_utf16(id, 80))))
    }

    /// 13.4: end the opens and stop the process; storage, keys and
    /// tickets stay.
    pub async fn disable(&self, id: &str) -> Res {
        self.must_row(id)?;
        let lock = self.lock_of(id);
        let _held = lock.lock().await;
        let now = self.clock.now() as i64;
        self.sql(|db| db.execute("UPDATE v1_plugins SET enabled=0, updated_at=? WHERE id=?", params![now, id]))?;
        self.clear_timer(id);
        self.end_opens(id).await;
        self.stop_conn(id).await;
        self.set_state(id, "stopped", None, "");
        Ok(json!({"ok": true, "state": "stopped"}))
    }

    /// 13.5: spawn and verify again.
    pub async fn enable(&self, id: &str) -> Res {
        let row = self.must_row(id)?;
        self.check_installed(&row.manifest(), &row.publisher_key_id)?;
        let lock = self.lock_of(id);
        let _held = lock.lock().await;
        let now = self.clock.now() as i64;
        self.sql(|db| db.execute("UPDATE v1_plugins SET enabled=1, updated_at=? WHERE id=?", params![now, id]))?;
        self.with_rt(id, |rt| rt.restarts.clear());
        if self.conn_of(id).is_some() {
            let (state, generation) = self.with_rt(id, |rt| (rt.state, rt.generation));
            return Ok(json!({"ok": true, "state": state, "generation": generation}));
        }
        self.clear_timer(id);
        self.spawn_locked(id).await;
        Ok(self.with_rt(id, |rt| {
            json!({"ok": rt.state == "running", "state": rt.state, "code": rt.code, "message": rt.message, "generation": rt.generation})
        }))
    }

    /// 13.6, broker side: buwiz.uninstall (best effort, 5 s), end the opens,
    /// stop, delete the key, remove the record.
    pub async fn uninstall(&self, id: &str) -> Res {
        let row = self.must_row(id)?;
        let lock = self.lock_of(id);
        let _held = lock.lock().await;
        let mut temp = None;
        let conn = match self.conn_of(id) {
            Some(c) => Some(c),
            None => match self.key(id).await {
                Some(seed) => match self.bootstrap(&row.spec(), &seed, None, id).await {
                    Ok(g) => {
                        temp = Some(g.conn.clone());
                        Some(g.conn)
                    }
                    Err(_) => None,
                },
                None => None,
            },
        };
        if let Some(conn) = &conn {
            let _ = self.reserved(conn, "buwiz.uninstall", json!({}), self.timeouts.reserved).await;
        }
        if let Some(t) = temp {
            t.close().await;
        }
        self.end_opens(id).await;
        self.clear_timer(id);
        self.stop_conn(id).await;
        self.remove_key(id).await;
        self.sql(|db| db.execute("DELETE FROM v1_plugins WHERE id=?", [id]))?;
        let generation = self.runtimes.lock().unwrap_or_else(|e| e.into_inner()).remove(id).map_or(0, |rt| rt.generation);
        let gone = json!({
            "id": id, "seq": self.seq.fetch_add(1, Ordering::SeqCst), "state": "uninstalled", "code": null, "message": "",
            "generation": generation, "enabled": false, "tools": [], "instructions": "", "pid": null, "timer": null, "next_delay_ms": null,
        });
        let _ = self.events.send(gone.clone());
        // the answer's snapshot is this one (a fresh one would read `stopped`)
        Ok(json!({"ok": true, "runtime": gone}))
    }

    /// 8.3: the per-tool switch.
    pub fn set_tool(&self, id: &str, tool: Option<&Value>, disabled: Option<&Value>) -> Res {
        let row = self.must_row(id)?;
        let known = parse(&row.tools, json!([]));
        if !known.as_array().into_iter().flatten().any(|t| crate::v1::same(get(t, "name"), tool)) {
            return V1Error::err("not_found", format!("{id} has no tool {}", js::slice_utf16(&tpl(tool), 80)));
        }
        if kind_of(disabled) != "boolean" {
            return V1Error::err("invalid_args", "disabled must be a boolean");
        }
        let name = tpl(tool);
        let mut off: Vec<String> = parse(&row.disabled_tools, json!([])).as_array().into_iter().flatten().map(js::string_of).collect();
        let on = disabled == Some(&Value::Bool(true));
        if on && !off.contains(&name) {
            off.push(name);
        } else if !on {
            off.retain(|t| *t != name);
        }
        let now = self.clock.now() as i64;
        self.sql(|db| db.execute("UPDATE v1_plugins SET disabled_tools=?, updated_at=? WHERE id=?", params![js::stringify(&json!(off)), now, id]))?;
        self.emit(id);
        Ok(json!({"ok": true, "tool": tool, "disabled": disabled}))
    }

    fn running_conn(&self, id: &str) -> Res<Arc<Conn>> {
        match self.with_rt(id, |rt| (rt.state == "running").then(|| rt.conn.clone()).flatten()) {
            Some(c) => Ok(c),
            None => V1Error::err("plugin_unavailable", format!("{id} is not running")),
        }
    }

    /// A call to a running plugin: `_meta` as given, the broker's timeout
    /// with MCP cancellation, `plugin_restarted` when the process goes.
    pub async fn call_tool(&self, id: &str, name: &str, args: &Value, meta: Option<&Value>, timeout: Option<Duration>) -> Res {
        let conn = self.running_conn(id)?;
        let timeout = timeout.unwrap_or(self.timeouts.call);
        match conn.call_tool(name, args, meta, timeout).await {
            Ok(r) => Ok(r),
            Err(e) if e.code == mcp::REQUEST_TIMEOUT => {
                self.hang_check(id, conn);
                let s = (timeout.as_millis() as f64 / 1000.0 + 0.5).floor();
                V1Error::err("timeout", format!("{name} took over {s} s; the broker cancelled it"))
            }
            Err(e) if conn.is_closed() || conn.stopping.load(Ordering::SeqCst) || e.code == mcp::CONNECTION_CLOSED => V1Error::err("plugin_restarted", "the plugin restarted during the call"),
            Err(e) if e.code == mcp::METHOD_NOT_FOUND => V1Error::err("not_found", format!("{id} has no tool {name}")),
            Err(e) => V1Error::err("internal", format!("{name}: {}", js::slice_utf16(&e.message, 300))),
        }
    }

    /// 14.4: a call past the broker limit is cancelled; a plugin that does
    /// not answer a ping within 5 s after that is restarted.
    fn hang_check(&self, id: &str, conn: Arc<Conn>) {
        let (me, id, wait) = (self.me.clone(), id.to_owned(), self.timeouts.cancel);
        tokio::spawn(async move {
            if conn.request("ping", json!({}), wait).await.is_err() {
                let current = me.upgrade().is_some_and(|h| h.conn_of(&id).is_some_and(|c| Arc::ptr_eq(&c, &conn)));
                if current {
                    conn.close().await;
                }
            }
        });
    }

    /// A reserved tool on a running plugin; -32601 is `null` (7.9).
    pub async fn call_reserved(&self, id: &str, name: &str, args: Value) -> Res {
        let conn = self.running_conn(id)?;
        match self.reserved(&conn, name, args, self.timeouts.reserved).await {
            Ok(v) => Ok(v),
            Err(Fail::Rpc(e)) if e.code == mcp::METHOD_NOT_FOUND => Ok(Value::Null),
            Err(Fail::V1(e)) => Err(e),
            Err(Fail::Rpc(e)) if e.code == mcp::REQUEST_TIMEOUT => V1Error::err("timeout", format!("{name} did not answer in time")),
            Err(Fail::Rpc(e)) => V1Error::err("plugin_unavailable", format!("{name}: {}", js::slice_utf16(&e.message, 200))),
        }
    }

    pub async fn read_resource(&self, id: &str, uri: &str) -> Res {
        let conn = self.running_conn(id)?;
        conn.request("resources/read", json!({"uri": uri}), mcp::DEFAULT_TIMEOUT).await.map_err(|e| V1Error::new("internal", e.message))
    }

    /// Marks a plugin failed (a bad certificate: 4.4 step 4, 14.4).
    pub async fn fail(&self, id: &str, code: &str, message: &str) -> Res {
        self.stop_conn(id).await;
        let code = code.to_owned();
        self.with_rt(id, |rt| {
            rt.state = "failed";
            rt.code = Some(code);
            rt.message = js::slice_utf16(message, 500);
        });
        self.emit(id);
        Ok(json!({"ok": true}))
    }

    /// Spawns every enabled plugin, all at once (the broker's start).
    pub async fn start_all(&self) {
        let Some(me) = self.me.upgrade() else { return };
        let starts: Vec<_> = self
            .rows()
            .into_iter()
            .filter(|r| r.enabled)
            .map(|r| {
                let host = me.clone();
                tokio::spawn(async move {
                    let lock = host.lock_of(&r.id);
                    let _held = lock.lock().await;
                    host.spawn_locked(&r.id).await;
                })
            })
            .collect();
        for s in starts {
            let _ = s.await;
        }
    }

    /// Stops every process (shutdown, tests).
    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.runtimes.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        for id in ids {
            self.clear_timer(&id);
            self.stop_conn(&id).await;
        }
    }
}

/// A spawn spec from a request body: stdio only (14.3). Also the body's
/// `args` and `env` as given, for the record.
pub fn spawn_spec(body: &Value) -> Res<(Spec, (Value, Value))> {
    let Some(o) = body.as_object() else { return V1Error::err("invalid_args", "send {command, args, env?}") };
    if o.contains_key("url") || o.get("transport").is_some_and(|t| t != "stdio") {
        return V1Error::err("not_supported", "v1 plugins run over stdio only");
    }
    let Some(command) = o.get("command").and_then(Value::as_str).filter(|c| !c.is_empty()) else {
        return V1Error::err("invalid_args", "command must be a non-empty string");
    };
    let args = o.get("args").filter(|a| !a.is_null()).cloned().unwrap_or(json!([]));
    let Some(list) = args.as_array().filter(|a| a.iter().all(Value::is_string)) else {
        return V1Error::err("invalid_args", "args must be an array of strings");
    };
    let env = o.get("env").filter(|e| !e.is_null()).cloned().unwrap_or(json!({}));
    let Some(map) = env.as_object().filter(|m| m.values().all(Value::is_string)) else {
        return V1Error::err("invalid_args", "env must map names to strings");
    };
    let spec = Spec {
        inherit_all: false,
        client: "rox-broker",
        command: command.to_owned(),
        args: list.iter().map(js::string_of).collect(),
        env: js::ordered(map).into_iter().map(|(k, v)| (k.clone(), js::string_of(v))).collect(),
    };
    Ok((spec, (args, env)))
}

/// Text of an MCP tool result as JSON: structuredContent, else the first
/// text parsed. Reserved tools answer this way.
pub fn result_data(r: &Value) -> Value {
    if let Some(s) = get(r, "structuredContent").filter(|s| s.is_object()) {
        return s.clone();
    }
    let text = get(r, "content").and_then(Value::as_array).into_iter().flatten().find(|c| get(c, "type") == Some(&json!("text"))).and_then(|c| get(c, "text")).and_then(Value::as_str);
    text.map(|t| parse(t, Value::Null)).unwrap_or(Value::Null)
}

/// The first text of a resources/read result.
fn first_text(read: &Value) -> Option<String> {
    get(read, "contents").and_then(Value::as_array)?.iter().find_map(|c| get(c, "text").and_then(Value::as_str)).map(str::to_owned)
}

async fn peek_id(conn: &Conn) -> String {
    let Ok(read) = conn.request("resources/read", json!({"uri": MANIFEST_URI}), mcp::DEFAULT_TIMEOUT).await else { return String::new() };
    first_text(&read).map(|t| parse(&t, json!({}))).and_then(|m| get(&m, "id").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default()
}

// ── a connection's hooks ─────────────────────────────────────────────────

struct ConnHooks {
    host: Weak<Host>,
    /// the plugin whose runtime this connection is meant to be
    running: Option<String>,
    conn: OnceLock<Weak<Conn>>,
    sink: Mutex<Sink>,
    leaked: AtomicBool,
}

impl ConnHooks {
    /// The host and plugin id, when this connection is that plugin's
    /// running one.
    fn current(&self) -> Option<(Arc<Host>, String, Arc<Conn>)> {
        let (host, id) = (self.host.upgrade()?, self.running.clone()?);
        let conn = self.conn.get()?.upgrade()?;
        host.conn_of(&id).filter(|c| Arc::ptr_eq(c, &conn)).map(|_| (host, id, conn))
    }
}

impl Hooks for ConnHooks {
    fn stderr(&self, chunk: &str) {
        let leaked = self.sink.lock().unwrap_or_else(|e| e.into_inner()).write(chunk);
        if !leaked {
            return;
        }
        self.leaked.store(true, Ordering::SeqCst);
        // a plugin that prints its key fails conformance (4.3) and is stopped
        if let Some((host, id, _)) = self.current() {
            tokio::spawn(async move {
                host.stop_conn(&id).await;
                host.set_state(&id, "failed", Some("plugin_invalid"), "the plugin wrote its key to stderr (4.3)");
            });
        }
    }

    fn list_changed(&self) {
        if let Some((host, id, conn)) = self.current() {
            tokio::spawn(async move { host.reread(&id, conn).await });
        }
    }

    fn closed(&self) {
        let Some(conn) = self.conn.get().and_then(Weak::upgrade) else { return };
        if conn.stopping.load(Ordering::SeqCst) {
            return;
        }
        if let Some((host, id, _)) = self.current() {
            host.with_rt(&id, |rt| rt.conn = None);
            host.schedule_restart(&id);
        }
    }
}

/// stderr → the plugin's log (5 MiB, one rotation), with the seed redacted.
struct Sink {
    file: Option<PathBuf>,
    variants: [String; 2],
    carry: String,
}

impl Sink {
    fn new(file: Option<PathBuf>, seed: &str) -> Self {
        let std = rox_v1::keys::unb64(seed).map(|b| rox_v1::keys::b64_std(&b).trim_end_matches('=').to_owned()).unwrap_or_default();
        Sink { file, variants: [seed.to_owned(), std], carry: String::new() }
    }

    /// Writes the chunk's whole lines; whether one held the key.
    fn write(&mut self, chunk: &str) -> bool {
        let text = format!("{}{chunk}", self.carry);
        let mut lines: Vec<&str> = text.split('\n').collect();
        let last = lines.pop().unwrap_or("");
        let skip = last.chars().count().saturating_sub(4096);
        self.carry = last.chars().skip(skip).collect();
        let mut leaked = false;
        for line in lines {
            let mut line = line.to_owned();
            if self.variants.iter().any(|v| !v.is_empty() && line.contains(v.as_str())) {
                for v in self.variants.iter().filter(|v| !v.is_empty()) {
                    line = line.replace(v.as_str(), "[redacted key]");
                }
                leaked = true;
            }
            let Some(file) = &self.file else { continue };
            let _ = (|| -> std::io::Result<()> {
                use std::io::Write as _;
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                if std::fs::metadata(file).is_ok_and(|m| m.len() > LOG_CAP) {
                    let mut rotated = file.clone().into_os_string();
                    rotated.push(".1");
                    std::fs::rename(file, rotated)?;
                }
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
                writeln!(f, "{} {line}", js::iso(now))
            })();
        }
        leaked
    }
}

/// Why an installed plugin can't start under this host: its record names
/// an older protocol major (a plugin built before a protocol bump).
fn older_protocol(row: &Row) -> Option<String> {
    let major = |p: &str| p.split('.').next().and_then(|m| m.parse::<u64>().ok());
    let (theirs, ours) = (major(&row.protocol)?, major(manifest::HOST_PROTOCOL)?);
    if theirs >= ours {
        return None;
    }
    let title = get(&row.manifest(), "title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| row.id.clone());
    Some(format!(
        "{title} {} ({}) was made for an older Buwiz Bot (mini app protocol {}; this one runs {}), so it can't start. Install its current version, or uninstall it.",
        row.version,
        row.id,
        row.protocol,
        manifest::HOST_PROTOCOL
    ))
}

/// An RPC error's message with a repeated `MCP error <code>: ` prefix
/// dropped (a plugin whose own message already carries it).
fn once_prefixed(msg: &str) -> &str {
    match msg.strip_prefix("MCP error ").and_then(|r| r.split_once(": ")).map(|(_, r)| r) {
        Some(inner) if inner.starts_with("MCP error ") => inner,
        _ => msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_of(id: &str, protocol: &str) -> Row {
        Row {
            id: id.into(),
            short: "gws".into(),
            command: "x".into(),
            args: "[]".into(),
            env: "{}".into(),
            version: "1.1.0".into(),
            protocol: protocol.into(),
            publisher_key_id: "k".into(),
            plugin_pub: String::new(),
            manifest: json!({"title": "Google Workspace"}).to_string(),
            manifest_sha256: String::new(),
            tools: "[]".into(),
            enabled: true,
            disabled_tools: "[]".into(),
            created_at: 0,
        }
    }

    #[test]
    fn a_record_from_an_older_protocol_says_why_it_cannot_start() {
        let old = row_of("dev.goldcoders.google-workspace", "1.0.0");
        assert_eq!(
            older_protocol(&old).unwrap(),
            format!("Google Workspace 1.1.0 (dev.goldcoders.google-workspace) was made for an older Buwiz Bot (mini app protocol 1.0.0; this one runs {}), so it can't start. Install its current version, or uninstall it.", manifest::HOST_PROTOCOL)
        );
        assert_eq!(older_protocol(&row_of("app.buwiz.google-workspace", manifest::HOST_PROTOCOL)), None);
        assert_eq!(older_protocol(&row_of("x", "not a version")), None, "unreadable: the gate decides");
    }

    #[test]
    fn a_doubled_mcp_error_prefix_is_said_once() {
        assert_eq!(once_prefixed("MCP error -32600: MCP error -32600: rox.provision must come first"), "MCP error -32600: rox.provision must come first");
        assert_eq!(once_prefixed("MCP error -32001: Request timed out"), "MCP error -32001: Request timed out");
        assert_eq!(once_prefixed("refused"), "refused");
    }

    #[test]
    fn a_spawn_spec_is_stdio_only_with_string_args_and_env() {
        let (spec, raw) = spawn_spec(&json!({"command": "node", "args": ["a"], "env": {"B": "1", "A": "2"}})).unwrap();
        assert_eq!(spec.args, ["a"]);
        assert_eq!(spec.env, [("B".to_owned(), "1".to_owned()), ("A".to_owned(), "2".to_owned())]);
        assert_eq!(raw.1, json!({"B": "1", "A": "2"}));
        assert_eq!(spawn_spec(&json!({"command": "x", "args": null})).unwrap().0.args, Vec::<String>::new());
        for (body, code) in [
            (json!([]), "invalid_args"),
            (json!({"url": "http://x", "command": "x"}), "not_supported"),
            (json!({"transport": "sse", "command": "x"}), "not_supported"),
            (json!({"command": ""}), "invalid_args"),
            (json!({"command": "x", "args": [1]}), "invalid_args"),
            (json!({"command": "x", "env": {"A": 1}}), "invalid_args"),
        ] {
            assert_eq!(spawn_spec(&body).unwrap_err().code, code, "{body}");
        }
    }

    #[test]
    fn result_data_prefers_structured_content_then_the_first_text() {
        assert_eq!(result_data(&json!({"structuredContent": {"a": 1}, "content": [{"type": "text", "text": "{\"b\":2}"}]})), json!({"a": 1}));
        assert_eq!(result_data(&json!({"content": [{"type": "image"}, {"type": "text", "text": "{\"b\":2}"}]})), json!({"b": 2}));
        assert_eq!(result_data(&json!({"content": [{"type": "text", "text": "not json"}]})), Value::Null);
    }

    #[test]
    fn the_sink_redacts_both_forms_of_the_seed_and_carries_partial_lines() {
        let seed = rox_v1::keys::b64(&[0xfb; 32]);
        let std = rox_v1::keys::b64_std(&[0xfb; 32]).trim_end_matches('=').to_owned();
        let dir = std::env::temp_dir().join(format!("rox-sink-{}", std::process::id()));
        let file = dir.join("p.log");
        let mut sink = Sink::new(Some(file.clone()), &seed);
        assert!(!sink.write("hello "));
        assert!(sink.write(&format!("key {seed}\nstd {std}")));
        assert!(sink.write("\n"));
        let log = std::fs::read_to_string(&file).unwrap();
        assert!(!log.contains(&seed) && !log.contains(&std));
        assert_eq!(log.matches("[redacted key]").count(), 2);
        assert!(log.contains("hello key [redacted key]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_manual_clock_fires_the_oldest_timer_when_told() {
        let c = Clock::manual();
        let hits = Arc::new(Mutex::new(Vec::new()));
        for n in [1, 2] {
            let h = hits.clone();
            c.set_timeout(1000, Box::new(move || h.lock().unwrap().push(n)));
        }
        assert!(c.manual_step(Some(5.0), false) == false && c.now() == 5.0);
        assert!(c.manual_step(None, true));
        assert_eq!(*hits.lock().unwrap(), [1]);
    }
}
