//! The v0 MCP servers on the model's surface (tools.mjs `mcpSurface`,
//! `appToolsLoad`, `toolAppToolsLoad`, `toolMcpAdmin`, `noteAppOpen`, the
//! MCP branch of `execTool`; mcp.mjs `anthropicTools`, `parseMcpName`,
//! `callTool`). The clients are McpHost's (`ctx.mcp`); an app with more
//! than 80 tools waits behind `app_tools_load`, loaded per thread for 15
//! minutes (`loadedApps`, process-wide as in Node).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::{Out, Run};
use crate::hub::ctx::HubCtx;
use crate::hub::library::botctx;
use crate::hub::routes::mcp_chat::mcp_name;
use crate::hub::store::now_ms;
use crate::js;

/// Apps with more tools than this load on demand.
pub const DEFER_OVER: usize = 80;
const LOADED_TTL: f64 = 15.0 * 60.0 * 1000.0;

/// thread id → (app id → last used, ms); a v1 plugin's app id is
/// `v1:<short>`.
static LOADED: Mutex<Option<HashMap<String, Vec<(String, f64)>>>> = Mutex::new(None);

/// `loadedFor(thread)`: the thread's loaded apps, the stale ones dropped.
pub fn loaded_for(thread: &str) -> Vec<String> {
    let mut g = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let now = now_ms() as f64;
    let m = g.get_or_insert_with(HashMap::new).entry(thread.to_owned()).or_default();
    m.retain(|(_, at)| now - at <= LOADED_TTL);
    m.iter().map(|(id, _)| id.clone()).collect()
}

/// Marks `app` loaded for `thread` now.
pub fn mark_loaded(thread: &str, app: &str) {
    let mut g = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new).entry(thread.to_owned()).or_default();
    let now = now_ms() as f64;
    match m.iter_mut().find(|(id, _)| id == app) {
        Some(x) => x.1 = now,
        None => m.push((app.to_owned(), now)),
    }
}

/// `markAppUsed(thread, id)`: a loaded app's clock restarts.
pub fn mark_used(thread: &str, app: &str) {
    if loaded_for(thread).iter().any(|x| x == app) {
        mark_loaded(thread, app);
    }
}

/// `parseMcpName(n)`: `mcp_<id>_<tool>` → (id, tool), ids `[a-z0-9-]+`.
pub fn parse_mcp_name(n: &str) -> Option<(String, String)> {
    let rest = n.strip_prefix("mcp_")?;
    // the regex's greedy `[a-z0-9-]+` stops at the first character outside
    // it, which must be the `_` before a non-empty tool
    let end = rest.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')).unwrap_or(rest.len());
    if end == 0 || !rest[end..].starts_with('_') || rest.len() <= end + 1 {
        return None;
    }
    Some((rest[..end].to_owned(), rest[end + 1..].to_owned()))
}

/// `anthropicTools(id, tools)`: a server's tools as the model sees them.
pub fn anthropic_tools(id: &str, tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let name = t.get("name").map(js::string_of).unwrap_or_default();
            let desc = t.get("description").filter(|d| js::truthy(d)).map(js::string_of).unwrap_or_else(|| name.clone());
            json!({
                "name": mcp_name(id, &name),
                "description": js::slice_utf16(&format!("[{id}] {desc}"), 1024),
                "input_schema": t.get("inputSchema").filter(|s| js::truthy(s)).cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            })
        })
        .collect()
}

/// An `mcp_servers` row, every column as JSON (`SELECT *`).
pub type Row = Map<String, Value>;

fn to_json(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(String::from_utf8_lossy(b)),
    }
}

