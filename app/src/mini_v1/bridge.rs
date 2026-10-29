//! The host half of the v1 bridge (spec 7, design §2): one `nsh/host`
//! channel per open, with handlers for only the methods the app's
//! permissions grant. Identity (plugin, app, open, session key) is in the
//! handlers' [`OpenContext`], never in what the app sends, so an app
//! cannot speak for another.
//!
//! Every rule is enforced here, whatever the shim does: the permission
//! lists, the size limits (args 256 KiB, results 1 MiB), at most 8 calls in
//! flight per open with the rest queued in order, 20 new calls a second
//! (`rate_limited`), 30 s per call (`timeout`). Handlers run on the engine
//! thread and must not block it: each moves its reply to a thread of its
//! own (the broker, the file system) or to the person ([`HostUi`]).
//!
//! What needs the person goes through [`HostUi`]: consent (part B draws
//! the dialogs; tests answer them), the full-screen stack `rox.open({app})`
//! pushes on, the file dialog, the pasteboard, the chat an attachment goes
//! to, the document view `rox.preview` shows.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use nsh_engine::{HostCall, HostChannel, HostReply};
use serde_json::{Map, Value, json};

use super::{
    RoxError, V1Key, host_api,
    keys,
    load::Prepared,
    perms,
    session::{Ended, Session},
    storage::AppStorage,
};

/// What needs the person, or the app around the bridge.
pub trait HostUi: Send + Sync {
    /// Asks the person (spec 9.7); `answer(true)` goes ahead. Any thread.
    fn consent(&self, ask: Consent, answer: Box<dyn FnOnce(bool) + Send>);
    /// Pushes `target` on the full-screen stack (`rox.open({app})`: the
    /// person is in the plugin already, so no consent).
    fn open_app(&self, target: V1Key);
    /// Opens an https URL the person agreed to.
    fn open_url(&self, url: &str);
    /// Whether the open has a chat to attach to (7.6).
    fn has_chat(&self) -> bool;
    /// Attaches canonical `token` (titled `title`) to the open's chat:
    /// false when it was attached already.
    fn attach(&self, token: &str, title: &str) -> bool;
    /// Shows the native file dialog; `answer` gets the files chosen (none
    /// on Cancel).
    fn pick_files(&self, pick: PickRequest, answer: Box<dyn FnOnce(Vec<PathBuf>) + Send>);
    /// Puts `text` on the pasteboard.
    fn copy(&self, text: &str);
    /// Shows `doc` over the app: its title, Print and Close, the page in a
    /// web view that runs no script and loads nothing. Hosts without one
    /// refuse `not_supported`.
    fn preview(&self, doc: Preview) -> Result<(), RoxError> {
        let _ = doc;
        Err(RoxError::new("not_supported", "this Rox shows no print previews"))
    }
    /// The open's session ended for `why` (a plugin restart shows
    /// "Reconnecting", 13.7).
    fn session_ended(&self, key: &V1Key, why: Ended);
    /// Sets the inline open's card to `height` (`rox.fit`, already
    /// clamped and paced: [`Fitter`]). Hosts without inline cards ignore
    /// it.
    fn fit(&self, height: f32) {
        let _ = height;
    }
}

/// A question for the person (spec 9.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Consent {
    /// "Open <url> in your browser?", from the app titled `app`.
    OpenUrl { url: String, app: String },
    /// "Attach <title> to this chat?", from the app titled `app`.
    Attach { title: String, app: String },
}

/// What the file dialog offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRequest {
    /// Lower-case, with the dot.
    pub extensions: Vec<String>,
    pub multiple: bool,
    /// Larger files are not offered.
    pub max_bytes: u64,
}

/// A document `rox.preview` shows: its title and the page, sealed
/// ([`seal`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub title: String,
    pub html: String,
}

/// The bridge's limits (spec 7.3, Appendix A); tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_args: usize,
    pub max_result: usize,
    /// A `rox.preview` document's HTML (the app's `preview.max_bytes` may
    /// lower it).
    pub max_preview: usize,
    pub in_flight: usize,
    pub per_second: usize,
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_args: 256 * 1024,
            max_result: 1024 * 1024,
            max_preview: perms::MAX_PREVIEW as usize,
            in_flight: 8,
            per_second: 20,
            timeout: host_api::CALL_TIMEOUT,
        }
    }
}

