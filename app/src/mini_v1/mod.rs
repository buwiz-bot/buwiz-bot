//! The Rox side of the Mini App Protocol v1 (`drafts/rox-mini-app-protocol-v1.md`,
//! host design in `drafts/mini-app-v1-host-design.md`): what runs between
//! a v1 plugin's app and the broker.
//!
//! The broker owns the plugin processes and checks every call; Rox owns
//! the app's sandbox and the key that signs its calls. An open goes:
//!
//! 1. [`load`]: the source from the broker, its fingerprint checked again,
//!    the import table checked before anything is added, props validated,
//!    the bridge shim prepended, compiled.
//! 2. [`session`]: a session key made here, `sessions/open` at the broker,
//!    the plugin's certificate verified here as well.
//! 3. [`bridge`]: a `nsh/host` channel with handlers for only what the
//!    app's permissions grant, and a policy granting exactly those names
//!    and nothing else (no network, files, clipboard, window).
//!
//! The person-facing parts (consent dialogs, the full-screen stack, the
//! file dialog, the pasteboard, the chat an attachment goes to) are behind
//! [`bridge::HostUi`], so the bridge runs headless in tests and part B
//! draws the dialogs.
//!
//! A v1 app's module key is `v1:<plugin id>/<app id>`, with the open's
//! `?ticket=` and `&props=` as in its `rox://app/` link ([`V1Key`]); v0's
//! `mcp:` and generated `R<n>` keys are untouched.

pub mod bridge;
pub mod cache;
// JCS, keys and the composer's tokens: shared with the broker
pub use rox_v1::{jcs, keys, tokens};
pub mod cards;
pub mod connect;
pub mod consent;
pub mod host_api;
pub mod lifecycle;
pub mod links;
pub mod load;
pub mod migrate;
#[cfg(target_os = "macos")]
pub mod native;
pub mod perms;
pub mod props;
pub mod records;
pub mod session;
pub mod storage;
pub mod ui;

use serde_json::{Value, json};

/// The protocol version this host implements (spec 2.2).
pub const PROTOCOL: &str = "1.0.0";

/// A bridge rejection (spec 14.1): what the app's promise rejects with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoxError {
    pub code: String,
    pub retryable: bool,
    pub message: String,
}

/// The codes spec 14.1 marks retryable.
const RETRYABLE: [&str; 5] = ["rate_limited", "timeout", "plugin_unavailable", "plugin_restarted", "internal"];

impl RoxError {
    /// `code` with its spec retryability; the message is capped at 500
    /// chars (Appendix A).
    pub fn new(code: &str, message: impl Into<String>) -> RoxError {
        let message: String = message.into();
        RoxError {
            code: code.to_owned(),
            retryable: RETRYABLE.contains(&code),
            message: message.chars().take(500).collect(),
        }
    }

    /// The broker's refusal `{ok:false, code, message, retryable}`; a body
    /// without a code is `internal`.
    pub fn from_body(body: &Value) -> RoxError {
        let code = body.get("code").and_then(Value::as_str).unwrap_or("internal");
        let message = body.get("message").and_then(Value::as_str).unwrap_or(code);
        RoxError::new(code, message)
    }

    /// The text the app's `Error` carries: the shim parses it back into a
    /// `RoxError`.
    pub fn to_wire(&self) -> String {
        json!({"code": self.code, "retryable": self.retryable, "message": self.message}).to_string()
    }
}

impl std::fmt::Display for RoxError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// A v1 app's module key: which app, and the open's ticket and props.
#[derive(Debug, Clone, PartialEq)]
pub struct V1Key {
    pub plugin: String,
    pub app: String,
    pub ticket: Option<String>,
    pub props: serde_json::Map<String, Value>,
}