/// `SELECT * FROM mcp_servers <where>` with `params`.
pub fn rows(ctx: &HubCtx, sql_where: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<Row> {
    ctx.db
        .with(|c| {
            let mut st = c.prepare(&format!("SELECT * FROM mcp_servers {sql_where}"))?;
            let names: Vec<String> = st.column_names().into_iter().map(str::to_owned).collect();
            let r = st.query_map(params, |r| {
                let mut m = Map::new();
                for (i, n) in names.iter().enumerate() {
                    m.insert(n.clone(), to_json(r.get_ref(i)?));
                }
                Ok(m)
            })?;
            r.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

fn row(ctx: &HubCtx, sql_where: &str, params: &[&dyn rusqlite::ToSql]) -> Option<Row> {
    rows(ctx, sql_where, params).into_iter().next()
}

fn s(r: &Row, k: &str) -> String {
    r.get(k).filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default()
}

fn off_of(r: &Row) -> Vec<String> {
    serde_json::from_str::<Value>(&s(r, "disabled_tools")).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(js::string_of).collect()
}

/// `ensureClient(row)` as the tool loop reads it: the client's tools, and
/// whether its calls would only fail (signed out, or unavailable); `None`
/// for a server that would not connect.
async fn ensure(ctx: &HubCtx, id: &str) -> Option<(Vec<Value>, bool)> {
    let mcp = ctx.mcp.as_ref()?;
    let r = mcp.ensure(id).await;
    if r.get("ok") != Some(&Value::Bool(true)) {
        return None;
    }
    let tools = r.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
    let blocked = r.get("needsAuth").is_some_and(js::truthy) || r.pointer("/status/status").and_then(Value::as_str) == Some("error");
    Some((tools, blocked))
}

/// A deferred app: `{id, name, count, sample}`.
pub type Deferred = Value;

/// `mcpSurface(thread, job)`: the callable defs, then the apps left to
/// load. A routine's v1 turn sees no v0 server.
pub async fn surface(run: &Run) -> (Vec<Value>, Vec<Deferred>) {
    let ctx = &run.ctx;
    let bot = ctx.store.read(|s| botctx::bot_of(s, &run.turn).map(|b| b.id.clone())).unwrap_or_default();
    let servers = if run.turn.surface.is_some() { Vec::new() } else { rows(ctx, "WHERE enabled=1 AND (bot IS NULL OR bot=?)", &[&bot]) };
    let loaded = loaded_for(&run.thread_id());
    let (mut defs, mut deferred) = (Vec::new(), Vec::new());
    for r in &servers {
        let id = s(r, "id");
        let tools = match ensure(ctx, &id).await {
            Some((_, true)) => continue,
            Some((tools, false)) => tools,
            None => Vec::new(),
        };
        let off = off_of(r);
        let tools: Vec<Value> = tools.into_iter().filter(|t| !off.contains(&t.get("name").map(js::string_of).unwrap_or_default())).collect();
        if tools.len() > DEFER_OVER && !loaded.contains(&id) {
            let name = if js::truthy(&r["name"]) { s(r, "name") } else { id.clone() };
            deferred.push(json!({"id": id, "name": name, "count": tools.len(), "sample": tools.iter().take(6).map(|t| t.get("name").cloned().unwrap_or(Value::Null)).collect::<Vec<_>>()}));
            continue;
        }
        defs.extend(anthropic_tools(&id, &tools));
    }
    // v1 plugins: their own names, approved plugins only; a plugin over 80
    // tools waits for app_tools_load like a v0 server
    let mut taken: Vec<String> = super::defs::builtin_names().map(str::to_owned).collect();
    taken.extend(defs.iter().map(|t| t.get("name").map(js::string_of).unwrap_or_default()));
    let plugins = super::v1::view(ctx);
    let v1 = super::v1::v1_tools(plugins.as_ref(), &taken, run.turn.surface.as_ref());
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for (plugin, t) in v1 {
        match groups.iter_mut().find(|(p, _)| *p == plugin) {
            Some((_, g)) => g.push(t),
            None => groups.push((plugin, vec![t])),
        }
    }
    for (plugin, tools) in groups {
        let row = plugins.row(&plugin);
        let short = row.as_ref().map(|r| r.short.clone()).unwrap_or_default();
        if tools.len() > DEFER_OVER && !loaded.contains(&format!("v1:{short}")) {
            let title = row.as_ref().map(|r| r.manifest().get("title").cloned().unwrap_or(Value::Null)).unwrap_or(Value::Null);
            deferred.push(json!({"id": short, "name": title, "count": tools.len(), "sample": tools.iter().take(6).map(|t| t["name"].clone()).collect::<Vec<_>>()}));
            continue;
        }
        defs.extend(tools);
    }
    (defs, deferred)
}

/// `appToolsLoad(deferred)`: the tool that loads one of them.
pub fn app_tools_load_def(deferred: &[Deferred]) -> Value {
    let apps: Vec<String> = deferred
        .iter()
        .map(|d| {
            let sample: Vec<String> = d["sample"].as_array().into_iter().flatten().map(js::string_of).collect();
            format!("{} ({}, {} tools, e.g. {})", js::string_of(&d["id"]), js::string_of(&d["name"]), js::string_of(&d["count"]), sample.join(", "))
        })
        .collect();
    json!({
        "name": "app_tools_load",
        "description": format!("Load a connected app's tools so you can call them (they become callable on your next step and stay loaded for this chat). Load one only when the request needs that app. Apps: {}", apps.join("; ")),
        "input_schema": {
            "type": "object",
            "properties": { "app": { "type": "string", "enum": deferred.iter().map(|d| d["id"].clone()).collect::<Vec<_>>(), "description": "The app id" } },
            "required": ["app"],
        },
    })
}

/// `toolAppToolsLoad(thread, input)`: the app joins the thread's surface
/// from the next model call on.
pub async fn app_tools_load(run: &Run, input: &Value) -> Out {
    let ctx = &run.ctx;
    let id = input.get("app").filter(|a| !a.is_null()).map(js::string_of).unwrap_or_default();
    let server = row(ctx, "WHERE id=? AND enabled=1", &[&id]);
    let plugins = super::v1::view(ctx);
    if server.is_none() {
        if let Some(p) = plugins.rows().into_iter().find(|r| r.short == id && r.enabled) {
            mark_loaded(&run.thread_id(), &format!("v1:{id}"));
            let names: Vec<String> = super::v1::v1_tools(plugins.as_ref(), &[], None).into_iter().filter(|(pl, _)| *pl == p.id).map(|(_, t)| js::string_of(&t["name"])).collect();
            return Out::ok(format!("Loaded {} {id} tools; call them from your next step:\n{}", names.len(), names.join("\n")));
        }
    }
    let Some(r) = server else { return Out::err(format!("no connected app {id}")) };
    let rec = match &ctx.mcp {
        Some(mcp) => mcp.ensure(&id).await,
        None => json!({"ok": false, "err": "failed to start"}),
    };
    if rec.get("ok") != Some(&Value::Bool(true)) {
        let err = rec.get("err").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_else(|| "failed to start".into());
        return Out::err(format!("{id} is not connected ({err})"));
    }
    mark_loaded(&run.thread_id(), &id);
    let off = off_of(&r);
    let names: Vec<String> = rec["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| !off.contains(&t.get("name").map(js::string_of).unwrap_or_default()))
        .map(|t| mcp_name(&id, &t.get("name").map(js::string_of).unwrap_or_default()))
        .collect();
    let label = if js::truthy(&r["name"]) { s(&r, "name") } else { id.clone() };
    Out::ok(format!("Loaded {} {label} tools; call them from your next step:\n{}", names.len(), names.join("\n")))
}

/// mcp.mjs `callTool(row, tool, input)`: the content folded into one
/// string, `is_error` always set, and the result's `_meta`.
async fn call_tool(ctx: &HubCtx, id: &str, tool: &str, input: &Value) -> Result<(Out, Option<Value>), String> {
    let Some(mcp) = &ctx.mcp else { return Ok((Out::err(format!("mcp server {id} is not connected (failed to start)")), None)) };
    let rec = mcp.ensure(id).await;
    if rec.get("ok") != Some(&Value::Bool(true)) {
        let err = rec.get("err").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_else(|| "failed to start".into());
        return Ok((Out::err(format!("mcp server {id} is not connected ({err})")), None));
    }
    if mcp.status(id).get("status").and_then(Value::as_str) == Some("needs-auth") {
        return Ok((Out::err(format!("{id} needs sign-in — ask the user to open Connect Apps, find this app and press Authenticate")), None));
    }
    let result = mcp.request(id, "tools/call", json!({"name": tool, "arguments": input}), Duration::from_secs(60)).await.map_err(|e| e.message)?;
    Ok((Out { content: json!(fold(&result)), is_error: Some(result.get("isError").is_some_and(js::truthy)) }, result.get("_meta").cloned()))
}

/// MCP content blocks folded into the one string the model reads.
pub fn fold(result: &Value) -> String {
    let parts: Vec<String> = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| match c.get("type").and_then(Value::as_str) {
            Some("text") => c.get("text").map(js::string_of).unwrap_or_else(|| "undefined".into()),
            Some("image") => {
                let mime = c.get("mimeType").filter(|m| js::truthy(m)).map(js::string_of).unwrap_or_else(|| "image/png".into());
                let len = c.get("data").and_then(Value::as_str).map(|d| d.encode_utf16().count()).unwrap_or(0);
                format!("[image: {mime}, {} bytes]", js::number((len as f64 * 0.75).round()))
            }
            _ => js::stringify(c),
        })
        .collect();
    let joined = parts.join("\n");
    if joined.is_empty() {
        "(empty)".into()
    } else {
        joined
    }
}

/// A model call to a v0 server's tool (`mcp_<id>_<tool>`), or `None` when
/// the name is not one.
pub async fn exec(run: &Run, name: &str, input: &Value) -> Option<Result<Out, String>> {
    let (server, tool) = parse_mcp_name(name)?;
    let ctx = &run.ctx;
    let Some(r) = row(ctx, "WHERE id=?", &[&server]) else { return Some(Ok(Out::err(format!("mcp server {server} is not configured")))) };
    if !js::truthy(&r["enabled"]) {
        return Some(Ok(Out::err(format!("mcp server {server} is switched off; the user enables it in Plugins"))));
    }
    let (phase, detail) = super::activity::tool_activity(ctx, name, &json!({}));
    run.set_job(&phase, &detail);
    // the thread's bot actually used this server → it joins its integrations
    let _ = botctx::record_tool_use(ctx, &run.turn, &server);
    mark_used(&run.thread_id(), &server);
    Some(
        async {
            let (out, meta) = call_tool(ctx, &server, &tool, input).await?;
            // a Drive file the AI made or changed joins the reply (drive.mjs)
            if server == super::drive::DRIVE_SERVER {
                super::drive::note_drive_call(run, &tool, input, &out);
            }
            // the Google address a call named: routine templates fall back on it
            if out.is_error != Some(true) {
                crate::hub::routines::templates::note_google_email(&ctx.db, input);
            }
            // a server asking to show one of its mini apps: it opens full
            // screen and its card joins the reply
            if let Some(open) = meta.as_ref().and_then(|m| m.get("buwiz/open")).filter(|o| o.get("uri").is_some_and(js::truthy)) {
                if out.is_error != Some(true) {
                    note_app_open(run, &server, open).await;
                }
            }
            Ok(out)
        }
        .await,
    )
}

/// `noteAppOpen(job, row, open)`: the job's `open` and a card on the reply.
async fn note_app_open(run: &Run, server: &str, open: &Value) {
    let uri = js::string_of(&open["uri"]);
    let app = uri.rsplit('/').next().unwrap_or("").to_owned();
    if app.is_empty() {
        return;
    }
    let ctx = &run.ctx;
    let apps = match &ctx.mcp {
        Some(m) => m.resources(server).await,
        None => Vec::new(),
    };
    let res = apps.into_iter().find(|r| r.get("mimeType").and_then(Value::as_str) == Some("text/x-nsh-tsx") && r.get("uri") == Some(&open["uri"]));
    let props = match open.get("props") {
        Some(p @ (Value::Object(_) | Value::Array(_))) => p.clone(),
        _ => json!({}),
    };
    let key = crate::hub::library::attachments::app_key(server, &app, &props);
    let pick = |v: Option<&Value>| v.filter(|x| js::truthy(x)).map(js::string_of);
    let title = pick(open.get("title")).or_else(|| pick(res.as_ref().and_then(|r| r.get("title")))).or_else(|| pick(res.as_ref().and_then(|r| r.get("name")))).unwrap_or_else(|| app.clone());
    let token = open.get("token").and_then(Value::as_str).unwrap_or("").to_owned();
    let enc = crate::hub::library::botctx::encode_uri_component;
    let link = format!("buwiz://app/{server}/{app}?props={}{}", enc(&js::stringify(&props)), if token.is_empty() { String::new() } else { format!("&token={}", enc(&token)) });
    let file_type = pick(res.as_ref().and_then(|r| r.get("description"))).unwrap_or_else(|| "Mini app".into());
    let mut card = json!({"provider": "mcp-app", "id": key, "name": title, "link": link, "action": "open", "kind": "app", "file_type": file_type});
    if !token.is_empty() {
        card["token"] = json!(token);
    }
    ctx.jobs.with(run.job, |j| j.open = Some(json!({"key": key, "title": title})));
    let mut st = run.state();
    let lt = st.link_turn.get_or_insert_with(Default::default);
    lt.links.retain(|l| l.get("id") != Some(&json!(key)));
    lt.links.push(card);
}

// ————— the admin tools (toolMcpAdmin) —————

/// `mcpServerRow({id, name})`: by id, else by name, else by its slug.
fn server_of(ctx: &HubCtx, input: &Value) -> Option<Row> {
    if let Some(id) = input.get("id").filter(|v| js::truthy(v)) {
        return row(ctx, "WHERE id=?", &[&js::string_of(id)]);
    }
    let n = input.get("name").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let n = n.trim_matches(js::is_space).to_owned();
    if n.is_empty() {
        return None;
    }
    let mut slug = String::new();
    let mut gap = false;
    for c in n.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            gap = false;
            slug.push(c);
        } else {
            gap = true;
        }
    }
    let slug: String = slug.chars().take(24).collect();
    row(ctx, "WHERE name=?", &[&n]).or_else(|| row(ctx, "WHERE id=?", &[&slug]))
}

/// `mcp_install` / `mcp_uninstall` / `mcp_enable` / `mcp_disable` / `mcp_list`.
pub async fn admin(run: &Run, tool: &str, input: &Value) -> Result<Out, String> {
    let ctx = &run.ctx;
    if tool == "mcp_list" {
        let all = rows(ctx, "ORDER BY created_at", &[]);
        if all.is_empty() {
            return Ok(Out::ok("no MCP servers configured".into()));
        }
        let lines: Vec<String> = all
            .iter()
            .map(|r| {
                let id = s(r, "id");
                let st = ctx.mcp.as_ref().map(|m| m.status(&id)).unwrap_or_else(|| json!({"status": "idle", "tools": 0}));
                let env: Vec<String> = serde_json::from_str::<Value>(&if js::truthy(&r["env"]) { s(r, "env") } else { "{}".into() })
                    .ok()
                    .and_then(|v| v.as_object().map(|o| js::ordered(o).into_iter().map(|(k, _)| k.clone()).collect()))
                    .unwrap_or_default();
                let state = if js::truthy(&r["enabled"]) {
                    format!("{} ({} tools)", js::string_of(&st["status"]), js::string_of(&st["tools"]))
                } else if js::truthy(&r["agent_pending"]) {
                    "off, waiting for the user to enable it in Plugins".into()
                } else {
                    "disabled".into()
                };
                let scope = if js::truthy(&r["bot"]) { format!("bot {}", s(r, "bot")) } else { "all bots".into() };
                format!(
                    "{id} — {} · {state} · scope: {scope}{}{}",
                    js::string_of(&r["name"]),
                    if env.is_empty() { String::new() } else { format!(" · env: {}", env.join(",")) },
                    st.get("err").filter(|e| js::truthy(e)).map(|e| format!(" · err: {}", js::string_of(e))).unwrap_or_default()
                )
            })
            .collect();
        return Ok(Out::ok(lines.join("\n")));
    }
    if tool == "mcp_install" {
        let bot = ctx.store.read(|s| botctx::bot_of(s, &run.turn).map(|b| b.id.clone()));
        let mut b = Map::new();
        for k in ["name", "command", "args", "env"] {
            if let Some(v) = input.get(k) {
                b.insert(k.into(), v.clone());
            }
        }
        b.insert(
            "bot".into(),
            match (input.get("scope").and_then(Value::as_str), bot) {
                (Some("bot"), Some(id)) => json!(id),
                _ => Value::Null,
            },
        );
        b.insert("byAgent".into(), json!(true));
        let r = crate::hub::routes::mcp_chat::upsert_server(ctx, &Value::Object(b)).await?;
        return Ok(Out::ok(format!(
            "added \"{}\" as server {}, switched off: it has not been started. The user must review and enable it in Plugins before its tools exist; tell them so. You cannot enable it yourself.",
            crate::hub::store::json::template(input.get("name")),
            js::string_of(&r["id"])
        )));
    }
    let Some(r) = server_of(ctx, input) else {
        let who = [input.get("id"), input.get("name")].into_iter().flatten().find(|v| js::truthy(v)).map(js::string_of).unwrap_or_else(|| "?".into());
        return Ok(Out::err(format!("no MCP server matches {who} — mcp_list shows the ids")));
    };
    let (id, name) = (s(&r, "id"), js::string_of(&r["name"]));
    if tool == "mcp_uninstall" {
        if let Some(m) = &ctx.mcp {
            m.invalidate(&id).await;
        }
        ctx.db.with(|c| c.execute("DELETE FROM mcp_servers WHERE id=?", [&id]))?;
        return Ok(Out::ok(format!("removed MCP server {id} ({name}) — its tools are gone")));
    }
    let enabled = tool == "mcp_enable";
    if enabled && js::truthy(&r["agent_pending"]) {
        return Ok(Out::err(format!("{id} ({name}) waits for the user to enable it in Plugins; only they can switch on a server you added")));
    }
    ctx.db.with(|c| c.execute("UPDATE mcp_servers SET enabled=? WHERE id=?", rusqlite::params![enabled as i64, id]))?;
    if let Some(m) = &ctx.mcp {
        m.invalidate(&id).await;
    }
    Ok(Out::ok(format!("{id} ({name}) {}", if enabled { "enabled — its tools return next step" } else { "disabled — its tools are withheld" })))
}

/// Whether `id` is an enabled server's (the name a deferred app has).
pub fn enabled_server(ctx: &HubCtx, id: &str) -> bool {
    ctx.db.with(|c| c.query_row("SELECT 1 FROM mcp_servers WHERE id=? AND enabled=1", [id], |_| Ok(())).optional()).ok().flatten().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_names_parse_as_parse_mcp_name_does() {
        assert_eq!(parse_mcp_name("mcp_many_echo"), Some(("many".into(), "echo".into())));
        assert_eq!(parse_mcp_name("mcp_my-fx_tool_1"), Some(("my-fx".into(), "tool_1".into())));
        assert_eq!(parse_mcp_name("mcp__x"), None);
        assert_eq!(parse_mcp_name("mcp_x_"), None);
        assert_eq!(parse_mcp_name("mcp_X_y"), None);
        assert_eq!(parse_mcp_name("mcp_install"), None);
        assert_eq!(
            anthropic_tools("many", &[json!({"name": "echo", "description": "Echo the arguments", "inputSchema": {"type": "object"}})]),
            vec![json!({"name": "mcp_many_echo", "description": "[many] Echo the arguments", "input_schema": {"type": "object"}})]
        );
        assert_eq!(anthropic_tools("a", &[json!({"name": "t"})])[0]["description"], "[a] t");
        assert_eq!(fold(&json!({"content": []})), "(empty)");
        assert_eq!(fold(&json!({"content": [{"type": "text", "text": "a"}, {"type": "image", "data": "AAAA"}, {"type": "x"}]})), "a\n[image: image/png, 3 bytes]\n{\"type\":\"x\"}");
    }

    #[test]
    fn mcp_app_tools_load_names_each_deferred_app() {
        let d = [json!({"id": "many", "name": "Many", "count": 87, "sample": ["echo", "secret", "tool_1", "tool_2", "tool_3", "tool_4"]})];
        assert_eq!(
            app_tools_load_def(&d),
            json!({
                "name": "app_tools_load",
                "description": "Load a connected app's tools so you can call them (they become callable on your next step and stay loaded for this chat). Load one only when the request needs that app. Apps: many (Many, 87 tools, e.g. echo, secret, tool_1, tool_2, tool_3, tool_4)",
                "input_schema": { "type": "object", "properties": { "app": { "type": "string", "enum": ["many"], "description": "The app id" } }, "required": ["app"] },
            })
        );
    }
}
