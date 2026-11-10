//! First-party native screens (protocol 1.0's `apps[].native: {screen,
//! abi}`): Rust screens compiled into this build, beside the TSX apps
//! third-party plugins ship. A native screen mounts in the same per-open
//! view a TSX app gets, through `App::Rust(root)` instead of a bundle, and
//! its [`Bridge`] answers from the same handlers ([`bridge::wire`]): the
//! same permissions, signing, limits, pacing and card chrome.
//!
//! The screens come from the plugins' screens crates, listed in
//! [`COMPILED`] by plugin id; a screen runs only when the manifest's
//! `(screen, abi)` is one compiled in.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use buwiz_app::{self as sdk, Bridge, BuwizError, Done, Listener, ScreenDef};
use nsh_core::element::Root;
use serde_json::{Map, Value};

use super::{
    RoxError,
    bridge::{self, Emit, Handler, OpenContext, Reply},
    load::say,
};

/// A screens crate's `screens()`.
type Screens = fn() -> &'static [ScreenDef];

/// The screens crates compiled in: each plugin's id and its crate's
/// `screens()`, each crate pinned by git rev.
const COMPILED: &[(&str, Screens)] = &[("app.buwiz.books", books_screens::screens), ("app.buwiz.google-workspace", gws_screens::screens)];

/// Native screens by plugin.
#[derive(Default)]
pub struct Registry {
    plugins: Vec<(String, &'static [ScreenDef])>,
}

impl Registry {
    /// The screens compiled in ([`COMPILED`]).
    pub fn compiled() -> Registry {
        let mut r = Registry::default();
        for (plugin, screens) in COMPILED {
            r.add(plugin, screens());
        }
        r
    }

    /// Adds `plugin`'s `screens`.
    pub fn add(&mut self, plugin: &str, screens: &'static [ScreenDef]) {
        self.plugins.push((plugin.to_owned(), screens));
    }

    /// `plugin`'s screen `screen` built against `abi`, if this build has
    /// one: the manifest's abi must be the screen's, and the screen's one
    /// this build's bridge serves.
    pub fn find(&self, plugin: &str, screen: &str, abi: u32) -> Option<ScreenDef> {
        self.plugins
            .iter()
            .filter(|(p, _)| p == plugin)
            .flat_map(|(_, screens)| screens.iter())
            .find(|d| d.id == screen && d.abi == abi && d.abi == sdk::ABI)
            .copied()
    }
}

fn registry() -> &'static Mutex<Registry> {
    static R: std::sync::OnceLock<Mutex<Registry>> = std::sync::OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::compiled()))
}

/// [`Registry::find`] among the screens of this build.
pub fn find(plugin: &str, screen: &str, abi: u32) -> Option<ScreenDef> {
    registry().lock().unwrap_or_else(|e| e.into_inner()).find(plugin, screen, abi)
}

/// Adds `plugin`'s `screens` to this build's (tests' screens).
#[cfg(test)]
pub fn register(plugin: &str, screens: &'static [ScreenDef]) {
    registry().lock().unwrap_or_else(|e| e.into_inner()).add(plugin, screens);
}

/// A native screen's call, answered: the host's error as a `BuwizError`.
struct Answer(Done);

impl Reply for Answer {
    fn resolve(self: Box<Self>, value: Value) {
        (self.0)(Ok(value));
    }

    fn reject(self: Box<Self>, e: RoxError) {
        (self.0)(Err(BuwizError::new(e.code, e.retryable, e.message)))
    }
}

/// The host side of a native screen's [`Bridge`]: the open's handlers, as
/// a TSX app's channel has them.
struct NativeHost {
    params: Arc<Mutex<Map<String, Value>>>,
    handlers: HashMap<&'static str, Handler>,
    listeners: Arc<Mutex<Vec<Listener>>>,
    fail: Arc<dyn Fn(&str) + Send + Sync>,
}

