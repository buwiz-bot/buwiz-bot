//! Native screens for first-party Buwiz Bot mini apps.
//!
//! A mini app's screen is TSX (a third-party plugin's, loaded from its
//! source at run time) or, for a first-party plugin, a native Rust screen
//! compiled into Buwiz Bot. A native screen is an nsh Rust component: its
//! manifest entry names it (`apps[].native: {screen, abi}`), the host looks
//! it up among the screens compiled in and mounts its [`Root`] in the same
//! per-open view a TSX app gets, with the same session, permissions,
//! signing, card chrome and limits.
//!
//! What a TSX app reaches through the `buwiz` global, a native screen
//! reaches through its [`Bridge`]: the host checks every call exactly as it
//! checks a TSX app's (the bridge is only the API's shape).
//!
//! A plugin's screens crate exports its screens:
//!
//! ```ignore
//! use buwiz_app::{ABI, Bridge, ScreenDef};
//! use nsh_core::prelude::*;
//!
//! pub fn screens() -> &'static [ScreenDef] {
//!     &[ScreenDef { id: "books.ledger", abi: ABI, build: ledger }]
//! }
//!
//! fn ledger(bridge: Bridge) -> Root {
//!     buwiz_app::root(bridge, |bridge| {
//!         let rows = signal(String::from("loading"));
//!         let b = bridge.clone();
//!         bridge.spawn(async move {
//!             match b.call("books_ledger", serde_json::json!({})).await {
//!                 Ok(v) => rows.set(v.to_string()),
//!                 Err(e) => rows.set(e.message),
//!             }
//!             b.ready();
//!         });
//!         text(move || rows.get())
//!     })
//! }
//! ```
//!
//! Everything on [`Bridge`] that touches signals or spawns tasks
//! ([`Bridge::viewport`], [`Bridge::on_action`], [`Bridge::on_session`],
//! [`Bridge::spawn`]) runs inside a component's view, on the engine
//! thread; the rest may be called from anywhere the bridge is.
//!
//! A panic reaches the mini app's error banner from the screen's view,
//! bindings, listeners and effects ([`root`]'s boundary) and from the tasks
//! [`Bridge::spawn`] runs; spawn a screen's tasks with it rather than with
//! nsh's `spawn`, whose panics reach no boundary.

mod screen;

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use base64::Engine as _;
use nsh_core::element::{ReadSignal, Root, Spawned, signal, spawn};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

pub use screen::root;

/// The ABI of this crate's screens: what a manifest's `native.abi` names.
/// A host runs a screen only when the manifest's abi is the one the screen
/// was compiled with.
pub const ABI: u32 = 1;

/// One native screen of a plugin: what its manifest names
/// (`apps[].native.screen`), the ABI it was built against, and how it is
/// made for an open.
#[derive(Clone, Copy)]
pub struct ScreenDef {
    /// The screen's id: 1 to 64 of `a-z 0-9 . _ -`, as the manifest names it.
    pub id: &'static str,
    /// The ABI it was built against ([`ABI`]).
    pub abi: u32,
    /// Makes the screen's root for an open (any thread; the root's
    /// components are made on the engine thread). Build it with [`root`] so
    /// a panic in the screen reaches the mini app's error banner.
    pub build: fn(Bridge) -> Root,
}

impl std::fmt::Debug for ScreenDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenDef").field("id", &self.id).field("abi", &self.abi).finish()
    }
}

/// A rejection, as a TSX app's `BuwizError`: a stable `code`
/// (`permission_denied`, `invalid_args`, `rate_limited`, `timeout`,
/// `tool_error`, ...), whether trying again may work, and a message for
/// people.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuwizError {
    pub code: String,
    pub retryable: bool,
    pub message: String,
}

impl BuwizError {
    /// An error of `code` with `message`; `retryable` as given.
    pub fn new(code: impl Into<String>, retryable: bool, message: impl Into<String>) -> BuwizError {
        BuwizError { code: code.into(), retryable, message: message.into() }
    }

    fn invalid(message: &str) -> BuwizError {
        BuwizError::new("invalid_args", false, message)
    }

    fn internal(message: &str) -> BuwizError {
        BuwizError::new("internal", true, message)
    }
}

impl std::fmt::Display for BuwizError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BuwizError {}

/// How the host answers a call: once, from any thread. Dropped unanswered,
/// the call fails `internal`.
pub type Done = Box<dyn FnOnce(Result<Value, BuwizError>) + Send>;

