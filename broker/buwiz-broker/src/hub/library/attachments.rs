//! `@` tokens attached to a chat or a bot (attachments.mjs): attach (the
//! token resolved through its server, stored, its card posted in the
//! chat), the list with where each is, and the display names. Detach and
//! rename are the leaf's (`leaf/mcp.rs`); `scopeSection` is the tool
//! loop's to call ([`scope_section`]).

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::store::json::{js_slice, template};
use crate::hub::store::write::to_json;
use crate::hub::store::{now_ms, uuidv7, Turn};
use crate::js;
use crate::leaf::mcp::McpRoutes;

/// grammar.mjs `parseToken`: `@<server>:<scope>:<value>` → its parts.
pub fn parse_token(s: &str) -> Option<(String, String, String)> {
    let s = s.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
    let rest = s.strip_prefix('@')?;
    let (server, rest) = rest.split_once(':')?;
    let (scope, value) = rest.split_once(':')?;
    let server_ok = !server.is_empty() && server.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let scope_ok = crate::helper::mcp_servers::is_name(scope);
    let value_ok = !value.is_empty() && !value.chars().any(|c| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
    (server_ok && scope_ok && value_ok).then(|| (server.into(), scope.into(), value.into()))
}

/// Every token in free text, in order (grammar.mjs `tokensIn`): (server, scope).
pub fn tokens_in(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, _) in text.match_indices('@') {
        if i < from {
            continue;
        }
        let rest = &text[i..];
        let end = rest.char_indices().skip(1).find(|(_, c)| js::is_space(*c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')).map(|(k, _)| k).unwrap_or(rest.len());
        // the server and scope parts are tight; the value takes what \S+ takes
        if let Some((server, scope, _)) = parse_token(&rest[..end]) {
            out.push((server, scope));
            from = i + end;
        }
    }
    out
}

/// `appKey(server, app, props)`.
pub fn app_key(server: &str, app: &str, props: &Value) -> String {
    let n = match props {
        Value::Object(o) => o.len(),
        Value::Array(a) => a.len(),
        _ => 0,
    };
    format!("mcp:{server}/{app}{}", if n > 0 { format!("|{}", js::stringify(props)) } else { String::new() })
}

/// `buwiz://app/<server>/<app>?props=<JSON>` → the mini app's key, or "".
pub fn key_of_open(open: &str) -> Result<String, String> {
    let Some(rest) = open.strip_prefix("buwiz://app/") else { return Ok(String::new()) };
    let server: String = rest.chars().take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-').collect();
    if server.is_empty() || !rest[server.len()..].starts_with('/') {
        return Ok(String::new());
    }
    let rest = &rest[server.len() + 1..];
    let app: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).collect();
    if app.is_empty() {
        return Ok(String::new());
    }
    let rest = &rest[app.len()..];
    let props = match rest.strip_prefix("?props=") {
        Some(p) => {
            let raw: String = p.chars().take_while(|c| *c != '&' && !js::is_space(*c) && !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')).collect();
            if raw.is_empty() {
                json!({})
            } else {
                let text = js::decode_uri_component(&raw).map_err(|_| "URI malformed".to_owned())?;
                match serde_json::from_str::<Value>(&text) {
                    Ok(v @ (Value::Object(_) | Value::Array(_))) => v,
                    _ => json!({}),
                }
            }
        }
        None => json!({}),
    };
    Ok(app_key(&server, &app, &props))
}

/// token → its display name.
pub fn names(conn: &Connection) -> std::collections::HashMap<String, Value> {
    let mut m = std::collections::HashMap::new();
    if let Ok(mut st) = conn.prepare("SELECT token,title FROM attachment_names") {
        if let Ok(rows) = st.query_map([], |r| Ok((to_json(r.get(0)?), to_json(r.get(1)?)))) {
            for (k, v) in rows.flatten() {
                m.insert(js::string_of(&k), v);
            }
        }
    }
    m
}

fn row_map(r: &rusqlite::Row) -> rusqlite::Result<Map<String, Value>> {
    let mut m = Map::new();
    for (i, n) in ["id", "server", "token", "scope", "title", "open", "key", "details", "thread", "bot", "turn", "created_at"].iter().enumerate() {
        m.insert((*n).into(), to_json(r.get(i)?));
    }
    Ok(m)
}

const COLS: &str = "id,server,token,scope,title,open,key,details,thread,bot,turn,created_at";

/// An attachment as the endpoints hand it out.
fn json_of(r: &Map<String, Value>, renamed: &std::collections::HashMap<String, Value>) -> Value {
    let g = |k: &str| r.get(k).cloned().unwrap_or(Value::Null);
    let title = renamed.get(&js::string_of(&g("token"))).filter(|t| js::truthy(t)).cloned().unwrap_or_else(|| g("title"));
    let details = g("details").as_str().and_then(|d| serde_json::from_str(d).ok()).unwrap_or(Value::Null);
    let or = |k: &str, d: Value| if js::truthy(&g(k)) { g(k) } else { d };
    json!({
        "id": g("id"),
        "server": g("server"),
        "token": g("token"),
        "scope": g("scope"),
        "title": title,
        "resolved_title": g("title"),
        "open": g("open"),
        "key": or("key", json!("")),
        "details": details,
        "thread": g("thread"),
        "bot": or("bot", Value::Null),
        "turn": or("turn", json!("")),
        "created_at": g("created_at"),
    })
}

fn rows(conn: &Connection, sql: &str, args: &[String]) -> rusqlite::Result<Vec<Map<String, Value>>> {
    let mut st = conn.prepare(sql)?;
    let out = st.query_map(rusqlite::params_from_iter(args.iter()), row_map)?.collect();
    out
}

/// `attachmentsOf(threadId)`.
pub fn attachments_of(conn: &Connection, thread: &str) -> Vec<Value> {
    if thread.is_empty() {
        return Vec::new();
    }
    let renamed = names(conn);
    rows(conn, &format!("SELECT {COLS} FROM attachments WHERE thread=? ORDER BY created_at"), &[thread.to_owned()]).unwrap_or_default().iter().map(|r| json_of(r, &renamed)).collect()
}

/// `GET /mcp/attachments?thread=|bot=|token=`: each with its chat's title
/// and its bot's name.
pub fn list(ctx: &HubCtx, thread: Option<String>, bot: Option<String>, token: Option<String>) -> Result<Vec<Value>, String> {
    let mut wh = Vec::new();
    let mut args = Vec::new();
    for (col, v) in [("thread", thread), ("bot", bot), ("token", token)] {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            wh.push(format!("{col}=?"));
            args.push(v);
        }
    }
    let sql = format!("SELECT {COLS} FROM attachments{} ORDER BY created_at", if wh.is_empty() { String::new() } else { format!(" WHERE {}", wh.join(" AND ")) });
    let (found, renamed) = ctx.db.with(|c| Ok((rows(c, &sql, &args)?, names(c))))?;
    Ok(ctx.store.read(|s| {
        found
            .iter()
            .map(|r| {
                let mut a = json_of(r, &renamed);
                let thread = a["thread"].clone();
                let bot = a["bot"].clone();
                let title = s.list.iter().find(|t| Some(t.id.as_str()) == thread.as_str()).map(|t| t.title.clone()).filter(|t| !t.is_empty()).unwrap_or_default();
                let name = s.bots.iter().find(|b| Some(b.id.as_str()) == bot.as_str()).map(|b| b.name.clone()).filter(|n| !n.is_empty()).unwrap_or_default();
                if let Some(o) = a.as_object_mut() {
                    o.insert("threadTitle".into(), json!(title));
                    o.insert("botName".into(), json!(name));
                }
                a
            })
            .collect()
    }))
}

/// `scopesOf(manifest)`.
fn scopes_of(m: &Value) -> Vec<Value> {
    m.get("scopes").and_then(Value::as_array).map(|a| a.iter().filter(|x| x.get("name").is_some_and(Value::is_string)).cloned().collect()).unwrap_or_default()
}

fn enabled_server(conn: &Connection, id: &str) -> rusqlite::Result<Option<(String, Value)>> {
    conn.query_row("SELECT id, name FROM mcp_servers WHERE id=? AND enabled=1", [id], |r| Ok((js::string_of(&to_json(r.get(0)?)), to_json(r.get(1)?)))).optional()
}

/// `POST /mcp/attach {server, token, thread | bot}`: `{attachment, existing}`.
pub async fn attach(ctx: &HubCtx, body: &Value) -> Result<Value, String> {
    let Some(mcp) = ctx.mcp.clone() else { return Err("the MCP servers are not hosted here".into()) };
    let thread = body.get("thread").filter(|v| !v.is_null());
    let bot = body.get("bot").filter(|v| js::truthy(v));
    let found = ctx.store.read(|s| -> Result<String, String> {
        let mut t = thread.filter(|v| js::truthy(v)).and_then(|v| s.thread(&js::string_of(v))).map(|t| t.id.clone());
        if t.is_none() {
            if let Some(b) = bot {
                let Some(b) = s.bots.iter().find(|x| Some(x.id.as_str()) == b.as_str()) else { return Err(format!("unknown bot {}", js::string_of(b))) };
                t = b.thread_id.as_deref().and_then(|id| s.thread(id)).map(|t| t.id.clone());
                if t.is_none() {
                    return Err(format!("{} has no chat", b.name));
                }
            }
        }
        t.ok_or_else(|| format!("unknown chat {}", thread.map(js::string_of).unwrap_or_default()))
    })?;
    let token_v = body.get("token");
    let token = token_v.filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    let Some((tok_server, scope_name, value)) = parse_token(&token) else { return Err(format!("not a token: {}", template(token_v))) };
    let server = body.get("server").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or(tok_server);
    let same = |tk: &str| -> Result<Option<Value>, String> {
        let r = ctx.db.with(|c| Ok((rows(c, &format!("SELECT {COLS} FROM attachments WHERE server=? AND token=? AND thread=?"), &[server.clone(), tk.to_owned(), found.clone()])?, names(c))))?;
        Ok(r.0.first().map(|row| json_of(row, &r.1)))
    };
    let trimmed = token.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')).to_owned();
    if let Some(had) = same(&trimmed)? {
        return Ok(json!({"attachment": had, "existing": true}));
    }
    if ctx.db.with(|c| enabled_server(c, &server))?.is_none() {
        return Err(format!("no such server {server}"));
    }
    let manifest = mcp.manifest(&server).await;
    let scope = scopes_of(&manifest).into_iter().find(|s| s.get("name").and_then(Value::as_str) == Some(scope_name.as_str()));
    let Some(scope) = scope.filter(|s| s.get("attach").is_some_and(js::truthy)) else { return Err(format!("@{server}:{scope_name} tokens can't be attached")) };
    let routes = McpRoutes::new(mcp.clone(), Vec::new());
    let r = routes.resolve(&json!({"server": server, "name": scope_name, "args": value, "thread": found})).await;
    if !r.get("ok").is_some_and(js::truthy) {
        let e = r.get("error").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_else(|| "couldn't resolve the token".into());
        return Err(e);
    }
    let canonical = r.get("token").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| token.clone());
    let canonical = canonical.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')).to_owned();
    if let Some(dup) = same(&canonical)? {
        return Ok(json!({"attachment": dup, "existing": true}));
    }
    let open = r.get("open").filter(|o| js::truthy(o)).map(js::string_of).unwrap_or_default();
    let key = key_of_open(&open)?;
    let title = js_slice(&r.get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| canonical.clone()), 120);
    let details = r.get("details").filter(|d| d.is_object() || d.is_array()).map(js::stringify);
    let id = uuidv7();
    let bot_id = ctx.store.read(|s| s.thread_bot(&found).map(|b| b.id.clone()));
    let turn = uuidv7();
    let at = now_ms();
    let file_type = scope.get("description").filter(|d| js::truthy(d)).cloned().unwrap_or(json!("Attached"));
    let link = json!({
        "provider": "mcp-app", "id": if key.is_empty() { open.clone() } else { key.clone() }, "name": title, "link": open, "action": "attached",
        "kind": "app", "file_type": file_type, "token": canonical, "attachment": id, "at": at,
    });
    let card = Turn { role: "ai".into(), id: Some(turn.clone()), text: Some(String::new()), label: Some(String::new()), links: Some(json!([link])), ..Default::default() };
    ctx.store.touch(|s| {
        if let Some(t) = s.thread_mut(&found) {
            t.turns.push(card);
        }
    });
    let title_sql = crate::hub::store::json::sql_text(&title);
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO attachments(id,server,token,scope,title,open,key,details,thread,bot,turn,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            rusqlite::params![id, server, canonical, scope_name, title_sql, open, key, details, found, bot_id, turn, at],
        )
    })?;
    ctx.store.write(|_| ())?;
    ctx.store.write_index();
    let made = ctx.db.with(|c| Ok((rows(c, &format!("SELECT {COLS} FROM attachments WHERE id=?"), &[id.clone()])?, names(c))))?;
    Ok(json!({"attachment": made.0.first().map(|r| json_of(r, &made.1)).unwrap_or(Value::Null), "existing": false}))
}

