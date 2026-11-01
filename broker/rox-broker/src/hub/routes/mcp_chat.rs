//! The chat's MCP routes (5.2): `POST /mcp/servers` (routes/market.mjs and
//! tools.mjs `mcpUpsertServer`), `POST /mcp/attach` and `GET
//! /mcp/attachments` (attachments.mjs). The servers are McpHost's
//! (`ctx.mcp`, `BUWIZ_BROKER_MCP_HOST=1`); without it the two that need a
//! server answer 501.

use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::chat::{answer, ok, parse, ready, refused, thrown};
use crate::hub::ctx::HubCtx;
use crate::hub::library::attachments;
use crate::hub::store::json::{js_slice, sql_text};
use crate::hub::store::now_ms;
use crate::hub::store::write::to_json;
use crate::hub::types::{not_ported, HubRequest, HubResponse, Pattern::Exact as X, Verb};
use crate::hub::Registry;
use crate::js;

pub fn install(routes: &mut Registry) {
    routes.route(Verb::Post, X("/mcp/servers"), servers);
    routes.route(Verb::Any, X("/mcp/attach"), attach);
    routes.route(Verb::Get, X("/mcp/attachments"), list);
}

async fn servers(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    if ctx.mcp.is_none() {
        return not_ported();
    }
    let r = match parse(&req.body) {
        Ok(b) => upsert_server(&ctx, &b).await,
        Err(e) => Err(e),
    };
    match r {
        Ok(v) => {
            let mut o = json!({"ok": true});
            if let (Some(o), Some(v)) = (o.as_object_mut(), v.as_object()) {
                o.extend(v.clone());
            }
            ok(&o)
        }
        Err(e) => refused(&e, true),
    }
}

async fn attach(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    if req.method != hyper::Method::POST {
        return answer(404, "not found".into(), false);
    }
    if ctx.mcp.is_none() {
        return not_ported();
    }
    let r = match parse(&req.body) {
        Ok(b) => attachments::attach(&ctx, &b).await,
        Err(e) => Err(e),
    };
    match r {
        Ok(v) => {
            let mut o = json!({"ok": true});
            if let (Some(o), Some(v)) = (o.as_object_mut(), v.as_object()) {
                o.extend(v.clone());
            }
            ok(&o)
        }
        Err(e) => refused(&e, true),
    }
}

async fn list(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    match attachments::list(&ctx, req.query("thread"), req.query("bot"), req.query("token")) {
        Ok(a) => ok(&json!({"attachments": a})),
        Err(e) => thrown(&e),
    }
}

/// grammar.mjs `isServerId`.
fn is_server_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `mcpName(id, tool)`: `mcp_<id>_<tool>`, other characters as `_`, 64 units.
pub fn mcp_name(id: &str, tool: &str) -> String {
    let s: String = format!("mcp_{id}_{tool}").chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c.to_string() } else { "_".repeat(c.len_utf16()) }).collect();
    js_slice(&s, 64)
}

/// `name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24)`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out.chars().take(24).collect()
}

type Row = Map<String, Value>;

fn server_row(ctx: &HubCtx, id: &str) -> Result<Option<Row>, String> {
    ctx.db.with(|c| {
        let mut st = c.prepare("SELECT * FROM mcp_servers WHERE id=?")?;
        let names: Vec<String> = st.column_names().into_iter().map(str::to_owned).collect();
        st.query_row([id], |r| {
            let mut m = Map::new();
            for (i, n) in names.iter().enumerate() {
                m.insert(n.clone(), to_json(r.get(i)?));
            }
            Ok(m)
        })
        .optional()
    })
}

