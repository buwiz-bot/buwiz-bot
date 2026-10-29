//! The user's v0 MCP servers (`mcp_servers`; `mcp.mjs`), hosted here (plan
//! 4.1, D12): one client per server, made on first use and shared, with the
//! server's tools, capabilities and resources. Node keeps the table's
//! config columns, the `/mcp/*` routes and what the model sees, and reaches
//! these clients over the line; `mcp.mjs` folds their answers as before.
//!
//! A stdio server starts the way mcp.mjs starts it: this process's whole
//! environment, `PATH` with the folders npx/uvx/bunx live in, then the
//! row's `env`. A server whose process died is started again on its next
//! use (Node kept a dead client until the row changed). Remote (`url`)
//! servers stay Node's until 4.2 brings OAuth.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use super::mcp::{self, Conn, Hooks, RpcError, Spec};

struct Live {
    conn: Arc<Conn>,
    tools: Vec<Value>,
}

pub struct McpHost {
    db: Mutex<Connection>,
    live: Mutex<HashMap<String, Live>>,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

/// An `mcp_servers` row, as far as a client needs it.
struct Row {
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    url: String,
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

impl McpHost {
    pub fn new(db: &Path) -> Result<Arc<McpHost>, String> {
        let conn = Connection::open(db).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
        Ok(Arc::new(McpHost { db: Mutex::new(conn), live: Mutex::new(HashMap::new()), locks: Mutex::new(HashMap::new()) }))
    }

    fn row(&self, id: &str) -> Result<Row, String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let got: Option<(String, String, String, Option<String>)> = db
            .query_row("SELECT command, args, env, url FROM mcp_servers WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()
            .map_err(|e| format!("app.db: {e}"))?;
        let Some((command, args, env, url)) = got else { return Err(format!("no mcp server {id}")) };
        let args: Vec<String> = serde_json::from_str::<Value>(&args).ok().and_then(|a| a.as_array().cloned()).unwrap_or_default().iter().map(crate::js::string_of).collect();
        let env: Vec<(String, String)> = serde_json::from_str::<Value>(&env)
            .ok()
            .and_then(|e| e.as_object().cloned())
            .map(|o| crate::js::ordered(&o).into_iter().map(|(k, v)| (k.clone(), crate::js::string_of(v))).collect())
            .unwrap_or_default();
        Ok(Row { command, args, env, url: url.unwrap_or_default() })
    }

    fn lock_of(&self, id: &str) -> Arc<AsyncMutex<()>> {
        self.locks.lock().unwrap_or_else(|e| e.into_inner()).entry(id.to_owned()).or_default().clone()
    }

    /// The live client of `id`, connecting (or reconnecting a dead one).
    async fn client(&self, id: &str) -> Result<(Arc<Conn>, Vec<Value>), String> {
        let lock = self.lock_of(id);
        let _held = lock.lock().await;
        if let Some(l) = self.live.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            if !l.conn.is_closed() {
                return Ok((l.conn.clone(), l.tools.clone()));
            }
        }
        self.live.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        let row = self.row(id)?;
        if !row.url.is_empty() {
            return Err(format!("{id} is a remote server, still Node's (plan 4.2)"));
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let mut env = vec![("PATH".to_owned(), tool_path(&std::env::var("PATH").unwrap_or_default(), &home))];
        env.extend(row.env);
        let spec = Spec { command: row.command, args: row.args, env, inherit_all: true, client: "grok-shell" };
        let conn = Conn::connect(&spec, Arc::new(LogHooks { id: id.to_owned() })).await.map_err(|e| crate::js::slice_utf16(&e.message, 300))?;
        let tools = match conn.request("tools/list", json!({}), mcp::DEFAULT_TIMEOUT).await {
            Ok(r) => r.get("tools").and_then(Value::as_array).cloned().unwrap_or_default(),
            Err(e) => {
                conn.close().await;
                return Err(crate::js::slice_utf16(&e.message, 300));
            }
        };
        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_owned(), Live { conn: conn.clone(), tools: tools.clone() });
        Ok((conn, tools))
    }

    /// mcp.mjs's `ensureClient`: `{ok, tools, capabilities}` or `{ok: false, err}`.
    pub async fn ensure(&self, id: &str) -> Value {
        match self.client(id).await {
            Ok((conn, tools)) => json!({"ok": true, "tools": tools, "capabilities": conn.capabilities()}),
            Err(err) => json!({"ok": false, "err": err}),
        }
    }

    /// One request to `id`'s server (tools/call, resources/list,
    /// resources/read): the raw result, or the error as the SDK words it.
    pub async fn request(&self, id: &str, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        let (conn, _) = self.client(id).await.map_err(RpcError::local)?;
        match conn.request(method, params.clone(), timeout).await {
            // its process died before this was sent: once more, on a new one
            Err(e) if e.unsent => {
                self.drop_dead(id, &conn);
                let (conn, _) = self.client(id).await.map_err(RpcError::local)?;
                conn.request(method, params, timeout).await
            }
            other => other,
        }
    }

    /// Forgets `conn` as `id`'s client if it still is.
    fn drop_dead(&self, id: &str, conn: &Arc<Conn>) {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if live.get(id).is_some_and(|l| Arc::ptr_eq(&l.conn, conn)) {
            live.remove(id);
        }
    }

    /// Drops `id`'s client (its row changed or was deleted); the next use
    /// connects again.
    pub async fn invalidate(&self, id: &str) {
        let gone = self.live.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        if let Some(l) = gone {
            l.conn.stopping.store(true, std::sync::atomic::Ordering::SeqCst);
            l.conn.abandon();
            l.conn.close().await;
        }
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.live.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        for id in ids {
            self.invalidate(&id).await;
        }
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
