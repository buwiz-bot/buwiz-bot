//! An open's session (spec 4.4) and its signed calls (8.1).
//!
//! Rox makes the session key, asks the broker to open the session (the
//! plugin signs a certificate over the key and the permissions digest),
//! and checks that certificate itself before the app gets any method: the
//! signature with the plugin's public key, and that every field is the one
//! asked for. Each `rox.call` is then an envelope signed here; the key
//! never leaves this module.
//!
//! A session ends at the earliest of close, `not_after`, and a new
//! generation of its plugin (a restart; the broker's `plugin` event):
//! [`on_plugin_event`] ends the sessions of the old one, and whoever holds
//! an open hears it through [`Session::on_end`] (the bridge tells the app;
//! part B shows "Reconnecting").

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use serde_json::{Value, json};

use super::{RoxError, host_api, keys};

/// The longest a certificate may live (spec 4.4, Q13).
pub const MAX_LIFETIME_MS: i64 = 8 * 3600 * 1000;
/// Slack for the plugin's clock in `not_after` (spec 8.5's skew).
const SKEW_MS: i64 = 60_000;

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The open closed.
    Closed,
    /// Its plugin restarted: a new generation (14.4).
    Restarted,
}

/// One open's session.
pub struct Session {
    pub plugin: String,
    pub app: String,
    pub open_id: String,
    pub not_after: i64,
    key: keys::SessionKey,
    ended: Mutex<Option<Ended>>,
    on_end: Mutex<Vec<Box<dyn Fn(Ended) + Send + Sync>>>,
}

/// The sessions alive in this process, for generation events.
static LIVE: Mutex<Vec<Weak<Session>>> = Mutex::new(Vec::new());
/// The last generation seen of each plugin.
static GENERATIONS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

impl Session {
    /// Opens a session for `app` of `plugin` whose permissions (as the
    /// verified manifest has them) are `permissions`; checks the
    /// certificate (4.4 step 4). `generation` is the plugin's as the broker
    /// last listed it.
    pub fn open(
        broker: &dyn host_api::Broker,
        plugin: &str,
        app: &str,
        permissions: Option<&Value>,
        generation: Option<u64>,
    ) -> Result<Arc<Session>, RoxError> {
        let key = keys::SessionKey::generate();
        let now = now_ms();
        let ask = host_api::SessionAsk {
            plugin: plugin.to_owned(),
            app: app.to_owned(),
            open_id: keys::random_id(),
            ks_pub: key.public(),
            perms_sha256: keys::perms_sha256(permissions).map_err(|e| RoxError::new("invalid_args", e))?,
            not_after: now + MAX_LIFETIME_MS,
        };
        let answer = host_api::session_open(broker, &ask)?;
        let not_after = verify_cert(&answer, &ask, now)?;
        if let Some(g) = generation {
            seen_generation(plugin, g);
        }
        let session = Arc::new(Session {
            plugin: ask.plugin,
            app: ask.app,
            open_id: ask.open_id,
            not_after,
            key,
            ended: Mutex::new(None),
            on_end: Mutex::new(Vec::new()),
        });
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        live.retain(|s| s.strong_count() > 0);
        live.push(Arc::downgrade(&session));
        Ok(session)
    }