/// The most text `rox.copy` takes.
const MAX_COPY: usize = 1024 * 1024;
/// The longest URL `rox.open` takes (Appendix A).
const MAX_URL: usize = 2048;
/// The longest title a preview shows.
const MAX_TITLE: usize = 120;

/// A file the person picked: only its id reaches the app.
struct Picked {
    path: PathBuf,
    max_bytes: u64,
}

/// The calls of one open: those running, those waiting, and when the
/// recent ones started.
#[derive(Default)]
struct Gate {
    running: usize,
    waiting: VecDeque<Box<dyn FnOnce() + Send>>,
    recent: VecDeque<Instant>,
}

/// `rox.fit`'s pacing (6.5, 7.11): a height within [`FIT_STEP`] of the
/// one set changes nothing, and at most [`FIT_PER_SECOND`] go out a
/// second; past that the latest waits for the next free slot, so the card
/// still ends at the app's last height.
pub const FIT_STEP: f32 = 2.0;
pub const FIT_PER_SECOND: usize = 8;

/// One inline open's `rox.fit` heights: the one set, when the recent ones
/// went out, and the one waiting past the limit.
#[derive(Debug, Default)]
pub struct Fitter {
    set: Option<f32>,
    recent: VecDeque<Instant>,
    waiting: Option<f32>,
}

impl Fitter {
    /// Height `h` (clamped) asked at `now`: `Some` to set it now; `None`
    /// when it changes nothing, or waits ([`Fitter::next`] says when).
    pub fn ask(&mut self, h: f32, now: Instant) -> Option<f32> {
        while self.recent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1)) {
            self.recent.pop_front();
        }
        if self.set.is_some_and(|s| (s - h).abs() < FIT_STEP) {
            self.waiting = None;
            return None;
        }
        if self.recent.len() >= FIT_PER_SECOND {
            self.waiting = Some(h);
            return None;
        }
        self.recent.push_back(now);
        self.set = Some(h);
        self.waiting = None;
        Some(h)
    }

    /// When the waiting height may go out, if one waits.
    pub fn next(&self) -> Option<Instant> {
        self.waiting?;
        self.recent.front().map(|t| *t + Duration::from_secs(1))
    }

    /// The waiting height, asked again at `now`.
    pub fn retry(&mut self, now: Instant) -> Option<f32> {
        let h = self.waiting.take()?;
        self.ask(h, now)
    }
}

/// One open's bridge state, shared by its handlers.
pub struct OpenContext {
    pub prepared: Arc<Prepared>,
    pub session: Arc<Session>,
    broker: Arc<dyn host_api::Broker>,
    ui: Arc<dyn HostUi>,
    storage: Option<AppStorage>,
    limits: Limits,
    gate: Mutex<Gate>,
    files: Mutex<HashMap<String, Picked>>,
    /// Storage operations, in the order the app made them.
    store_jobs: Mutex<Option<mpsc::Sender<Box<dyn FnOnce() + Send>>>>,
}

impl OpenContext {
    /// The bridge state of an open of `prepared` with `session`; its store
    /// under `plugins_dir` when storage is granted.
    pub fn new(
        prepared: Arc<Prepared>,
        session: Arc<Session>,
        broker: Arc<dyn host_api::Broker>,
        ui: Arc<dyn HostUi>,
        plugins_dir: &std::path::Path,
        limits: Limits,
    ) -> Arc<OpenContext> {
        let storage = prepared
            .permissions
            .storage
            .map(|quota| AppStorage::new(plugins_dir, &prepared.plugin.id, &prepared.key.app, quota));
        Arc::new(OpenContext {
            prepared,
            session,
            broker,
            ui,
            storage,
            limits,
            gate: Mutex::new(Gate::default()),
            files: Mutex::new(HashMap::new()),
            store_jobs: Mutex::new(None),
        })
    }

