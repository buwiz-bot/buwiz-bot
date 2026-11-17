//! Plugin sources. A v1 plugin offers a widget some of its tools by marking them: a tool
//! whose `_meta["buwiz/audience"]` has `"widget"` is an action of the plugin's source. The
//! source is named by the plugin's manifest `short`; an action by the tool's name without
//! `<short>_`; it changes data unless the tool says `annotations.readOnlyHint`. Only a
//! plugin that is installed, enabled, approved and running has a source, and a tool the
//! person switched off in the plugin is not one of its actions. The Bot's switch is the
//! plugin's own, `v1:<plugin id>`.

use serde_json::{json, Value};

use super::{Action, CallError, Caller, Origin, Ready, Source};
use crate::hub::ctx::HubCtx;
use crate::hub::policy::keys::declared_read;
use crate::hub::routines::plugins::disabled_of;
use crate::hub::routines::text::approved_text;
use crate::hub::tools::v1::{contributing, view};
use crate::js;

/// What a result may weigh: 1 MiB.
const RESULT_MAX: usize = 1 << 20;
/// Most names of an action's arguments the prompt shows.
const ARGS_SHOWN: usize = 12;

/// Whether plugin tool `t` offers itself to widgets.
pub fn for_widget(t: &Value) -> bool {
    crate::helper::sessions::audience_names(t, "widget") == crate::helper::sessions::Audience::Array(true)
}

/// Whether plugin tool `t` also offers itself to the model (`"agent"`).
pub fn for_agent(t: &Value) -> bool {
    crate::helper::sessions::open_to_agents(t)
}

/// An argument name the prompt may show: plain, short.
fn plain(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `{a,b?}` from a tool's input schema's top-level properties, required ones plain.
fn args_of(schema: Option<&Value>) -> String {
    let props = schema.and_then(|s| s.get("properties")).and_then(Value::as_object);
    let required: Vec<&str> = schema.and_then(|s| s.get("required")).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let names: Vec<String> = props.into_iter().flat_map(|p| p.keys()).filter(|k| plain(k)).take(ARGS_SHOWN).map(|k| if required.contains(&k.as_str()) { k.clone() } else { format!("{k}?") }).collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("{{{}}}", names.join(","))
    }
}

/// The first line of a description, cleaned and cut.
fn about_of(t: &Value) -> String {
    let d = t.get("description").and_then(Value::as_str).unwrap_or_default();
    let first = d.trim().lines().next().unwrap_or_default();
    approved_text(first, 160)
}

/// Every plugin source on this system: one per plugin that is enabled, approved and running
/// and has at least one tool for widgets.
pub fn all(ctx: &HubCtx) -> Vec<Source> {
    let p = view(ctx);
    let mut out = Vec::new();
    for row in contributing(p.as_ref()) {
        let off = disabled_of(&row);
        let prefix = format!("{}_", row.short);
        let mut actions = Vec::new();
        for t in p.runtime(&row.id).tools {
            let Some(name) = t.get("name").and_then(Value::as_str) else { continue };
            if crate::v1::is_v1("reservedTool", t.get("name")) || off.iter().any(|o| o == name) || !for_widget(&t) {
                continue;
            }
            let Some(action) = name.strip_prefix(&prefix).filter(|a| !a.is_empty()) else { continue };
            actions.push(Action { name: action.to_owned(), about: about_of(&t), write: !declared_read(&t), args: args_of(t.get("inputSchema")), returns: String::new() });
        }
        // a built-in's name is never a plugin's
        if actions.is_empty() || super::builtin::is_name(&row.short) {
            continue;
        }
        let m = row.manifest();
        let title = m.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or(&row.short).to_owned();
        let about = m.get("description").and_then(Value::as_str).map(|d| approved_text(d.lines().next().unwrap_or_default(), 160)).unwrap_or_default();
        let ready = if needs_account(&p.accounts(&row.id)) { Ready::NeedsAccount } else { Ready::Ready };
        out.push(Source { name: row.short.clone(), title, about, key: format!("v1:{}", row.id), ready, actions, off: false, origin: Origin::Plugin { id: row.id.clone(), short: row.short.clone() } });
    }
    out
}

/// Whether a required account of the plugin has no connected sign-in (`accounts` is the
/// plugin's accounts view).
fn needs_account(accounts: &Value) -> bool {
    let rows = accounts.as_array().map(Vec::as_slice).unwrap_or_default();
    let connected = |id: &Value| rows.iter().any(|r| r["account"] == *id && r["state"] == "connected");
    rows.iter().any(|r| r["required"] == json!(true) && !connected(&r["account"]))
}

/// The text of a tool result's first text block.
fn first_text(r: &Value) -> String {
    r.get("content").and_then(Value::as_array).into_iter().flatten().find(|c| c.get("type") == Some(&json!("text"))).and_then(|c| c.get("text")).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// Calls `<short>_<action>` on plugin `id`, as a widget's (`origin: "widget"`, with the
/// widget) or, from `query_source`, as the model's (`origin: "agent"`).
pub async fn run(ctx: &HubCtx, caller: Caller<'_>, id: &str, short: &str, action: &str, args: &Value) -> Result<Value, CallError> {
    let tool = format!("{short}_{action}");
    let ts = crate::hub::policy::now(ctx);
    let meta = match caller {
        Caller::Widget(w) => json!({"buwiz/caller": {"origin": "widget", "plugin": id, "widget": w, "ts": ts}}),
        Caller::Agent => json!({"buwiz/caller": {"origin": "agent", "plugin": id, "ts": ts}}),
    };
    let args = if args.is_object() { args.clone() } else { json!({}) };
    // the plugin host's own limit for a call is 120 s
    let r = view(ctx).call_tool(id, &tool, args, Some(meta)).await.map_err(|e| CallError::new("source_error", super::cut(&format!("{}: {}", if e.code.is_empty() { "error" } else { &e.code }, e.message))))?;
    if r.get("isError").is_some_and(js::truthy) {
        let text = first_text(&r);
        return Err(CallError::new("source_error", super::cut(if text.is_empty() { "the action failed" } else { &text })));
    }
    if crate::helper::sessions::result_bytes(&r) > RESULT_MAX {
        return Err(CallError::new("source_error", format!("{action}'s result is over 1 MiB")));
    }
    let data = crate::helper::plugins::result_data(&r);
    Ok(if data.is_null() { json!(first_text(&r)) } else { data })
}
