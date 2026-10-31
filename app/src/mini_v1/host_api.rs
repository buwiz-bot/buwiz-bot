//! The broker's host-internal API (`/v1/host/*`, design §4) as typed
//! functions. Every request carries the host token (`X-Rox-Host`); every
//! refusal comes back as the spec's [`RoxError`].
//!
//! The bridge's handlers run on the engine thread and hand their work to
//! threads of their own, so the transport here is blocking: a small
//! HTTP/1.1 client over a socket to the loopback broker ([`HttpBroker`]).
//! Everything goes through the [`Broker`] trait, which the tests implement
//! with a fake broker, so no test reaches a real one.

use std::{
    io::{BufRead as _, Read as _, Write as _},
    net::{TcpStream, ToSocketAddrs as _},
    time::Duration,
};

use serde_json::{Value, json};

use super::RoxError;

/// The broker, as Rox reaches it.
pub trait Broker: Send + Sync {
    /// `method` on `route` (`/v1/host/…`) with a JSON `body`: the answer's
    /// JSON, or the broker's refusal.
    fn request(&self, method: &str, route: &str, body: Option<Value>, timeout: Duration) -> Result<Value, RoxError>;
}

/// The bridge's call limit (spec 7.3), also the HTTP read timeout of a call.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// `rox.session.open` and the relay around it (5 s at the plugin, 14.2).
const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
/// Resolve tools, ticket titles and app source (14.2).
const SHORT_TIMEOUT: Duration = Duration::from_secs(10);

fn post(b: &dyn Broker, route: &str, body: Value, timeout: Duration) -> Result<Value, RoxError> {
    b.request("POST", route, Some(body), timeout)
}

/// `GET /v1/host/plugins`: the installed plugins with their manifests'
/// apps, commands, scopes and tools.
pub fn plugins(b: &dyn Broker) -> Result<Vec<Value>, RoxError> {
    let v = b.request("GET", "/v1/host/plugins", None, SHORT_TIMEOUT)?;
    Ok(v.get("plugins").and_then(Value::as_array).cloned().unwrap_or_default())
}

/// `POST /v1/host/apps/source`: an app's source and fingerprint.
pub fn app_source(b: &dyn Broker, plugin: &str, app: &str) -> Result<(String, String), RoxError> {
    let v = post(b, "/v1/host/apps/source", json!({"plugin": plugin, "app": app}), SHORT_TIMEOUT)?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    match (text("source"), text("fingerprint")) {
        (Some(s), Some(f)) => Ok((s, f)),
        _ => Err(RoxError::new("internal", "the broker sent no source")),
    }
}

/// What `sessions/open` asks for.
#[derive(Debug, Clone)]
pub struct SessionAsk {
    pub plugin: String,
    pub app: String,
    pub open_id: String,
    pub ks_pub: String,
    pub perms_sha256: String,
    pub not_after: i64,
}

/// `POST /v1/host/sessions/open`: `{cert, cert_sig, plugin_pub, permissions}`.
pub fn session_open(b: &dyn Broker, ask: &SessionAsk) -> Result<Value, RoxError> {
    let body = json!({
        "plugin": ask.plugin, "app": ask.app, "open_id": ask.open_id,
        "ks_pub": ask.ks_pub, "perms_sha256": ask.perms_sha256, "not_after": ask.not_after,
    });
    post(b, "/v1/host/sessions/open", body, SESSION_TIMEOUT)
}

/// `POST /v1/host/sessions/close` (best effort).
pub fn session_close(b: &dyn Broker, open_id: &str) -> Result<(), RoxError> {
    post(b, "/v1/host/sessions/close", json!({"open_id": open_id}), SHORT_TIMEOUT).map(drop)
}