    /// `Ok` while calls may go out: not ended, not past `not_after`.
    pub fn check_live(&self) -> Result<(), RoxError> {
        match *self.ended.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(Ended::Closed) => return Err(RoxError::new("closed", "the app was closed")),
            Some(Ended::Restarted) => return Err(RoxError::new("plugin_restarted", "the plugin restarted")),
            None => {}
        }
        if now_ms() >= self.not_after {
            return Err(RoxError::new("session_invalid", "the app's session expired; reopen it"));
        }
        Ok(())
    }

    /// The signed envelope of a call of `tool` with `args` (8.1):
    /// `(envelope, sig)`.
    pub fn sign_call(&self, tool: &str, args: &Value) -> Result<(Value, String), RoxError> {
        self.check_live()?;
        let envelope = json!({
            "v": 1, "typ": "rox-call",
            "plugin": self.plugin, "app": self.app, "open_id": self.open_id,
            "tool": tool, "args": args,
            "ts": now_ms(), "nonce": keys::random_id(),
        });
        let sig = self.key.sign(&envelope).map_err(|e| RoxError::new("invalid_args", e))?;
        Ok((envelope, sig))
    }

    /// The public half of the session key (`ks_pub`).
    pub fn public_key(&self) -> String {
        self.key.public()
    }

    /// Calls `f` when the session ends (at once if it has).
    pub fn on_end(&self, f: impl Fn(Ended) + Send + Sync + 'static) {
        if let Some(why) = *self.ended.lock().unwrap_or_else(|e| e.into_inner()) {
            return f(why);
        }
        self.on_end.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(f));
    }

    /// Ends the session for `why` (once); its listeners hear it.
    pub fn end(&self, why: Ended) {
        {
            let mut ended = self.ended.lock().unwrap_or_else(|e| e.into_inner());
            if ended.is_some() {
                return;
            }
            *ended = Some(why);
        }
        let listeners = std::mem::take(&mut *self.on_end.lock().unwrap_or_else(|e| e.into_inner()));
        for f in listeners {
            f(why);
        }
    }

    /// Closes the session: ends it here and tells the broker on a thread
    /// of its own (`sessions/close`, best effort).
    pub fn close(&self, broker: Arc<dyn host_api::Broker>) {
        let restarted = matches!(*self.ended.lock().unwrap_or_else(|e| e.into_inner()), Some(Ended::Restarted));
        self.end(Ended::Closed);
        if restarted {
            return;
        }
        let open_id = self.open_id.clone();
        std::thread::spawn(move || {
            if let Err(e) = host_api::session_close(&*broker, &open_id) {
                log::info!("mini v1: sessions/close: {e}");
            }
        });
    }
}

/// Checks `sessions/open`'s answer against what was asked (`ask`, at
/// `now`): the plugin's signature over JCS(cert), every field, the
/// lifetime and the permissions the broker holds. The certificate's
/// `not_after`, or `plugin_bad_signature`.
pub fn verify_cert(answer: &Value, ask: &host_api::SessionAsk, now: i64) -> Result<i64, RoxError> {
    let bad = |m: &str| RoxError::new("plugin_bad_signature", format!("the session certificate {m}"));
    let cert = answer.get("cert").filter(|c| c.is_object()).ok_or_else(|| bad("is missing"))?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let plugin_pub = text(answer, "plugin_pub").ok_or_else(|| bad("comes without the plugin's key"))?;
    let sig = text(answer, "cert_sig").ok_or_else(|| bad("is not signed"))?;
    keys::verify(&plugin_pub, cert, &sig).map_err(|_| bad("does not verify with the plugin's key"))?;
    let expected = [
        ("typ", "rox-session"),
        ("plugin", &ask.plugin),
        ("app", &ask.app),
        ("open_id", &ask.open_id),
        ("ks_pub", &ask.ks_pub),
        ("perms_sha256", &ask.perms_sha256),
    ];
    if cert.get("v") != Some(&json!(1)) {
        return Err(bad("is not version 1"));
    }
    for (k, want) in expected {
        if text(cert, k).as_deref() != Some(want) {
            return Err(bad(&format!("names another {k}")));
        }
    }
    let not_after = cert.get("not_after").and_then(Value::as_i64).ok_or_else(|| bad("has no not_after"))?;
    if not_after <= now {
        return Err(bad("has expired"));
    }
    if not_after > now + MAX_LIFETIME_MS + SKEW_MS {
        return Err(bad("lives longer than 8 hours"));
    }
    if let Some(p) = answer.get("permissions") {
        let held = keys::perms_sha256(Some(p)).map_err(|_| bad("comes with bad permissions"))?;
        if held != ask.perms_sha256 {
            return Err(bad("comes with other permissions than the manifest's"));
        }
    }
    Ok(not_after)
}

/// Records `generation` as `plugin`'s current one.
fn seen_generation(plugin: &str, generation: u64) {
    let mut g = GENERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new).entry(plugin.to_owned()).or_insert(generation);
}

/// A broker `plugin` event (`{id, generation, state, …}`): on a new
/// generation, the sessions of the plugin's old one end (14.4). Returns
/// how many ended. The seam the event listener calls; tests call it too.
pub fn on_plugin_event(event: &Value) -> usize {
    let Some(plugin) = event.get("id").or_else(|| event.get("plugin")).and_then(Value::as_str) else { return 0 };
    let generation = event.get("generation").and_then(Value::as_u64);
    let gone = matches!(event.get("state").and_then(Value::as_str), Some("disabled" | "uninstalled" | "failed"));
    let changed = {
        let mut g = GENERATIONS.lock().unwrap_or_else(|e| e.into_inner());
        let g = g.get_or_insert_with(HashMap::new);
        match generation {
            Some(n) => g.insert(plugin.to_owned(), n).is_some_and(|old| old != n),
            None => false,
        }
    };
    if !(changed || gone) {
        return 0;
    }
    end_plugin(plugin, Ended::Restarted)
}

