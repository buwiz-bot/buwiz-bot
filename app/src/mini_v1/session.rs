//! An open's session (spec 4.4) and its signed calls (8.1).
//!
//! Rox makes the session key, asks the broker to open the session (the
//! plugin signs a certificate over the key and the permissions digest),
//! and checks that certificate itself before the app gets any method: the
//! signature with the plugin's public key, and that every field is the one
//! asked for. Each `buwiz.call` is then an envelope signed here; the key
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

type EndListener = Box<dyn Fn(Ended) + Send + Sync>;

/// One open's session.
pub struct Session {
    pub plugin: String,
    pub app: String,
    pub open_id: String,
    pub not_after: i64,
    key: keys::SessionKey,
    ended: Mutex<Option<Ended>>,
    on_end: Mutex<Vec<EndListener>>,
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
        self.sign(tool, args, None)
    }

    /// [`Session::sign_call`] for `buwiz.preview`: the envelope says
    /// `"purpose": "preview"`, which the broker takes only from an app
    /// granted `preview`, and answers with the larger document limit.
    pub fn sign_preview(&self, tool: &str, args: &Value) -> Result<(Value, String), RoxError> {
        self.sign(tool, args, Some("preview"))
    }

    fn sign(&self, tool: &str, args: &Value, purpose: Option<&str>) -> Result<(Value, String), RoxError> {
        self.check_live()?;
        let mut envelope = json!({
            "v": 1, "typ": "buwiz-call",
            "plugin": self.plugin, "app": self.app, "open_id": self.open_id,
            "tool": tool, "args": args,
            "ts": now_ms(), "nonce": keys::random_id(),
        });
        if let Some(p) = purpose {
            envelope["purpose"] = json!(p);
        }
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

/// How long a pre-opened session waits for its card (spec 14.6).
pub const PREOPEN_TTL: Duration = Duration::from_secs(60);

type Preopened = (String, String, String, std::time::Instant, Arc<Session>);

/// Sessions opened ahead of their card (a tool result's inline card that
/// is about to mount): `(plugin, app, perms digest, opened at, session)`.
/// Each is used by one open at most (4.4); unused ones are closed after
/// [`PREOPEN_TTL`].
static POOL: Mutex<Vec<Preopened>> = Mutex::new(Vec::new());

/// Opens a session for `app` of `plugin` ahead of its open and keeps it
/// for [`take_preopened`]. Nothing when one is waiting already.
pub fn preopen(
    broker: &dyn host_api::Broker,
    plugin: &str,
    app: &str,
    permissions: Option<&Value>,
    generation: Option<u64>,
) -> Result<(), RoxError> {
    let digest = keys::perms_sha256(permissions).map_err(|e| RoxError::new("invalid_args", e))?;
    let waiting = POOL.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(p, a, d, _, s)| {
        p == plugin && a == app && *d == digest && s.check_live().is_ok()
    });
    if waiting {
        return Ok(());
    }
    let s = Session::open(broker, plugin, app, permissions, generation)?;
    POOL.lock().unwrap_or_else(|e| e.into_inner()).push((plugin.to_owned(), app.to_owned(), digest, std::time::Instant::now(), s));
    Ok(())
}

/// A session pre-opened for `app` of `plugin` with these permissions, if
/// one is waiting, fresh and live (it leaves the pool: one open each).
pub fn take_preopened(plugin: &str, app: &str, permissions: Option<&Value>) -> Option<Arc<Session>> {
    take_preopened_at(plugin, app, permissions, std::time::Instant::now())
}

fn take_preopened_at(plugin: &str, app: &str, permissions: Option<&Value>, now: std::time::Instant) -> Option<Arc<Session>> {
    let digest = keys::perms_sha256(permissions).ok()?;
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let at = pool.iter().position(|(p, a, d, t, s)| {
        p == plugin && a == app && *d == digest && now.duration_since(*t) < PREOPEN_TTL && s.check_live().is_ok()
    })?;
    Some(pool.remove(at).4)
}

