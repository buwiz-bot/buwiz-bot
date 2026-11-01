//! A bot's tool ceiling as the turn sees it (opengrok-server
//! `agui/ceiling.rs` `rows`, `opengrok-policy`'s ceiling ∩ profile): the
//! rows a bot's owner switches — every built-in, every attached MCP
//! server, every skill — and the test a tool must pass to be offered or
//! run. The ceiling only narrows: a global switch (`/skills/state`,
//! `/mcp/tools/state`) that is off stays off whatever the ceiling says.

use serde_json::{json, Value};

use super::{defs, mcp};
use crate::hub::ctx::HubCtx;
use crate::hub::library::botctx;
use crate::hub::store::{Bot, Ceiling};
use crate::hub::types::TurnCtx;
use crate::js;

/// opengrok-server `agui/ceiling.rs`: a built-in tool's row.
pub const KIND_BUILTIN: &str = "builtin";
/// opengrok-server `agui/ceiling.rs`: a plugin's row — here an attached
/// MCP server, switched whole as opengrok switches a plugin.
pub const KIND_PLUGIN: &str = "plugin";
/// A skill's row. opengrok folds skills into its `use_skill` built-in;
/// Buwiz Bot injects each skill into the prompt, so each is its own row.
pub const KIND_SKILL: &str = "skill";

/// A known tool or group: the row it is switched by.
#[derive(Clone, Debug, PartialEq)]
pub struct Known {
    pub name: String,
    pub kind: &'static str,
    pub description: String,
    pub label: Option<String>,
    pub connector: Option<String>,
    /// The global switch (`/mcp/tools/state`, `/skills/state`): off here
    /// stays off whatever the ceiling says. Built-ins have none.
    pub on: bool,
}

/// A skill's row name. Skill ids and MCP server ids are both slugs of a
/// name, so a skill and a server called "github" would otherwise share
/// one row and one switch.
pub fn skill_row(id: &str) -> String {
    format!("skill:{id}")
}

/// What the model reads when the ceiling refuses `what` (fail closed,
/// `opengrok-policy`: a refusal is a result, not a panic).
pub fn refusal_text(what: &str) -> String {
    format!("tool refused: {what} is switched off in this Bot's tool ceiling. Carry on without it, or ask the person to switch it on in the Bot's Tools.")
}

/// Every row a bot's ceiling can switch, in the order the card shows
/// them: built-ins as `defs.json` lists them (TOOLS, ROUTINE_TOOLS,
/// BOT_TOOLS, AGENT_TOOLS), then the MCP servers this bot sees, then the
/// skills.
pub fn known(ctx: &HubCtx, bot: &Bot) -> Vec<Known> {
    let mut out: Vec<Known> = defs::tools()
        .iter()
        .chain(defs::routine_tools())
        .chain(defs::bot_tools())
        .chain(defs::agent_tools())
        .map(|t| Known {
            name: defs::name_of(t).to_owned(),
            kind: KIND_BUILTIN,
            description: t.get("description").map(js::string_of).unwrap_or_default(),
            label: None,
            connector: None,
            on: true,
        })
        .collect();
    for r in mcp::rows(ctx, "WHERE bot IS NULL OR bot=? ORDER BY rowid", &[&bot.id]) {
        let id = r.get("id").map(js::string_of).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let name = r.get("name").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_else(|| id.clone());
        let connector = r.get("transport").filter(|v| js::truthy(v)).map(js::string_of);
        let on = r.get("enabled").is_none_or(js::truthy);
        out.push(Known { name: id, kind: KIND_PLUGIN, description: String::new(), label: Some(name), connector, on });
    }
    let skills = ctx.db.with(|db| {
        let mut st = db.prepare("SELECT id, name, description, enabled FROM skills ORDER BY created_at")?;
        let r = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?)))?;
        r.collect::<rusqlite::Result<Vec<_>>>()
    });
    for (id, name, description, enabled) in skills.unwrap_or_default() {
        if id.is_empty() {
            continue;
        }
        out.push(Known { name: skill_row(&id), kind: KIND_SKILL, description: description.unwrap_or_default(), label: name, connector: None, on: enabled != Some(0) });
    }
    out
}

/// `GET /coworkers/{id}/ceiling`'s `tools` (opengrok-server
/// `agui/ceiling.rs` `rows`): every known row with the ceiling's answer
/// for it, then the rows the ceiling holds for tools nobody here knows
/// (`available: false`, as opengrok marks a plugin that is not loaded).
pub fn rows(ctx: &HubCtx, bot: &Bot) -> Vec<Value> {
    let c = &bot.tools;
    let known = known(ctx, bot);
    let mut out: Vec<Value> = known
        .iter()
        .map(|k| {
            let mut row = json!({ "name": k.name, "kind": k.kind, "description": k.description, "enabled": c.allows(&k.name) });
            if let Some(l) = &k.label {
                row["label"] = json!(l);
            }
            if let Some(cn) = &k.connector {
                row["connector"] = json!(cn);
            }
            if !k.on {
                row["available"] = json!(false);
            }
            if let Some(stored) = c.row(&k.name).and_then(Value::as_object) {
                for (key, v) in stored {
                    if !matches!(key.as_str(), "name" | "kind" | "description" | "enabled" | "label" | "connector" | "available") {
                        row[key] = v.clone();
                    }
                }
            }
            row
        })
        .collect();
    for r in &c.rows {
        let name = r.get("name").map(js::string_of).unwrap_or_default();
        if name.is_empty() || known.iter().any(|k| k.name == name) {
            continue;
        }
        let mut row = r.clone();
        if !row.is_object() {
            continue;
        }
        if row.get("kind").is_none() {
            row["kind"] = json!(KIND_PLUGIN);
        }
        row["enabled"] = json!(c.allows(&name));
        row["available"] = json!(false);
        out.push(row);
    }
    out
}

