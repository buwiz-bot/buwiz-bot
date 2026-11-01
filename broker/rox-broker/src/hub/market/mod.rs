//! The plugin market and private skills (plan 5.8): Node's `plugins.mjs`
//! (`catalog`), `plugin_resolve.mjs` (`resolve`), the installs from git and
//! the connector rows (`install`), the skill tools and prompt sections
//! (`skills`); the routes are `hub/routes/market.rs`. Owns `skills`,
//! `installed_plugins`, `plugin_settings` and the `mcp_servers` rows it
//! materializes (it drops McpHost's client for each, as Node does).

pub mod catalog;
pub mod install;
pub mod resolve;
pub mod review;
pub mod skills;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

use self::catalog::Market;
use super::ctx::HubCtx;
use super::Registry;
use crate::js;

/// Adds the market's routes and the six skill tools, and makes its tables
/// at start (Node's DDL; the store's schema (5.1) makes them too).
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    let (server, port) = node_paths();
    let market = Arc::new(Market::load(&server, &ctx.data, port));
    super::routes::market::register(routes, market.clone());
    // the tool loop's system prompt reads its plugin skills (5.4)
    ctx.ext.insert(market.clone());
    // a bot's integrations read the catalog's entries (5.2)
    ctx.ext.insert(crate::hub::library::botctx::Catalog(Arc::new(BotCatalog(market.clone()))));
    skills::register(ctx);
    routes.on_start(|ctx| {
        if let Err(e) = schema(&ctx) {
            eprintln!("rox-broker: market tables: {e}");
        }
    });
}

/// plugins.mjs's `entryOf` for botctx (5.2's [`Catalog`]).
///
/// [`Catalog`]: crate::hub::library::botctx::Catalog
struct BotCatalog(Arc<Market>);

impl crate::hub::library::botctx::PluginCatalog for BotCatalog {
    fn entry(&self, key: &str) -> Option<Value> {
        self.0.entry_of(key).cloned()
    }
}

/// Node's SERVER_DIR (`--assets`, else the folder of `--node-script`,
/// default `src/app-server.mjs`) and the broker's port, read as main.rs
/// reads them.
pub fn node_paths() -> (PathBuf, u16) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = crate::config::Config::parse(&args, std::env::var("PORT").ok().as_deref()).ok();
    let server = config.as_ref().map(|c| c.server_dir()).unwrap_or_else(|| PathBuf::from("src"));
    let server = std::env::current_dir().map(|d| d.join(&server)).unwrap_or(server);
    (server, rox_v1::env::var("BUWIZ_PUBLIC_PORT").and_then(|p| p.parse().ok()).or(config.map(|c| c.port)).unwrap_or(8787))
}

/// The tables the market writes, as state.mjs makes them (IF NOT EXISTS,
/// ALTERs whose failure means the column is there).
pub fn schema(ctx: &HubCtx) -> Result<(), String> {
    ctx.db.with(|db| {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS mcp_servers(
               id TEXT PRIMARY KEY, name TEXT, command TEXT, args TEXT, env TEXT,
               enabled INTEGER DEFAULT 1, bot TEXT, created_at INTEGER, url TEXT);",
        )?;
        for col in ["url TEXT", "disabled_tools TEXT", "auth TEXT", "account TEXT", "headers TEXT", "oauth_client TEXT", "transport TEXT", "agent_pending INTEGER"] {
            let _ = db.execute_batch(&format!("ALTER TABLE mcp_servers ADD COLUMN {col}"));
        }
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS plugin_settings(plugin TEXT PRIMARY KEY, vals TEXT, updated_at INTEGER);
             CREATE TABLE IF NOT EXISTS skills(id TEXT PRIMARY KEY, name TEXT, description TEXT, body TEXT, enabled INTEGER DEFAULT 1, created_at INTEGER);
             CREATE TABLE IF NOT EXISTS installed_plugins(name TEXT PRIMARY KEY, meta TEXT, created_at INTEGER);",
        )
    })
}

// ————— JavaScript's view of JSON values —————

/// `o?.[k]`, with JSON null read as a value (as `?.` does).
pub(crate) fn at<'a>(v: Option<&'a Value>, k: &str) -> Option<&'a Value> {
    v.and_then(|v| v.as_object()).and_then(|o| o.get(k))
}