impl sdk::Host for NativeHost {
    fn params(&self) -> Map<String, Value> {
        self.params.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn post(&self, method: &str, data: Value) {
        if let Some(h) = self.handlers.get(method) {
            h(data, None);
        }
    }

    fn call(&self, method: &str, data: Value, done: Done) {
        match self.handlers.get(method) {
            Some(h) => h(data, Some(Box::new(Answer(done)))),
            // a TSX app has no such method on `buwiz`; here it is refused
            None => done(Err(BuwizError::new("permission_denied", false, format!("{method} is not granted to this app")))),
        }
    }

    fn listen(&self, listener: Listener) {
        self.listeners.lock().unwrap_or_else(|e| e.into_inner()).push(listener);
    }

    fn fail(&self, message: &str) {
        (self.fail)(message);
    }
}

/// A native screen ready to load: its root, and where the open's events
/// (the viewport) go.
pub struct Mounted {
    pub root: Root,
    pub emit: Emit,
}

/// The root of the open `ctx`'s native screen, with a bridge on the open's
/// handlers. `viewport` is the first one; `fail` hears the screen's
/// failures (its error banner). A screen this build lacks says it needs a
/// newer Buwiz Bot; one whose build panics fails as a TSX app's load does.
pub fn mount(ctx: &Arc<OpenContext>, viewport: Value, fail: Arc<dyn Fn(&str) + Send + Sync>) -> Result<Mounted, RoxError> {
    mount_from(ctx, viewport, fail, None)
}

/// `props` as a screen reads them: the open's checked props, and under
/// `origin` the tool call that made its card (`{tool, args}`), when the
/// chat knows it. The origin is the Bot's word, added after the props
/// were checked against the app's schema, and written nowhere.
pub fn props_with_origin(mut params: serde_json::Map<String, Value>, origin: Option<Value>) -> serde_json::Map<String, Value> {
    if let (Some(origin), Some(Value::Object(props))) = (origin, params.get_mut("props")) {
        props.insert("origin".to_owned(), origin);
    }
    params
}

/// [`mount`] for a card whose chat knows the tool call that made it
/// (`origin`, [`super::cards::origin`]).
pub fn mount_from(ctx: &Arc<OpenContext>, viewport: Value, fail: Arc<dyn Fn(&str) + Send + Sync>, origin: Option<Value>) -> Result<Mounted, RoxError> {
    let t0 = std::time::Instant::now();
    let p = &ctx.prepared;
    let def = p
        .native
        .as_ref()
        .and_then(|n| find(&p.plugin.id, &n.screen, n.abi))
        .ok_or_else(|| RoxError::new("not_supported", say::native(&p.title)))?;
    let inline = viewport.get("placement").and_then(Value::as_str) == Some("inline");
    let params = Arc::new(Mutex::new(props_with_origin(bridge::params(p, viewport), origin)));
    let listeners: Arc<Mutex<Vec<Listener>>> = Arc::default();
    let emit: Emit = {
        let (params, listeners) = (params.clone(), listeners.clone());
        Arc::new(move |name: &str, data: Value| {
            // a bridge made later starts from the viewport now
            if name == "buwiz.viewport" {
                params.lock().unwrap_or_else(|e| e.into_inner()).insert("viewport".to_owned(), data.clone());
            }
            for l in listeners.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                l(name, &data);
            }
        })
    };
    let handlers = bridge::wire(ctx, inline, emit.clone()).into_iter().collect();
    let host = Arc::new(NativeHost { params, handlers, listeners, fail });
    let bridge = Bridge::new(host);
    let root = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || (def.build)(bridge))).map_err(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("it panicked");
        RoxError::new("internal", say::runtime(&p.title, message))
    })?;
    log::info!("mini v1 timing: {}/{} native screen {} built {} ms", p.plugin.id, p.key.app, def.id, t0.elapsed().as_millis());
    Ok(Mounted { root, emit })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: Bridge) -> Root {
        unreachable!("not built")
    }

    static ONE: &[ScreenDef] = &[ScreenDef { id: "fx.ledger", abi: sdk::ABI, build: none }];
    static OLD: &[ScreenDef] = &[ScreenDef { id: "fx.old", abi: 0, build: none }, ScreenDef { id: "fx.next", abi: sdk::ABI + 1, build: none }];

    #[test]
    fn a_screens_props_carry_its_cards_origin_and_nothing_else_changes() {
        use serde_json::json;
        let params = |props: Value| json!({"props": props, "app": {"plugin": "p"}}).as_object().unwrap().clone();
        let origin = json!({"tool": "gws_search_mail", "args": {"query": "in:inbox"}});
        let with = props_with_origin(params(json!({"search": "tkt_x"})), Some(origin.clone()));
        assert_eq!(Value::Object(with), json!({"props": {"search": "tkt_x", "origin": origin}, "app": {"plugin": "p"}}));
        let without = props_with_origin(params(json!({"search": "tkt_x"})), None);
        assert_eq!(Value::Object(without), json!({"props": {"search": "tkt_x"}, "app": {"plugin": "p"}}));
    }

    #[test]
    fn a_screen_is_found_by_plugin_id_and_abi() {
        let mut r = Registry::default();
        r.add("app.buwiz.fixture", ONE);
        r.add("app.buwiz.other", OLD);
        assert_eq!(r.find("app.buwiz.fixture", "fx.ledger", sdk::ABI).map(|d| d.id), Some("fx.ledger"));
        // another plugin's screen of the same id is not this one's
        assert!(r.find("app.buwiz.other", "fx.ledger", sdk::ABI).is_none());
        assert!(r.find("app.buwiz.fixture", "fx.board", sdk::ABI).is_none());
        // the manifest's abi is the screen's
        assert!(r.find("app.buwiz.fixture", "fx.ledger", sdk::ABI + 1).is_none());
        assert!(r.find("app.buwiz.fixture", "fx.ledger", 0).is_none());
        // and the screen's is the one this build serves
        assert!(r.find("app.buwiz.other", "fx.old", 0).is_none());
        assert!(r.find("app.buwiz.other", "fx.next", sdk::ABI + 1).is_none());
    }

    #[test]
    fn books_screens_are_compiled_in_by_plugin_id() {
        let r = Registry::compiled();
        for screen in ["books.quiet", "books.ledger"] {
            assert_eq!(r.find("app.buwiz.books", screen, sdk::ABI).map(|d| d.id), Some(screen));
            // keyed by the plugin id, not its short name
            assert!(r.find("books", screen, sdk::ABI).is_none());
        }
    }

    #[test]
    fn google_workspace_screens_are_compiled_in_by_plugin_id() {
        let r = Registry::compiled();
        for screen in ["gws.mail", "gws.calendar", "gws.drive"] {
            assert_eq!(r.find("app.buwiz.google-workspace", screen, sdk::ABI).map(|d| d.id), Some(screen));
            assert!(r.find("gws", screen, sdk::ABI).is_none());
            assert!(r.find("app.buwiz.books", screen, sdk::ABI).is_none());
        }
    }

    /// A screens crate's features reach the whole app. serde_json's
    /// `preserve_order` changes the order `to_string` writes keys in, which
    /// anything hashing serialised JSON would feel. It must stay off here.
    #[test]
    fn no_screens_crate_turns_on_preserve_order() {
        assert_eq!(serde_json::json!({"b": 1, "a": 2}).to_string(), r#"{"a":2,"b":1}"#);
    }

    /// The Books manifest at the pinned rev: every `apps[].native` it
    /// names opens here, and the registry holds first-party plugins only.
    #[test]
    fn every_native_screen_the_books_manifest_names_is_compiled_in() {
        let manifest = books_mini_app::manifest::unsigned("test");
        let id = manifest["id"].as_str().unwrap();
        assert_eq!(manifest["publisher"]["id"], "app.buwiz");
        let r = Registry::compiled();
        let named: Vec<(&str, u32)> = manifest["apps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a.get("native"))
            .map(|n| (n["screen"].as_str().unwrap(), u32::try_from(n["abi"].as_u64().unwrap()).unwrap()))
            .collect();
        assert!(named.len() >= 2, "{named:?}");
        for (screen, abi) in &named {
            assert_eq!(r.find(id, screen, *abi).map(|d| d.id), Some(*screen), "{id} names {screen} (abi {abi})");
        }
        // first-party only: each compiled-in plugin is of the app.buwiz publisher
        for (plugin, _) in COMPILED {
            assert!(plugin.starts_with("app.buwiz."), "{plugin}");
        }
    }
}
