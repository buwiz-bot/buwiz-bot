//! The BetterOffice MCP child (plan D31, until 6.1): office.mjs's
//! `ensureOfficeMcp`, `officeMcpTools` and `callOfficeTool`, over the
//! stdio client of `mcp.rs`. Node runs `server/src/office/mcp-main.js
//! --root <files>` (the bundled build) or `--experimental-transform-types
//! mcp-main.ts` (a checkout); started once, in the background at the hub's
//! start; a child that failed to start leaves the office tools out for
//! good, as Node's cached failure does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::OnceCell;

use super::mcp::{self, Conn, Hooks, Spec};

/// The child's stderr, as office.mjs logs it.
struct LogHooks;

impl Hooks for LogHooks {
    fn stderr(&self, chunk: &str) {
        let line = chunk.trim();
        if !line.is_empty() && !line.starts_with("(node:") && !line.contains("ExperimentalWarning") {
            eprintln!("  office-mcp: {line}");
        }
    }
    fn list_changed(&self) {}
    fn closed(&self) {}
}

pub struct Office {
    spec: Spec,
    started: OnceCell<Result<(Arc<Conn>, Vec<Value>), String>>,
}

impl Office {
    /// The child Node would start: `node` on `<src>/office/mcp-main.*`
    /// (`src` is Node's SERVER_DIR) with the files dir (`generated/files`)
    /// as its root, in the SDK's default environment.
    pub fn new(node: &Path, src: &Path, root: &Path) -> Self {
        let dir = src.join("office");
        let bundled = dir.join("mcp-main.js");
        let root = root.to_string_lossy().into_owned();
        let args = if bundled.exists() {
            vec![bundled.to_string_lossy().into_owned(), "--root".into(), root]
        } else {
            vec!["--experimental-transform-types".into(), dir.join("mcp-main.ts").to_string_lossy().into_owned(), "--root".into(), root]
        };
        Self { spec: Spec { command: node.to_string_lossy().into_owned(), args, env: Vec::new(), inherit_all: false, client: "grok-shell" }, started: OnceCell::new() }
    }

    /// The command line it runs (for tests and logs).
    pub fn command(&self) -> (&str, &[String]) {
        (&self.spec.command, &self.spec.args)
    }

    /// `ensureOfficeMcp(root)`: the connection and its tools, started once.
    pub async fn ensure(&self) -> Result<(Arc<Conn>, Vec<Value>), String> {
        self.started
            .get_or_init(|| async {
                let made: Result<(Arc<Conn>, Vec<Value>), String> = async {
                    let conn = Conn::connect(&self.spec, Arc::new(LogHooks)).await.map_err(|e| e.message)?;
                    let listed = conn.request("tools/list", json!({}), mcp::DEFAULT_TIMEOUT).await.map_err(|e| e.message)?;
                    let tools = listed
                        .get("tools")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|t| {
                            json!({
                                "name": t.get("name").cloned().unwrap_or(Value::Null),
                                "description": t.get("description").filter(|d| crate::js::truthy(d)).cloned().unwrap_or(json!("")),
                                "input_schema": t.get("inputSchema").filter(|s| crate::js::truthy(s)).cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                            })
                        })
                        .collect();
                    Ok((conn, tools))
                }
                .await;
                if let Err(e) = &made {
                    eprintln!("  office mcp unavailable: {}", crate::js::slice_utf16(e, 160));
                }
                made
            })
            .await
            .clone()
    }

    /// `officeMcpTools()`: the child's tools once it is up, `office_create`
    /// left out (the tool loop's own adds the file card).
    pub fn tools(&self) -> Vec<Value> {
        match self.started.get() {
            Some(Ok((_, tools))) => tools.iter().filter(|t| t["name"] != "office_create").cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// `callOfficeTool(root, name, input)`: (content, is_error); `Err` is a
    /// failed request (Node's thrown error).
    pub async fn call(&self, name: &str, input: &Value) -> Result<(String, bool), String> {
        let conn = match self.ensure().await {
            Ok((c, _)) => c,
            Err(e) => return Ok((format!("office tools unavailable — the BetterOffice MCP server failed to start ({e})"), true)),
        };
        let args = if input.is_object() { input.clone() } else { json!({}) };
        let r = conn.call_tool(name, &args, None, mcp::DEFAULT_TIMEOUT).await.map_err(|e| e.message)?;
        Ok((crate::hub::tools::mcp::fold(&r), r.get("isError").is_some_and(crate::js::truthy)))
    }

    /// Ends the child (the broker's shutdown).
    pub async fn stop(&self) {
        if let Some(Ok((conn, _))) = self.started.get() {
            conn.close().await;
        }
    }
}

/// The office child for this broker: Node's binary and server dir as
/// main.rs reads them (`--node`, `--node-script`), the files dir as root.
pub fn for_hub(data: &Path) -> Office {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = crate::config::Config::parse(&args, std::env::var("PORT").ok().as_deref()).ok();
    let node = config.as_ref().map(|c| c.node.clone()).unwrap_or_else(|| PathBuf::from("node"));
    let (src, _) = crate::hub::market::node_paths();
    Office::new(&node, &src, &data.join("generated").join("files"))
}
