//! App opens and the calls they make (spec 4.4, 7.3, 8, 9.2;
//! `v1/sessions.mjs`).
//!
//! Rox opens an app by asking for a session: `rox.session.open` goes to
//! the plugin, its certificate is verified with the plugin key from the
//! install record and kept. From then on every bridge call arrives as
//! Rox's signed envelope and passes the nine checks of 8.2, in the spec's
//! order, before anything reaches the plugin; the plugin then sees
//! `_meta["rox/caller"]` (8.4). Ticket titles, resolve tools and app
//! sources are here too: they share the validation of plugin answers.
//! A plugin's opens end when it is disabled, uninstalled or updated
//! ([`Host::on_close`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::plugins::{parse, result_data, Host};
use crate::js;
use crate::v1::manifest::{self, limits::PREVIEW_MAX_BYTES as LIMITS_PREVIEW_MAX, perms_digest};
use crate::v1::{canonicalize, ed_verify, from_b64u, get, is_v1, same, safe_int, signed_bytes, tpl, V1Error};

pub const SKEW_MS: f64 = 60_000.0;
pub const NONCE_WINDOW_MS: f64 = 5.0 * 60_000.0;
pub const NONCE_MAX: usize = 10_000;
pub const CERT_MAX_MS: f64 = 8.0 * 3_600_000.0;
pub const MAX_INFLIGHT: usize = 8;
pub const MAX_PER_SECOND: usize = 20;
pub const ARGS_MAX: usize = 256 * 1024;
pub const RESULT_MAX: usize = 1024 * 1024;
pub const SOURCE_MAX: usize = 512 * 1024;

type Res<T = Value> = Result<T, V1Error>;

