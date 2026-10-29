//! The user's v0 MCP servers (`mcp_servers`; `mcp.mjs`), hosted here (plan
//! 4.1, 4.2, D12): one client per server, made on first use and shared, with
//! the server's tools, capabilities and resources. Node keeps the table's
//! config columns, the `/mcp/*` routes and what the model sees, and reaches
//! these clients over the line; `mcp.mjs` folds their answers as before.
//!
//! A stdio server starts the way mcp.mjs starts it: this process's whole
//! environment, `PATH` with the folders npx/uvx/bunx live in, then the
//! row's `env`. A remote (`url`) server is streamable HTTP, or legacy SSE
//! when its `transport` says so, with the row's `headers`; once it has
//! OAuth tokens (oauth.rs) they sign every request, and its `Authorization`
//! header goes. A server whose process or stream died is started again on
//! its next use (Node kept a dead client until the row changed).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

use super::mcp::{self, Conn, Hooks, RpcError, Spec};
use super::oauth::{self, AuthStore, OAuthState};
use super::remote::{AuthCtx, Authority, Remote};

#[derive(Clone)]
enum Client {
    Local(Arc<Conn>),
    Remote(Arc<Remote>),
}

impl Client {
    fn is_closed(&self) -> bool {
        match self {
            Client::Local(c) => c.is_closed(),
            Client::Remote(r) => r.is_closed(),
        }
    }