/// `POST /v1/host/call`: the tool result (its `_meta` stripped).
pub fn call(b: &dyn Broker, envelope: &Value, sig: &str, timeout: Duration) -> Result<Value, RoxError> {
    let v = post(b, "/v1/host/call", json!({"envelope": envelope, "sig": sig}), timeout)?;
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

/// `POST /v1/host/tickets/title`: the ticket's title, `None` when the
/// plugin says it is not valid.
pub fn ticket_title(b: &dyn Broker, plugin: &str, ticket: &str) -> Result<Option<String>, RoxError> {
    let v = post(b, "/v1/host/tickets/title", json!({"plugin": plugin, "ticket": ticket}), SHORT_TIMEOUT)?;
    if v.get("valid").and_then(Value::as_bool) != Some(true) {
        return Ok(None);
    }
    Ok(v.get("title").and_then(Value::as_str).map(str::to_owned))
}

/// What a resolve tool is asked (spec 9.2).
#[derive(Debug, Clone)]
pub struct ResolveAsk {
    pub plugin: String,
    pub kind: &'static str,
    pub name: String,
    pub value: String,
    pub complete: bool,
    pub context: Value,
}

/// `POST /v1/host/resolve`: the spec 9.2 result.
pub fn resolve(b: &dyn Broker, ask: &ResolveAsk) -> Result<Value, RoxError> {
    let body = json!({
        "plugin": ask.plugin, "kind": ask.kind, "name": ask.name,
        "value": ask.value, "complete": ask.complete, "context": ask.context,
    });
    post(b, "/v1/host/resolve", body, SHORT_TIMEOUT)
}

/// `POST /v1/host/plugins/inspect`: a temporary spawn's manifest and review
/// lines (13.1 step 3); nothing is installed.
pub fn inspect(b: &dyn Broker, command: &str, args: &[String], env: Option<Value>) -> Result<Value, RoxError> {
    let mut body = json!({"command": command, "args": args});
    if let Some(env) = env {
        body["env"] = env;
    }
    post(b, "/v1/host/plugins/inspect", body, Duration::from_secs(30))
}

/// `POST /v1/host/plugins/install`: installs what was reviewed (refused
/// when the manifest's hash differs).
pub fn install(
    b: &dyn Broker,
    command: &str,
    args: &[String],
    env: Option<Value>,
    manifest_sha256: &str,
) -> Result<Value, RoxError> {
    let mut body = json!({"command": command, "args": args, "manifest_sha256": manifest_sha256});
    if let Some(env) = env {
        body["env"] = env;
    }
    post(b, "/v1/host/plugins/install", body, Duration::from_secs(30))
}

/// What the broker does to an installed plugin (13.4 to 13.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginAction {
    Enable,
    Disable,
    Uninstall,
}

/// `POST /v1/host/plugins/:id/{enable,disable,uninstall}`, broker side
/// only: Rox's own cascade (pins, attachments, storage) is part B's, with
/// [`super::storage::delete_plugin`] for the stores.
pub fn plugin_action(b: &dyn Broker, plugin: &str, action: PluginAction) -> Result<Value, RoxError> {
    if !super::perms::plugin_id_ok(plugin) {
        return Err(RoxError::new("invalid_args", "not a plugin id"));
    }
    let verb = match action {
        PluginAction::Enable => "enable",
        PluginAction::Disable => "disable",
        PluginAction::Uninstall => "uninstall",
    };
    post(b, &format!("/v1/host/plugins/{plugin}/{verb}"), json!({}), Duration::from_secs(30))
}

/// `POST /v1/host/plugins/:id/tools`: the per-tool switch (8.3).
pub fn set_tool_disabled(b: &dyn Broker, plugin: &str, tool: &str, disabled: bool) -> Result<Value, RoxError> {
    if !super::perms::plugin_id_ok(plugin) {
        return Err(RoxError::new("invalid_args", "not a plugin id"));
    }
    post(b, &format!("/v1/host/plugins/{plugin}/tools"), json!({"tool": tool, "disabled": disabled}), SHORT_TIMEOUT)
}

/// What the person does to a routine (5.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutineAction {
    /// The one-time approval of what it reads.
    Approve,
    /// Runs it in a chat.
    Enable,
    Disable,
}

/// `POST /v1/host/routines/{approve,enable,disable}`: `{plugin, routine}`,
/// with the chat it runs in for Enable. The routine as the broker now has it.
pub fn routine_action(
    b: &dyn Broker,
    plugin: &str,
    routine: &str,
    action: RoutineAction,
    thread: Option<&str>,
) -> Result<Value, RoxError> {
    if !super::perms::plugin_id_ok(plugin) || !super::perms::app_id_ok(routine) {
        return Err(RoxError::new("invalid_args", "not a plugin or routine id"));
    }
    let mut body = json!({"plugin": plugin, "routine": routine});
    let verb = match action {
        RoutineAction::Approve => "approve",
        RoutineAction::Enable => {
            body["thread"] = json!(thread.unwrap_or_default());
            "enable"
        }
        RoutineAction::Disable => "disable",
    };
    let v = post(b, &format!("/v1/host/routines/{verb}"), body, SHORT_TIMEOUT)?;
    Ok(v.get("routine").cloned().unwrap_or(Value::Null))
}

/// What the person does to one routine instance (5.9, "Routine instances").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceAction {
    /// One run now, out of schedule; the broker answers at once.
    Run,
    /// On (in the chat it reports to); `approve` in the body consents to
    /// what it reads too (its consent card's Enable).
    Enable,
    Disable,
    /// Its params, trigger, interval or label.
    Update,
    Delete,
    /// A new webhook secret: the answer's `path`, shown once.
    Hook,
}