/// `[...s].slice(0, n).join("")`: the first `n` code points.
fn cap(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn first_text(r: &Value) -> String {
    get(r, "content").and_then(Value::as_array).into_iter().flatten().find(|c| get(c, "type") == Some(&json!("text"))).and_then(|c| get(c, "text")).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// A tool result's size as 7.3 counts it: structured plus text (and an
/// embedded resource's text or blob), in bytes.
pub fn result_bytes(r: &Value) -> usize {
    let piece = |c: &Value| match get(c, "type").and_then(Value::as_str) {
        Some("text") => get(c, "text").filter(|t| js::truthy(t)).map_or(0, |t| js::string_of(t).len()),
        Some("resource") => {
            let res = get(c, "resource");
            let v = res.and_then(|r| get(r, "text")).filter(|v| !v.is_null()).or_else(|| res.and_then(|r| get(r, "blob")).filter(|v| !v.is_null()));
            v.map_or(0, |v| js::string_of(v).len())
        }
        _ => 0,
    };
    let text: usize = get(r, "content").and_then(Value::as_array).into_iter().flatten().map(piece).sum();
    text + get(r, "structuredContent").map_or(0, |s| js::stringify(s).len())
}

/// A tool result without `_meta` (8.4, 9.6: never shown past the broker).
pub fn strip_meta(r: &Value) -> Value {
    let mut out = r.clone();
    if let Some(o) = out.as_object_mut() {
        o.shift_remove("_meta");
    }
    out
}

/// An open request a plugin made (resolve `open`) checked against its own
/// manifest `m`: one of its own apps, props valid (6.6), a ticket of the
/// grammar. The normalized open.
pub fn check_open(m: &Value, o: &Value) -> Res {
    if !o.is_object() {
        return V1Error::err("invalid_args", "an open must be an object");
    }
    if let Some(p) = get(o, "plugin") {
        if !same(Some(p), get(m, "id")) {
            return V1Error::err("not_declared", format!("an open of another plugin's app ({}) is refused", js::slice_utf16(&js::string_of(p), 80)));
        }
    }
    let app = get(m, "apps").and_then(Value::as_array).into_iter().flatten().find(|a| same(get(a, "id"), get(o, "app")));
    let Some(app) = app else {
        return V1Error::err("not_declared", format!("{} is not an app of {}", js::slice_utf16(&tpl(get(o, "app")), 60), tpl(get(m, "id"))));
    };
    if get(o, "ticket").is_some() && !is_v1("ticket", get(o, "ticket")) {
        return V1Error::err("invalid_args", "ticket breaks the ticket grammar");
    }
    let placement = get(app, "placement").and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(p) = get(o, "placement") {
        if !placement.iter().any(|x| same(Some(x), Some(p))) {
            return V1Error::err("invalid_args", format!("{} does not declare placement {}", tpl(get(app, "id")), js::slice_utf16(&js::string_of(p), 20)));
        }
    }
    if let Some(size) = get(o, "size") {
        if size != "small" && size != "main" {
            return V1Error::err("invalid_args", "size must be small or main");
        }
    }
    let props = get(o, "props").filter(|p| !p.is_null()).cloned().unwrap_or(json!({}));
    manifest::validate_props(get(app, "props"), &props)?;
    let title = match get(o, "title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        Some(t) => t.to_owned(),
        None => tpl(get(app, "title")),
    };
    let placement = get(o, "placement").filter(|p| js::truthy(p)).cloned().unwrap_or_else(|| placement.first().cloned().unwrap_or(Value::Null));
    let mut out = json!({"plugin": get(m, "id"), "app": get(app, "id"), "props": props, "title": cap(&title, 60), "placement": placement});
    if let Some(t) = get(o, "ticket") {
        out["ticket"] = t.clone();
    }
    if get(o, "size") == Some(&json!("small")) {
        out["size"] = json!("small");
    }
    Ok(out)
}

struct Session {
    plugin: String,
    app: String,
    cert: Value,
    cert_sig: Value,
    generation: u64,
    nonces: VecDeque<(String, f64)>,
    seen: HashSet<String>,
    recent: VecDeque<f64>,
    slots: Arc<Semaphore>,
    queued: Arc<AtomicUsize>,
}

pub struct Sessions {
    host: Arc<Host>,
    opens: Mutex<HashMap<String, Session>>,
}

impl Sessions {
    /// The sessions of `host`; a plugin's opens end with it.
    pub fn new(host: Arc<Host>) -> Arc<Sessions> {
        let s = Arc::new(Sessions { host: host.clone(), opens: Mutex::new(HashMap::new()) });
        let me: Weak<Sessions> = Arc::downgrade(&s);
        host.on_close(Arc::new(move |plugin: String| {
            let me = me.clone();
            Box::pin(async move {
                let Some(s) = me.upgrade() else { return };
                let ids: Vec<String> = s.lock().iter().filter(|(_, x)| x.plugin == plugin).map(|(k, _)| k.clone()).collect();
                for id in ids {
                    let _ = s.close(&json!({"open_id": id})).await;
                }
            })
        }));
        s
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.opens.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> f64 {
        self.host.clock.now()
    }

    // ── open / close (4.4) ───────────────────────────────────────────────

    fn check_open_request(&self, b: &Value) -> Res<()> {
        if !b.is_object() {
            return V1Error::err("invalid_args", "send {plugin, app, open_id, ks_pub, perms_sha256, not_after}");
        }
        if !is_v1("pluginId", get(b, "plugin")) || !is_v1("appId", get(b, "app")) {
            return V1Error::err("invalid_args", "plugin and app must be ids");
        }
        if from_b64u(get(b, "open_id"), 16).is_none() {
            return V1Error::err("invalid_args", "open_id must be 128 bits of base64url");
        }
        if from_b64u(get(b, "ks_pub"), 32).is_none() {
            return V1Error::err("invalid_args", "ks_pub must be a 32-byte ed25519 key in base64url");
        }
        let digest = tpl(get(b, "perms_sha256"));
        if !(digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))) {
            return V1Error::err("invalid_args", "perms_sha256 must be 64 lowercase hex");
        }
        let now = self.now();
        match safe_int(get(b, "not_after")) {
            Some(t) if t > now && t <= now + CERT_MAX_MS => {}
            _ => return V1Error::err("invalid_args", "not_after must be in the next 8 hours (unix ms)"),
        }
        if self.lock().contains_key(&tpl(get(b, "open_id"))) {
            return V1Error::err("invalid_args", "open_id is already open");
        }
        Ok(())
    }

    /// POST /v1/host/sessions/open → {cert, cert_sig, plugin_pub, permissions}.
    pub async fn open(&self, b: &Value) -> Res {
        self.check_open_request(b)?;
        let (plugin, app_id, open_id) = (tpl(get(b, "plugin")), tpl(get(b, "app")), tpl(get(b, "open_id")));
        let Some(row) = self.host.row(&plugin) else { return V1Error::err("not_found", format!("{plugin} is not installed")) };
        let (state, generation, _) = self.host.runtime_of(&plugin);
        if !row.enabled || state != "running" {
            return V1Error::err("plugin_unavailable", format!("{plugin} is not running"));
        }
        let m = row.manifest();
        let Some(app) = get(&m, "apps").and_then(Value::as_array).into_iter().flatten().find(|a| get(a, "id").and_then(Value::as_str) == Some(app_id.as_str())).cloned() else {
            return V1Error::err("not_found", format!("{plugin} has no app {app_id}"));
        };
        if perms_digest(get(&app, "permissions")).ok().as_deref() != get(b, "perms_sha256").and_then(Value::as_str) {
            return V1Error::err("invalid_args", "perms_sha256 is not the digest of the installed app's permissions");
        }
        let req = json!({
            "app": get(b, "app"), "open_id": get(b, "open_id"), "ks_pub": get(b, "ks_pub"),
            "perms_sha256": get(b, "perms_sha256"), "not_after": get(b, "not_after"),
        });
        let res = self.host.call_reserved(&plugin, "rox.session.open", req).await?;
        if res.is_null() {
            self.host.fail(&plugin, "plugin_invalid", "the plugin does not implement rox.session.open").await?;
            return V1Error::err("plugin_invalid", "the plugin does not implement rox.session.open");
        }
        let cert = get(&res, "cert").cloned().unwrap_or(Value::Null);
        let sig = get(&res, "cert_sig").cloned().unwrap_or(Value::Null);
        let good = cert_matches(&cert, b)
            && signed_bytes(&cert).is_ok_and(|bytes| sig.as_str().is_some_and(|sig| ed_verify(&row.plugin_pub, &bytes, sig)));
        if !good {
            self.host.fail(&plugin, "plugin_bad_signature", "a session certificate failed verification").await?;
            return V1Error::err("plugin_bad_signature", "the plugin's session certificate does not verify");
        }
        self.lock().insert(
            open_id,
            Session {
                plugin,
                app: app_id,
                cert: cert.clone(),
                cert_sig: sig.clone(),
                generation,
                nonces: VecDeque::new(),
                seen: HashSet::new(),
                recent: VecDeque::new(),
                slots: Arc::new(Semaphore::new(MAX_INFLIGHT)),
                queued: Arc::new(AtomicUsize::new(0)),
            },
        );
        Ok(json!({"ok": true, "cert": cert, "cert_sig": sig, "plugin_pub": row.plugin_pub, "permissions": get(&app, "permissions").cloned().unwrap_or(json!({}))}))
    }

    /// POST /v1/host/sessions/close: forget the open; tell the plugin (best effort).
    pub async fn close(&self, b: &Value) -> Res {
        let id = get(b, "open_id").and_then(Value::as_str).unwrap_or_default();
        let Some(s) = self.lock().remove(id) else { return Ok(json!({"ok": true})) };
        if self.host.runtime_of(&s.plugin).0 == "running" {
            let _ = self.host.call_reserved(&s.plugin, "rox.session.close", json!({"open_id": id})).await;
        }
        Ok(json!({"ok": true}))
    }

    /// An open's live state (tests): `{active, queued}`, or `null`.
    pub fn stat(&self, open_id: &str) -> Value {
        match self.lock().get(open_id) {
            Some(s) => json!({"active": MAX_INFLIGHT - s.slots.available_permits(), "queued": s.queued.load(Ordering::SeqCst)}),
            None => Value::Null,
        }
    }

    // ── the call (8.2) ───────────────────────────────────────────────────

    /// Checks 1 to 9, in order, and the rate (20 a second); → the open's
    /// plugin, app, the app's permissions, its call slots.
    fn check(&self, envelope: &Value, sig: &str) -> Res<(String, String, Value, Arc<Semaphore>, Arc<AtomicUsize>)> {
        let now = self.now();
        let open_id = tpl(get(envelope, "open_id"));
        let found = get(envelope, "open_id").and_then(Value::as_str).and_then(|id| {
            let opens = self.lock();
            opens.get(id).map(|s| (s.plugin.clone(), s.app.clone(), s.cert.clone(), s.cert_sig.clone(), s.generation))
        });
        let row = found.as_ref().and_then(|f| self.host.row(&f.0));
        // 1: a registered open whose certificate verifies and has not expired
        let valid = match (&found, &row) {
            (Some((_, _, cert, sig, _)), Some(row)) => signed_bytes(cert).is_ok_and(|b| sig.as_str().is_some_and(|sig| ed_verify(&row.plugin_pub, &b, sig))),
            _ => false,
        };
        let (Some((plugin, app_id, cert, _, open_generation)), Some(row), true) = (found, row, valid) else {
            return V1Error::err("session_invalid", "no open with a valid certificate has this open_id");
        };
        if safe_int(get(&cert, "not_after")).unwrap_or(0.0) <= now {
            self.lock().remove(&open_id);
            return V1Error::err("session_invalid", "the session certificate expired");
        }
        // 2
        let c = &cert;
        let names = get(envelope, "v") == Some(&json!(1))
            && get(envelope, "typ") == Some(&json!("rox-call"))
            && same(get(envelope, "plugin"), get(c, "plugin"))
            && same(get(envelope, "app"), get(c, "app"))
            && same(get(envelope, "open_id"), get(c, "open_id"));
        if !names {
            return V1Error::err("session_invalid", "the envelope names another plugin, app or open");
        }
        // 3
        let ks = get(c, "ks_pub").and_then(Value::as_str).unwrap_or_default();
        if !signed_bytes(envelope).is_ok_and(|b| ed_verify(ks, &b, sig)) {
            return V1Error::err("bad_signature", "the envelope's signature does not verify with the session key");
        }
        // 4
        if !safe_int(get(envelope, "ts")).is_some_and(|ts| (now - ts).abs() <= SKEW_MS) {
            return V1Error::err("stale", "the envelope's time is more than 60 s from the broker's");
        }
        // 5
        if from_b64u(get(envelope, "nonce"), 16).is_none() {
            return V1Error::err("invalid_args", "nonce must be 128 bits of base64url");
        }
        let (slots, queued) = {
            let mut opens = self.lock();
            let Some(s) = opens.get_mut(&open_id) else { return V1Error::err("session_invalid", "no open with a valid certificate has this open_id") };
            while let Some((n, at)) = s.nonces.front() {
                if now - at <= NONCE_WINDOW_MS {
                    break;
                }
                s.seen.remove(n);
                s.nonces.pop_front();
            }
            let nonce = tpl(get(envelope, "nonce"));
            if s.seen.contains(&nonce) {
                return V1Error::err("replayed", "this envelope was already sent");
            }
            s.seen.insert(nonce.clone());
            s.nonces.push_back((nonce, now));
            while s.nonces.len() > NONCE_MAX {
                if let Some((n, _)) = s.nonces.pop_front() {
                    s.seen.remove(&n);
                }
            }
            (s.slots.clone(), s.queued.clone())
        };
        // 6
        let (state, generation, _) = self.host.runtime_of(&plugin);
        if open_generation != generation {
            return V1Error::err("plugin_restarted", "the plugin restarted since this open began");
        }
        // 7: the installed permissions, and the certificate covers exactly them
        let m = row.manifest();
        let app = get(&m, "apps").and_then(Value::as_array).into_iter().flatten().find(|a| get(a, "id").and_then(Value::as_str) == Some(app_id.as_str()));
        let perms = app.and_then(|a| get(a, "permissions")).filter(|p| js::truthy(p)).cloned().unwrap_or(json!({}));
        let tool = get(envelope, "tool");
        let listed = get(&perms, "tools").and_then(Value::as_array).is_some_and(|t| t.iter().any(|x| same(Some(x), tool)));
        let digest = perms_digest(Some(&perms)).ok().map(Value::String);
        if app.is_none() || digest.as_ref() != get(c, "perms_sha256") || !listed {
            return V1Error::err("permission_denied", format!("{} is not in this app's permissions", js::slice_utf16(&tpl(tool), 80)));
        }
        // 8
        let off = parse(&row.disabled_tools, json!([]));
        if off.as_array().is_some_and(|o| o.iter().any(|x| same(Some(x), tool))) {
            return V1Error::err("disabled", format!("{} is switched off", tpl(tool)));
        }
        // 9
        if !row.enabled || state != "running" {
            return V1Error::err("plugin_unavailable", format!("{plugin} is not running"));
        }
        Ok((plugin, app_id, perms, slots, queued))
    }

    fn rate(&self, open_id: &str) -> Res<()> {
        let now = self.now();
        let mut opens = self.lock();
        let Some(s) = opens.get_mut(open_id) else { return Ok(()) };
        while s.recent.front().is_some_and(|t| now - t >= 1000.0) {
            s.recent.pop_front();
        }
        if s.recent.len() >= MAX_PER_SECOND {
            return V1Error::err("rate_limited", format!("more than {MAX_PER_SECOND} calls in a second"));
        }
        s.recent.push_back(now);
        Ok(())
    }

    /// POST /v1/host/call {envelope, sig} → {ok, result} with `_meta` stripped.
    pub async fn call(&self, b: &Value) -> Res {
        let (Some(envelope), Some(sig)) = (get(b, "envelope").filter(|e| e.is_object()), get(b, "sig").and_then(Value::as_str)) else {
            return V1Error::err("invalid_args", "send {envelope, sig}");
        };
        if let Err(e) = signed_bytes(envelope) {
            return V1Error::err("invalid_args", e);
        }
        let (plugin, app, perms, slots, queued) = self.check(envelope, sig)?;
        let args = get(envelope, "args").filter(|a| !a.is_null()).cloned().unwrap_or(json!({}));
        // rox.preview: Rox calls the app's own tool for a document; only an
        // app with permissions.preview gets the larger result limit
        let preview = get(envelope, "purpose").is_some();
        if preview && get(envelope, "purpose") != Some(&json!("preview")) {
            return V1Error::err("invalid_args", "purpose must be \"preview\" when present");
        }
        if preview && !get(&perms, "preview").is_some_and(Value::is_object) {
            return V1Error::err("permission_denied", "this app has no preview permission");
        }
        let max = if preview {
            let wanted = get(&perms, "preview").and_then(|p| get(p, "max_bytes")).filter(|v| !v.is_null()).and_then(Value::as_f64).unwrap_or(LIMITS_PREVIEW_MAX as f64);
            wanted.min(LIMITS_PREVIEW_MAX as f64)
        } else {
            RESULT_MAX as f64
        };
        if !args.is_object() {
            return V1Error::err("invalid_args", "args must be a JSON object");
        }
        if canonicalize(&args).len() > ARGS_MAX {
            return V1Error::err("invalid_args", format!("args are over {ARGS_MAX} bytes"));
        }
        let tool = tpl(get(envelope, "tool"));
        let (_, _, tools) = self.host.runtime_of(&plugin);
        if !tools.iter().any(|t| get(t, "name").and_then(Value::as_str) == Some(tool.as_str())) {
            return V1Error::err("not_found", format!("{plugin} has no tool {tool}"));
        }
        let open_id = tpl(get(envelope, "open_id"));
        self.rate(&open_id)?;
        queued.fetch_add(1, Ordering::SeqCst);
        let permit = slots.acquire_owned().await;
        queued.fetch_sub(1, Ordering::SeqCst);
        let caller = json!({"origin": "app", "plugin": plugin, "app": app, "open_id": open_id, "ts": get(envelope, "ts")});
        let r = self.host.call_tool(&plugin, &tool, &args, Some(&json!({"rox/caller": caller})), None).await;
        drop(permit);
        let r = r?;
        if get(&r, "isError").is_some_and(js::truthy) {
            let text = Some(first_text(&r)).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("{tool} failed"));
            return V1Error::err("tool_error", cap(&text, 500));
        }
        if result_bytes(&r) as f64 > max {
            let limit = if preview { format!("{} MiB", js::number(((max / 1_048_576.0) * 10.0).round() / 10.0)) } else { "1 MiB".to_owned() };
            return V1Error::err("result_too_large", format!("{tool}'s result is over {limit}"));
        }
        Ok(json!({"ok": true, "result": strip_meta(&r)}))
    }

    // ── tickets, resolve, app source ─────────────────────────────────────

    fn running(&self, plugin: Option<&Value>) -> Res<super::plugins::Row> {
        let id = plugin.map(js::string_of).unwrap_or_default();
        let Some(row) = self.host.row(&id) else { return V1Error::err("not_found", format!("{} is not installed", js::slice_utf16(&tpl(plugin), 80))) };
        if !row.enabled || self.host.runtime_of(&id).0 != "running" {
            return V1Error::err("plugin_unavailable", format!("{} is not running", tpl(plugin)));
        }
        Ok(row)
    }

    /// POST /v1/host/tickets/title {plugin, ticket} → {valid, title?} (7.4).
    pub async fn ticket_title(&self, b: &Value) -> Res {
        if !is_v1("ticket", get(b, "ticket")) {
            return V1Error::err("invalid_args", "ticket breaks the ticket grammar");
        }
        let row = self.running(get(b, "plugin"))?;
        let res = self.host.call_reserved(&row.id, "rox.ticket.resolve", json!({"ticket": get(b, "ticket")})).await?;
        if res.is_null() {
            return V1Error::err("not_supported", format!("{} does not resolve tickets", tpl(get(b, "plugin"))));
        }
        // a title and nothing else crosses: never a subject (P3)
        match (get(&res, "valid"), get(&res, "title").and_then(Value::as_str)) {
            (Some(Value::Bool(true)), Some(title)) => Ok(json!({"ok": true, "valid": true, "title": cap(title, 120)})),
            _ => Ok(json!({"ok": true, "valid": false})),
        }
    }

    /// POST /v1/host/resolve {plugin, kind, name, value, complete, context}
    /// → the 9.2 result (10 s).
    pub async fn resolve(&self, b: &Value) -> Res {
        let kind = get(b, "kind").and_then(Value::as_str).filter(|k| *k == "command" || *k == "scope");
        let (true, Some(kind)) = (b.is_object(), kind) else { return V1Error::err("invalid_args", "kind must be command or scope") };
        let row = self.running(get(b, "plugin"))?;
        let m = row.manifest();
        let list = get(&m, if kind == "command" { "commands" } else { "scopes" }).and_then(Value::as_array);
        let Some(entry) = list.into_iter().flatten().find(|x| same(get(x, "name"), get(b, "name"))) else {
            return V1Error::err("not_found", format!("{} has no {kind} {}", tpl(get(b, "plugin")), js::slice_utf16(&tpl(get(b, "name")), 40)));
        };
        let resolver = tpl(get(entry, "resolve"));
        if parse(&row.disabled_tools, json!([])).as_array().is_some_and(|o| o.iter().any(|x| x == &json!(resolver))) {
            return V1Error::err("disabled", format!("{resolver} is switched off"));
        }
        let attached: Vec<Value> = get(b, "context")
            .and_then(|c| get(c, "attached"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let scope = get(a, "scope").and_then(Value::as_str)?;
                let token = get(a, "token").and_then(Value::as_str).filter(|t| rox_v1::tokens::canonical(t).is_some())?;
                Some(json!({"scope": scope, "token": token}))
            })
            .collect();
        let value = match get(b, "value") {
            None | Some(Value::Null) => String::new(),
            Some(v) => js::string_of(v),
        };
        let input = json!({"kind": kind, "name": get(b, "name"), "value": value, "complete": get(b, "complete") == Some(&json!(true)), "context": {"attached": attached}});
        let meta = json!({"rox/caller": {"origin": "resolve", "plugin": get(b, "plugin"), "ts": self.now() as i64}});
        let r = self.host.call_tool(&row.id, &resolver, &input, Some(&meta), Some(self.host.timeouts.resolve)).await?;
        if get(&r, "isError").is_some_and(js::truthy) {
            let text = first_text(&r);
            return Ok(json!({"ok": true, "result": "error", "message": cap(if text.is_empty() { "Could not resolve that" } else { &text }, 200)}));
        }
        let mut out = json!({"ok": true});
        for (k, v) in resolved(&m, kind, get(b, "name"), &result_data(&r))?.as_object().into_iter().flatten() {
            out[k] = v.clone();
        }
        Ok(out)
    }

    /// POST /v1/host/apps/source {plugin, app} → {source, fingerprint} (6.1, 6.2).
    pub async fn app_source(&self, b: &Value) -> Res {
        let row = self.running(get(b, "plugin"))?;
        let m = row.manifest();
        let Some(app) = get(&m, "apps").and_then(Value::as_array).into_iter().flatten().find(|a| same(get(a, "id"), get(b, "app"))).cloned() else {
            return V1Error::err("not_found", format!("{} has no app {}", tpl(get(b, "plugin")), js::slice_utf16(&tpl(get(b, "app")), 60)));
        };
        let entry = tpl(get(&app, "entry"));
        let read = self.host.read_resource(&row.id, &entry).await?;
        let contents = get(&read, "contents").and_then(Value::as_array).cloned().unwrap_or_default();
        let source = match contents.as_slice() {
            [one] => get(one, "text").and_then(Value::as_str).map(str::to_owned),
            _ => None,
        };
        let Some(source) = source else { return V1Error::err("plugin_invalid", format!("{entry} must answer exactly one text content")) };
        if let Some(mime) = get(&contents[0], "mimeType") {
            if !same(Some(mime), get(&app, "mime")) {
                return V1Error::err("plugin_invalid", format!("{entry} is {}, not {}", js::string_of(mime), tpl(get(&app, "mime"))));
            }
        }
        if source.len() > SOURCE_MAX {
            return V1Error::err("result_too_large", format!("{entry} is over 512 KiB"));
        }
        let fingerprint = crate::v1::signing::fingerprint_of(source.as_bytes());
        if get(&app, "fingerprint").and_then(Value::as_str) != Some(fingerprint.as_str()) {
            return V1Error::err("fingerprint_mismatch", "This app's code does not match its signed manifest. Update the plugin.");
        }
        Ok(json!({"ok": true, "source": source, "fingerprint": fingerprint}))
    }
}