    /// Ends the open: its session closes (the broker is told), its picked
    /// files can no longer be read, its storage worker stops.
    pub fn close(&self) {
        self.session.close(self.broker.clone());
        self.files.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.store_jobs.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// `host.params` of an open: `rox.props`, `rox.app`, the granted methods
/// and the first viewport.
pub fn params(prepared: &Prepared, viewport: Value) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("props".to_owned(), Value::Object(prepared.key.props.clone()));
    p.insert("app".to_owned(), prepared.app_info());
    p.insert("granted".to_owned(), json!(prepared.permissions.methods()));
    p.insert("viewport".to_owned(), viewport);
    p
}

/// Wires `channel` for the open `ctx`: its parameters, a handler for each
/// granted method and nothing else, and the session's end.
pub fn install(channel: &HostChannel, ctx: &Arc<OpenContext>, viewport: Value) {
    let inline = viewport.get("placement").and_then(Value::as_str) == Some("inline");
    channel.set_params(params(&ctx.prepared, viewport));
    install_fit(channel, ctx, inline);
    for method in ctx.prepared.permissions.methods().into_iter().filter(|m| *m != perms::FIT) {
        let c = ctx.clone();
        channel.handle(method, move |mut call: HostCall| {
            let data = call.take_data();
            // a post has no answer to give: the bridge only takes calls
            let Some(reply) = call.into_reply() else { return };
            c.dispatch(method, data, reply);
        });
    }
    let (ch, ui, key) = (channel.clone(), ctx.ui.clone(), ctx.prepared.key.clone());
    ctx.session.on_end(move |why| {
        if why == Ended::Restarted {
            let _ = ch.send("rox.session", json!({"state": "reconnecting"}));
        }
        ui.session_ended(&key, why);
    });
}

/// `rox.fit({height})`, a post: an `inline` open's card takes the height,
/// clamped to its manifest's `inline` range and paced by a [`Fitter`]; a
/// full-screen open's is ignored. It needs no permission: it sizes only
/// the app's own frame.
fn install_fit(channel: &HostChannel, ctx: &Arc<OpenContext>, inline: bool) {
    let app = ctx.prepared.plugin.app(&ctx.prepared.key.app).cloned().unwrap_or(Value::Null);
    let (ui, fitter) = (ctx.ui.clone(), Arc::new(Mutex::new(Fitter::default())));
    channel.handle(perms::FIT, move |mut call: HostCall| {
        let data = call.take_data();
        if let Some(reply) = call.into_reply() {
            reply.resolve(Value::Null);
        }
        let Some(h) = data.get("height").and_then(Value::as_f64).filter(|h| h.is_finite() && *h > 0.0) else { return };
        if !inline {
            return;
        }
        let h = super::cards::fit_height(&app, h.ceil() as f32);
        let mut f = fitter.lock().unwrap_or_else(|e| e.into_inner());
        let waited = f.next().is_some();
        if let Some(h) = f.ask(h, Instant::now()) {
            return ui.fit(h);
        }
        // the first to wait sets a timer for the slot; later ones only
        // replace the height it sends
        let Some(mut at) = f.next().filter(|_| !waited) else { return };
        let (ui, fitter) = (ui.clone(), fitter.clone());
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(at.saturating_duration_since(Instant::now()));
                let mut f = fitter.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(h) = f.retry(Instant::now()) {
                    ui.fit(h);
                }
                let Some(next) = f.next() else { return };
                at = next;
            }
        });
    });
}

fn reject(reply: HostReply, e: RoxError) {
    reply.reject(e.to_wire());
}

fn answer(reply: HostReply, r: Result<Value, RoxError>) {
    match r {
        Ok(v) => reply.resolve(v),
        Err(e) => reject(reply, e),
    }
}

fn invalid(m: &str) -> RoxError {
    RoxError::new("invalid_args", m)
}

fn denied(m: &str) -> RoxError {
    RoxError::new("permission_denied", m)
}

/// Field `k` of `data` as a string.
fn text<'a>(data: &'a Value, k: &str) -> Option<&'a str> {
    data.get(k).and_then(Value::as_str)
}

impl OpenContext {
    /// Runs granted method `method`'s handler.
    fn dispatch(self: &Arc<Self>, method: &'static str, data: Value, reply: HostReply) {
        match method {
            perms::CALL => self.call(data, reply),
            perms::TICKET_TITLE => self.ticket_title(data, reply),
            perms::OPEN => self.open(data, reply),
            perms::ATTACH => self.attach(data, reply),
            perms::PICK => self.pick(data, reply),
            perms::COPY => self.copy(data, reply),
            perms::PREVIEW => self.preview(data, reply),
            storage => self.storage_named(storage, data, reply),
        }
    }