impl V1Key {
    /// Parses `v1:<plugin>/<app>[?ticket=…][&props=<percent-encoded JSON>]`.
    pub fn parse(key: &str) -> Result<V1Key, RoxError> {
        let bad = |m: &str| RoxError::new("invalid_args", m);
        let rest = key.strip_prefix("v1:").ok_or_else(|| bad("not a v1 app key"))?;
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (plugin, app) = path.split_once('/').ok_or_else(|| bad("a v1 key names a plugin and an app"))?;
        if !perms::plugin_id_ok(plugin) || !perms::app_id_ok(app) {
            return Err(bad("bad plugin or app id"));
        }
        let mut out = V1Key { plugin: plugin.to_owned(), app: app.to_owned(), ticket: None, props: Default::default() };
        for kv in query.split('&').filter(|kv| !kv.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let v = crate::client::urldec(v);
            match k {
                "ticket" if perms::ticket_ok(&v) => out.ticket = Some(v),
                "ticket" => return Err(bad("bad ticket")),
                "props" => match jcs::parse_ijson(&v) {
                    Ok(Value::Object(m)) => out.props = m,
                    _ => return Err(bad("props are a JSON object")),
                },
                _ => {}
            }
        }
        Ok(out)
    }

    /// The key text: [`V1Key::parse`]'s inverse.
    pub fn to_key(&self) -> String {
        let mut key = format!("v1:{}/{}", self.plugin, self.app);
        let mut sep = '?';
        if let Some(t) = &self.ticket {
            key.push_str(&format!("{sep}ticket={t}"));
            sep = '&';
        }
        if !self.props.is_empty() {
            let props = Value::Object(self.props.clone()).to_string();
            key.push_str(&format!("{sep}props={}", crate::client::urlenc(&props)));
        }
        key
    }
}

/// Whether module key `key` is a v1 app's.
pub fn is_v1(key: &str) -> bool {
    key.starts_with("v1:")
}

/// The apps the store prepared, by module key: the last few of each, so
/// the component finds the one whose module it was handed. Shared: the
/// store runs on the engine thread, the component on the main thread.
static PREPARED: std::sync::Mutex<Vec<(String, std::sync::Arc<load::Prepared>)>> = std::sync::Mutex::new(Vec::new());

/// How many prepared modules are kept per key.
const KEEP_PER_KEY: usize = 4;

/// Keeps `prepared` for the opens of module `key`.
pub fn keep_prepared(key: &str, prepared: load::Prepared) {
    let mut m = PREPARED.lock().unwrap_or_else(|e| e.into_inner());
    m.push((key.to_owned(), std::sync::Arc::new(prepared)));
    let mine: Vec<usize> = m.iter().enumerate().filter(|(_, (k, _))| k == key).map(|(i, _)| i).collect();
    if mine.len() > KEEP_PER_KEY {
        m.remove(mine[0]);
    }
}

/// The app prepared for module `key` whose compiled module is `js` (any
/// thread).
pub fn prepared_js(key: &str, js: &str) -> Option<std::sync::Arc<load::Prepared>> {
    let m = PREPARED.lock().unwrap_or_else(|e| e.into_inner());
    m.iter().rev().find(|(k, p)| k == key && p.js == js).map(|(_, p)| p.clone())
}

/// The app last prepared for module `key`.
pub fn prepared(key: &str) -> Option<std::sync::Arc<load::Prepared>> {
    let m = PREPARED.lock().unwrap_or_else(|e| e.into_inner());
    m.iter().rev().find(|(k, _)| k == key).map(|(_, p)| p.clone())
}