/// Ends every live session of `plugin` for `why`; how many ended.
pub fn end_plugin(plugin: &str, why: Ended) -> usize {
    let sessions: Vec<Arc<Session>> = {
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        live.retain(|s| s.strong_count() > 0);
        live.iter().filter_map(Weak::upgrade).filter(|s| s.plugin == plugin).collect()
    };
    let mut n = 0;
    for s in sessions {
        if s.check_live().is_ok() {
            n += 1;
        }
        s.end(why);
    }
    n
}

/// Follows the broker's events on a thread of its own, once per process,
/// reconnecting after a pause when the stream ends.
pub fn listen_for_generations() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new()
            .name("rox-broker-events".to_owned())
            .spawn(|| {
                let broker = host_api::HttpBroker::configured();
                loop {
                    let _ = broker.events(|name, data| {
                        if name == "plugin"
                            && let Ok(v) = serde_json::from_str::<Value>(data)
                        {
                            on_plugin_event(&v);
                        }
                        true
                    });
                    std::thread::sleep(Duration::from_secs(5));
                }
            })
            .ok();
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::mini_v1::host_api::Broker;

    /// A broker that plays the plugin's part of `sessions/open`, signing
    /// certificates with its own plugin key; `tamper` changes the cert
    /// before it is signed or the answer after.
    pub(crate) struct CertBroker {
        pub plugin_key: keys::SessionKey,
        pub tamper: fn(&mut Value, &mut Value),
    }

    impl CertBroker {
        pub(crate) fn honest() -> CertBroker {
            CertBroker { plugin_key: keys::SessionKey::generate(), tamper: |_, _| {} }
        }
    }

    impl Broker for CertBroker {
        fn request(&self, _: &str, route: &str, body: Option<Value>, _: Duration) -> Result<Value, RoxError> {
            assert_eq!(route, "/v1/host/sessions/open");
            let b = body.unwrap();
            let mut cert = json!({
                "v": 1, "typ": "rox-session", "plugin": b["plugin"], "app": b["app"],
                "open_id": b["open_id"], "ks_pub": b["ks_pub"], "perms_sha256": b["perms_sha256"],
                "not_after": b["not_after"],
            });
            let mut answer = json!({"plugin_pub": self.plugin_key.public()});
            (self.tamper)(&mut cert, &mut Value::Null);
            answer["cert_sig"] = json!(self.plugin_key.sign(&cert).unwrap());
            answer["cert"] = cert;
            (self.tamper)(&mut Value::Null, &mut answer);
            Ok(answer)
        }
    }

    fn open_with(tamper: fn(&mut Value, &mut Value)) -> Result<Arc<Session>, RoxError> {
        let b = CertBroker { plugin_key: keys::SessionKey::generate(), tamper };
        Session::open(&b, "dev.goldcoders.fixture", "card", Some(&json!({"tools": ["fx_echo"]})), None)
    }

    #[test]
    fn a_session_opens_on_a_certificate_that_verifies() {
        let s = open_with(|_, _| {}).unwrap();
        assert_eq!((s.plugin.as_str(), s.app.as_str()), ("dev.goldcoders.fixture", "card"));
        assert_eq!(s.open_id.len(), 22);
        assert!(s.not_after > now_ms() && s.not_after <= now_ms() + MAX_LIFETIME_MS);
        s.check_live().unwrap();
    }

    #[test]
    fn cert_failures_refuse_the_open_with_plugin_bad_signature() {
        let cases: [(&str, fn(&mut Value, &mut Value)); 11] = [
            ("signed by another key", |_, a| {
                if a.is_object() {
                    a["plugin_pub"] = json!(keys::SessionKey::generate().public());
                }
            }),
            ("changed after signing", |_, a| {
                if a.is_object() {
                    a["cert"]["not_after"] = json!(now_ms() + 1000);
                }
            }),
            ("another app", |c, _| if c.is_object() { c["app"] = json!("board") }),
            ("another plugin", |c, _| if c.is_object() { c["plugin"] = json!("dev.goldcoders.bir") }),
            ("another open", |c, _| if c.is_object() { c["open_id"] = json!(keys::random_id()) }),
            ("another session key", |c, _| if c.is_object() { c["ks_pub"] = json!(keys::SessionKey::generate().public()) }),
            ("another digest", |c, _| if c.is_object() { c["perms_sha256"] = json!(keys::perms_sha256(None).unwrap()) }),
            ("expired", |c, _| if c.is_object() { c["not_after"] = json!(now_ms() - 1) }),
            ("over 8 hours", |c, _| if c.is_object() { c["not_after"] = json!(now_ms() + MAX_LIFETIME_MS + 120_000) }),
            ("wrong type", |c, _| if c.is_object() { c["typ"] = json!("rox-call") }),
            ("other permissions held", |_, a| if a.is_object() { a["permissions"] = json!({"tools": ["fx_fail"]}) }),
        ];
        for (name, tamper) in cases {
            let e = open_with(tamper).err().unwrap_or_else(|| panic!("{name}: opened"));
            assert_eq!(e.code, "plugin_bad_signature", "{name}: {e}");
        }
        // the broker's own refusal comes through as is
        struct Refuses;
        impl Broker for Refuses {
            fn request(&self, _: &str, _: &str, _: Option<Value>, _: Duration) -> Result<Value, RoxError> {
                Err(RoxError::new("plugin_unavailable", "not running"))
            }
        }
        let e = Session::open(&Refuses, "dev.goldcoders.fixture", "card", None, None).err().unwrap();
        assert_eq!(e.code, "plugin_unavailable");
    }

    #[test]
    fn brg_03_envelopes_are_signed_over_jcs_with_a_fresh_nonce() {
        let s = open_with(|_, _| {}).unwrap();
        let args = json!({"period": "2026-09", "code": "1601C"});
        let (env, sig) = s.sign_call("fx_echo", &args).unwrap();
        assert_eq!(env["v"], 1);
        assert_eq!(env["typ"], "rox-call");
        assert_eq!(env["open_id"], s.open_id.as_str());
        assert_eq!(env["args"], args);
        let ts = env["ts"].as_i64().unwrap();
        assert!((now_ms() - ts).abs() < 5000);
        assert_eq!(keys::unb64(env["nonce"].as_str().unwrap()).unwrap().len(), 16);
        // an independent check: the canonical bytes written out by hand,
        // verified with ed25519-dalek directly (not through keys::verify)
        let by_hand = format!(
            r#"{{"app":"card","args":{{"code":"1601C","period":"2026-09"}},"nonce":"{}","open_id":"{}","plugin":"dev.goldcoders.fixture","tool":"fx_echo","ts":{ts},"typ":"rox-call","v":1}}"#,
            env["nonce"].as_str().unwrap(),
            s.open_id
        );
        let public: [u8; 32] = keys::unb64(&s.public_key()).unwrap().try_into().unwrap();
        let sig: [u8; 64] = keys::unb64(&sig).unwrap().try_into().unwrap();
        ed25519_dalek::VerifyingKey::from_bytes(&public)
            .unwrap()
            .verify_strict(by_hand.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
            .expect("the envelope's signature verifies over its JCS bytes");
        // every call gets its own nonce
        let (env2, _) = s.sign_call("fx_echo", &args).unwrap();
        assert_ne!(env["nonce"], env2["nonce"]);
        // floats are not signed
        assert_eq!(s.sign_call("fx_echo", &json!({"x": 0.5})).unwrap_err().code, "invalid_args");
    }

    #[test]
    fn a_new_plugin_generation_ends_its_sessions() {
        let b = CertBroker::honest();
        let plugin = "dev.goldcoders.genfixture";
        let s = Session::open(&b, plugin, "card", None, Some(3)).unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        s.on_end(move |why| h.lock().unwrap().push(why));
        // the same generation (or another plugin's change) ends nothing
        assert_eq!(on_plugin_event(&json!({"id": plugin, "generation": 3, "state": "running"})), 0);
        assert_eq!(on_plugin_event(&json!({"id": "dev.goldcoders.other", "generation": 9})), 0);
        s.check_live().unwrap();
        assert_eq!(on_plugin_event(&json!({"id": plugin, "generation": 4, "state": "running"})), 1);
        assert_eq!(s.check_live().unwrap_err().code, "plugin_restarted");
        assert_eq!(*heard.lock().unwrap(), [Ended::Restarted]);
        assert_eq!(s.sign_call("fx_echo", &json!({})).unwrap_err().code, "plugin_restarted");
    }
}