    /// Counts a new call against the per-second limit.
    fn admit(&self) -> Result<(), RoxError> {
        let mut g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        while g.recent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1)) {
            g.recent.pop_front();
        }
        if g.recent.len() >= self.limits.per_second {
            return Err(RoxError::new("rate_limited", "too many calls; try again in a moment"));
        }
        g.recent.push_back(now);
        Ok(())
    }

    /// Runs `job` on a thread once fewer than the limit run; waits in
    /// order otherwise.
    fn run_gated(self: &Arc<Self>, job: Box<dyn FnOnce() + Send>) {
        let mut g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        if g.running >= self.limits.in_flight {
            g.waiting.push_back(job);
            return;
        }
        g.running += 1;
        drop(g);
        self.spawn_gated(job);
    }

    fn spawn_gated(self: &Arc<Self>, job: Box<dyn FnOnce() + Send>) {
        let me = self.clone();
        std::thread::spawn(move || {
            job();
            let next = {
                let mut g = me.gate.lock().unwrap_or_else(|e| e.into_inner());
                let next = g.waiting.pop_front();
                if next.is_none() {
                    g.running -= 1;
                }
                next
            };
            if let Some(next) = next {
                me.spawn_gated(next);
            }
        });
    }

    /// A reply that a timer rejects with `timeout` unless it is answered
    /// first: `(answer, done)`; `done` stops the timer.
    fn timed(&self, reply: HostReply) -> (Arc<Mutex<Option<HostReply>>>, mpsc::Sender<()>) {
        let slot = Arc::new(Mutex::new(Some(reply)));
        let (done, rx) = mpsc::channel::<()>();
        let (s, timeout) = (slot.clone(), self.limits.timeout);
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(timeout)
                && let Some(r) = s.lock().unwrap_or_else(|e| e.into_inner()).take()
            {
                reject(r, RoxError::new("timeout", format!("no answer in {} s", timeout.as_secs())));
            }
        });
        (slot, done)
    }

    /// Admits a broker request (rate, then the in-flight gate, then the
    /// timer) and answers `reply` with what `work` returns.
    fn gated(
        self: &Arc<Self>,
        reply: HostReply,
        work: impl FnOnce(&OpenContext) -> Result<Value, RoxError> + Send + 'static,
    ) {
        if let Err(e) = self.admit() {
            return reject(reply, e);
        }
        let (slot, done) = self.timed(reply);
        let me = self.clone();
        self.run_gated(Box::new(move || {
            // timed out while it waited: nothing to send
            if slot.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                return;
            }
            let r = work(&me);
            if let Some(reply) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                answer(reply, r);
            }
            let _ = done.send(());
        }));
    }

    /// `rox.call(tool, args)` (7.3).
    fn call(self: &Arc<Self>, data: Value, reply: HostReply) {
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        let tool = text(&data, "tool").unwrap_or_default().to_owned();
        if !self.prepared.permissions.tools.contains(&tool) {
            let _ = self.admit();
            return reject(reply, denied(&format!("{tool:?} is not in this app's tools")));
        }
        let args = match data.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(a @ Value::Object(_)) => a.clone(),
            Some(_) => return reject(reply, invalid("args is a JSON object")),
        };
        if args.to_string().len() > self.limits.max_args {
            return reject(reply, invalid("args are over 256 KiB"));
        }
        let max = self.limits.max_result;
        self.gated(reply, move |me| {
            // signed when it goes out: a queued call is not stale
            let (envelope, sig) = me.session.sign_call(&tool, &args)?;
            let result = host_api::call(&*me.broker, &envelope, &sig, me.limits.timeout);
            // which signed call went where, never its arguments or result
            let p = &me.prepared;
            match &result {
                Ok(_) => log::info!("mini v1: {}/{} signed call {tool}: ok", p.plugin.id, p.key.app),
                Err(e) => log::info!("mini v1: {}/{} signed call {tool}: {}", p.plugin.id, p.key.app, e.code),
            }
            tool_result(result?, max)
        });
    }

    /// `rox.ticket.title(ticket)` (7.4), rate-limited as calls are.
    fn ticket_title(self: &Arc<Self>, data: Value, reply: HostReply) {
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        let ticket = text(&data, "ticket").unwrap_or_default().to_owned();
        if !perms::ticket_ok(&ticket) {
            return reject(reply, invalid("not a ticket"));
        }
        self.gated(reply, move |me| {
            let title = host_api::ticket_title(&*me.broker, &me.prepared.plugin.id, &ticket)?;
            Ok(title.map_or(Value::Null, Value::String))
        });
    }

    /// `rox.open({app, props?, ticket?})` or `rox.open({url})` (7.5).
    fn open(self: &Arc<Self>, data: Value, reply: HostReply) {
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        let grant = self.prepared.permissions.open.clone().unwrap_or_default();
        let r = match (data.get("app"), data.get("url")) {
            (Some(_), None) => self.open_app(&grant, &data),
            (None, Some(_)) => return self.open_url(&grant, &data, reply),
            _ => Err(invalid("rox.open takes {app} or {url}")),
        };
        answer(reply, r.map(|()| Value::Null));
    }

    fn open_app(&self, grant: &perms::OpenGrant, data: &Value) -> Result<(), RoxError> {
        let o = data.as_object().ok_or_else(|| invalid("rox.open takes an object"))?;
        if let Some(k) = o.keys().find(|k| !["app", "props", "ticket"].contains(&k.as_str())) {
            return Err(invalid(&format!("rox.open does not take {k}")));
        }
        let app = text(data, "app").ok_or_else(|| invalid("app is a string"))?;
        if !grant.apps.iter().any(|a| a == app) {
            return Err(denied(&format!("{app:?} is not in this app's open.apps")));
        }
        let target = self.prepared.plugin.app(app).ok_or_else(|| {
            RoxError::new("not_declared", format!("{} has no app {app:?}", self.prepared.plugin.title))
        })?;
        let props = match data.get("props") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(p)) => p.clone(),
            Some(_) => return Err(invalid("props is a JSON object")),
        };
        super::props::validate(&props, target.get("props"))?;
        let ticket = match data.get("ticket") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) if perms::ticket_ok(t) => Some(t.clone()),
            Some(_) => return Err(invalid("not a ticket")),
        };
        let plugin = self.prepared.plugin.id.clone();
        self.ui.open_app(V1Key { plugin, app: app.to_owned(), ticket, props });
        Ok(())
    }

    fn open_url(self: &Arc<Self>, grant: &perms::OpenGrant, data: &Value, reply: HostReply) {
        let url = text(data, "url").unwrap_or_default().to_owned();
        if url.len() > MAX_URL {
            return reject(reply, invalid("a URL is at most 2048 characters"));
        }
        if !url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) {
            return reject(reply, RoxError::new("not_supported", "apps open https URLs only"));
        }
        let Some(origin) = perms::https_origin(&url).filter(|_| !url.contains(char::is_whitespace)) else {
            return reject(reply, invalid("not a URL"));
        };
        if !grant.external.contains(&origin) {
            return reject(reply, denied(&format!("{origin} is not in this app's open.external")));
        }
        let ask = Consent::OpenUrl { url: url.clone(), app: self.prepared.title.clone() };
        let ui = self.ui.clone();
        self.ui.consent(
            ask,
            Box::new(move |yes| {
                if !yes {
                    return reject(reply, RoxError::new("consent_declined", "the person said no"));
                }
                ui.open_url(&url);
                reply.resolve(Value::Null);
            }),
        );
    }

    /// `rox.attach(scope, value)` (7.6): resolved by the plugin, then asked.
    fn attach(self: &Arc<Self>, data: Value, reply: HostReply) {
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        let scope = text(&data, "scope").unwrap_or_default().to_owned();
        if !self.prepared.permissions.attach.contains(&scope) || !self.prepared.plugin.scope_attaches(&scope) {
            return reject(reply, denied(&format!("{scope:?} is not in this app's attach")));
        }
        let Some(value) = text(&data, "value").filter(|v| v.len() <= MAX_URL).map(str::to_owned) else {
            return reject(reply, invalid("value is a string"));
        };
        if !self.ui.has_chat() {
            return reject(reply, RoxError::new("no_chat", "this app was opened without a chat"));
        }
        let me = self.clone();
        std::thread::spawn(move || match me.resolve_token(&scope, &value) {
            Err(e) => reject(reply, e),
            Ok((token, title)) => {
                let ask = Consent::Attach { title: title.clone(), app: me.prepared.title.clone() };
                let ui = me.ui.clone();
                me.ui.consent(
                    ask,
                    Box::new(move |yes| {
                        if !yes {
                            return reject(reply, RoxError::new("consent_declined", "the person said no"));
                        }
                        let attached = ui.attach(&token, &title);
                        reply.resolve(json!({"token": token, "title": title, "attached": attached}));
                    }),
                );
            }
        });
    }

    /// The canonical token and title `value` resolves to in `scope` (9.2).
    fn resolve_token(&self, scope: &str, value: &str) -> Result<(String, String), RoxError> {
        let ask = host_api::ResolveAsk {
            plugin: self.prepared.plugin.id.clone(),
            kind: "scope",
            name: scope.to_owned(),
            value: value.to_owned(),
            complete: false,
            context: json!({"attached": []}),
        };
        let v = host_api::resolve(&*self.broker, &ask)?;
        let r = v.get("result").filter(|r| r.is_object()).unwrap_or(&v);
        match text(r, "result") {
            Some("token") => {
                let ticket = text(r, "ticket").filter(|t| perms::ticket_ok(t));
                let title: String = text(r, "title").unwrap_or_default().chars().take(60).collect();
                let ticket = ticket.ok_or_else(|| RoxError::new("internal", "the plugin sent no ticket"))?;
                Ok((format!("@{}:{scope}:{ticket}", self.prepared.plugin.short), title))
            }
            Some("error") => Err(RoxError::new("tool_error", text(r, "message").unwrap_or("it did not resolve"))),
            _ => Err(RoxError::new("internal", "the plugin's resolve gave no token")),
        }
    }

    /// `rox.pickFile(options)` and its files' `read()` (7.7).
    fn pick(self: &Arc<Self>, data: Value, reply: HostReply) {
        if let Some(id) = data.get("read") {
            return self.read_picked(id.as_str().unwrap_or_default().to_owned(), reply);
        }
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        let grant = self.prepared.permissions.pick_file.clone().unwrap_or_default();
        match pick_request(&grant, &data) {
            Err(e) => reject(reply, e),
            Ok(pick) => {
                let me = self.clone();
                let max = pick.max_bytes;
                self.ui.pick_files(pick, Box::new(move |paths| answer(reply, me.picked(paths, max))));
            }
        }
    }

    /// The app's view of the files chosen: ids, names, sizes, types.
    fn picked(&self, paths: Vec<PathBuf>, max_bytes: u64) -> Result<Value, RoxError> {
        let mut out = Vec::new();
        let mut files = Vec::new();
        for path in paths {
            let meta = std::fs::metadata(&path).map_err(|_| RoxError::new("not_found", "the file is gone"))?;
            if !meta.is_file() {
                return Err(invalid("not a file"));
            }
            if meta.len() > max_bytes {
                return Err(RoxError::new("quota_exceeded", format!("files are at most {max_bytes} bytes")));
            }
            let id = keys::random_id();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(json!({"id": id, "name": name, "size": meta.len(), "type": mime(&name)}));
            files.push((id, Picked { path, max_bytes }));
        }
        self.session.check_live()?;
        self.files.lock().unwrap_or_else(|e| e.into_inner()).extend(files);
        Ok(Value::Array(out))
    }

    fn read_picked(self: &Arc<Self>, id: String, reply: HostReply) {
        if self.session.check_live().is_err() {
            return reject(reply, RoxError::new("closed", "the app was closed"));
        }
        let file = self.files.lock().unwrap_or_else(|e| e.into_inner()).get(&id).map(|f| (f.path.clone(), f.max_bytes));
        let Some((path, max)) = file else { return reject(reply, invalid("no such file")) };
        let me = self.clone();
        std::thread::spawn(move || {
            let gone = || RoxError::new("not_found", "the file can't be read");
            let r = match std::fs::metadata(&path) {
                Ok(m) if m.len() > max => Err(RoxError::new("quota_exceeded", "the file grew past the limit")),
                Ok(_) => std::fs::read(&path).map_err(|_| gone()),
                Err(_) => Err(gone()),
            };
            let r = match r {
                Ok(_) if me.session.check_live().is_err() => Err(RoxError::new("closed", "the app was closed")),
                r => r.map(|bytes| Value::String(keys::b64_std(&bytes))),
            };
            answer(reply, r);
        });
    }

    /// `rox.copy(text)`.
    fn copy(self: &Arc<Self>, data: Value, reply: HostReply) {
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        match text(&data, "text") {
            Some(t) if t.len() <= MAX_COPY => {
                self.ui.copy(t);
                reply.resolve(Value::Null);
            }
            _ => reject(reply, invalid("copy takes text up to 1 MiB")),
        }
    }

    /// `rox.preview({tool, args?, title?})` (7.10): the app's own tool,
    /// called by the host as the app (signed, checked by the broker) with
    /// the preview's limit, answers with an HTML document; the person sees
    /// it over the app. The page never reaches the app.
    fn preview(self: &Arc<Self>, data: Value, reply: HostReply) {
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        let Some(o) = data.as_object() else { return reject(reply, invalid("rox.preview takes {tool, args?, title?}")) };
        if let Some(k) = o.keys().find(|k| !["tool", "args", "title"].contains(&k.as_str())) {
            return reject(reply, invalid(&format!("rox.preview does not take {k}")));
        }
        let Some(tool) = text(&data, "tool").map(str::to_owned) else { return reject(reply, invalid("tool is a string")) };
        if !self.prepared.permissions.tools.contains(&tool) {
            let _ = self.admit();
            return reject(reply, RoxError::new("not_declared", format!("{tool:?} is not in this app's tools")));
        }
        let args = match data.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(a @ Value::Object(_)) => a.clone(),
            Some(_) => return reject(reply, invalid("args is a JSON object")),
        };
        if args.to_string().len() > self.limits.max_args {
            return reject(reply, invalid("args are over 256 KiB"));
        }
        let title = match data.get("title") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) => Some(t.trim().chars().take(MAX_TITLE).collect::<String>()).filter(|t| !t.is_empty()),
            Some(_) => return reject(reply, invalid("title is a string")),
        };
        let grant = self.prepared.permissions.preview.unwrap_or(perms::MAX_PREVIEW) as usize;
        let max = grant.min(self.limits.max_preview);
        self.gated(reply, move |me| {
            let (envelope, sig) = me.session.sign_preview(&tool, &args)?;
            let result = host_api::call(&*me.broker, &envelope, &sig, me.limits.timeout);
            let p = &me.prepared;
            match &result {
                Ok(_) => log::info!("mini v1: {}/{} signed preview {tool}: ok", p.plugin.id, p.key.app),
                Err(e) => log::info!("mini v1: {}/{} signed preview {tool}: {}", p.plugin.id, p.key.app, e.code),
            }
            let (html, named) = preview_document(result?, max)?;
            let title = title.or(named).unwrap_or_else(|| p.title.clone());
            me.session.check_live()?;
            me.ui.preview(Preview { title, html: seal(&html) })?;
            Ok(Value::Null)
        });
    }

    /// `rox.storage.get|set|remove|keys` (11), on the open's storage
    /// thread, in order.
    fn storage_named(self: &Arc<Self>, name: &'static str, data: Value, reply: HostReply) {
        if let Err(e) = self.session.check_live() {
            return reject(reply, e);
        }
        let Some(store) = self.storage.clone() else {
            return reject(reply, RoxError::new("not_supported", "storage is not granted"));
        };
        let job: Box<dyn FnOnce() + Send> = Box::new(move || {
            let key = text(&data, "key").unwrap_or_default();
            let r = match name {
                "rox.storage.get" => store.get(key).map(|v| v.map_or(Value::Null, Value::String)),
                "rox.storage.set" => match data.get("value").and_then(Value::as_str) {
                    Some(v) => store.set(key, v).map(|()| Value::Null),
                    None => Err(invalid("values are strings")),
                },
                "rox.storage.remove" => store.remove(key).map(|()| Value::Null),
                _ => store.keys().map(|k| json!(k)),
            };
            answer(reply, r);
        });
        let mut jobs = self.store_jobs.lock().unwrap_or_else(|e| e.into_inner());
        let tx = jobs.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
            std::thread::spawn(move || rx.into_iter().for_each(|job| job()));
            tx
        });
        if let Err(mpsc::SendError(job)) = tx.send(job) {
            job();
        }
    }
}