/// Hears the host's events: `buwiz.viewport`, `buwiz.action` and
/// `buwiz.session`, with their data (any thread).
pub type Listener = Box<dyn Fn(&str, &Value) + Send + Sync>;

/// The host's side of a [`Bridge`], one per open. The method names and
/// their data are the TSX bridge's (`buwiz.call` takes `{tool, args}`, ...):
/// the host runs the same checks for both.
pub trait Host: Send + Sync + 'static {
    /// The open's parameters: `props`, `app`, `granted` (the methods its
    /// permissions grant) and the `viewport` now.
    fn params(&self) -> Map<String, Value>;
    /// A post (`buwiz.fit`, `buwiz.ready`): nothing answers.
    fn post(&self, method: &str, data: Value);
    /// A call; `done` gets its answer. It must not block: the screen calls
    /// it on the engine thread.
    fn call(&self, method: &str, data: Value, done: Done);
    /// Adds `listener` for the open's events.
    fn listen(&self, listener: Listener);
    /// The screen failed (a panic in its view, a binding, a listener, an
    /// effect or a task): the host shows its error banner.
    fn fail(&self, message: &str);
}

/// The size an open lays out at, and where it is placed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    pub placement: Placement,
}

impl Viewport {
    fn from_json(v: &Value) -> Option<Viewport> {
        let n = |k: &str| v.get(k).and_then(Value::as_f64).map(|n| n as f32);
        Some(Viewport {
            width: n("width")?,
            height: n("height")?,
            placement: match v.get("placement").and_then(Value::as_str) {
                Some("inline") => Placement::Inline,
                _ => Placement::Full,
            },
        })
    }
}

/// Where an open shows: full screen, or as a card inline in a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    #[default]
    Full,
    Inline,
}

/// A footer button of an inline card (`buwiz.actions`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Action {
    /// What [`Bridge::on_action`] hears: 1 to 32 of `A-Z a-z 0-9 _ . : -`.
    pub id: String,
    pub label: String,
    pub primary: bool,
    pub disabled: bool,
}

impl Action {
    /// A button `id` labelled `label`.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Action {
        Action { id: id.into(), label: label.into(), ..Default::default() }
    }

    /// Drawn as the card's main button.
    pub fn primary(mut self) -> Action {
        self.primary = true;
        self
    }

    /// Shown but not pressable.
    pub fn disabled(mut self, disabled: bool) -> Action {
        self.disabled = disabled;
        self
    }

    fn to_json(&self) -> Value {
        json!({"id": self.id, "label": self.label, "primary": self.primary, "disabled": self.disabled})
    }
}

/// An inline card's header status (`buwiz.meta`): dim text, or a pill.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Meta {
    pub text: String,
    pub pill: bool,
}

/// What the open's session went through (`buwiz.session`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionEvent {
    /// The plugin restarted: the open reconnects (a new session) and the
    /// host reopens the screen on it.
    Reconnecting,
}

/// What [`Bridge::attach`] attached.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attached {
    /// The canonical token (`@short:scope:ticket`).
    pub token: String,
    pub title: String,
    /// False when the chat had it already.
    pub attached: bool,
}

/// What [`Bridge::pick_file`]'s dialog offers: by default the app's
/// granted extensions, one file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PickOptions {
    /// With or without the dot; each must be granted.
    pub extensions: Vec<String>,
    pub multiple: bool,
}

/// A file the person picked: its bytes are read on demand.
#[derive(Clone)]
pub struct PickedFile {
    pub name: String,
    pub size: u64,
    /// Its MIME type, by extension.
    pub mime: String,
    id: String,
    bridge: Bridge,
}

impl std::fmt::Debug for PickedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickedFile").field("name", &self.name).field("size", &self.size).field("mime", &self.mime).finish()
    }
}

impl PickedFile {
    /// The file's bytes (it may not have grown past the grant's limit).
    pub fn read(&self) -> impl Future<Output = Result<Vec<u8>, BuwizError>> + 'static {
        let r = self.bridge.ask("buwiz.pickFile", json!({"read": self.id}));
        async move {
            let v = r.await?;
            let s = v.as_str().ok_or_else(|| BuwizError::internal("the host sent no file bytes"))?;
            base64::engine::general_purpose::STANDARD
                .decode(s)
                .map_err(|_| BuwizError::internal("the host sent bad file bytes"))
        }
    }
}

/// The app's own storage (`buwiz.storage`), when granted: string values
/// under string keys, within its quota.
#[derive(Clone, Debug)]
pub struct Storage {
    bridge: Bridge,
}

