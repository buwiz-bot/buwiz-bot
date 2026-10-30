//! A bot's own context (botctx.mjs): profile, memories and integrations,
//! the system-prompt section and the five bot tools. The bots live in the
//! store; these are the operations on them.
//!
//! The plugin catalog (5.8, track C) is a seam: [`Catalog`] in `ctx.ext`.
//! Without it no catalog entry exists (a plugin integration resolves only
//! when installed, named by its id; an MCP server has no description or
//! icon).

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::store::json::{js_slice, template};
use crate::hub::store::{now_ms, uuidv7, Bot, State};
use crate::hub::types::{ToolResult, TurnCtx};
use crate::js;

/// plugins.mjs's `entryOf` and `iconFile`, as the catalog (track C) answers them.
pub trait PluginCatalog: Send + Sync {
    /// The catalog entry for a key (`{key, name, displayName, description, …}`).
    fn entry(&self, key: &str) -> Option<Value>;
    /// Whether the key has a logo (`iconFile(key)`).
    fn has_icon(&self, key: &str) -> bool;
}

/// The seam in `ctx.ext`.
#[derive(Clone)]
pub struct Catalog(pub Arc<dyn PluginCatalog>);

fn catalog(ctx: &HubCtx) -> Option<&Arc<dyn PluginCatalog>> {
    ctx.ext.get::<Catalog>().map(|c| &c.0)
}

/// `String(s ?? "").trim().slice(0, n)`.
pub fn clip(s: Option<&Value>, n: usize) -> String {
    let s = match s {
        None | Some(Value::Null) => String::new(),
        Some(v) => js::string_of(v),
    };
    js_slice(s.trim_matches(js::is_space), n)
}

/// Bot Details' "Last updated".
pub fn touch(bot: &mut Bot) {
    bot.updated_at = Some(now_ms() as f64);
}

/// `setBotProfile(bot, {label, description, instructions})`: `None` is
/// undefined (left as it is).
pub fn set_profile(bot: &mut Bot, label: Option<&Value>, description: Option<&Value>, instructions: Option<&Value>) {
    if label.is_some() {
        bot.label = clip(label, 40);
    }
    if description.is_some() {
        bot.description = Some(clip(description, 600));
    }
    if instructions.is_some() {
        bot.instructions = Some(clip(instructions, 6000));
    }
    if description.is_some() || instructions.is_some() {
        touch(bot);
    }
}

fn mem_text(m: &Value) -> String {
    m.get("text").map(js::string_of).unwrap_or_default()
}

/// `memoryAdd`: a duplicate (case aside) returns the one there.
pub fn memory_add(bot: &mut Bot, text: Option<&Value>) -> Result<Value, String> {
    let t = clip(text, 1000);
    if t.is_empty() {
        return Err("memory needs text".into());
    }
    if let Some(dup) = bot.memories.iter().find(|m| mem_text(m).to_lowercase() == t.to_lowercase()) {
        return Ok(dup.clone());
    }
    let m = json!({"id": uuidv7(), "text": t, "at": now_ms()});
    bot.memories.push(m.clone());
    touch(bot);
    if bot.memories.len() > 200 {
        let n = bot.memories.len() - 200;
        bot.memories.drain(..n);
    }
    Ok(m)
}

/// `memoryUpdate`: a memory's new text.
pub fn memory_update(bot: &mut Bot, memory_id: &str, text: Option<&Value>) -> Result<(), String> {
    let t = clip(text, 1000);
    if t.is_empty() {
        return Err("memory needs text".into());
    }
    let Some(m) = bot.memories.iter_mut().find(|x| x.get("id").and_then(Value::as_str) == Some(memory_id)) else {
        return Err(format!("bot has no memory {memory_id}"));
    };
    if let Some(o) = m.as_object_mut() {
        o.insert("text".into(), json!(t));
        o.insert("at".into(), json!(now_ms()));
    }
    touch(bot);
    Ok(())
}