    fn capabilities(&self) -> Value {
        match self {
            Client::Local(c) => c.capabilities(),
            Client::Remote(r) => r.capabilities(),
        }
    }

    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        match self {
            Client::Local(c) => c.request(method, params, timeout).await,
            Client::Remote(r) => r.request(method, params, timeout).await,
        }
    }

    async fn close(&self) {
        match self {
            Client::Local(c) => {
                c.stopping.store(true, std::sync::atomic::Ordering::SeqCst);
                c.abandon();
                c.close().await;
            }
            Client::Remote(r) => r.close(),
        }
    }

    fn same(&self, other: &Client) -> bool {
        match (self, other) {
            (Client::Local(a), Client::Local(b)) => Arc::ptr_eq(a, b),
            (Client::Remote(a), Client::Remote(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

struct Live {
    client: Client,
    tools: Vec<Value>,
    /// A remote server that answers without a token but publishes OAuth
    /// metadata (Node's `rec.needsAuth`).
    needs_auth: Option<String>,
}

pub struct McpHost {
    db: Mutex<Connection>,
    live: Mutex<HashMap<String, Live>>,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    oauth: OAuthState,
    /// The front door's port: the OAuth redirect's.
    port: u16,
    me: Weak<McpHost>,
}

/// An `mcp_servers` row, as far as a client needs it.
struct Row {
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    url: String,
    headers: Vec<(String, String)>,
    oauth_client: Option<String>,
    transport: String,
    auth: Value,
}

impl Row {
    fn oauth(&self, id: &str) -> oauth::Row {
        oauth::Row { id: id.to_owned(), url: self.url.clone(), fixed: oauth::Row::fixed_of(self.oauth_client.as_deref()), auth: self.auth.clone() }
    }

    fn has_tokens(&self) -> bool {
        self.auth.get("tokens").is_some_and(crate::js::truthy)
    }
}

/// mcp.mjs's `toolPath()`: an app started from Finder gets launchd's bare
/// PATH, without the folders npx, uvx and bunx usually live in.
pub fn tool_path(path: &str, home: &str) -> String {
    let mut have: Vec<String> = path.split(':').filter(|p| !p.is_empty()).map(str::to_owned).collect();
    for extra in [format!("{home}/.local/bin"), format!("{home}/.cargo/bin"), format!("{home}/.bun/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into()] {
        if !have.contains(&extra) {
            have.push(extra);
        }
    }
    have.join(":")
}

/// An object column (`env`, `headers`) as ordered string pairs.
fn pairs(text: &str) -> Vec<(String, String)> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|e| e.as_object().cloned())
        .map(|o| crate::js::ordered(&o).into_iter().map(|(k, v)| (k.clone(), crate::js::string_of(v))).collect())
        .unwrap_or_default()
}

/// mcp.mjs's `googleApiOff`: Google's MCP hosts answer a signed-in request
/// with a bare 403 when the service is off in the client's project.
fn google_api_off(row: &Row, e: &RpcError) -> Option<String> {
    if e.code != 403 || !row.has_tokens() {
        return None;
    }
    let host = Url::parse(&row.url).ok()?.host_str()?.to_owned();
    if !host.ends_with(".googleapis.com") {
        return None;
    }
    let fixed = oauth::Row::fixed_of(row.oauth_client.as_deref());
    let project = fixed
        .as_ref()
        .and_then(|f| f.get("client_id"))
        .and_then(Value::as_str)
        .and_then(|c| c.split_once('-').map(|(d, _)| d.to_owned()))
        .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    let link = format!("https://console.developers.google.com/apis/api/{host}/overview{}", project.as_ref().map(|p| format!("?project={p}")).unwrap_or_default());
    Some(format!(
        "Google refused the request: the {host} service is probably turned off in your Google Cloud project{}. Enable it at {link}, wait a minute, then Reconnect.",
        project.map(|p| format!(" {p}")).unwrap_or_default()
    ))
}

/// Logs a server's stderr as mcp.mjs does: `  mcp <id>: <chunk>`, Node's
/// own warnings left out.
struct LogHooks {
    id: String,
}

impl Hooks for LogHooks {
    fn stderr(&self, chunk: &str) {
        let line = chunk.trim();
        if !line.is_empty() && !line.starts_with("(node:") && !line.contains("ExperimentalWarning") {
            eprintln!("  mcp {}: {}", self.id, crate::js::slice_utf16(line, 200));
        }
    }
    fn list_changed(&self) {}
    fn closed(&self) {}
}

impl AuthStore for McpHost {
    fn load_auth(&self, id: &str) -> Value {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let text: Option<Option<String>> = db.query_row("SELECT auth FROM mcp_servers WHERE id=?", [id], |r| r.get(0)).optional().ok().flatten();
        text.flatten().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
    }

    fn save_auth(&self, id: &str, auth: &Value) {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = db.execute("UPDATE mcp_servers SET auth=? WHERE id=?", rusqlite::params![crate::js::stringify(auth), id]) {
            eprintln!("  mcp {id}: saving its sign-in failed: {e}");
        }
    }
}

impl Authority for McpHost {
    fn store(&self) -> &dyn AuthStore {
        self
    }
    fn oauth(&self) -> &OAuthState {
        &self.oauth
    }
    fn port(&self) -> u16 {
        self.port
    }
}

impl McpHost {
    /// `port`: the front door's, which OAuth redirects name.
    pub fn new(db: &Path, port: u16) -> Result<Arc<McpHost>, String> {
        let conn = Connection::open(db).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
        Ok(Arc::new_cyclic(|me| McpHost {
            db: Mutex::new(conn),
            live: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            oauth: OAuthState::default(),
            port,
            me: me.clone(),
        }))
    }

    fn row(&self, id: &str) -> Result<Row, String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        type Cols = (String, String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
        let got: Option<Cols> = db
            .query_row("SELECT command, args, env, url, headers, oauth_client, transport, auth FROM mcp_servers WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
            })
            .optional()
            .map_err(|e| format!("app.db: {e}"))?;
        let Some((command, args, env, url, headers, oauth_client, transport, auth)) = got else { return Err(format!("no mcp server {id}")) };
        let args: Vec<String> = serde_json::from_str::<Value>(&args).ok().and_then(|a| a.as_array().cloned()).unwrap_or_default().iter().map(crate::js::string_of).collect();
        let auth = auth.and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
        Ok(Row {
            command,
            args,
            env: pairs(&env),
            url: url.unwrap_or_default(),
            headers: pairs(headers.as_deref().unwrap_or("{}")),
            oauth_client,
            transport: transport.unwrap_or_default(),
            auth,
        })
    }

    fn lock_of(&self, id: &str) -> Arc<AsyncMutex<()>> {
        self.locks.lock().unwrap_or_else(|e| e.into_inner()).entry(id.to_owned()).or_default().clone()
    }

    /// The live client of `id`, connecting (or reconnecting a dead one).
    async fn client(&self, id: &str) -> Result<(Client, Vec<Value>, Option<String>), RpcError> {
        let lock = self.lock_of(id);
        let _held = lock.lock().await;
        if let Some(l) = self.live.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            if !l.client.is_closed() {
                return Ok((l.client.clone(), l.tools.clone(), l.needs_auth.clone()));
            }
        }
        self.live.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        let row = self.row(id).map_err(RpcError::local)?;
        let (client, needs_auth) = if row.url.is_empty() {
            let home = std::env::var("HOME").unwrap_or_default();
            let mut env = vec![("PATH".to_owned(), tool_path(&std::env::var("PATH").unwrap_or_default(), &home))];
            env.extend(row.env.clone());
            let spec = Spec { command: row.command.clone(), args: row.args.clone(), env, inherit_all: true, client: "grok-shell" };
            let conn = Conn::connect(&spec, Arc::new(LogHooks { id: id.to_owned() })).await.map_err(|e| RpcError { message: crate::js::slice_utf16(&e.message, 300), ..e })?;
            (Client::Local(conn), false)
        } else {
            self.connect_remote(id, &row).await?
        };
        let tools = match client.request("tools/list", json!({}), mcp::DEFAULT_TIMEOUT).await {
            Ok(r) => r.get("tools").and_then(Value::as_array).cloned().unwrap_or_default(),
            Err(e) => {
                client.close().await;
                return Err(self.worded(&row, e));
            }
        };
        let needs_auth = match needs_auth {
            true => self.oauth.wants_oauth(&row.url).await.then(|| "Sign-in required".to_owned()),
            false => None,
        };
        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_owned(), Live { client: client.clone(), tools: tools.clone(), needs_auth: needs_auth.clone() });
        Ok((client, tools, needs_auth))
    }

    /// A remote server's client, and whether its sign-in must be checked
    /// (no tokens and no credential header).
    async fn connect_remote(&self, id: &str, row: &Row) -> Result<(Client, bool), RpcError> {
        let url = Url::parse(&row.url).map_err(|e| RpcError::local(format!("Invalid URL: {e}")))?;
        let has_tokens = row.has_tokens();
        let mut headers = row.headers.clone();
        if has_tokens {
            headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
        }
        let me: Weak<dyn Authority> = self.me.clone();
        let auth = has_tokens.then(|| AuthCtx { host: me, row: row.oauth(id) });
        let remote = Remote::connect(url, row.transport == "sse", headers.clone(), auth, "grok-shell").await.map_err(|e| self.worded(row, e))?;
        let cred = headers.iter().any(|(k, v)| {
            let k = k.to_ascii_lowercase();
            !v.is_empty() && ["auth", "key", "token", "secret"].iter().any(|w| k.contains(w))
        });
        Ok((Client::Remote(remote), !has_tokens && !cred))
    }

    fn worded(&self, row: &Row, e: RpcError) -> RpcError {
        let message = google_api_off(row, &e).unwrap_or_else(|| crate::js::slice_utf16(&e.message, 300));
        RpcError { message, ..e }
    }

    /// mcp.mjs's `ensureClient`: `{ok, tools, capabilities, needsAuth?}` or
    /// `{ok: false, err, code}`.
    pub async fn ensure(&self, id: &str) -> Value {
        match self.client(id).await {
            Ok((client, tools, needs_auth)) => {
                let mut r = json!({"ok": true, "tools": tools, "capabilities": client.capabilities()});
                if let Some(n) = needs_auth {
                    r["needsAuth"] = json!(n);
                }
                r
            }
            Err(e) => json!({"ok": false, "err": e.message, "code": e.code}),
        }
    }

    /// One request to `id`'s server (tools/call, resources/list,
    /// resources/read): the raw result, or the error as the SDK words it.
    pub async fn request(&self, id: &str, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        let (client, _, _) = self.client(id).await?;
        match client.request(method, params.clone(), timeout).await {
            // its process or stream died before this was sent: once more,
            // on a new one
            Err(e) if e.unsent => {
                self.drop_dead(id, &client);
                let (client, _, _) = self.client(id).await?;
                client.request(method, params, timeout).await
            }
            other => other,
        }
    }

    /// Forgets `client` as `id`'s if it still is.
    fn drop_dead(&self, id: &str, client: &Client) {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if live.get(id).is_some_and(|l| l.client.same(client)) {
            live.remove(id);
        }
    }

    /// Drops `id`'s client (its row changed or was deleted, or it signed
    /// in); the next use connects again.
    pub async fn invalidate(&self, id: &str) {
        let gone = self.live.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        if let Some(l) = gone {
            l.client.close().await;
        }
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.live.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        for id in ids {
            self.invalidate(&id).await;
        }
    }

    /// `POST /mcp/auth`'s flow: `{ok, authorized}` (stored tokens do),
    /// `{ok, redirected, authorizeUrl?}`, or `{ok: false, error}`.
    pub async fn auth_begin(&self, id: &str, open: bool) -> Value {
        let row = match self.row(id) {
            Ok(r) => r,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        match self.oauth.begin(self, self.port, &row.oauth(id), open).await {
            Ok(r) => {
                if r.get("authorized") == Some(&json!(true)) {
                    self.invalidate(id).await;
                }
                r
            }
            Err(e) => json!({"ok": false, "error": e}),
        }
    }

    /// The callback: `{result: "connected" | "refused" | "failed", error?}`.
    pub async fn auth_finish(&self, id: &str, code: Option<&str>, state: Option<&str>, error: Option<&str>) -> Value {
        let (result, why) = self.oauth.finish(self, self.port, id, code, state, error).await;
        if result == "connected" {
            self.invalidate(id).await;
        }
        json!({"ok": true, "result": result, "error": why})
    }
}

#[cfg(test)]
mod tests {
    use super::tool_path;

    #[test]
    fn tool_path_adds_the_usual_tool_folders_once() {
        assert_eq!(tool_path("/usr/bin:/bin", "/h"), "/usr/bin:/bin:/h/.local/bin:/h/.cargo/bin:/h/.bun/bin:/opt/homebrew/bin:/usr/local/bin");
        assert_eq!(tool_path("/opt/homebrew/bin::/usr/bin", "/h"), "/opt/homebrew/bin:/usr/bin:/h/.local/bin:/h/.cargo/bin:/h/.bun/bin:/usr/local/bin");
    }
}