fn cert_matches(cert: &Value, b: &Value) -> bool {
    cert.is_object()
        && get(cert, "v") == Some(&json!(1))
        && get(cert, "typ") == Some(&json!("rox-session"))
        && ["plugin", "app", "open_id", "ks_pub", "perms_sha256", "not_after"].iter().all(|k| same(get(cert, k), get(b, k)))
}

fn choices(list: Option<&Value>) -> Res<Vec<Value>> {
    let Some(list) = list.and_then(Value::as_array) else { return V1Error::err("plugin_invalid", "choices must be an array") };
    list.iter()
        .take(20)
        .map(|c| {
            let (Some(label), Some(value)) = (get(c, "label").and_then(Value::as_str), get(c, "value").and_then(Value::as_str)) else {
                return V1Error::err("plugin_invalid", "each choice needs a label and a value");
            };
            let mut out = json!({"label": cap(label, 80), "value": cap(value, 512)});
            if let Some(d) = get(c, "detail").and_then(Value::as_str) {
                out["detail"] = json!(cap(d, 160));
            }
            Ok(out)
        })
        .collect()
}

/// A resolve tool's answer as 9.2 allows it.
fn resolved(m: &Value, kind: &str, name: Option<&Value>, data: &Value) -> Res {
    match get(data, "result").and_then(Value::as_str) {
        Some("choices") => Ok(json!({"result": "choices", "choices": choices(get(data, "choices"))?})),
        Some("open") => {
            let mut out = json!({"result": "open"});
            for (k, v) in check_open(m, data)?.as_object().into_iter().flatten() {
                out[k] = v.clone();
            }
            Ok(out)
        }
        Some("error") => {
            let msg = get(data, "message").and_then(Value::as_str).unwrap_or("Could not resolve that");
            Ok(json!({"result": "error", "message": cap(msg, 200)}))
        }
        Some("token") => {
            if !is_v1("ticket", get(data, "ticket")) {
                return V1Error::err("plugin_invalid", "a token result needs a ticket");
            }
            let title = get(data, "title").and_then(Value::as_str).unwrap_or_default();
            let mut out = json!({"result": "token", "ticket": get(data, "ticket"), "title": cap(title, 60)});
            if kind == "scope" {
                out["token"] = json!(format!("@{}:{}:{}", tpl(get(m, "short")), tpl(name), tpl(get(data, "ticket"))));
            }
            Ok(out)
        }
        _ => V1Error::err(
            "plugin_invalid",
            format!("a resolve tool answered result {}; 9.2 allows choices, open, token, error", js::slice_utf16(&crate::v1::shown(get(data, "result"), usize::MAX), 40)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_size_counts_text_resources_and_structured_content() {
        let r = json!({"content": [{"type": "text", "text": "héllo"}, {"type": "resource", "resource": {"blob": "abcd"}}, {"type": "image", "data": "zzzz"}], "structuredContent": {"a": 1}});
        assert_eq!(result_bytes(&r), 6 + 4 + 7);
        assert_eq!(strip_meta(&json!({"_meta": {}, "content": []})), json!({"content": []}));
    }

    #[test]
    fn an_open_names_one_of_the_plugins_own_apps() {
        let m = json!({"id": "dev.a.b", "apps": [{"id": "card", "title": "Card", "placement": ["full", "inline"], "props": {"type": "object"}}]});
        let o = check_open(&m, &json!({"app": "card", "placement": "inline", "size": "small", "title": ""})).unwrap();
        assert_eq!(o, json!({"plugin": "dev.a.b", "app": "card", "props": {}, "title": "Card", "placement": "inline", "size": "small"}));
        assert_eq!(check_open(&m, &json!({"plugin": "dev.x.y", "app": "card"})).unwrap_err().code, "not_declared");
        assert_eq!(check_open(&m, &json!({"app": "nope"})).unwrap_err().message, "nope is not an app of dev.a.b");
        assert_eq!(check_open(&m, &json!({"app": "card", "ticket": "x"})).unwrap_err().code, "invalid_args");
        assert_eq!(check_open(&m, &json!({"app": "card", "size": "huge"})).unwrap_err().message, "size must be small or main");
    }

    #[test]
    fn resolve_answers_are_held_to_9_2() {
        let m = json!({"id": "dev.a.b", "short": "ab", "apps": []});
        let many: Vec<Value> = (0..25).map(|i| json!({"label": "x".repeat(90), "value": format!("v{i}")})).collect();
        let r = resolved(&m, "scope", Some(&json!("item")), &json!({"result": "choices", "choices": many})).unwrap();
        assert_eq!(r["choices"].as_array().unwrap().len(), 20);
        assert_eq!(r["choices"][0]["label"].as_str().unwrap().len(), 80);
        let t = resolved(&m, "scope", Some(&json!("item")), &json!({"result": "token", "ticket": "tkt_0123456789ABCDEFGHJKMNPQRS"})).unwrap();
        assert_eq!(t["token"], "@ab:item:tkt_0123456789ABCDEFGHJKMNPQRS");
        assert_eq!(resolved(&m, "command", None, &json!({"result": "nope"})).unwrap_err().message, "a resolve tool answered result \"nope\"; 9.2 allows choices, open, token, error");
        assert_eq!(resolved(&m, "command", None, &Value::Null).unwrap_err().message, "a resolve tool answered result undefined; 9.2 allows choices, open, token, error");
    }
}