pub fn memory_remove(bot: &mut Bot, memory_id: &str) -> Result<(), String> {
    let before = bot.memories.len();
    bot.memories.retain(|m| m.get("id").and_then(Value::as_str) != Some(memory_id));
    if bot.memories.len() == before {
        return Err(format!("no memory {memory_id}"));
    }
    touch(bot);
    Ok(())
}

const KINDS: [&str; 3] = ["mcp", "plugin", "skill"];

fn icon_path(key: &str) -> String {
    format!("/plugins/icon/{}", encode_uri_component(key))
}

/// `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn text_col(conn: &Connection, sql: &str, args: &[&str]) -> Option<Vec<Value>> {
    let mut st = conn.prepare(sql).ok()?;
    let n = st.column_count();
    st.query_row(rusqlite::params_from_iter(args.iter()), |r| (0..n).map(|i| r.get::<_, rusqlite::types::Value>(i).map(crate::hub::store::write::to_json)).collect::<rusqlite::Result<Vec<Value>>>())
        .optional()
        .ok()
        .flatten()
}

fn or(v: &Value, d: &Value) -> Value {
    if js::truthy(v) {
        v.clone()
    } else {
        d.clone()
    }
}

/// botctx.mjs `lookup(kind, id)`: what an integration resolves to now.
pub fn lookup(ctx: &HubCtx, conn: &Connection, kind: &str, id: &str) -> Option<Value> {
    let cat = catalog(ctx);
    let entry = |key: &str| cat.and_then(|c| c.entry(key));
    let field = |e: &Option<Value>, k: &str| e.as_ref().and_then(|e| e.get(k)).filter(|v| js::truthy(v)).cloned();
    match kind {
        "mcp" => {
            let r = text_col(conn, "SELECT id,name FROM mcp_servers WHERE id=? OR name=?", &[id, id])?;
            let (rid, name) = (r[0].clone(), r[1].clone());
            let name = or(&name, &rid);
            let e = entry(&js::string_of(&name));
            let key = field(&e, "key").map(|k| js::string_of(&k));
            let icon = match (&e, key) {
                (Some(_), Some(k)) if cat.is_some_and(|c| c.has_icon(&k)) => icon_path(&k),
                _ => String::new(),
            };
            Some(json!({"id": rid, "name": name, "kind": kind, "description": field(&e, "description").unwrap_or(json!("")), "icon": icon}))
        }
        "plugin" => {
            let inst = text_col(conn, "SELECT name FROM installed_plugins WHERE name=?", &[id]);
            let e = entry(id);
            if inst.is_none() && e.is_none() {
                return None;
            }
            let name = field(&e, "displayName").or_else(|| field(&e, "name")).unwrap_or(json!(id));
            let icon = if cat.is_some_and(|c| c.has_icon(id)) { icon_path(id) } else { String::new() };
            Some(json!({"id": id, "name": name, "kind": kind, "description": field(&e, "description").unwrap_or(json!("")), "icon": icon}))
        }
        "skill" => {
            let r = text_col(conn, "SELECT id,name,description FROM skills WHERE id=? OR name=?", &[id, id])?;
            Some(json!({"id": r[0], "name": or(&r[1], &r[0]), "kind": kind, "description": or(&r[2], &json!("")), "icon": ""}))
        }
        _ => None,
    }
}

/// `kind:id` or `{kind, id}`: (kind, id) as botctx.mjs reads a spec.
fn spec_parts(spec: Option<&Value>) -> (Option<Value>, Value) {
    match spec {
        Some(Value::Object(o)) => (o.get("kind").cloned().filter(|k| !k.is_null()), o.get("id").cloned().unwrap_or(Value::Null)),
        other => {
            let s = other.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
            for k in KINDS {
                if let Some(rest) = s.strip_prefix(k).and_then(|r| r.strip_prefix(':')) {
                    if !rest.is_empty() && !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                        return (Some(json!(k)), json!(rest));
                    }
                }
            }
            (None, json!(s))
        }
    }
}

