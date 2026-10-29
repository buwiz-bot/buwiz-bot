//! A remote MCP server's client (plan 4.2), as the broker's Node SDK (1.32)
//! runs one: streamable HTTP (client/streamableHttp.js), or the legacy SSE
//! transport (client/sse.js) for a row whose `transport` is "sse".
//!
//! Streamable HTTP POSTs each message (`accept: application/json,
//! text/event-stream`, the session id and protocol version once known) and
//! reads a JSON or an SSE answer; after `notifications/initialized` it opens
//! the optional GET stream (405: none). Legacy SSE GETs the stream, takes
//! its `endpoint` event, and POSTs messages there; answers come on the
//! stream. With stored tokens the bearer goes on every request, and a 401
//! runs OAuth (oauth.rs) in the background (a refresh; a sign-in only
//! records its URL) then sends once more; a second 401 right after is the
//! SDK's circuit breaker. Without tokens there is no auth at all, as Node
//! gave the transport no provider then.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::oneshot;
use url::Url;

use super::http::{self, Req};
use super::mcp::{RpcError, CONNECTION_CLOSED, METHOD_NOT_FOUND, PROTOCOL_VERSION, REQUEST_TIMEOUT};
use super::oauth::{self, AuthStore, Fetcher, OAuthState, Outcome, Provider};

const SUPPORTED: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];
/// The GET stream's reconnection (DEFAULT_STREAMABLE_HTTP_RECONNECTION_OPTIONS).
const RECONNECT_FIRST: Duration = Duration::from_millis(1000);
const RECONNECT_MAX: Duration = Duration::from_millis(30_000);
const RECONNECT_TRIES: u32 = 2;

/// Who keeps a row's OAuth: McpHost.
pub trait Authority: Send + Sync {
    fn store(&self) -> &dyn AuthStore;
    fn oauth(&self) -> &OAuthState;
    fn port(&self) -> u16;
}

/// The transport's auth provider: the row, in the background.
pub struct AuthCtx {
    pub host: Weak<dyn Authority>,
    pub row: oauth::Row,
}

impl AuthCtx {
    fn bearer(&self) -> Option<String> {
        let host = self.host.upgrade()?;
        let p = Provider { state: host.oauth(), store: host.store(), row: &self.row, port: host.port(), background: true };
        p.access_token()
    }

    /// The SDK's `auth()` from a transport. `init`: the row's headers on
    /// every OAuth request too (`fetchWithInit`), except on upscoping.
    async fn run(&self, url: &Url, rmu: Option<Url>, scope: Option<String>, init: Vec<(String, String)>) -> Result<Outcome, RpcError> {
        let Some(host) = self.host.upgrade() else { return Err(RpcError::local("the broker is stopping")) };
        let p = Provider { state: host.oauth(), store: host.store(), row: &self.row, port: host.port(), background: true };
        let o = oauth::AuthOpts { server_url: url.to_string(), scope, resource_metadata_url: rmu, fetcher: Fetcher { init }, ..Default::default() };
        oauth::auth(&p, &o).await.map_err(|e| err(0, e.message()))
    }
}

fn err(code: i64, message: impl Into<String>) -> RpcError {
    RpcError { code, message: message.into(), unsent: false }
}

fn unauthorized() -> RpcError {
    err(0, "Unauthorized")
}

/// `extractWWWAuthenticateParams`: resource_metadata, scope, error.
pub fn www_authenticate(header: Option<&str>) -> (Option<Url>, Option<String>, Option<String>) {
    let Some(h) = header else { return (None, None, None) };
    let mut parts = h.split(' ');
    let (kind, rest) = (parts.next().unwrap_or_default(), parts.next());
    if !kind.eq_ignore_ascii_case("bearer") || rest.is_none_or(str::is_empty) {
        return (None, None, None);
    }
    let field = |name: &str| -> Option<String> {
        let at = h.find(&format!("{name}="))? + name.len() + 1;
        let rest = &h[at..];
        if let Some(q) = rest.strip_prefix('"') {
            if let Some(end) = q.find('"').filter(|&e| e > 0) {
                return Some(q[..end].to_owned());
            }
        }
        let v: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != ',').collect();
        (!v.is_empty()).then_some(v)
    };
    (field("resource_metadata").and_then(|u| Url::parse(&u).ok()), field("scope"), field("error"))
}

/// One server-sent event.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Event {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
}

/// The event-stream format, fed chunk by chunk.
#[derive(Default)]
pub struct SseParser {
    line: String,
    cur: Event,
    has_data: bool,
}

