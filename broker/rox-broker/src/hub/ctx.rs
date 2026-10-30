//! What every hub handler and tool gets: the data dir, app.db, the MCP and
//! v1 plugin hosts when this broker runs them, the clock, the jobs table,
//! the tools, and the seams later tracks fill (`traits`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;

use super::jobs::Jobs;
use super::traits::{Clock, LocalTools, ModelApi, Nsh, ToolRegistry, TurnRunner};
use crate::helper::mcp_servers::McpHost;
use crate::helper::plugins::Host;

pub struct HubCtx {
    /// Node's data dir (`GROK_DATA_DIR`)
    pub data: PathBuf,
    pub db: Db,
    /// the v0 MCP servers' clients, when this broker hosts them
    pub mcp: Option<Arc<McpHost>>,
    /// the v1 plugin host, when this broker hosts the plugins
    pub plugins: Option<Arc<Host>>,
    pub clock: Arc<Clock>,
    pub jobs: Jobs,
    pub tools: ToolRegistry,
    pub turns: Option<Arc<dyn TurnRunner>>,
    pub model: Option<Arc<dyn ModelApi>>,
    pub nsh: Option<Arc<dyn Nsh>>,
    pub local: Option<Arc<dyn LocalTools>>,
}

impl HubCtx {
    pub fn new(data: PathBuf, clock: Clock) -> Self {
        let db = Db::new(data.join("generated").join("app.db"));
        Self { data, db, mcp: None, plugins: None, clock: Arc::new(clock), jobs: Jobs::default(), tools: ToolRegistry::default(), turns: None, model: None, nsh: None, local: None }
    }
}

/// app.db, opened on first use with the busy timeout Node uses (5 s), its
/// journal mode left as Node set it. Statements run inside `with`, so no
/// lock is held across an await.
pub struct Db {
    path: PathBuf,
    conn: Mutex<Option<Connection>>,
}

impl Db {
    pub fn new(path: PathBuf) -> Self {
        Self { path, conn: Mutex::new(None) }
    }

    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        let mut slot = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
            conn.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
            *slot = Some(conn);
        }
        f(slot.as_ref().expect("opened above")).map_err(|e| e.to_string())
    }
}