/// `resolveIntegration(spec)`.
pub fn resolve_integration(ctx: &HubCtx, conn: &Connection, spec: Option<&Value>) -> Result<Value, String> {
    let (kind, id) = spec_parts(spec);
    let id = if js::truthy(&id) { js::string_of(&id) } else { String::new() };
    let id = id.trim_matches(js::is_space).to_owned();
    if id.is_empty() {
        return Err("integration needs an id".into());
    }
    let kind = kind.filter(|k| js::truthy(k)).map(|k| js::string_of(&k));
    if let Some(k) = &kind {
        if !KINDS.contains(&k.as_str()) {
            return Err(format!("unknown integration kind {k}"));
        }
    }
    let kinds: Vec<&str> = match &kind {
        Some(k) => vec![k.as_str()],
        None => KINDS.to_vec(),
    };
    for k in kinds {
        if let Some(hit) = lookup(ctx, conn, k, &id) {
            return Ok(hit);
        }
    }
    Err(format!("no integration matches {}{id}", kind.map(|k| format!("{k}:")).unwrap_or_default()))
}

/// installed plugin name → the mcp_servers ids its meta lists.
pub fn plugin_servers(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let Ok(mut st) = conn.prepare("SELECT name, meta FROM installed_plugins") else { return Vec::new() };
    let rows = st.query_map([], |r| Ok((r.get::<_, rusqlite::types::Value>(0)?, r.get::<_, rusqlite::types::Value>(1)?)));
    let Ok(rows) = rows else { return Vec::new() };
    rows.filter_map(|r| r.ok())
        .filter_map(|(n, m)| {
            let name = js::string_of(&crate::hub::store::write::to_json(n));
            let meta = match m {
                rusqlite::types::Value::Text(t) if !t.is_empty() => serde_json::from_str::<Value>(&t).ok()?,
                _ => json!({}),
            };
            let ids = meta.get("mcp").filter(|v| js::truthy(v)).and_then(Value::as_array).map(|a| a.iter().map(js::string_of).collect()).unwrap_or_default();
            Some((name, ids))
        })
        .collect()
}

/// `pluginOfServer(id)`: the installed plugin that registered the server.
pub fn plugin_of_server(plugins: &[(String, Vec<String>)], server: &str) -> Option<String> {
    plugins.iter().find(|(_, ids)| ids.iter().any(|i| i == server)).map(|(n, _)| n.clone())
}

/// `integrationsOf(bot)`: names live from the rows and the catalog; a
/// removed one keeps its id; a server a listed plugin owns is that plugin.
pub fn integrations_of(ctx: &HubCtx, conn: &Connection, bot: &Bot) -> Value {
    let owners = plugin_servers(conn);
    let field = |x: &Value, k: &str| x.get(k).cloned().unwrap_or(Value::Null);
    let plugins: Vec<Value> = bot.integrations.iter().filter(|x| field(x, "kind") == json!("plugin")).map(|x| field(x, "id")).collect();
    let out: Vec<Value> = bot
        .integrations
        .iter()
        .filter(|x| !(field(x, "kind") == json!("mcp") && plugin_of_server(&owners, &js::string_of(&field(x, "id"))).is_some_and(|p| plugins.contains(&json!(p)))))
        .map(|x| {
            let (kind, id) = (field(x, "kind"), field(x, "id"));
            match lookup(ctx, conn, &js::string_of(&kind), &js::string_of(&id)) {
                Some(hit) => json!({"id": hit["id"], "name": hit["name"], "kind": kind, "description": hit["description"], "icon": hit["icon"]}),
                None => json!({"id": id, "name": id, "kind": kind, "missing": true}),
            }
        })
        .collect();
    Value::Array(out)
}