/// `POST /v1/host/routines/{run,enable,disable,update,delete,hook}`:
/// `{plugin, routine, instance?, ...extra}` (`instance` "" for a broker
/// before instances). The broker's whole answer.
pub fn instance_action(
    b: &dyn Broker,
    plugin: &str,
    routine: &str,
    instance: &str,
    action: InstanceAction,
    extra: Value,
) -> Result<Value, RoxError> {
    if !super::perms::plugin_id_ok(plugin) || !super::perms::app_id_ok(routine) {
        return Err(RoxError::new("invalid_args", "not a plugin or routine id"));
    }
    let mut body = json!({"plugin": plugin, "routine": routine});
    if !instance.is_empty() {
        body["instance"] = json!(instance);
    }
    if let (Some(o), Some(x)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            o.insert(k.clone(), v.clone());
        }
    }
    let verb = match action {
        InstanceAction::Run => "run",
        InstanceAction::Enable => "enable",
        InstanceAction::Disable => "disable",
        InstanceAction::Update => "update",
        InstanceAction::Delete => "delete",
        InstanceAction::Hook => "hook",
    };
    post(b, &format!("/v1/host/routines/{verb}"), body, SHORT_TIMEOUT)
}

/// The broker at `client::backend()`, over HTTP/1.1 on the loopback.
pub struct HttpBroker {
    origin: String,
}

impl HttpBroker {
    /// The broker Rox is configured for (`ROX_BACKEND`).
    pub fn configured() -> HttpBroker {
        HttpBroker { origin: crate::client::backend().to_owned() }
    }

    /// `(host, port)` of an `http://` origin; `https` brokers are not
    /// reached by this transport (the broker is on the loopback).
    fn host_port(&self) -> Result<(String, u16), RoxError> {
        let rest = self.origin.strip_prefix("http://").ok_or_else(|| unavailable("the broker is not at an http:// address"))?;
        let (host, port) = rest.rsplit_once(':').unwrap_or((rest, "80"));
        let port = port.parse().map_err(|_| unavailable("the broker's port is not a number"))?;
        Ok((host.trim_matches(['[', ']']).to_owned(), port))
    }

    /// A connection with `timeout` for connecting, reading and writing.
    fn connect(&self, timeout: Duration) -> Result<(TcpStream, String), RoxError> {
        let (host, port) = self.host_port()?;
        let addr = (host.as_str(), port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .ok_or_else(|| unavailable("the broker's address does not resolve"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map_err(|_| unavailable("the broker is not running"))?;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
        Ok((stream, format!("{host}:{port}")))
    }

    /// The request head and body, token included.
    fn head(method: &str, route: &str, authority: &str, body: &[u8], accept: &str) -> Vec<u8> {
        let mut head = format!(
            "{method} {route} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept: {accept}\r\n"
        );
        if let Some(token) = crate::client::host_token() {
            head.push_str(&format!("X-Rox-Host: {token}\r\n"));
        }
        if method != "GET" {
            head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// Follows the broker's server-sent events (`GET /v1/host/events`):
    /// `on_event(name, data)` for each until the stream ends or
    /// `on_event` returns false.
    pub fn events(&self, mut on_event: impl FnMut(&str, &str) -> bool) -> Result<(), RoxError> {
        let (mut stream, authority) = self.connect(Duration::from_secs(3600))?;
        let req = Self::head("GET", "/v1/host/events", &authority, &[], "text/event-stream");
        stream.write_all(&req).map_err(|_| unavailable("the broker closed the connection"))?;
        let mut lines = std::io::BufReader::new(stream);
        let (mut name, mut data, mut in_body) = (String::new(), String::new(), false);
        let mut line = String::new();
        loop {
            line.clear();
            if lines.read_line(&mut line).map_err(|_| unavailable("the event stream ended"))? == 0 {
                return Ok(());
            }
            let l = line.trim_end_matches(['\r', '\n']);
            if !in_body {
                in_body = l.is_empty();
                continue;
            }
            // a chunked stream's size lines are no field lines; skip them
            if let Some(v) = l.strip_prefix("event:") {
                name = v.trim().to_owned();
            } else if let Some(v) = l.strip_prefix("data:") {
                data.push_str(v.trim_start());
            } else if l.is_empty() && !data.is_empty() {
                let keep = on_event(if name.is_empty() { "message" } else { &name }, &data);
                name.clear();
                data.clear();
                if !keep {
                    return Ok(());
                }
            }
        }
    }
}

fn unavailable(m: &str) -> RoxError {
    RoxError::new("plugin_unavailable", m)
}

impl Broker for HttpBroker {
    fn request(&self, method: &str, route: &str, body: Option<Value>, timeout: Duration) -> Result<Value, RoxError> {
        let body = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        match self.send(method, route, &body, timeout) {
            // the broker makes a new token when it starts: the file is read
            // again for one more try
            Err(e) if e.code == "unauthorized" => self.send(method, route, &body, timeout),
            r => r,
        }
    }
}

impl HttpBroker {
    fn send(&self, method: &str, route: &str, body: &[u8], timeout: Duration) -> Result<Value, RoxError> {
        let (mut stream, authority) = self.connect(timeout)?;
        stream
            .write_all(&Self::head(method, route, &authority, body, "application/json"))
            .map_err(|_| unavailable("the broker closed the connection"))?;
        let mut raw = Vec::new();
        stream.take(MAX_RESPONSE as u64 + 1).read_to_end(&mut raw).map_err(|e| match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => RoxError::new("timeout", "the broker took too long"),
            _ => unavailable("the broker closed the connection"),
        })?;
        let (status, body) = parse_response(&raw)?;
        answer(status, &body)
    }
}

/// The most of an answer read (results are capped at 1 MiB, sources at
/// 512 KiB; picked files never come this way).
const MAX_RESPONSE: usize = 16 << 20;

/// Status and body of a raw HTTP/1.1 response (chunked or not).
fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>), RoxError> {
    let bad = || RoxError::new("internal", "the broker's answer is not HTTP");
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(bad)?;
    let head = std::str::from_utf8(&raw[..end]).map_err(|_| bad())?;
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).ok_or_else(bad)?;
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let body = &raw[end + 4..];
    Ok((status, if chunked { dechunk(body).ok_or_else(bad)? } else { body.to_vec() }))
}

fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line = body.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&body[..line]).ok()?.split(';').next()?.trim();
        let size = usize::from_str_radix(size, 16).ok()?;
        body = &body[line + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(body.get(..size)?);
        body = body.get(size + 2..)?;
    }
}