/// Closes and drops pre-opened sessions no open took within
/// [`PREOPEN_TTL`] (or that ended): how many.
pub fn sweep_preopened(broker: Arc<dyn host_api::Broker>) -> usize {
    sweep_preopened_at(broker, std::time::Instant::now())
}

fn sweep_preopened_at(broker: Arc<dyn host_api::Broker>, now: std::time::Instant) -> usize {
    let stale: Vec<Arc<Session>> = {
        let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
        let (stale, keep): (Vec<_>, Vec<_>) =
            std::mem::take(&mut *pool).into_iter().partition(|(_, _, _, t, s)| now.duration_since(*t) >= PREOPEN_TTL || s.check_live().is_err());
        *pool = keep;
        stale.into_iter().map(|e| e.4).collect()
    };
    let n = stale.len();
    for s in stale {
        s.close(broker.clone());
    }
    n
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
        ("typ", "buwiz-session"),
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

type EventListener = Arc<dyn Fn(&Value) + Send + Sync>;

/// Who else hears the broker's `plugin` events (part B: the list, and
/// the opens waiting to reconnect), after their sessions are handled.
static LISTENER: Mutex<Option<EventListener>> = Mutex::new(None);

/// Sets who hears the broker's `plugin` events after [`on_plugin_event`].
pub fn set_event_listener(f: Arc<dyn Fn(&Value) + Send + Sync>) {
    *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
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
                            let f = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            if let Some(f) = f {
                                f(&v);
                            }
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
                "v": 1, "typ": "buwiz-session", "plugin": b["plugin"], "app": b["app"],
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
        Session::open(&b, "app.buwiz.fixture", "card", Some(&json!({"tools": ["fx_echo"]})), None)
    }

    #[test]
    fn a_session_opens_on_a_certificate_that_verifies() {
        let s = open_with(|_, _| {}).unwrap();
        assert_eq!((s.plugin.as_str(), s.app.as_str()), ("app.buwiz.fixture", "card"));
        assert_eq!(s.open_id.len(), 22);
        assert!(s.not_after > now_ms() && s.not_after <= now_ms() + MAX_LIFETIME_MS);
        s.check_live().unwrap();
    }

    #[test]
    fn cert_failures_refuse_the_open_with_plugin_bad_signature() {
        type CertCase = (&'static str, fn(&mut Value, &mut Value));
        let cases: [CertCase; 11] = [
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
            ("another plugin", |c, _| if c.is_object() { c["plugin"] = json!("app.buwiz.ebirforms") }),
            ("another open", |c, _| if c.is_object() { c["open_id"] = json!(keys::random_id()) }),
            ("another session key", |c, _| if c.is_object() { c["ks_pub"] = json!(keys::SessionKey::generate().public()) }),
            ("another digest", |c, _| if c.is_object() { c["perms_sha256"] = json!(keys::perms_sha256(None).unwrap()) }),
            ("expired", |c, _| if c.is_object() { c["not_after"] = json!(now_ms() - 1) }),
            ("over 8 hours", |c, _| if c.is_object() { c["not_after"] = json!(now_ms() + MAX_LIFETIME_MS + 120_000) }),
            ("wrong type", |c, _| if c.is_object() { c["typ"] = json!("buwiz-call") }),
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
        let e = Session::open(&Refuses, "app.buwiz.fixture", "card", None, None).err().unwrap();
        assert_eq!(e.code, "plugin_unavailable");
    }

    #[test]
    fn brg_03_envelopes_are_signed_over_jcs_with_a_fresh_nonce() {
        let s = open_with(|_, _| {}).unwrap();
        let args = json!({"period": "2026-09", "code": "1601C"});
        let (env, sig) = s.sign_call("fx_echo", &args).unwrap();
        assert_eq!(env["v"], 1);
        assert_eq!(env["typ"], "buwiz-call");
        assert_eq!(env["open_id"], s.open_id.as_str());
        assert_eq!(env["args"], args);
        let ts = env["ts"].as_i64().unwrap();
        assert!((now_ms() - ts).abs() < 5000);
        assert_eq!(keys::unb64(env["nonce"].as_str().unwrap()).unwrap().len(), 16);
        // an independent check: the canonical bytes written out by hand,
        // verified with ed25519-dalek directly (not through keys::verify)
        let by_hand = format!(
            r#"{{"app":"card","args":{{"code":"1601C","period":"2026-09"}},"nonce":"{}","open_id":"{}","plugin":"app.buwiz.fixture","tool":"fx_echo","ts":{ts},"typ":"buwiz-call","v":1}}"#,
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

    /// Counts `sessions/open` and `sessions/close` and signs like
    /// [`CertBroker`].
    struct Counting {
        inner: CertBroker,
        opened: std::sync::atomic::AtomicUsize,
        closed: std::sync::atomic::AtomicUsize,
    }

    impl Broker for Counting {
        fn request(&self, m: &str, route: &str, body: Option<Value>, t: Duration) -> Result<Value, RoxError> {
            if route == "/v1/host/sessions/close" {
                self.closed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Ok(json!({"ok": true}));
            }
            self.opened.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.request(m, route, body, t)
        }
    }

    #[test]
    fn perf_04_a_preopened_session_is_used_once_or_dropped_after_a_minute() {
        let b = Arc::new(Counting { inner: CertBroker::honest(), opened: 0.into(), closed: 0.into() });
        let (p, perms) = ("app.buwiz.poolfx", Some(json!({"tools": ["fx_echo"]})));
        preopen(&*b, p, "card", perms.as_ref(), None).unwrap();
        preopen(&*b, p, "card", perms.as_ref(), None).unwrap();
        assert_eq!(b.opened.load(std::sync::atomic::Ordering::SeqCst), 1, "one waiting session per app");
        // other permissions or another app: not this session
        assert!(take_preopened(p, "card", Some(&json!({"tools": []}))).is_none());
        assert!(take_preopened(p, "board", perms.as_ref()).is_none());
        let s = take_preopened(p, "card", perms.as_ref()).expect("reused");
        s.check_live().unwrap();
        assert!(take_preopened(p, "card", perms.as_ref()).is_none(), "one open each (4.4)");
        // unused for a minute: closed and dropped
        preopen(&*b, p, "card", perms.as_ref(), None).unwrap();
        let later = std::time::Instant::now() + PREOPEN_TTL + Duration::from_secs(1);
        assert!(take_preopened_at(p, "card", perms.as_ref(), later).is_none());
        let broker: Arc<dyn host_api::Broker> = b.clone();
        assert_eq!(sweep_preopened_at(broker, later), 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(b.closed.load(std::sync::atomic::Ordering::SeqCst), 1, "the broker hears it closed");
        assert!(take_preopened(p, "card", perms.as_ref()).is_none());
    }

    #[test]
    fn a_new_plugin_generation_ends_its_sessions() {
        let b = CertBroker::honest();
        let plugin = "app.buwiz.genfixture";
        let s = Session::open(&b, plugin, "card", None, Some(3)).unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        s.on_end(move |why| h.lock().unwrap().push(why));
        // the same generation (or another plugin's change) ends nothing
        assert_eq!(on_plugin_event(&json!({"id": plugin, "generation": 3, "state": "running"})), 0);
        assert_eq!(on_plugin_event(&json!({"id": "app.buwiz.other", "generation": 9})), 0);
        s.check_live().unwrap();
        assert_eq!(on_plugin_event(&json!({"id": plugin, "generation": 4, "state": "running"})), 1);
        assert_eq!(s.check_live().unwrap_err().code, "plugin_restarted");
        assert_eq!(*heard.lock().unwrap(), [Ended::Restarted]);
        assert_eq!(s.sign_call("fx_echo", &json!({})).unwrap_err().code, "plugin_restarted");
    }
}