type StoreJob = Box<dyn FnOnce(&'static crate::store::Store) + Send>;

/// Work for the store from other threads (the main thread's components,
/// the bridge, the broker's events): the store and its signals live on
/// the engine thread, which runs these ([`run_store_jobs`]).
static STORE_JOBS: std::sync::Mutex<Vec<StoreJob>> = std::sync::Mutex::new(Vec::new());

/// Runs `f` with the store on the engine thread, soon.
pub fn to_store(f: impl FnOnce(&'static crate::store::Store) + Send + 'static) {
    STORE_JOBS.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(f));
}

/// Runs the queued [`to_store`] work (the engine thread, on a timer).
pub fn run_store_jobs(store: &'static crate::store::Store) {
    let jobs = std::mem::take(&mut *STORE_JOBS.lock().unwrap_or_else(|e| e.into_inner()));
    for job in jobs {
        job(store);
    }
}

/// Rox's data directory for plugins: `<ROX_DATA_DIR or ~/Library/Application Support/Rox>/plugins`.
pub fn plugins_dir() -> std::path::PathBuf {
    crate::vault::data_dir().join("plugins")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_v1_key_carries_the_opens_ticket_and_props() {
        let k = V1Key::parse("v1:dev.goldcoders.fixture/card").unwrap();
        assert_eq!((k.plugin.as_str(), k.app.as_str(), k.ticket.clone()), ("dev.goldcoders.fixture", "card", None));
        assert_eq!(k.to_key(), "v1:dev.goldcoders.fixture/card");
        let mut k2 = k.clone();
        k2.ticket = Some("tkt_0123456789ABCDEFGHJKMNPQRS".to_owned());
        k2.props.insert("label".to_owned(), json!("a b&c"));
        let text = k2.to_key();
        assert_eq!(V1Key::parse(&text).unwrap(), k2);
        assert!(V1Key::parse("mcp:bir/forms").is_err());
        assert!(V1Key::parse("v1:bir/forms").is_err(), "a plugin id is reverse-DNS");
        assert!(V1Key::parse("v1:dev.goldcoders.fixture/Card").is_err());
        assert!(V1Key::parse("v1:dev.goldcoders.fixture/card?ticket=123").is_err());
        assert!(V1Key::parse("v1:dev.goldcoders.fixture/card?props=%5B%5D").is_err());
    }

    fn prep(js: &str) -> load::Prepared {
        load::Prepared {
            key: V1Key::parse("v1:dev.goldcoders.prepfx/card").unwrap(),
            plugin: Default::default(),
            title: "Card".into(),
            fingerprint: String::new(),
            permissions_json: None,
            permissions: Default::default(),
            js: js.into(),
        }
    }

    #[test]
    fn a_prepared_app_is_found_by_its_module_from_another_thread_and_after_a_newer_fetch() {
        // the store prepares on the engine thread; the component asks on
        // the main thread, maybe after another fetch of the same key
        let key = "v1:dev.goldcoders.prepfx/card";
        std::thread::spawn(move || {
            keep_prepared(key, prep("js // open 1"));
            keep_prepared(key, prep("js // open 2"));
        })
        .join()
        .unwrap();
        assert_eq!(prepared_js(key, "js // open 1").map(|p| p.js.clone()).as_deref(), Some("js // open 1"));
        assert_eq!(prepared_js(key, "js // open 2").map(|p| p.js.clone()).as_deref(), Some("js // open 2"));
        assert_eq!(prepared(key).map(|p| p.js.clone()).as_deref(), Some("js // open 2"));
        for n in 3..10 {
            keep_prepared(key, prep(&format!("js // open {n}")));
        }
        assert!(prepared_js(key, "js // open 1").is_none(), "only the last few are kept");
        assert!(prepared_js(key, "js // open 9").is_some());
    }

    #[test]
    fn rox_errors_take_the_specs_retryability() {
        assert!(RoxError::new("timeout", "").retryable);
        assert!(!RoxError::new("permission_denied", "").retryable);
        let e = RoxError::from_body(&json!({"ok": false, "code": "replayed", "message": "seen", "retryable": true}));
        assert_eq!((e.code.as_str(), e.retryable), ("replayed", false));
        assert_eq!(RoxError::from_body(&json!({})).code, "internal");
        assert_eq!(RoxError::new("x", "y".repeat(900)).message.len(), 500);
    }
}