/// tools.mjs `mcpUpsertServer`: add or edit a server; a field left out
/// keeps its stored value; an enabled one is probed now.
pub async fn upsert_server(ctx: &HubCtx, b: &Value) -> Result<Value, String> {
    let mcp = ctx.mcp.clone().ok_or("the MCP servers are not hosted here")?;
    let given = |k: &str| b.get(k).filter(|v| !v.is_null());
    let id_v = b.get("id").filter(|v| js::truthy(v));
    let id = id_v.map(js::string_of);
    let cur = match &id {
        Some(id) => server_row(ctx, id)?,
        None => None,
    };
    let cur_s = |k: &str| cur.as_ref().and_then(|c| c.get(k)).filter(|v| !v.is_null()).cloned();
    let pick = |k: &str| given(k).cloned().or_else(|| cur_s(k)).map(|v| js::string_of(&v)).unwrap_or_default().trim_matches(js::is_space).to_owned();
    let (name, url, cmd) = (pick("name"), pick("url"), pick("command"));
    if name.is_empty() {
        return Err("server needs a name".into());
    }
    if cmd.is_empty() && url.is_empty() {
        return Err("server needs a command or url".into());
    }
    if let (Some(id), None) = (&id, &cur) {
        if !is_server_id(id) {
            return Err(format!("server id {} must be [a-z0-9-]+", js_slice(id, 40)));
        }
    }
    let sid = match &id {
        Some(id) => id.clone(),
        None => {
            let base = match slug(&name) {
                s if s.is_empty() => "srv".to_owned(),
                s => s,
            };
            let mut s = base.clone();
            let mut i = 2;
            while server_row(ctx, &s)?.is_some() {
                s = format!("{base}-{i}");
                i += 1;
            }
            s
        }
    };
    let args: Value = match b.get("args") {
        None => cur_s("args").and_then(|a| serde_json::from_str(&js::string_of(&a)).ok()).unwrap_or(json!([])),
        Some(Value::Array(a)) => Value::Array(a.iter().map(|x| json!(js::string_of(x))).collect()),
        Some(v) => {
            let s = if js::truthy(v) { js::string_of(v) } else { String::new() };
            Value::Array(s.split(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')).filter(|x| !x.is_empty()).map(|x| json!(x)).collect())
        }
    };
    let env: Value = match b.get("env") {
        None => cur_s("env").and_then(|e| serde_json::from_str(&js::string_of(&e)).ok()).unwrap_or(json!({})),
        Some(v @ (Value::Object(_) | Value::Array(_))) => v.clone(),
        Some(v) => {
            let s = if js::truthy(v) { js::string_of(v) } else { String::new() };
            let mut m = Map::new();
            for line in s.split('\n') {
                let Some((k, val)) = line.split_once('=') else { continue };
                let key_ok = k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if key_ok && !val.contains('\r') {
                    m.insert(k.into(), json!(val));
                }
            }
            Value::Object(m)
        }
    };
    let bot: Option<String> = match b.get("bot") {
        None => cur_s("bot").map(|v| js::string_of(&v)),
        Some(v) if js::truthy(v) => v.as_str().filter(|id| ctx.store.read(|s| s.bot(id).is_some())).map(str::to_owned),
        Some(_) => None,
    };
    let keep = |k: &str| -> Option<String> {
        match b.get(k) {
            None => cur_s(k).map(|v| js::string_of(&v)),
            Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(v) => Some(js::stringify(v)),
        }
    };
    let by_agent = b.get("byAgent").is_some_and(js::truthy);
    let probe = b.get("probe").is_none_or(js::truthy);
    let cur_text = |k: &str| {
        cur_s(k).map(|v| match v {
            Value::String(s) => rusqlite::types::Value::Text(s),
            Value::Number(n) => n.as_i64().map(rusqlite::types::Value::Integer).unwrap_or_else(|| rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))),
            other => rusqlite::types::Value::Text(js::stringify(&other)),
        })
    };
    let pending: rusqlite::types::Value = if cur.is_some() {
        cur_text("agent_pending").unwrap_or(rusqlite::types::Value::Null)
    } else if by_agent {
        rusqlite::types::Value::Integer(1)
    } else {
        rusqlite::types::Value::Null
    };
    let null = rusqlite::types::Value::Null;
    ctx.db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO mcp_servers(id,name,command,args,env,enabled,bot,created_at,url,headers,oauth_client,transport,auth,account,disabled_tools,agent_pending) VALUES(?,?,?,?,?,COALESCE((SELECT enabled FROM mcp_servers WHERE id=?),?),?,COALESCE((SELECT created_at FROM mcp_servers WHERE id=?),?),?,?,?,?,?,?,?,?)",
            rusqlite::params![
                sid,
                sql_text(&js_slice(&name, 60)),
                cmd,
                js::stringify(&args),
                js::stringify(&env),
                sid,
                if by_agent { 0 } else { 1 },
                bot,
                sid,
                now_ms(),
                if url.is_empty() { None } else { Some(url.clone()) },
                keep("headers"),
                keep("oauth_client"),
                keep("transport"),
                cur_text("auth").unwrap_or(null.clone()),
                cur_text("account").unwrap_or(null.clone()),
                cur_text("disabled_tools").unwrap_or(null.clone()),
                pending,
            ],
        )
    })?;
    mcp.invalidate(&sid).await;
    let row = server_row(ctx, &sid)?.unwrap_or_default();
    if !row.get("enabled").is_some_and(js::truthy) {
        return Ok(json!({"id": sid, "status": "off", "tools": 0, "toolNames": [], "err": null}));
    }
    if !probe {
        return Ok(json!({"id": sid, "status": "idle", "tools": 0, "toolNames": [], "err": null}));
    }
    let rec = mcp.ensure(&sid).await;
    let st = mcp.status(&sid);
    let names: Vec<Value> =
        rec.get("tools").and_then(Value::as_array).map(|ts| ts.iter().map(|t| json!(mcp_name(&sid, &t.get("name").map(js::string_of).unwrap_or_default()))).collect()).unwrap_or_default();
    Ok(json!({
        "id": sid,
        "status": if rec.get("ok") == Some(&Value::Bool(true)) { "connected" } else { "error" },
        "tools": st.get("tools").cloned().unwrap_or(json!(0)),
        "toolNames": names,
        "err": st.get("err").filter(|e| js::truthy(e)).cloned().unwrap_or(Value::Null),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_chat_names_and_slugs() {
        assert_eq!(mcp_name("my-fixture", "echo"), "mcp_my-fixture_echo");
        assert_eq!(mcp_name("a", "b.c d"), "mcp_a_b_c_d");
        assert_eq!(slug("My Fixture!"), "my-fixture");
        assert_eq!(slug("!!"), "");
    }
}