/// `renamedKeys()`: a pin's display name, by the app key its token opens.
pub fn renamed_keys(conn: &Connection) -> std::collections::HashMap<String, Value> {
    let mut m = std::collections::HashMap::new();
    if let Ok(mut st) = conn.prepare("SELECT a.key AS key, n.title AS title FROM attachments a JOIN attachment_names n ON n.token=a.token WHERE a.key<>''") {
        if let Ok(rows) = st.query_map([], |r| Ok((to_json(r.get(0)?), to_json(r.get(1)?)))) {
            for (k, v) in rows.flatten() {
                m.insert(js::string_of(&k), v);
            }
        }
    }
    m
}

/// `scopeSection(thread, prompt)`: the system prompt's "Scope of this
/// chat", or "" when nothing is attached or named.
pub async fn scope_section(ctx: &HubCtx, thread: &str, prompt: &str) -> String {
    let atts = ctx.db.with(|c| Ok(attachments_of(c, thread))).unwrap_or_default();
    let mut used: Vec<(String, Vec<String>)> = Vec::new();
    let mut note = |server: &str, scope: &str| match used.iter_mut().find(|(s, _)| s == server) {
        Some((_, scopes)) => {
            if !scopes.iter().any(|x| x == scope) {
                scopes.push(scope.into());
            }
        }
        None => used.push((server.into(), vec![scope.into()])),
    };
    for a in &atts {
        note(&js::string_of(&a["server"]), &js::string_of(&a["scope"]));
    }
    for (server, scope) in tokens_in(prompt) {
        note(&server, &scope);
    }
    if used.is_empty() {
        return String::new();
    }
    let (mut lines, mut hints) = (Vec::new(), Vec::new());
    for (server, scopes) in &used {
        let row = ctx.db.with(|c| enabled_server(c, server)).ok().flatten();
        let m = match (&row, &ctx.mcp) {
            (Some(_), Some(mcp)) => mcp.manifest(server).await,
            _ => json!({}),
        };
        let title =
            m.get("title").filter(|t| js::truthy(t)).map(js::string_of).or_else(|| row.as_ref().map(|r| r.1.clone()).filter(js::truthy).map(|n| js::string_of(&n))).unwrap_or_else(|| server.clone());
        let defs = scopes_of(&m);
        for a in atts.iter().filter(|a| a["server"].as_str() == Some(server.as_str())) {
            let desc = defs.iter().find(|s| s.get("name") == a.get("scope")).and_then(|s| s.get("description")).filter(|d| js::truthy(d)).map(js::string_of).unwrap_or_default();
            let noun = desc.trim().split(|c: char| c.is_whitespace()).next().map(str::to_lowercase).filter(|n| !n.is_empty()).unwrap_or_else(|| "item".into());
            lines.push(format!("Attached: {} ({}) — {title} tools default to this {noun} here", js::string_of(&a["token"]), js::string_of(&a["title"])));
        }
        for s in &defs {
            let name = s.get("name").and_then(Value::as_str).unwrap_or_default();
            if let Some(h) = s.get("hint").and_then(Value::as_str).map(str::trim).filter(|h| !h.is_empty()) {
                if scopes.iter().any(|x| x == name) {
                    hints.push(h.to_owned());
                }
            }
        }
    }
    lines.extend(hints);
    format!("\n\n## Scope of this chat\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_tokens_parse_as_grammar_mjs_does() {
        assert_eq!(parse_token(" @apps:item:42 "), Some(("apps".into(), "item".into(), "42".into())));
        assert_eq!(parse_token("@apps:item:a:b/c"), Some(("apps".into(), "item".into(), "a:b/c".into())));
        assert_eq!(parse_token("apps:item"), None);
        assert_eq!(parse_token("@Apps:item:1"), None);
        assert_eq!(parse_token("@apps:item:"), None);
        assert_eq!(parse_token("@apps:item:a b"), None);
        assert_eq!(tokens_in("see @a:b:1 and @c:d:2."), vec![("a".into(), "b".into()), ("c".into(), "d".into())]);
        assert_eq!(tokens_in("@a:b:1,@c:d:2"), vec![("a".into(), "b".into())]);
        assert_eq!(key_of_open("buwiz://app/apps/hello").unwrap(), "mcp:apps/hello");
        assert_eq!(key_of_open("buwiz://app/apps/hello?props=%7B%22a%22%3A1%7D").unwrap(), "mcp:apps/hello|{\"a\":1}");
        assert_eq!(key_of_open("https://x").unwrap(), "");
    }
}