/// The answer's JSON, or its refusal as a [`RoxError`].
fn answer(status: u16, body: &[u8]) -> Result<Value, RoxError> {
    let v: Option<Value> = serde_json::from_slice(body).ok();
    match (status, v) {
        (_, Some(v)) if v.get("ok") == Some(&Value::Bool(false)) || (status >= 400 && v.get("code").is_some()) => {
            Err(RoxError::from_body(&v))
        }
        (401, _) => Err(RoxError::new("internal", "the broker refused Buwiz Bot's host token")),
        (200..=299, Some(v)) => Ok(v),
        (s, _) => Err(RoxError::new("internal", format!("the broker answered {s}"))),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn broker_answers_map_to_rox_errors() {
        let ok = super::parse_response(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true,\"result\":1}").unwrap();
        assert_eq!(super::answer(ok.0, &ok.1).unwrap()["result"], 1);
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        let (s, b) = super::parse_response(chunked).unwrap();
        assert_eq!(super::answer(s, &b).unwrap(), json!({"a": 1}));
        let refused = br#"{"ok":false,"code":"permission_denied","message":"no","retryable":false}"#;
        assert_eq!(super::answer(403, refused).unwrap_err().code, "permission_denied");
        assert_eq!(super::answer(200, refused).unwrap_err().code, "permission_denied");
        assert_eq!(super::answer(401, b"").unwrap_err().code, "internal");
        assert_eq!(super::answer(502, b"<html>").unwrap_err().code, "internal");
        assert!(super::parse_response(b"garbage").is_err());
    }

    #[test]
    fn requests_name_the_route_and_carry_json() {
        let head = super::HttpBroker::head("POST", "/v1/host/call", "127.0.0.1:9", b"{}", "application/json");
        let text = String::from_utf8(head).unwrap();
        assert!(text.starts_with("POST /v1/host/call HTTP/1.1\r\nHost: 127.0.0.1:9\r\n"), "{text}");
        assert!(text.contains("Content-Type: application/json\r\nContent-Length: 2\r\n"), "{text}");
        assert!(text.ends_with("\r\n\r\n{}"), "{text}");
        // an unreachable broker is plugin_unavailable, not a hang
        let b = super::HttpBroker { origin: "http://127.0.0.1:1".to_owned() };
        let e = super::Broker::request(&b, "GET", "/v1/host/plugins", None, std::time::Duration::from_secs(1)).unwrap_err();
        assert_eq!(e.code, "plugin_unavailable");
        assert!(super::plugin_action(&b, "../x", super::PluginAction::Uninstall).is_err());
    }
}