/// The dialog `options` asks for, within `grant` (7.7).
fn pick_request(grant: &perms::PickGrant, options: &Value) -> Result<PickRequest, RoxError> {
    let o = match options {
        Value::Null => &Map::new(),
        Value::Object(o) => o,
        _ => return Err(invalid("pickFile takes an options object")),
    };
    if let Some(k) = o.keys().find(|k| !["extensions", "multiple"].contains(&k.as_str())) {
        return Err(invalid(&format!("pickFile does not take {k}")));
    }
    let extensions = match o.get("extensions") {
        None | Some(Value::Null) => grant.extensions.clone(),
        Some(Value::Array(list)) => {
            let mut out = Vec::new();
            for e in list {
                let e = e.as_str().ok_or_else(|| invalid("extensions are strings"))?.to_ascii_lowercase();
                let e = if e.starts_with('.') { e } else { format!(".{e}") };
                if !grant.extensions.contains(&e) {
                    return Err(denied(&format!("{e} is not in this app's pick_file")));
                }
                out.push(e);
            }
            out
        }
        Some(_) => return Err(invalid("extensions is a list")),
    };
    let multiple = match o.get("multiple") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(m)) => *m,
        Some(_) => return Err(invalid("multiple is a boolean")),
    };
    if multiple && !grant.multiple {
        return Err(denied("this app may pick one file at a time"));
    }
    Ok(PickRequest { extensions, multiple, max_bytes: grant.max_bytes })
}

