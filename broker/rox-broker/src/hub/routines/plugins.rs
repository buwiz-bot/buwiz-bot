//! What routines and the plugin list read of the v1 plugins
//! (`createViews` in v1/plugins.mjs): the records, each plugin's runtime,
//! the trust store, tool calls, and the plugin list GET /v1/host/plugins
//! hands Rox with each routine's state merged (`describe`). The host is
//! `helper::plugins::Host`; tests hand a fake.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tokio::sync::broadcast;

use super::text::approved_text;
use crate::helper::plugins::{Host, Row, RuntimeView};
use crate::hub::types::BoxFuture;
use crate::js;
use crate::v1::trust::Trust;
use crate::v1::V1Error;

/// 9.8: the first-party publisher.
pub const FIRST_PARTY: &str = "app.buwiz";

/// The plugin host as routines read it.
pub trait PluginView: Send + Sync {
    /// `rowOf(id)`: the `v1_plugins` record.
    fn row(&self, id: &str) -> Option<Row>;
    /// `rows()`: every record, oldest first.
    fn rows(&self) -> Vec<Row>;
    /// `runtime(id)`: the plugin's live state (`stopped` when it has none).
    fn runtime(&self, id: &str) -> RuntimeView;
    /// The trust store, read now.
    fn trust(&self) -> Trust;
    /// `plugins.callTool(id, tool, args, {meta})`: the raw tools/call result.
    fn call_tool(&self, id: &str, tool: &str, args: Value, meta: Option<Value>) -> BoxFuture<'static, Result<Value, V1Error>>;
    /// The host's state changes (`plugins.events`), when it has them.
    fn events(&self) -> Option<broadcast::Receiver<Value>>;
    /// A 2.1 package's accounts section (`Host::accounts_view`).
    fn accounts(&self, _id: &str) -> Value {
        json!([])
    }
    /// The accounts a 2.1 package declares.
    fn declared_accounts(&self, _id: &str) -> Vec<crate::v1::package::Account> {
        Vec::new()
    }
}

impl PluginView for Arc<Host> {
    fn row(&self, id: &str) -> Option<Row> {
        Host::row(self, id)
    }
    fn rows(&self) -> Vec<Row> {
        Host::rows(self)
    }
    fn runtime(&self, id: &str) -> RuntimeView {
        self.runtime_view(id)
    }
    fn trust(&self) -> Trust {
        Host::trust(self)
    }
    fn call_tool(&self, id: &str, tool: &str, args: Value, meta: Option<Value>) -> BoxFuture<'static, Result<Value, V1Error>> {
        let (host, id, tool) = (self.clone(), id.to_owned(), tool.to_owned());
        Box::pin(async move { Host::call_tool(&host, &id, &tool, &args, meta.as_ref(), None).await })
    }
    fn events(&self) -> Option<broadcast::Receiver<Value>> {
        Some(Host::events(self))
    }
    fn accounts(&self, id: &str) -> Value {
        self.accounts_view(id)
    }
    fn declared_accounts(&self, id: &str) -> Vec<crate::v1::package::Account> {
        Host::declared_accounts(self, id)
    }
}

/// No plugin host (the hub without one): nothing installed.
pub struct NoPlugins;

impl PluginView for NoPlugins {
    fn row(&self, _: &str) -> Option<Row> {
        None
    }
    fn rows(&self) -> Vec<Row> {
        Vec::new()
    }
    fn runtime(&self, _: &str) -> RuntimeView {
        RuntimeView { state: "stopped", ..RuntimeView::default() }
    }
    fn trust(&self) -> Trust {
        Trust::default()
    }
    fn call_tool(&self, id: &str, _: &str, _: Value, _: Option<Value>) -> BoxFuture<'static, Result<Value, V1Error>> {
        let id = id.to_owned();
        Box::pin(async move { V1Error::err("plugin_unavailable", format!("{id} is not running")) })
    }
    fn events(&self) -> Option<broadcast::Receiver<Value>> {
        None
    }
}