impl Storage {
    /// The value under `key`, if any.
    pub fn get(&self, key: &str) -> impl Future<Output = Result<Option<String>, BuwizError>> + 'static {
        let r = self.bridge.ask("buwiz.storage.get", json!({"key": key}));
        async move { Ok(r.await?.as_str().map(str::to_owned)) }
    }

    /// Sets `key` to `value`.
    pub fn set(&self, key: &str, value: &str) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        unit(self.bridge.ask("buwiz.storage.set", json!({"key": key, "value": value})))
    }

    /// Removes `key`.
    pub fn remove(&self, key: &str) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        unit(self.bridge.ask("buwiz.storage.remove", json!({"key": key})))
    }

    /// Every key.
    pub fn keys(&self) -> impl Future<Output = Result<Vec<String>, BuwizError>> + 'static {
        let r = self.bridge.ask("buwiz.storage.keys", Value::Null);
        async move {
            let v = r.await?;
            Ok(v.as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_owned)).collect())
        }
    }
}

/// A native screen's bridge to its open: what the `buwiz` global is to a
/// TSX app. Cheap to clone (the clones are one bridge); `Send` and `Sync`.
#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge").field("granted", &self.inner.granted).finish()
    }
}

struct Inner {
    host: Arc<dyn Host>,
    props: Value,
    app: Value,
    granted: Vec<String>,
    events: Arc<Mutex<Events>>,
}

/// The screen's listeners: each a task's channel, gone when its task is.
#[derive(Default)]
struct Events {
    viewport: Viewport,
    viewports: Vec<mpsc::UnboundedSender<Viewport>>,
    actions: Vec<mpsc::UnboundedSender<String>>,
    sessions: Vec<mpsc::UnboundedSender<SessionEvent>>,
}

/// Sends `v` to every listener in `list`, dropping those whose task ended.
fn fan_out<T: Clone>(list: &mut Vec<mpsc::UnboundedSender<T>>, v: &T) {
    list.retain(|tx| tx.send(v.clone()).is_ok());
}

