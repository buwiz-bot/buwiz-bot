//! What v0 routines read of the MCP servers (mcp.mjs through the line,
//! now `helper::mcp_servers::McpHost` in process): the enabled rows,
//! `ensureClient`'s tools, `callToolJson`, `readManifest` and `statusOf`.
//! Tests hand a fake.

use std::sync::Arc;
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use crate::helper::mcp_servers::McpHost;
use crate::hub::types::BoxFuture;
use crate::js;

/// An `mcp_servers` row, as far as routines read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// the `env` column (JSON)
    pub env: String,
    pub account: Option<String>,
}

/// `ensureClient(row)`: its tools when it connected, else why not.
pub type Client = Result<Vec<Value>, String>;

pub trait McpView: Send + Sync {
    /// `SELECT * FROM mcp_servers WHERE enabled=1 ORDER BY created_at`.
    fn servers(&self) -> Vec<Server>;
    /// `SELECT * FROM mcp_servers WHERE id=? AND enabled=1`.
    fn server(&self, id: &str) -> Option<Server> {
        self.servers().into_iter().find(|s| s.id == id)
    }
    /// `ensureClient(row)`.
    fn ensure(&self, id: &str) -> BoxFuture<'static, Client>;
    /// One tools/call: the raw result, or the error as the client words it.
    fn call(&self, id: &str, tool: &str, args: Value) -> BoxFuture<'static, Result<Value, String>>;
    /// `readManifest(row)`: `{}` when it has none.
    fn manifest(&self, id: &str) -> BoxFuture<'static, Value>;
    /// `statusOf(id)`: `{status, tools, err?}`.
    fn status(&self, id: &str) -> Value;
}

/// The hub's MCP host, its rows read from app.db.
pub struct HostMcp(pub Arc<McpHost>);

impl McpView for HostMcp {
    fn servers(&self) -> Vec<Server> {
        self.0
            .db(|db| {
                let mut st = db.prepare("SELECT * FROM mcp_servers WHERE enabled=1 ORDER BY created_at")?;
                let rows = st.query_map([], row)?;
                rows.collect()
            })
            .unwrap_or_default()
    }
    fn server(&self, id: &str) -> Option<Server> {
        self.0.db(|db| db.query_row("SELECT * FROM mcp_servers WHERE id=? AND enabled=1", [id], row).optional()).ok().flatten()
    }
    fn ensure(&self, id: &str) -> BoxFuture<'static, Client> {
        let (host, id) = (self.0.clone(), id.to_owned());
        Box::pin(async move {
            let r = host.ensure(&id).await;
            if r.get("ok") == Some(&Value::Bool(true)) {
                Ok(r.get("tools").and_then(Value::as_array).cloned().unwrap_or_default())
            } else {
                Err(r.get("err").map(js::string_of).unwrap_or_default())
            }
        })
    }
    fn call(&self, id: &str, tool: &str, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        let (host, id, tool) = (self.0.clone(), id.to_owned(), tool.to_owned());
        Box::pin(async move { host.request(&id, "tools/call", json!({"name": tool, "arguments": args}), Duration::from_secs(120)).await.map_err(|e| e.message) })
    }
    fn manifest(&self, id: &str) -> BoxFuture<'static, Value> {
        let (host, id) = (self.0.clone(), id.to_owned());
        Box::pin(async move { host.manifest(&id).await })
    }
    fn status(&self, id: &str) -> Value {
        self.0.status(id)
    }
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<Server> {
    Ok(Server {
        id: r.get("id")?,
        name: r.get::<_, Option<String>>("name")?.unwrap_or_default(),
        env: r.get::<_, Option<String>>("env")?.unwrap_or_default(),
        account: r.get::<_, Option<String>>("account").ok().flatten(),
    })
}

/// No MCP host: no server is connected.
pub struct NoMcp;

impl McpView for NoMcp {
    fn servers(&self) -> Vec<Server> {
        Vec::new()
    }
    fn ensure(&self, _: &str) -> BoxFuture<'static, Client> {
        Box::pin(async { Err("no MCP host".to_owned()) })
    }
    fn call(&self, _: &str, _: &str, _: Value) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async { Err("no MCP host".to_owned()) })
    }
    fn manifest(&self, _: &str) -> BoxFuture<'static, Value> {
        Box::pin(async { json!({}) })
    }
    fn status(&self, _: &str) -> Value {
        json!({"status": "idle", "tools": 0})
    }
}

/// `callToolJson(row, tool, input)`: the result as data, structuredContent,
/// else the text parsed as JSON, else the text; `{ok: false, error}` for a
/// tool error or a server that isn't connected. A failed request throws
/// (`Err`).
pub async fn call_tool_json(mcp: &dyn McpView, id: &str, tool: &str, input: Value) -> Result<Value, String> {
    if let Err(e) = mcp.ensure(id).await {
        let why = if e.is_empty() { "failed to start".to_owned() } else { e };
        return Ok(json!({"ok": false, "error": format!("{id} is not connected ({why})")}));
    }
    let result = mcp.call(id, tool, input).await?;
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .map(|c| c.get("text").map(js::string_of).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    if result.get("isError").is_some_and(js::truthy) {
        return Ok(json!({"ok": false, "error": if text.is_empty() { "tool failed".to_owned() } else { text }}));
    }
    if let Some(sc) = result.get("structuredContent") {
        if let Some(o) = sc.as_object().filter(|o| o.len() == 1 && o.contains_key("result")) {
            return Ok(json!({"ok": true, "result": o["result"]}));
        }
        return Ok(json!({"ok": true, "result": sc}));
    }
    Ok(match serde_json::from_str::<Value>(&text) {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(_) => json!({"ok": true, "result": text}),
    })
}