/// `JSON.parse(s) ?? d`.
pub fn parse(s: &str, d: Value) -> Value {
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Null) | Err(_) => d,
        Ok(v) => v,
    }
}

/// `disabledOf(row)`: the switched-off tool names.
pub fn disabled_of(row: &Row) -> Vec<String> {
    parse(&row.disabled_tools, json!([])).as_array().into_iter().flatten().map(js::string_of).collect()
}

/// `(t.annotations?.readOnlyHint ?? t.readOnly) === true`.
pub fn read_only(t: &Value) -> bool {
    let hint = t.get("annotations").and_then(|a| a.get("readOnlyHint")).filter(|h| !h.is_null());
    hint.or_else(|| t.get("readOnly")) == Some(&Value::Bool(true))
}

/// The tools the views read: the live ones when connected, else the record's.
fn known_tools(p: &dyn PluginView, row: &Row) -> Vec<Value> {
    let rt = p.runtime(&row.id);
    if rt.conn {
        rt.tools
    } else {
        parse(&row.tools, json!([])).as_array().cloned().unwrap_or_default()
    }
}

/// 5.9: a routine reads only approved plugins' readOnlyHint tools
/// (`readsStatus`): one `{plugin, tools, allowed, reason}` per `reads` entry.
pub fn reads_status(p: &dyn PluginView, r: &Value) -> Vec<Value> {
    let tr = p.trust();
    r.get("reads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|x| {
            let plugin = x.get("plugin").map(js::string_of).unwrap_or_default();
            let tools: Vec<String> = x.get("tools").and_then(Value::as_array).into_iter().flatten().map(js::string_of).collect();
            let row = p.row(&plugin);
            let known = row.as_ref().map(|row| known_tools(p, row)).unwrap_or_default();
            let ro = |name: &str| known.iter().any(|t| t.get("name").and_then(Value::as_str) == Some(name) && read_only(t));
            let reason = match &row {
                None => format!("{plugin} is not installed"),
                Some(row) if !tr.approved.iter().any(|(id, key)| *id == plugin && *key == row.publisher_key_id) => format!("{plugin} is not an approved plugin"),
                Some(row) if !row.enabled => format!("{plugin} is disabled"),
                Some(_) if !tools.iter().all(|t| ro(t)) => format!("only read-only tools may be read: {}", tools.iter().filter(|t| !ro(t)).cloned().collect::<Vec<_>>().join(", ")),
                Some(_) => String::new(),
            };
            json!({"plugin": x.get("plugin"), "tools": x.get("tools"), "allowed": reason.is_empty(), "reason": if reason.is_empty() { Value::Null } else { json!(reason) }})
        })
        .collect()
}

/// The routines a plugin's manifest declares (`manifestOf(row).routines || []`).
pub fn routines_of(row: &Row) -> Vec<Value> {
    row.manifest().get("routines").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// The plugin's title (`manifestOf(row).title || id`), `id` when it has no record.
pub fn title_of(p: &dyn PluginView, id: &str) -> String {
    match p.row(id) {
        Some(row) => row.manifest().get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| id.to_owned()),
        None => id.to_owned(),
    }
}

/// A routine's prompt with its label (10.2), or `None` when the plugin
/// may not contribute text: enabled, running and on the approved list as
/// installed (`v1RoutinePrompt`, `contributing`).
pub fn routine_prompt(p: &dyn PluginView, plugin: &str, routine: &str) -> Option<String> {
    let row = p.row(plugin).filter(|row| row.enabled && p.runtime(&row.id).state == "running" && p.trust().is_approved(&row.id, &row.publisher_key_id))?;
    let r = routines_of(&row).into_iter().find(|r| r.get("id").and_then(Value::as_str) == Some(routine))?;
    let s = |k: &str| r.get(k).map_or_else(|| "undefined".to_owned(), js::string_of);
    Some(format!("## Routine: {} (approved plugin {})\n{}", approved_text(&s("name"), 80), row.id, approved_text(&s("prompt"), 4000)))
}

