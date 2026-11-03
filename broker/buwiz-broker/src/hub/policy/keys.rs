//! A Bot's tool switches: each model tool's key (`shell`, `web`, `apps`, `office`,
//! `routines`, `memory`, `agents`, `skill:<id>`, `mcp:<server>`, `v1:<plugin>`), or none for
//! a tool that is always on, and the rows This Bot shows.

use serde_json::{json, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::tools::{defs, mcp, miniapps, v1};
use crate::js;

/// The group keys, in the order This Bot lists them.
pub const GROUPS: [&str; 7] = ["shell", "web", "apps", "office", "routines", "memory", "agents"];

/// The tools that switch with `apps` beside the mini app tools: the artifact tools.
const APP_TOOLS: [&str; 8] = ["app_icon", "app_details_update", "launch_app", "close_app", "promote_app", "demote_app", "set_version", "view_code"];
const MEMORY_TOOLS: [&str; 3] = ["memory_add", "memory_list", "memory_remove"];

/// The key of the group tool `name`, without reading any plugin or server.
fn group_of(name: &str) -> Option<&'static str> {
    match name {
        "shell_run" => Some("shell"),
        "fetch_url" | "web_search" | "query_source" => Some("web"),
        _ if miniapps::NAMES.contains(&name) || APP_TOOLS.contains(&name) => Some("apps"),
        _ if name.starts_with("office_") => Some("office"),
        _ if defs::has(defs::routine_tools(), name) => Some("routines"),
        _ if MEMORY_TOOLS.contains(&name) => Some("memory"),
        _ if defs::has(defs::agent_tools(), name) => Some("agents"),
        _ => None,
    }
}

/// The key a model tool switches with; `None` for one that is always on (the bot controls
/// themselves, making and changing cards, reactions, the skill and MCP admin tools,
/// `bot_profile_update`, `integrations_list`, `app_tools_load`).
pub fn key_of(ctx: &HubCtx, name: &str, surface: Option<&Value>) -> Option<String> {
    if let Some(g) = group_of(name) {
        return Some(g.to_owned());
    }
    if super::super::tools::controls::NAMES.contains(&name) {
        return None;
    }
    if let Some((server, _)) = mcp::parse_mcp_name(name) {
        return Some(format!("mcp:{server}"));
    }
    if defs::builtin_names().any(|b| b == name) {
        return None;
    }
    v1::tool_plugin(v1::view(ctx).as_ref(), name, surface).map(|p| format!("v1:{p}"))
}

/// The key of a deferred app's id (`app_tools_load`'s `app`): a server's, else a v1
/// plugin's by its short name.
pub fn key_of_app(ctx: &HubCtx, app: &str) -> Option<String> {
    if mcp::rows(ctx, "WHERE id=?", &[&app]).first().is_some() {
        return Some(format!("mcp:{app}"));
    }
    v1::view(ctx).rows().into_iter().find(|r| r.short == app).map(|r| format!("v1:{}", r.id))
}

/// A key's row: (label, sub line), or `None` for no such key.
pub fn label_of(ctx: &HubCtx, key: &str) -> Option<(String, String)> {
    let fixed = |l: &str, s: &str| Some((l.to_owned(), s.to_owned()));
    match key {
        "shell" => return fixed("Shell on this Mac", "Runs commands on this Mac. This Mac's rules still apply."),
        "web" => return fixed("Web", "Reads web pages and searches the web."),
        "apps" => return fixed("Mini apps", "Installs, sets up and opens mini apps and cards."),
        "office" => return fixed("Office documents", "Makes and changes Word, Excel and PowerPoint files."),
        "routines" => return fixed("Routines", "Makes, changes and runs routines."),
        "memory" => return fixed("Memory", "Remembers things about you."),
        "agents" => return fixed("Other agents", "Messages and makes other agents."),
        _ => {}
    }
    if let Some(id) = key.strip_prefix("skill:") {
        let row = ctx.db.with(|c| c.query_row("SELECT name, description, enabled FROM skills WHERE id=?", [id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?)))).ok()?;
        let sub = if row.2 == Some(0) { "Off for every Bot.".to_owned() } else { row.1.filter(|d| !d.is_empty()).unwrap_or_else(|| "A private skill.".into()) };
        return Some((row.0.filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_owned()), sub));
    }
    if let Some(id) = key.strip_prefix("mcp:") {
        let r = mcp::rows(ctx, "WHERE id=?", &[&id]).into_iter().next()?;
        let name = r.get("name").filter(|n| js::truthy(n)).map(js::string_of).unwrap_or_else(|| id.to_owned());
        let sub = if js::truthy(&r["enabled"]) { "A connected app." } else { "Off for every Bot." };
        return Some((name, sub.to_owned()));
    }
    if let Some(id) = key.strip_prefix("v1:") {
        let r = v1::view(ctx).row(id)?;
        let title = r.manifest().get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| id.to_owned());
        return Some((title, if r.enabled { "A mini app.".into() } else { "Off for every Bot.".into() }));
    }
    None
}

/// What a receipt or note calls the key: "the shell", "the web", a skill's name.
pub fn phrase_of(ctx: &HubCtx, key: &str) -> String {
    match key {
        "shell" => "the shell".into(),
        "web" => "the web".into(),
        "apps" => "mini apps".into(),
        "office" => "office documents".into(),
        "routines" => "routines".into(),
        "memory" => "its memory".into(),
        "agents" => "other agents".into(),
        _ => label_of(ctx, key).map(|(l, _)| l).unwrap_or_else(|| key.to_owned()),
    }
}

/// Every key This Bot lists for `bot`: the groups, then each private skill, each MCP server
/// the Bot sees and each v1 plugin.
pub fn all_keys(ctx: &HubCtx, bot: &str) -> Vec<String> {
    let mut out: Vec<String> = GROUPS.iter().map(|g| (*g).to_owned()).collect();
    let skills = ctx.db.with(|c| {
        let mut q = c.prepare("SELECT id FROM skills ORDER BY created_at")?;
        let r = q.query_map([], |r| r.get::<_, String>(0))?;
        r.collect::<rusqlite::Result<Vec<_>>>()
    });
    out.extend(skills.unwrap_or_default().into_iter().map(|id| format!("skill:{id}")));
    for r in mcp::rows(ctx, "WHERE bot IS NULL OR bot=?", &[&bot]) {
        out.push(format!("mcp:{}", js::string_of(&r["id"])));
    }
    out.extend(v1::view(ctx).rows().into_iter().map(|r| format!("v1:{}", r.id)));
    out
}

/// Whether `key` is one This Bot could list (a group, or an existing skill, server or plugin).
pub fn known(ctx: &HubCtx, key: &str) -> bool {
    GROUPS.contains(&key) || label_of(ctx, key).is_some()
}

/// The words in a user's message that name `key` (for unrequested and cooldown checks).
pub fn names_key(ctx: &HubCtx, key: &str, text: &str) -> bool {
    let t = text.to_lowercase();
    let mut words = vec![key.to_lowercase()];
    if let Some((_, id)) = key.split_once(':') {
        words.push(id.to_lowercase());
    }
    if let Some((l, _)) = label_of(ctx, key) {
        words.push(l.to_lowercase());
    }
    words.iter().any(|w| !w.is_empty() && t.contains(w.as_str()))
}

/// This Bot's row for `key` as `GET /v1/host/tools` answers it.
pub fn row_json(ctx: &HubCtx, key: &str, on: bool) -> Value {
    let (label, sub) = label_of(ctx, key).unwrap_or_else(|| (key.to_owned(), String::new()));
    json!({"key": key, "label": label, "sub": sub, "on": on})
}