/// The rows a PUT leaves behind: every known row with `enabled` as the
/// write says, and every row the write (or the stored ceiling) names
/// that nobody here knows, kept as sent. `on(name)` is the write's answer
/// for a known name; `sent` the rows it carried, `enabled` the names it
/// listed.
pub fn settle(ctx: &HubCtx, bot: &Bot, sent: &[Value], enabled: Option<&[String]>) -> Vec<Value> {
    let known = known(ctx, bot);
    let sent_row = |name: &str| sent.iter().find(|r| r.get("name").and_then(Value::as_str) == Some(name));
    let on = |name: &str| -> bool {
        if let Some(list) = enabled {
            return list.iter().any(|n| n == name);
        }
        match sent_row(name) {
            Some(r) => r.get("enabled").map(js::truthy).unwrap_or(true),
            None => bot.tools.allows(name),
        }
    };
    let mut out: Vec<Value> = known
        .iter()
        .map(|k| {
            let mut row = sent_row(&k.name).cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
            row["name"] = json!(k.name);
            row["kind"] = json!(k.kind);
            row["enabled"] = json!(on(&k.name));
            row
        })
        .collect();
    let mut keep = |row: Value| {
        let name = row.get("name").map(js::string_of).unwrap_or_default();
        if name.is_empty() || out.iter().any(|r| r.get("name").and_then(Value::as_str) == Some(name.as_str())) {
            return;
        }
        let mut row = row;
        row["enabled"] = json!(on(&name));
        out.push(row);
    };
    for r in sent.iter().filter(|r| r.is_object()) {
        keep(r.clone());
    }
    for name in enabled.unwrap_or(&[]) {
        keep(json!({ "name": name, "kind": KIND_PLUGIN }));
    }
    for r in &bot.tools.rows {
        keep(r.clone());
    }
    out
}

/// The row a tool's name is switched by: an MCP tool by its server, a
/// built-in by itself; `None` for a tool no row names (a v1 plugin's, an
/// office tool, the server's search), which the ceiling leaves alone.
pub fn row_for(name: &str) -> Option<String> {
    if defs::builtin_names().any(|b| b == name) {
        return Some(name.to_owned());
    }
    mcp::parse_mcp_name(name).map(|(server, _)| server)
}

/// Whether `ceiling` lets the bot load the app `id` offers to load
/// (`app_tools_load`): an MCP server by its row; a v1 plugin has none.
pub fn offers_app(ctx: &HubCtx, ceiling: &Ceiling, id: &str) -> bool {
    mcp::rows(ctx, "WHERE id=?", &[&id]).is_empty() || ceiling.allows(id)
}

/// Whether `ceiling` offers a tool named `name` to the model.
pub fn offers(ceiling: &Ceiling, name: &str) -> bool {
    match row_for(name) {
        Some(row) => ceiling.allows(&row),
        None => true,
    }
}

/// The acting bot's ceiling for this turn; `None` in a thread no bot
/// acts in, where nothing narrows the list.
pub fn of_turn(ctx: &HubCtx, turn: &TurnCtx) -> Option<Ceiling> {
    ctx.store.read(|s| botctx::bot_of(s, turn).map(|b| b.tools.clone()))
}

/// Why a call to `name` is refused under this turn's ceiling, as the
/// tool result the model reads; `None` when it may run. Fails closed: a
/// switched-off tool is refused whether or not it was offered.
pub fn refusal(ctx: &HubCtx, turn: &TurnCtx, name: &str) -> Option<String> {
    let c = of_turn(ctx, turn)?;
    if offers(&c, name) {
        return None;
    }
    let what = match row_for(name) {
        Some(row) if row != name => format!("the server `{row}` ({name})"),
        _ => format!("`{name}`"),
    };
    Some(refusal_text(&what))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(rows: Vec<Value>) -> Ceiling {
        Ceiling { rows, version: 1 }
    }

    #[test]
    fn ceiling_untouched_allows_everything_and_set_rows_narrow() {
        let none = Ceiling::default();
        assert!(none.allows("shell_run"));
        assert!(offers(&none, "mcp_acme_echo"));
        let c = set(vec![json!({"name": "shell_run", "kind": "builtin", "enabled": false}), json!({"name": "acme", "kind": "plugin", "enabled": true})]);
        assert!(!c.allows("shell_run"));
        assert!(!offers(&c, "shell_run"));
        assert!(offers(&c, "mcp_acme_echo"), "an MCP tool is switched by its server's row");
        assert!(!offers(&c, "mcp_other_echo"), "a server without a row is off once the ceiling is set");
        assert!(!offers(&c, "fetch_url"), "a built-in without a row is off once the ceiling is set");
        assert!(offers(&c, "fx_echo"), "a name no row can switch is left alone");
        assert_eq!(c.to_json(), Some(json!({"rows": c.rows, "version": 1})));
        assert_eq!(Ceiling::from_json(&json!({"rows": c.rows, "version": 1})), c);
        assert_eq!(Ceiling::default().to_json(), None);
    }
}