/// The plugin as GET /v1/host/plugins lists it (`describe`); `info` is a
/// routine's user state (v1 routines' `info`). A 2.1 package's record
/// adds `package` (its folder), `skills` and `accounts` (no tokens).
pub fn describe(p: &dyn PluginView, row: &Row, info: &dyn Fn(&str, &Value) -> Value) -> Value {
    let m = row.manifest();
    let rt = p.runtime(&row.id);
    let off = disabled_of(row);
    let tools: Vec<Value> = if rt.conn { rt.tools.clone() } else { parse(&row.tools, json!([])).as_array().cloned().unwrap_or_default() };
    let mut out = Map::new();
    out.insert("id".into(), json!(row.id));
    out.insert("short".into(), json!(row.short));
    if let Some(t) = m.get("title") {
        out.insert("title".into(), t.clone());
    }
    out.insert("version".into(), json!(row.version));
    out.insert("protocol".into(), json!(row.protocol));
    if let Some(p) = m.get("publisher") {
        out.insert("publisher".into(), p.clone());
    }
    out.insert("state".into(), json!(rt.state));
    out.insert("code".into(), json!(rt.code));
    out.insert("message".into(), json!(rt.message));
    out.insert("enabled".into(), json!(row.enabled));
    out.insert("generation".into(), json!(rt.generation));
    out.insert("plugin_pub".into(), json!(row.plugin_pub));
    let first = m.get("publisher").and_then(|p| p.get("id")).and_then(Value::as_str) == Some(FIRST_PARTY) && p.trust().is_approved(&row.id, &row.publisher_key_id);
    out.insert("first_party".into(), json!(first));
    out.insert("manifest_sha256".into(), json!(row.manifest_sha256));
    let apps: Vec<Value> = m
        .get("apps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|a| {
            let mut a = a.clone();
            if let Some(o) = a.as_object_mut() {
                let digest = crate::v1::manifest::perms_digest(o.get("permissions")).unwrap_or_default();
                o.insert("perms_sha256".into(), json!(digest));
            }
            a
        })
        .collect();
    out.insert("apps".into(), json!(apps));
    out.insert("commands".into(), m.get("commands").filter(|c| js::truthy(c)).cloned().unwrap_or(json!([])));
    out.insert("scopes".into(), m.get("scopes").filter(|c| js::truthy(c)).cloned().unwrap_or(json!([])));
    let routines: Vec<Value> = m
        .get("routines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            let mut o = r.as_object().cloned().unwrap_or_default();
            o.insert("reads_status".into(), json!(reads_status(p, r)));
            if let Value::Object(extra) = info(&row.id, r) {
                for (k, v) in extra {
                    o.insert(k, v);
                }
            }
            Value::Object(o)
        })
        .collect();
    out.insert("routines".into(), json!(routines));
    let listed: Vec<Value> = tools
        .iter()
        .filter(|t| !crate::v1::is_v1("reservedTool", t.get("name")))
        .map(|t| {
            let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
            let description = t.get("description").filter(|d| js::truthy(d)).cloned().unwrap_or(json!(""));
            json!({"name": t.get("name"), "description": description, "disabled": off.iter().any(|o| o == name), "readOnly": read_only(t)})
        })
        .collect();
    out.insert("tools".into(), json!(listed));
    if let Some(dir) = &row.package {
        out.insert("package".into(), json!(dir));
        let skills: Vec<Value> = parse(&row.skills, json!([])).as_array().into_iter().flatten().map(|s| json!({"name": s.get("name"), "description": s.get("description")})).collect();
        out.insert("skills".into(), json!(skills));
        out.insert("accounts".into(), p.accounts(&row.id));
    }
    Value::Object(out)
}