/// `integrationAdd(bot, spec, via)` once `spec` resolved to `hit`: true
/// when it was added (and the bot touched).
pub fn integration_add(bot: &mut Bot, hit: &Value, via: &str) -> bool {
    if bot.integrations.iter().any(|x| x.get("kind") == hit.get("kind") && x.get("id") == hit.get("id")) {
        return false;
    }
    bot.integrations.push(json!({"id": hit["id"], "kind": hit["kind"], "at": now_ms(), "via": via}));
    touch(bot);
    true
}

/// `integrationRemove(bot, spec)`; `owners` is [`plugin_servers`].
pub fn integration_remove(bot: &mut Bot, spec: Option<&Value>, owners: &[(String, Vec<String>)]) -> Result<(), String> {
    let (kind, id) = match spec {
        Some(Value::Object(o)) => (o.get("kind").cloned(), o.get("id").cloned().unwrap_or(Value::Null)),
        other => {
            let (k, id) = spec_parts(other);
            (k, id)
        }
    };
    let kind_s = kind.as_ref().filter(|k| js::truthy(k)).map(js::string_of);
    let before = bot.integrations.len();
    bot.integrations.retain(|x| {
        let xid = x.get("id").cloned().unwrap_or(Value::Null);
        let xk = x.get("kind").map(js::string_of).unwrap_or_default();
        let same = xid == id && kind_s.as_ref().is_none_or(|k| *k == xk);
        let owned = kind_s.as_deref() == Some("plugin") && xk == "mcp" && plugin_of_server(owners, &js::string_of(&xid)).map(Value::String) == Some(id.clone());
        !same && !owned
    });
    if bot.integrations.len() == before {
        let shown = match spec {
            Some(Value::Object(o)) => o.get("id").filter(|v| !v.is_null()).map(js::string_of).unwrap_or_else(|| "[object Object]".into()),
            other => template(other),
        };
        return Err(format!("bot has no integration {shown}"));
    }
    touch(bot);
    Ok(())
}

/// The bot a thread answers as: a delivery turn's own, else `threadBot`.
pub fn bot_of<'a>(s: &'a State, turn: &TurnCtx) -> Option<&'a Bot> {
    if let Some(b) = turn.acting_bot.as_deref().and_then(|id| s.bot(id)) {
        return Some(b);
    }
    if turn.thread.is_empty() {
        return None;
    }
    s.thread_bot(&turn.thread)
}

/// `recordToolUse(thread, serverId)`: a bot's MCP call records the plugin
/// that registered the server, else the server.
pub fn record_tool_use(ctx: &HubCtx, turn: &TurnCtx, server: &str) -> Result<(), String> {
    let owners = ctx.db.with(|c| Ok(plugin_servers(c)))?;
    let Some(bot_id) = ctx.store.read(|s| bot_of(s, turn).map(|b| b.id.clone())) else { return Ok(()) };
    let (kind, id) = match plugin_of_server(&owners, server) {
        Some(p) => ("plugin", p),
        None => ("mcp", server.to_owned()),
    };
    ctx.store.write(|s| {
        if let Some(b) = s.bot_mut(&bot_id) {
            if !b.integrations.iter().any(|x| x.get("kind") == Some(&json!(kind)) && x.get("id") == Some(&json!(id))) {
                b.integrations.push(json!({"id": id, "kind": kind, "at": now_ms(), "via": "use"}));
            }
        }
    })
}

/// `botSection(thread)`: the system prompt's "Who you are" for a bot's thread.
pub fn bot_section(bot: Option<&Bot>) -> String {
    let Some(bot) = bot else { return String::new() };
    let desc = bot.description.clone().unwrap_or_default();
    let who = format!("You are {}{}.{}", bot.name, if bot.label.is_empty() { String::new() } else { format!(", {}", bot.label) }, if desc.is_empty() { String::new() } else { format!(" {desc}") });
    let mut parts = vec![format!("\n\n## Who you are\n{who}")];
    if let Some(i) = bot.instructions.as_deref().filter(|i| !i.is_empty()) {
        parts.push(format!("Instructions:\n{i}"));
    }
    if !bot.memories.is_empty() {
        parts.push(format!("Memories (things the user asked you to keep):\n{}", bot.memories.iter().map(|m| format!("- {}", mem_text(m))).collect::<Vec<_>>().join("\n")));
    }
    parts.push("Your profile and memories are yours to maintain: memory_add / memory_list / memory_remove, bot_profile_update, integrations_list.".into());
    parts.join("\n\n")
}