fn unit(r: impl Future<Output = Result<Value, BuwizError>> + 'static) -> impl Future<Output = Result<(), BuwizError>> + 'static {
    async move { r.await.map(|_| ()) }
}

impl Bridge {
    /// The bridge of the open `host` serves (the host makes it).
    pub fn new(host: Arc<dyn Host>) -> Bridge {
        let params = host.params();
        let granted = params
            .get("granted")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.as_str().map(str::to_owned))
            .collect();
        let events = Arc::new(Mutex::new(Events {
            viewport: params.get("viewport").and_then(Viewport::from_json).unwrap_or_default(),
            ..Default::default()
        }));
        let heard = events.clone();
        host.listen(Box::new(move |name, data| {
            let mut e = heard.lock().unwrap_or_else(|e| e.into_inner());
            match name {
                "buwiz.viewport" => {
                    if let Some(v) = Viewport::from_json(data) {
                        e.viewport = v;
                        fan_out(&mut e.viewports, &v);
                    }
                }
                "buwiz.action" => {
                    if let Some(id) = data.get("id").and_then(Value::as_str) {
                        fan_out(&mut e.actions, &id.to_owned());
                    }
                }
                "buwiz.session" if data.get("state").and_then(Value::as_str) == Some("reconnecting") => {
                    fan_out(&mut e.sessions, &SessionEvent::Reconnecting);
                }
                _ => {}
            }
        }));
        let props = params.get("props").cloned().unwrap_or_else(|| json!({}));
        let app = params.get("app").cloned().unwrap_or_else(|| json!({}));
        Bridge { inner: Arc::new(Inner { host, props, app, granted, events }) }
    }

    fn has(&self, method: &str) -> bool {
        self.inner.granted.iter().any(|m| m == method)
    }

    /// Calls `method` now; the future is its answer.
    fn ask(&self, method: &str, data: Value) -> impl Future<Output = Result<Value, BuwizError>> + 'static {
        let (tx, rx) = oneshot::channel();
        self.inner.host.call(
            method,
            data,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        async move { rx.await.unwrap_or_else(|_| Err(BuwizError::internal("the host did not answer"))) }
    }

    /// The open's props (`buwiz.props`), as its manifest's schema checked them.
    pub fn props(&self) -> Value {
        self.inner.props.clone()
    }

    /// Which app this is (`buwiz.app`): `{plugin, app, version, protocol, ...}`.
    pub fn app(&self) -> Value {
        self.inner.app.clone()
    }

    /// The viewport now.
    pub fn viewport_now(&self) -> Viewport {
        self.inner.events.lock().unwrap_or_else(|e| e.into_inner()).viewport
    }

    /// The viewport, as a signal that follows it (`buwiz.viewport()`).
    /// Inside a component's view: it follows while the component lives.
    pub fn viewport(&self) -> ReadSignal<Viewport> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let now = {
            let mut e = self.inner.events.lock().unwrap_or_else(|e| e.into_inner());
            e.viewports.push(tx);
            e.viewport
        };
        let s = signal(now);
        spawn(async move {
            while let Some(v) = rx.recv().await {
                s.set(v);
            }
        });
        s.read_only()
    }

    /// An inline card's content height in points (`buwiz.fit`): the host
    /// sizes the card to it within the manifest's inline range, paced. Full
    /// screen: ignored.
    pub fn fit(&self, height: f32) -> Result<(), BuwizError> {
        if !(height.is_finite() && height > 0.0) {
            return Err(BuwizError::invalid("buwiz.fit takes a height in points"));
        }
        self.inner.host.post("buwiz.fit", json!({"height": height.ceil()}));
        Ok(())
    }

    /// An inline card's data is in (`buwiz.ready`): with the manifest's
    /// `inline.wait_ready` the card holds its placeholder until this. Full
    /// screen: ignored.
    pub fn ready(&self) {
        self.inner.host.post("buwiz.ready", json!({}));
    }

    /// An inline card's footer buttons (`buwiz.actions`; empty clears them).
    pub fn actions(&self, actions: Vec<Action>) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        let list: Vec<Value> = actions.iter().map(Action::to_json).collect();
        unit(self.ask("buwiz.actions", json!({"actions": list})))
    }

    /// Runs `f` with the id of each footer button pressed
    /// (`buwiz.onAction`). Inside a component's view: it listens while the
    /// component lives, or until the task is cancelled.
    pub fn on_action(&self, f: impl Fn(&str) + 'static) -> Spawned<Option<()>> {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        self.inner.events.lock().unwrap_or_else(|e| e.into_inner()).actions.push(tx);
        self.spawn(async move {
            while let Some(id) = rx.recv().await {
                f(&id);
            }
        })
    }

    /// An inline card's header status (`buwiz.meta`; `None` clears it).
    pub fn meta(&self, meta: Option<Meta>) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        let meta = meta.map_or(Value::Null, |m| json!({"text": m.text, "pill": m.pill}));
        unit(self.ask("buwiz.meta", json!({"meta": meta})))
    }

    /// The inline card collapses to the outcome line `text` and the screen
    /// unmounts (`buwiz.settle`).
    pub fn settle(&self, text: &str) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        unit(self.ask("buwiz.settle", json!({"text": text})))
    }

    /// Runs `f` on what the open's session goes through (`buwiz.session`).
    /// Inside a component's view, as [`Bridge::on_action`].
    pub fn on_session(&self, f: impl Fn(SessionEvent) + 'static) -> Spawned<Option<()>> {
        let (tx, mut rx) = mpsc::unbounded_channel::<SessionEvent>();
        self.inner.events.lock().unwrap_or_else(|e| e.into_inner()).sessions.push(tx);
        self.spawn(async move {
            while let Some(e) = rx.recv().await {
                f(e);
            }
        })
    }

    /// Runs `future` on the engine thread, as nsh's `spawn` does, and sends
    /// a panic in it to the error banner (`None` then). Inside a
    /// component's view: it ends with the component. A task spawned with
    /// nsh's `spawn` itself reaches no error boundary: its panic is only
    /// logged.
    pub fn spawn<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Spawned<Option<T>> {
        let bridge = self.clone();
        spawn(Caught { future: Box::pin(future), bridge })
    }

    /// Calls the plugin's tool `tool` with `args` (an object; `null` is
    /// `{}`), signed by the open's session key (`buwiz.call`): its
    /// `structuredContent`, else its text.
    pub fn call(&self, tool: &str, args: Value) -> impl Future<Output = Result<Value, BuwizError>> + 'static {
        let args = if args.is_null() { json!({}) } else { args };
        self.ask("buwiz.call", json!({"tool": tool, "args": args}))
    }

    /// Opens the plugin's app `app` with `props` full screen
    /// (`buwiz.open({app, props})`).
    pub fn open(&self, app: &str, props: Value) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        unit(self.ask("buwiz.open", json!({"app": app, "props": props})))
    }

    /// Opens `app` on the subject `ticket` stands for
    /// (`buwiz.open({app, props, ticket})`).
    pub fn open_ticket(&self, app: &str, ticket: &str, props: Value) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        unit(self.ask("buwiz.open", json!({"app": app, "props": props, "ticket": ticket})))
    }

    /// Opens an https `url` of the app's `open.external` in the browser,
    /// once the person agrees (`buwiz.open({url})`).
    pub fn open_url(&self, url: &str) -> impl Future<Output = Result<(), BuwizError>> + 'static {
        unit(self.ask("buwiz.open", json!({"url": url})))
    }

    /// The title of the subject `ticket` stands for, if the plugin still
    /// knows it (`buwiz.ticket.title`).
    pub fn ticket_title(&self, ticket: &str) -> impl Future<Output = Result<Option<String>, BuwizError>> + 'static {
        let r = self.ask("buwiz.ticket.title", json!({"ticket": ticket}));
        async move { Ok(r.await?.as_str().map(str::to_owned)) }
    }

    /// The app's storage, when its permissions grant it.
    pub fn storage(&self) -> Option<Storage> {
        self.has("buwiz.storage.get").then(|| Storage { bridge: self.clone() })
    }

    /// Attaches the subject `value` of scope `scope` to the open's chat,
    /// once the person agrees (`buwiz.attach`), when granted.
    pub fn attach(&self, scope: &str, value: &str) -> Option<impl Future<Output = Result<Attached, BuwizError>> + 'static> {
        if !self.has("buwiz.attach") {
            return None;
        }
        let r = self.ask("buwiz.attach", json!({"scope": scope, "value": value}));
        Some(async move {
            let v = r.await?;
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            Ok(Attached { token: s("token"), title: s("title"), attached: v.get("attached") == Some(&Value::Bool(true)) })
        })
    }

    /// Shows the file dialog (`buwiz.pickFile`), when granted: the files
    /// chosen, none on Cancel.
    pub fn pick_file(&self, options: PickOptions) -> Option<impl Future<Output = Result<Vec<PickedFile>, BuwizError>> + 'static> {
        if !self.has("buwiz.pickFile") {
            return None;
        }
        let mut o = Map::new();
        if !options.extensions.is_empty() {
            o.insert("extensions".to_owned(), json!(options.extensions));
        }
        o.insert("multiple".to_owned(), Value::Bool(options.multiple));
        let r = self.ask("buwiz.pickFile", Value::Object(o));
        let bridge = self.clone();
        Some(async move {
            let v = r.await?;
            let files = v.as_array().ok_or_else(|| BuwizError::internal("the host sent no files"))?;
            Ok(files
                .iter()
                .map(|f| {
                    let s = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
                    PickedFile {
                        name: s("name"),
                        size: f.get("size").and_then(Value::as_u64).unwrap_or(0),
                        mime: s("type"),
                        id: s("id"),
                        bridge: bridge.clone(),
                    }
                })
                .collect())
        })
    }

    /// Puts `text` on the pasteboard (`buwiz.copy`), when granted.
    pub fn copy(&self, text: &str) -> Option<impl Future<Output = Result<(), BuwizError>> + 'static> {
        self.has("buwiz.copy").then(|| unit(self.ask("buwiz.copy", json!({"text": text}))))
    }

    /// Shows the HTML document the app's tool `tool` makes from `args` over
    /// the screen (`buwiz.preview`), titled `title` (else the document's),
    /// when granted.
    pub fn preview(&self, tool: &str, args: Value, title: Option<&str>) -> Option<impl Future<Output = Result<(), BuwizError>> + 'static> {
        if !self.has("buwiz.preview") {
            return None;
        }
        let mut o = Map::new();
        o.insert("tool".to_owned(), Value::String(tool.to_owned()));
        if !args.is_null() {
            o.insert("args".to_owned(), args);
        }
        if let Some(t) = title {
            o.insert("title".to_owned(), Value::String(t.to_owned()));
        }
        Some(unit(self.ask("buwiz.preview", Value::Object(o))))
    }

    /// The screen failed: the host shows its error banner.
    pub(crate) fn fail(&self, message: &str) {
        self.inner.host.fail(message);
    }
}

/// A future whose panic fails the screen.
struct Caught<F: Future> {
    future: Pin<Box<F>>,
    bridge: Bridge,
}

impl<F: Future> Future for Caught<F> {
    type Output = Option<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.future.as_mut().poll(cx)));
        match polled {
            Ok(Poll::Ready(v)) => Poll::Ready(Some(v)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(panic) => {
                self.bridge.fail(&screen::panic_message(&*panic));
                Poll::Ready(None)
            }
        }
    }
}

#[cfg(test)]
mod tests;
