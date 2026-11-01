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
/// `screens()`. None yet: the first lands with its plugin's screens crate
/// (pinned by git rev).
const COMPILED: &[(&str, Screens)] = &[];

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
    let t0 = std::time::Instant::now();
    let p = &ctx.prepared;
    let def = p
        .native
        .as_ref()
        .and_then(|n| find(&p.plugin.id, &n.screen, n.abi))
        .ok_or_else(|| RoxError::new("not_supported", say::native(&p.title)))?;
    let inline = viewport.get("placement").and_then(Value::as_str) == Some("inline");
    let params = Arc::new(Mutex::new(bridge::params(p, viewport)));
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
    fn this_build_compiles_no_screens_in_yet() {
        assert!(Registry::compiled().plugins.is_empty());
    }
}