/// The bot tools' names (`BOT_TOOLS`; their schemas are the tool loop's defs).
pub const BOT_TOOLS: [&str; 5] = ["bot_profile_update", "memory_add", "memory_list", "memory_remove", "integrations_list"];

fn says(content: String) -> ToolResult {
    ToolResult { content: json!(content), is_error: false }
}

fn fails(content: String) -> ToolResult {
    ToolResult { content: json!(content), is_error: true }
}

/// `toolBot(thread, name, input)`.
pub fn tool_bot(ctx: &HubCtx, turn: &TurnCtx, name: &str, input: &Value) -> ToolResult {
    let Some(bot_id) = ctx.store.read(|s| bot_of(s, turn).map(|b| b.id.clone())) else { return fails(format!("{name} only works in a bot's thread")) };
    let edit = |f: &mut dyn FnMut(&mut Bot) -> Result<String, String>| -> ToolResult {
        match ctx.store.write(|s| s.bot_mut(&bot_id).map(&mut *f)) {
            Ok(Some(Ok(text))) => says(text),
            Ok(Some(Err(e))) | Err(e) => fails(e),
            Ok(None) => fails(format!("{name} only works in a bot's thread")),
        }
    };
    match name {
        "bot_profile_update" => {
            let keys: Vec<&str> = ["label", "description", "instructions"].into_iter().filter(|k| input.get(*k).is_some()).collect();
            if keys.is_empty() {
                return fails("nothing to update — pass label, description or instructions".into());
            }
            edit(&mut |b| {
                set_profile(b, input.get("label"), input.get("description"), input.get("instructions"));
                Ok(format!("updated {} — applies from your next step", keys.join(", ")))
            })
        }
        "memory_add" => edit(&mut |b| memory_add(b, input.get("text")).map(|m| format!("remembered ({})", js::string_of(&m["id"])))),
        "memory_list" => {
            let ms = ctx.store.read(|s| s.bot(&bot_id).map(|b| b.memories.clone())).unwrap_or_default();
            says(if ms.is_empty() { "no memories yet".into() } else { ms.iter().map(|m| format!("{} — {}", js::string_of(&m["id"]), mem_text(m))).collect::<Vec<_>>().join("\n") })
        }
        "memory_remove" => {
            let id = input.get("id").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
            edit(&mut |b| memory_remove(b, &id).map(|_| format!("forgot {}", template(input.get("id")))))
        }
        "integrations_list" => {
            let bot = ctx.store.read(|s| s.bot(&bot_id).cloned());
            let xs = match bot {
                Some(b) => ctx.db.with(|c| Ok(integrations_of(ctx, c, &b))).unwrap_or(json!([])),
                None => json!([]),
            };
            let xs = xs.as_array().cloned().unwrap_or_default();
            says(if xs.is_empty() {
                "no integrations recorded yet (the global catalog is still available)".into()
            } else {
                xs.iter()
                    .map(|x| format!("{}:{} — {}{}", js::string_of(&x["kind"]), js::string_of(&x["id"]), js::string_of(&x["name"]), if x.get("missing").is_some() { " (removed)" } else { "" }))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        _ => fails(format!("unknown bot tool {name}")),
    }
}

/// The five bot tools, registered by name (the tool loop offers them only
/// in a bot's thread).
pub fn register(ctx: &mut HubCtx) {
    for name in BOT_TOOLS {
        if ctx.tools.get(name).is_none() {
            ctx.tools.register(name, move |ctx: Arc<HubCtx>, turn: TurnCtx, input: Value| async move { tool_bot(&ctx, &turn, name, &input) });
        }
    }
}