impl SseParser {
    pub fn push(&mut self, chunk: &str) -> Vec<Event> {
        let mut out = Vec::new();
        for c in chunk.chars() {
            if c != '\n' {
                self.line.push(c);
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let line = line.strip_suffix('\r').unwrap_or(&line);
            if line.is_empty() {
                let ev = std::mem::take(&mut self.cur);
                if self.has_data || ev.id.is_some() {
                    out.push(ev);
                }
                self.has_data = false;
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (k, v) = match line.split_once(':') {
                Some((k, v)) => (k, v.strip_prefix(' ').unwrap_or(v)),
                None => (line, ""),
            };
            match k {
                "event" => self.cur.event = v.to_owned(),
                "data" => {
                    if self.has_data {
                        self.cur.data.push('\n');
                    }
                    self.cur.data.push_str(v);
                    self.has_data = true;
                }
                "id" => self.cur.id = Some(v.to_owned()),
                "retry" => self.cur.retry = v.parse().ok(),
                _ => {}
            }
        }
        out
    }
}

type Pending = oneshot::Sender<Result<Value, RpcError>>;

pub struct Remote {
    sse: bool,
    url: Url,
    /// The row's headers (requestInit).
    headers: Vec<(String, String)>,
    auth: Option<AuthCtx>,
    session: Mutex<Option<String>>,
    protocol: Mutex<Option<String>>,
    rmu: Mutex<Option<Url>>,
    scope: Mutex<Option<String>>,
    completed_auth: AtomicBool,
    last_upscope: Mutex<Option<String>>,
    /// Legacy SSE: where messages go.
    endpoint: Mutex<Option<Url>>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    capabilities: Mutex<Value>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Remote {
    /// Connects and runs the handshake as the SDK's Client.connect does.
    pub async fn connect(url: Url, sse: bool, headers: Vec<(String, String)>, auth: Option<AuthCtx>, client: &str) -> Result<Arc<Remote>, RpcError> {
        let r = Arc::new(Remote {
            sse,
            url,
            headers,
            auth,
            session: Mutex::new(None),
            protocol: Mutex::new(None),
            rmu: Mutex::new(None),
            scope: Mutex::new(None),
            completed_auth: AtomicBool::new(false),
            last_upscope: Mutex::new(None),
            endpoint: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            capabilities: Mutex::new(Value::Null),
            tasks: Mutex::new(Vec::new()),
        });
        if sse {
            if let Err(e) = r.start_sse().await {
                r.close();
                return Err(e);
            }
        }
        let init = r
            .request("initialize", json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": client, "version": "1.0.0"}}), super::mcp::DEFAULT_TIMEOUT)
            .await;
        let init = match init {
            Ok(v) => v,
            Err(e) => {
                r.close();
                return Err(e);
            }
        };
        let version = init.get("protocolVersion").and_then(Value::as_str).unwrap_or_default().to_owned();
        if !SUPPORTED.contains(&version.as_str()) {
            r.close();
            return Err(err(0, format!("Server's protocol version is not supported: {version}")));
        }
        *r.protocol.lock().unwrap_or_else(|e| e.into_inner()) = Some(version);
        *r.capabilities.lock().unwrap_or_else(|e| e.into_inner()) = init.get("capabilities").cloned().unwrap_or(Value::Null);
        if let Err(e) = r.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await {
            r.close();
            return Err(e);
        }
        Ok(r)
    }

    pub fn capabilities(&self) -> Value {
        self.capabilities.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// One request: its result, or the error (the transport's own, as the
    /// SDK rejects with it, or the server's as `MCP error <code>: …`).
    pub async fn request(self: &Arc<Self>, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError { unsent: true, ..err(CONNECTION_CLOSED, "MCP error -32000: Connection closed") });
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        // the SDK leaves out params it wasn't given (tools/list)
        let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if params.as_object().is_none_or(|o| !o.is_empty()) {
            msg["params"] = params;
        }
        let sent = tokio::time::timeout(timeout, async {
            self.send(&msg).await?;
            rx.await.unwrap_or_else(|_| Err(err(CONNECTION_CLOSED, "MCP error -32000: Connection closed")))
        })
        .await;
        match sent {
            Ok(r) => {
                self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                r
            }
            Err(_) => {
                self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                let me = self.clone();
                tokio::spawn(async move {
                    let reason = "McpError: MCP error -32001: Request timed out";
                    let _ = me.send(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": reason}})).await;
                });
                Err(err(REQUEST_TIMEOUT, "MCP error -32001: Request timed out"))
            }
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for t in self.tasks.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            t.abort();
        }
        let pending: Vec<Pending> = self.pending.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, tx)| tx).collect();
        for tx in pending {
            let _ = tx.send(Err(err(CONNECTION_CLOSED, "MCP error -32000: Connection closed")));
        }
    }

    /// `_commonHeaders()`: the bearer, the session, the protocol version,
    /// then the row's own.
    fn common_headers(&self, req: &mut Req) {
        if let Some(t) = self.auth.as_ref().and_then(AuthCtx::bearer) {
            req.set("Authorization", &format!("Bearer {t}"));
        }
        if !self.sse {
            if let Some(s) = self.session.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                req.set("mcp-session-id", &s);
            }
        }
        if let Some(v) = self.protocol.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            req.set("mcp-protocol-version", &v);
        }
        for (k, v) in &self.headers {
            req.set(k, v);
        }
    }

    fn remember_challenge(&self, res: &http::Resp) -> Option<String> {
        let h = res.header("www-authenticate");
        let (rmu, scope, error) = www_authenticate(h.as_deref());
        *self.rmu.lock().unwrap_or_else(|e| e.into_inner()) = rmu;
        *self.scope.lock().unwrap_or_else(|e| e.into_inner()) = scope;
        error
    }

    async fn auth_now(&self, upscope: bool) -> Result<(), RpcError> {
        let Some(a) = &self.auth else { return Err(unauthorized()) };
        let rmu = self.rmu.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let scope = self.scope.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let init = if upscope { Vec::new() } else { self.headers.clone() };
        match a.run(&self.url, rmu, scope, init).await? {
            Outcome::Authorized => Ok(()),
            Outcome::Redirect => Err(unauthorized()),
        }
    }

    /// The transport's `send`.
    fn send<'a>(self: &'a Arc<Self>, msg: &'a Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RpcError>> + Send + 'a>> {
        Box::pin(async move {
            if self.sse {
                self.send_sse(msg).await
            } else {
                self.send_http(msg).await
            }
        })
    }

    async fn send_http(self: &Arc<Self>, msg: &Value) -> Result<(), RpcError> {
        let mut req = Req::post(self.url.clone(), serde_json::to_vec(msg).unwrap_or_default());
        self.common_headers(&mut req);
        req.set("content-type", "application/json");
        req.set("accept", "application/json, text/event-stream");
        let res = http::fetch(req).await.map_err(|e| err(0, e))?;
        if let Some(s) = res.header("mcp-session-id") {
            *self.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
        }
        if !res.ok() {
            let status = res.status;
            let redirect = http::redirect_target(&res);
            let www = res.header("www-authenticate");
            let error = self.remember_challenge(&res);
            let text = res.text().await.unwrap_or_default();
            if status == 401 && self.auth.is_some() {
                if self.completed_auth.load(Ordering::SeqCst) {
                    return Err(err(401, "Streamable HTTP error: Server returned 401 after successful authentication"));
                }
                self.auth_now(false).await?;
                self.completed_auth.store(true, Ordering::SeqCst);
                return self.send(msg).await;
            }
            if status == 403 && self.auth.is_some() && error.as_deref() == Some("insufficient_scope") {
                let mut last = self.last_upscope.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if last.is_some() && last == www {
                    return Err(err(403, "Streamable HTTP error: Server returned 403 after trying upscoping"));
                }
                last = www;
                *self.last_upscope.lock().unwrap_or_else(|e| e.into_inner()) = last;
                self.auth_now(true).await?;
                return self.send(msg).await;
            }
            let why = redirect.map(|t| format!("Redirect to {t} not followed (redirectPolicy: 'same-origin')")).unwrap_or(text);
            return Err(err(status as i64, format!("Streamable HTTP error: Error POSTing to endpoint: {why}")));
        }
        self.completed_auth.store(false, Ordering::SeqCst);
        *self.last_upscope.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if res.status == 202 {
            res.cancel();
            if msg.get("method").and_then(Value::as_str) == Some("notifications/initialized") {
                let me = self.clone();
                self.spawn(async move { me.get_stream(0).await });
            }
            return Ok(());
        }
        if msg.get("id").is_none() {
            res.cancel();
            return Ok(());
        }
        let ct = res.header("content-type").unwrap_or_default();
        let essence = ct.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        match essence.as_str() {
            "text/event-stream" => {
                let me = self.clone();
                self.spawn(async move {
                    me.read_stream(res).await;
                });
                Ok(())
            }
            "application/json" => {
                let v: Value = serde_json::from_str(&res.text().await.unwrap_or_default()).map_err(|e| err(0, e.to_string()))?;
                for m in v.as_array().cloned().unwrap_or_else(|| vec![v]) {
                    self.dispatch(m);
                }
                Ok(())
            }
            _ => Err(err(-1, format!("Streamable HTTP error: Unexpected content type: {ct}"))),
        }
    }

    fn spawn(&self, f: impl std::future::Future<Output = ()> + Send + 'static) {
        let t = tokio::spawn(f);
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.retain(|t| !t.is_finished());
        tasks.push(t);
    }

    /// The optional GET stream (`_startOrAuthSse`), reconnected as the SDK
    /// does when it ends.
    fn get_stream(self: Arc<Self>, attempt: u32) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            if self.is_closed() {
                return;
            }
            let mut req = Req::get(self.url.clone());
            self.common_headers(&mut req);
            req.set("Accept", "text/event-stream");
            let res = match http::fetch(req).await {
                Ok(r) => r,
                Err(_) => return self.reconnect(attempt + 1, None).await,
            };
            if !res.ok() {
                let status = res.status;
                res.cancel();
                if status == 401 && self.auth.is_some() {
                    if self.auth_now(false).await.is_ok() {
                        return self.get_stream(attempt).await;
                    }
                    return;
                }
                return;
            }
            let retry = self.read_stream(res).await;
            self.reconnect(0, retry).await
        })
    }

    async fn reconnect(self: Arc<Self>, attempt: u32, retry: Option<u64>) {
        if self.is_closed() || attempt >= RECONNECT_TRIES {
            return;
        }
        let delay = retry.map(Duration::from_millis).unwrap_or_else(|| RECONNECT_FIRST.mul_f64(1.5f64.powi(attempt as i32)).min(RECONNECT_MAX));
        tokio::time::sleep(delay).await;
        self.get_stream(attempt).await
    }

    /// Reads an event stream to its end, handing over its messages; the
    /// server's last `retry`.
    async fn read_stream(self: &Arc<Self>, mut res: http::Resp) -> Option<u64> {
        let mut parser = SseParser::default();
        let mut retry = None;
        let mut rest = Vec::new();
        while let Ok(Some(chunk)) = res.chunk().await {
            rest.extend_from_slice(&chunk);
            let upto = match std::str::from_utf8(&rest) {
                Ok(_) => rest.len(),
                Err(e) => e.valid_up_to(),
            };
            let text = String::from_utf8_lossy(&rest[..upto]).into_owned();
            rest.drain(..upto);
            for ev in parser.push(&text) {
                if ev.retry.is_some() {
                    retry = ev.retry;
                }
                self.on_event(ev);
            }
        }
        retry
    }

    fn on_event(self: &Arc<Self>, ev: Event) {
        match ev.event.as_str() {
            "endpoint" if self.sse => {
                let at = self.url.join(&ev.data).ok();
                *self.endpoint.lock().unwrap_or_else(|e| e.into_inner()) = at;
                if let Some(tx) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&u64::MAX) {
                    let _ = tx.send(Ok(Value::Null));
                }
            }
            "" | "message" if !ev.data.is_empty() => {
                if let Ok(v) = serde_json::from_str::<Value>(&ev.data) {
                    self.dispatch(v);
                }
            }
            _ => {}
        }
    }

    /// A message from the server: an answer resolves its request; a
    /// request gets the client's answer (ping, else method not found).
    fn dispatch(self: &Arc<Self>, msg: Value) {
        let id = msg.get("id").cloned();
        match (msg.get("method").and_then(Value::as_str), id) {
            (Some(method), Some(id)) => {
                let reply = if method == "ping" {
                    json!({"jsonrpc": "2.0", "id": id, "result": {}})
                } else {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": METHOD_NOT_FOUND, "message": "Method not found"}})
                };
                let me = self.clone();
                self.spawn(async move {
                    let _ = me.send(&reply).await;
                });
            }
            (None, Some(id)) => {
                let Some(n) = id.as_u64() else { return };
                let Some(tx) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&n) else { return };
                let answer = match (msg.get("result"), msg.get("error")) {
                    (Some(r), None) => Ok(r.clone()),
                    (_, Some(e)) => {
                        let code = e.get("code").and_then(Value::as_i64).unwrap_or(-32603);
                        Err(err(code, format!("MCP error {code}: {}", e.get("message").and_then(Value::as_str).unwrap_or(""))))
                    }
                    _ => Err(err(-32603, "MCP error -32603: Invalid response")),
                };
                let _ = tx.send(answer);
            }
            _ => {}
        }
    }

    /// Legacy SSE: the stream, until its `endpoint` event.
    fn start_sse(self: &Arc<Self>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RpcError>> + Send + '_>> {
        Box::pin(async move {
            let mut req = Req::get(self.url.clone());
            self.common_headers(&mut req);
            req.set("Accept", "text/event-stream");
            let res = http::fetch(req).await.map_err(|e| err(0, format!("SSE error: TypeError: {e}")))?;
            if res.status == 401 && res.header("www-authenticate").is_some() {
                self.remember_challenge(&res);
            }
            if res.status != 200 {
                let status = res.status;
                res.cancel();
                if status == 401 && self.auth.is_some() {
                    self.auth_now(false).await?;
                    return self.start_sse().await;
                }
                return Err(err(status as i64, format!("SSE error: Non-200 status code ({status})")));
            }
            let ct = res.header("content-type").unwrap_or_default();
            if !ct.to_ascii_lowercase().starts_with("text/event-stream") {
                return Err(err(0, format!("SSE error: Invalid content type, expected \"text/event-stream\" (got \"{ct}\")")));
            }
            // the endpoint event resolves this slot
            let (tx, rx) = oneshot::channel();
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(u64::MAX, tx);
            let me = self.clone();
            self.spawn(async move {
                me.read_stream(res).await;
                // the stream is gone: so is this connection (McpHost starts
                // a new one on the next use)
                me.close();
            });
            rx.await.map_err(|_| err(CONNECTION_CLOSED, "SSE error: the stream ended before its endpoint"))??;
            let ep = self.endpoint.lock().unwrap_or_else(|e| e.into_inner()).clone();
            match ep {
                Some(e) if e.origin() == self.url.origin() => Ok(()),
                Some(e) => Err(err(0, format!("Endpoint origin does not match connection origin: {}", e.origin().ascii_serialization()))),
                None => Err(err(0, "SSE error: no endpoint")),
            }
        })
    }

    async fn send_sse(self: &Arc<Self>, msg: &Value) -> Result<(), RpcError> {
        let Some(endpoint) = self.endpoint.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
            return Err(err(0, "Not connected"));
        };
        let mut req = Req::post(endpoint.clone(), serde_json::to_vec(msg).unwrap_or_default());
        self.common_headers(&mut req);
        req.set("content-type", "application/json");
        let res = http::fetch(req).await.map_err(|e| err(0, e))?;
        if !res.ok() {
            let status = res.status;
            let redirect = http::redirect_target(&res);
            self.remember_challenge(&res);
            let text = res.text().await.unwrap_or_default();
            if status == 401 && self.auth.is_some() {
                self.auth_now(false).await?;
                return self.send(msg).await;
            }
            let why = redirect.map(|t| format!("Redirect to {t} not followed (redirectPolicy: 'same-origin')")).unwrap_or(text);
            return Err(err(status as i64, format!("Error POSTing to endpoint (HTTP {status}): {why}")));
        }
        res.cancel();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn www_authenticate_reads_bearer_parameters_quoted_or_not() {
        let (u, s, e) = www_authenticate(Some(r#"Bearer resource_metadata="https://a.test/.well-known/oauth-protected-resource/mcp", scope="a b", error=insufficient_scope"#));
        assert_eq!(u.unwrap().as_str(), "https://a.test/.well-known/oauth-protected-resource/mcp");
        assert_eq!(s.as_deref(), Some("a b"));
        assert_eq!(e.as_deref(), Some("insufficient_scope"));
        assert_eq!(www_authenticate(Some("Basic realm=x")), (None, None, None));
        assert_eq!(www_authenticate(Some("Bearer")), (None, None, None));
        assert_eq!(www_authenticate(None), (None, None, None));
    }

    #[test]
    fn the_sse_parser_takes_events_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.push("event: endpoint\ndata: /messages?s").is_empty());
        let got = p.push("=1\n\n: comment\nid: 7\nretry: 500\ndata: {\"a\":1}\r\ndata: x\n\n");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], Event { event: "endpoint".into(), data: "/messages?s=1".into(), id: None, retry: None });
        assert_eq!(got[1], Event { event: String::new(), data: "{\"a\":1}\nx".into(), id: Some("7".into()), retry: Some(500) });
    }
}