/// A MIME type by extension; the bytes are the app's to interpret.
fn mime(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "csv" => "text/csv",
        "txt" => "text/plain",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "xml" => "application/xml",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// What `rox.call` resolves to from a tool result (7.3): its
/// `structuredContent`, else its text joined with `\n`; `isError` rejects
/// `tool_error` with its first text; over the limit, `result_too_large`.
pub fn tool_result(result: Value, max: usize) -> Result<Value, RoxError> {
    if result.to_string().len() > max {
        return Err(RoxError::new("result_too_large", "the result is over 1 MiB"));
    }
    let texts = || {
        result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|c| c.get("text").and_then(Value::as_str))
    };
    if result.get("isError") == Some(&Value::Bool(true)) {
        let first = texts().next().unwrap_or("the tool failed");
        return Err(RoxError::new("tool_error", first));
    }
    if let Some(s) = result.get("structuredContent").filter(|s| !s.is_null()) {
        return Ok(s.clone());
    }
    Ok(Value::String(texts().collect::<Vec<_>>().join("\n")))
}

/// The document in a `rox.preview` tool result (7.10): the first embedded
/// resource `{type: "resource", resource: {mimeType: "text/html", text}}`,
/// and the title in `structuredContent.title`, if any. `isError` rejects
/// `tool_error`; no HTML, `tool_error`; HTML over `max` bytes,
/// `result_too_large`.
pub fn preview_document(result: Value, max: usize) -> Result<(String, Option<String>), RoxError> {
    if result.get("isError") == Some(&Value::Bool(true)) {
        // rejects tool_error with its first text, as rox.call does
        tool_result(result.clone(), usize::MAX)?;
    }
    let is_html = |m: &str| m.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/html");
    let content = result.get("content").and_then(Value::as_array).into_iter().flatten();
    let html = content
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("resource"))
        .filter_map(|c| c.get("resource"))
        .find(|r| r.get("mimeType").and_then(Value::as_str).is_some_and(is_html))
        .and_then(|r| r.get("text").and_then(Value::as_str));
    let Some(html) = html else {
        return Err(RoxError::new("tool_error", "the tool returned no HTML document to preview"));
    };
    if html.len() > max {
        return Err(RoxError::new("result_too_large", format!("the document is over {} KiB", max / 1024)));
    }
    let title = result
        .pointer("/structuredContent/title")
        .and_then(Value::as_str)
        .map(|t| t.trim().chars().take(MAX_TITLE).collect::<String>())
        .filter(|t| !t.is_empty());
    Ok((html.to_owned(), title))
}

/// The policy a preview's page gets, before anything in it: no script,
/// nothing loaded but inline styles and `data:` images, fonts and objects
/// (a PDF receipt), no form posts, no `<base>`. The viewer also turns JavaScript off and keeps the
/// page from navigating; this holds even where those would not.
pub const PREVIEW_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; object-src data:; \
    form-action 'none'; base-uri 'none'; frame-src 'none'";

/// `html` with [`PREVIEW_CSP`] and a style that keeps links from being
/// followed, put after its doctype (before it, the page would lay out in
/// quirks mode) so the parser places them first in the head.
pub fn seal(html: &str) -> String {
    let head = format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{PREVIEW_CSP}\"><style>a{{pointer-events:none}}</style>"
    );
    let lead = html.len() - html.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}').len();
    let at = html[lead..]
        .get(..9)
        .filter(|s| s.eq_ignore_ascii_case("<!doctype"))
        .and_then(|_| html[lead..].find('>'))
        .map_or(0, |end| lead + end + 1);
    format!("{}{head}{}", &html[..at], &html[at..])
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