/// JavaScript truthiness of a value that may be undefined.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(js::truthy)
}

/// `a || b`.
pub(crate) fn or(v: Option<&Value>, d: Value) -> Value {
    match v {
        Some(v) if js::truthy(v) => v.clone(),
        _ => d,
    }
}

/// `a ?? b` (undefined or null).
pub(crate) fn nn<'a>(v: Option<&'a Value>) -> Option<&'a Value> {
    v.filter(|v| !v.is_null())
}

/// `String(v)`, with undefined as "undefined".
pub(crate) fn string(v: Option<&Value>) -> String {
    v.map_or_else(|| "undefined".to_owned(), js::string_of)
}

/// A string when the value is one.
pub(crate) fn text(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

/// `JSON.parse(s) ?? d`, `d` when it throws.
pub(crate) fn parse(s: Option<&str>, d: Value) -> Value {
    match s.map(serde_json::from_str::<Value>) {
        Some(Ok(v)) if !v.is_null() => v,
        _ => d,
    }
}

/// `Object.entries(o)`, in JavaScript's key order (an array's indices too).
pub(crate) fn entries(v: Option<&Value>) -> Vec<(String, Value)> {
    match v {
        Some(Value::Object(m)) => js::ordered(m).into_iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Some(Value::Array(a)) => a.iter().enumerate().map(|(i, v)| (i.to_string(), v.clone())).collect(),
        Some(Value::String(s)) => s.chars().enumerate().map(|(i, c)| (i.to_string(), Value::String(c.to_string()))).collect(),
        _ => Vec::new(),
    }
}

/// `{...o}` as a map (undefined, null and scalars spread to nothing).
pub(crate) fn spread(v: Option<&Value>) -> Map<String, Value> {
    entries(v.filter(|v| !v.is_string())).into_iter().collect()
}

/// `s.toLowerCase().replace(/[^a-z0-9]+/g, sep)`.
pub(crate) fn dashes(s: &str, sep: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            gap = false;
        } else if !gap {
            out.push_str(sep);
            gap = true;
        }
    }
    out
}

/// `[...new Set(list)]` of strings.
pub(crate) fn unique(list: impl IntoIterator<Item = Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for v in list {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// `Date.now()` on the hub's clock.
pub(crate) fn now(ctx: &HubCtx) -> i64 {
    ctx.clock.now() as i64
}

/// Node's `path.join` for these paths: segments joined, `.` and `..`
/// resolved, no trailing slash.
pub(crate) fn join(base: &Path, rel: &str) -> PathBuf {
    PathBuf::from(posix_normalize(&format!("{}/{rel}", base.display())))
}

/// `path.posix.normalize`.
pub(crate) fn posix_normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".to_owned();
    }
    let absolute = p.starts_with('/');
    let trailing = p.ends_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !absolute {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let mut s = out.join("/");
    if s.is_empty() && !absolute {
        s = ".".to_owned();
    }
    if trailing && !s.is_empty() && s != "." {
        s.push('/');
    } else if trailing && s == "." {
        s = "./".to_owned();
    }
    if absolute {
        format!("/{s}")
    } else {
        s
    }
}

/// `path.posix.join(a, b)`.
pub(crate) fn posix_join(a: &str, b: &str) -> String {
    let joined = [a, b].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("/");
    posix_normalize(&joined)
}

/// `path.posix.dirname`.
pub(crate) fn posix_dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(i) => t[..i].to_owned(),
        None => ".".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mkt_posix_paths_as_nodes() {
        assert_eq!(posix_normalize(""), ".");
        assert_eq!(posix_normalize("a/./b/../c/"), "a/c/");
        assert_eq!(posix_normalize("../x"), "../x");
        assert_eq!(posix_join("", "plugins/x"), "plugins/x");
        assert_eq!(posix_join("a", "../.."), "..");
        assert_eq!(posix_dirname("a/b/SKILL.md"), "a/b");
        assert_eq!(posix_dirname("SKILL.md"), ".");
        assert_eq!(join(Path::new("/d/plugins"), "temporal"), PathBuf::from("/d/plugins/temporal"));
        assert_eq!(dashes("Weekly Report!", "-"), "weekly-report-");
    }
}
