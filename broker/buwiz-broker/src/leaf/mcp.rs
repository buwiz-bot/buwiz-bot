//! The `/mcp/*` routes Rox calls (plan 4.3), answered here when this broker
//! hosts the MCP servers (`BUWIZ_BROKER_MCP_HOST=1`), as routes/market.mjs
//! answered them: the same shapes, status codes and texts. Three stay
//! Node's until the chats move (phase 5), since they touch its in-memory
//! chats: `POST /mcp/attach`, `GET /mcp/attachments` and `POST
//! /mcp/servers`.
//!
//! The servers themselves are McpHost's (helper/mcp_servers.rs); its status
//! is what `/mcp/servers` shows and Node mirrors. `/mcp/open` hands only
//! Rox links to the system (`open`, from PATH), an app link with a one-time
//! pass that `/mcp/open/pass` redeems (open-pass.mjs). `/oauth/callback/`
//! also finishes a mini app account's sign-in (§20.2) when the plugin
//! host started it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::ORIGIN;
use hyper::{Method, Request, Response};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{parse_body, respond, Answer};
use crate::helper::mcp_servers::McpHost;
use crate::helper::plugins::Host;
use crate::js;
use crate::proxy::Body;

const PASS_FOR: Duration = Duration::from_millis(15_000);
const APP_MIME: &str = "text/x-nsh-tsx";
/// `callToolJson`'s timeout (a mini app's call).
const APP_CALL: Duration = Duration::from_secs(120);

/// The routes this module answers, as market.mjs matched them.
pub fn route(method: &Method, url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url);
    let get = method == Method::GET;
    let post = method == Method::POST;
    match path {
        "/mcp/servers" => get,
        "/mcp/apps" | "/mcp/manifest" => get,
        "/mcp/server/state" | "/mcp/server/delete" | "/mcp/tools/state" | "/mcp/call" | "/mcp/resolve" | "/mcp/detach" | "/mcp/attachment/rename" | "/mcp/auth"
        | "/mcp/account" | "/mcp/open" | "/mcp/open/pass" => post,
        _ => (get && url.starts_with("/mcp/tools") && !url.starts_with("/mcp/tools/state")) || (get && url.starts_with("/oauth/callback/")),
    }
}

pub struct McpRoutes {
    mcp: Arc<McpHost>,
    allowed: Vec<String>,
    passes: Mutex<HashMap<String, Instant>>,
    plugins: Option<Arc<Host>>,
}

/// A body field as JS's `String(x || "")`.
fn text(b: &Value, k: &str) -> String {
    b.get(k).filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()
}

fn ok() -> Answer {
    Answer::json(200, r#"{"ok":true}"#)
}

fn out(status: u16, v: &Value) -> Answer {
    Answer::json(status, js::stringify(v))
}

/// routes/market.mjs's done page.
fn done_page(title: &str) -> String {
    format!(
        "<body style=\"background:#1f1f1f;color:#f5f5f5;font:15px -apple-system,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0\"><div style=\"text-align:center\"><div style=\"font-size:22px;font-weight:600\">{title}</div><div style=\"color:#f5f5f58f;margin-top:8px\">You can close this window.</div></div></body>"
    )
}

fn html(status: u16, title: &str) -> Answer {
    Answer::bare(status, done_page(title)).header("content-type", "text/html")
}

/// `buwiz://app/<server>/<app>[?…]`.
fn app_link(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("buwiz://app/") else { return false };
    let (path, query) = rest.split_once('?').map_or((rest, None), |(p, q)| (p, Some(q)));
    let Some((server, app)) = path.split_once('/') else { return false };
    let server_ok = !server.is_empty() && server.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let app_ok = !app.is_empty() && app.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    server_ok && app_ok && query.is_none_or(|q| !q.chars().any(char::is_whitespace))
}

/// `buwiz://view?path=<absolute path>`: the path, when it is absolute.
fn view_path(url: &str) -> Option<String> {
    let q = url.strip_prefix("buwiz://view?")?;
    if q.chars().any(char::is_whitespace) {
        return None;
    }
    let u = url::Url::parse(url).ok()?;
    let path = u.query_pairs().find(|(k, _)| k == "path").map(|(_, v)| v.into_owned())?;
    path.starts_with('/').then_some(path)
}

fn chat_link(url: &str) -> bool {
    url.strip_prefix("buwiz://chat/new?draft=").is_some_and(|r| !r.chars().any(char::is_whitespace))
}

/// The tool's data as `callToolJson` hands it to a mini app.
fn tool_json(result: &Value) -> Value {
    let text: Vec<String> = result
        .get("content")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter(|x| x.get("type") == Some(&json!("text"))).map(|x| js::string_of(x.get("text").unwrap_or(&Value::Null))).collect())
        .unwrap_or_default();
    let text = text.join("\n");
    if result.get("isError").is_some_and(js::truthy) {
        return json!({"ok": false, "error": if text.is_empty() { "tool failed".to_owned() } else { text }});
    }
    if let Some(sc) = result.get("structuredContent") {
        if let Some(o) = sc.as_object() {
            if o.len() == 1 {
                if let Some(r) = o.get("result") {
                    return json!({"ok": true, "result": r});
                }
            }
        }
        return json!({"ok": true, "result": sc});
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(_) => json!({"ok": true, "result": text}),
    }
}

/// `appOfUri`: the last path segment.
fn app_of(uri: &str) -> String {
    uri.rsplit('/').next().unwrap_or_default().to_owned()
}

impl McpRoutes {
    pub fn new(mcp: Arc<McpHost>, allowed: Vec<String>) -> Self {
        Self { mcp, allowed, passes: Mutex::new(HashMap::new()), plugins: None }
    }

    /// The plugin host whose account sign-ins come back here.
    pub fn with_plugins(mut self, host: Arc<Host>) -> Self {
        self.plugins = Some(host);
        self
    }

    pub async fn serve(&self, req: Request<Incoming>) -> Response<Body> {
        let origin = req.headers().get(ORIGIN).cloned();
        if let Some((status, error)) = crate::guard::refusal(req.method(), req.headers(), &self.allowed) {
            return respond(Answer::refused(status.as_u16(), true, &error), None);
        }
        let method = req.method().clone();
        let url = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/").to_owned();
        let body = match req.into_body().collect().await {
            Ok(b) => String::from_utf8_lossy(&b.to_bytes()).into_owned(),
            Err(_) => String::new(),
        };
        let answer = self.answer(&method, &url, &body).await;
        respond(answer, origin)
    }

    async fn answer(&self, method: &Method, url: &str, body: &str) -> Answer {
        let path = url.split('?').next().unwrap_or(url);
        let query = |k: &str| url::form_urlencoded::parse(url.split_once('?').map(|(_, q)| q).unwrap_or("").as_bytes()).find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
        if method == Method::GET {
            return match path {
                "/mcp/servers" => self.servers(),
                "/mcp/apps" => self.apps().await,
                "/mcp/manifest" => self.manifest().await,
                p if p.starts_with("/oauth/callback/") => self.callback(url).await,
                _ => self.tools(&query("id").unwrap_or_default()).await,
            };
        }
        let parsed = parse_body(body);
        let b = parsed.clone().unwrap_or_else(|_| json!({}));
        match path {
            "/mcp/server/state" => self.state(parsed).await,
            "/mcp/server/delete" => {
                let Ok(b) = parsed else { return Answer::refused(400, true, &parsed.err().unwrap_or_default()) };
                let id = text(&b, "id");
                self.mcp.invalidate(&id).await;
                match self.mcp.db(|db| db.execute("DELETE FROM mcp_servers WHERE id=?", [&id])) {
                    Ok(_) => ok(),
                    Err(e) => Answer::refused(400, true, &e),
                }
            }
            "/mcp/tools/state" => self.tool_state(parsed),
            "/mcp/call" => match parsed {
                Ok(b) => out(200, &self.app_call(&text(&b, "server"), &text(&b, "tool"), b.get("args").filter(|a| js::truthy(a)).cloned().unwrap_or(json!({}))).await),
                Err(e) => out(200, &json!({"ok": false, "error": e})),
            },
            "/mcp/resolve" => match parsed {
                Ok(b) => out(200, &self.resolve(&b).await),
                Err(e) => out(200, &json!({"ok": false, "error": e})),
            },
            "/mcp/detach" => {
                let Ok(b) = parsed else { return Answer::refused(400, true, &parsed.err().unwrap_or_default()) };
                let id = text(&b, "id");
                let id = id.strip_prefix("attach:").unwrap_or(&id).to_owned();
                match self.mcp.db(|db| db.execute("DELETE FROM attachments WHERE id=?", [&id])) {
                    Ok(0) => Answer::refused(400, true, &format!("no attachment {id}")),
                    Ok(_) => ok(),
                    Err(e) => Answer::refused(400, true, &e),
                }
            }
            "/mcp/attachment/rename" => {
                let Ok(b) = parsed else { return Answer::refused(400, true, &parsed.err().unwrap_or_default()) };
                let token = text(&b, "token").trim().to_owned();
                if token.is_empty() {
                    return Answer::refused(400, true, "rename needs a token");
                }
                let title = b.get("title").filter(|t| !t.is_null()).map(js::string_of).unwrap_or_default();
                let title = js::slice_utf16(title.trim(), 120);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                let r = self.mcp.db(|db| {
                    if title.is_empty() {
                        db.execute("DELETE FROM attachment_names WHERE token=?", [&token])
                    } else {
                        db.execute("INSERT OR REPLACE INTO attachment_names(token,title,updated_at) VALUES(?,?,?)", rusqlite::params![token, title, now])
                    }
                });
                match r {
                    Ok(_) => ok(),
                    Err(e) => Answer::refused(400, true, &e),
                }
            }
            "/mcp/auth" => {
                let Ok(b) = parsed else { return Answer::refused(400, true, &parsed.err().unwrap_or_default()) };
                let id = text(&b, "id");
                if !self.exists(&id) {
                    return Answer::refused(400, true, "no such server");
                }
                let r = self.mcp.auth_begin(&id, b.get("open") != Some(&Value::Bool(false))).await;
                if r.get("ok") == Some(&Value::Bool(false)) {
                    return out(400, &json!({"ok": false, "error": r.get("error").cloned().unwrap_or(Value::Null)}));
                }
                out(200, &r)
            }
            "/mcp/account" => self.account(&b).await,
            "/mcp/open" => self.open(&text(&b, "url")).await,
            "/mcp/open/pass" => out(200, &json!({"ok": self.redeem(&text(&b, "pass"))})),
            _ => Answer::json(404, r#"{"error":"not found"}"#),
        }
    }

    fn exists(&self, id: &str) -> bool {
        self.mcp.db(|db| db.query_row("SELECT 1 FROM mcp_servers WHERE id=?", [id], |_| Ok(())).optional()).ok().flatten().is_some()
    }

    /// `GET /mcp/servers`: each server with its live status (env values
    /// never echoed).
    fn servers(&self) -> Answer {
        type Cols = (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, Option<String>);
        let rows: Result<Vec<Cols>, String> = self.mcp.db(|db| {
            let mut st = db.prepare("SELECT id, name, command, url, args, env, enabled, bot FROM mcp_servers ORDER BY created_at")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?;
            rows.collect()
        });
        let rows = match rows {
            Ok(r) => r,
            Err(e) => return Answer::thrown(&e),
        };
        let servers: Vec<Value> = rows
            .into_iter()
            .map(|(id, name, command, url, args, env, enabled, bot)| {
                let st = self.mcp.status(&id);
                let args: Value = serde_json::from_str(args.as_deref().filter(|a| !a.is_empty()).unwrap_or("[]")).unwrap_or(json!([]));
                let env: Value = serde_json::from_str(env.as_deref().filter(|e| !e.is_empty()).unwrap_or("{}")).unwrap_or(json!({}));
                let env_keys: Vec<String> = env.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
                json!({
                    "id": id,
                    "name": name,
                    "command": command,
                    "url": url.filter(|u| !u.is_empty()),
                    "args": args,
                    "envKeys": env_keys,
                    "enabled": enabled != 0,
                    "bot": bot,
                    "status": if enabled != 0 { st["status"].clone() } else { json!("off") },
                    "tools": st["tools"].clone(),
                    "err": st.get("err").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        out(200, &json!({"servers": servers}))
    }

    /// The enabled servers' ids, in creation order.
    fn enabled(&self) -> Vec<(String, String)> {
        self.mcp
            .db(|db| {
                let mut st = db.prepare("SELECT id, name FROM mcp_servers WHERE enabled=1")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
                rows.collect()
            })
            .unwrap_or_default()
    }

    fn app_resources(list: &[Value]) -> Vec<Value> {
        list.iter().filter(|r| r.get("mimeType") == Some(&json!(APP_MIME))).cloned().collect()
    }

    /// `GET /mcp/apps`: the mini apps every enabled server ships.
    async fn apps(&self) -> Answer {
        let mut set = tokio::task::JoinSet::new();
        let rows = self.enabled();
        for (i, (id, _)) in rows.iter().enumerate() {
            let (mcp, id) = (self.mcp.clone(), id.clone());
            set.spawn(async move { (i, id.clone(), Self::app_resources(&mcp.resources(&id).await)) });
        }
        let mut got: Vec<(usize, String, Vec<Value>)> = set.join_all().await;
        got.sort_by_key(|g| g.0);
        let apps: Vec<Value> = got
            .into_iter()
            .flat_map(|(_, server, list)| {
                list.into_iter().map(move |r| {
                    let uri = r.get("uri").map(js::string_of).unwrap_or_default();
                    json!({
                        "server": server,
                        "app": app_of(&uri),
                        "uri": uri,
                        "title": first_truthy(&r, &["title", "name"]),
                        "description": first_truthy(&r, &["description"]),
                    })
                })
            })
            .collect();
        out(200, &json!({"apps": apps}))
    }

    /// `GET /mcp/manifest`: each enabled server's apps, commands and
    /// scopes; one with none of them is left out.
    async fn manifest(&self) -> Answer {
        let mut set = tokio::task::JoinSet::new();
        for (i, (id, name)) in self.enabled().into_iter().enumerate() {
            let mcp = self.mcp.clone();
            set.spawn(async move {
                let apps = Self::app_resources(&mcp.resources(&id).await);
                let m = mcp.manifest(&id).await;
                (i, id, name, apps, m)
            });
        }
        let mut got = set.join_all().await;
        got.sort_by_key(|g| g.0);
        let named = |xs: Option<&Value>| -> Vec<Value> {
            xs.and_then(Value::as_array).map(|a| a.iter().filter(|x| x.get("name").and_then(Value::as_str).is_some_and(|n| !n.is_empty())).cloned().collect()).unwrap_or_default()
        };
        let servers: Vec<Value> = got
            .into_iter()
            .map(|(_, id, name, apps, m)| {
                let title = m.get("title").filter(|t| js::truthy(t)).map(js::string_of).filter(|t| !t.is_empty()).unwrap_or_else(|| if name.is_empty() { id.clone() } else { name.clone() });
                let apps: Vec<Value> = apps
                    .iter()
                    .map(|r| json!({"id": app_of(&r.get("uri").map(js::string_of).unwrap_or_default()), "title": first_truthy(r, &["title", "name"]), "description": first_truthy(r, &["description"])}))
                    .collect();
                json!({"id": id, "title": title, "apps": apps, "commands": named(m.get("commands")), "scopes": named(m.get("scopes"))})
            })
            .filter(|s| ["apps", "commands", "scopes"].iter().any(|k| s[k].as_array().is_some_and(|a| !a.is_empty())))
            .collect();
        out(200, &json!({"servers": servers}))
    }

    /// `GET /mcp/tools?id=`: the tools, each with its switch; an off server
    /// isn't started to list them.
    async fn tools(&self, id: &str) -> Answer {
        let row: Option<(i64, Option<String>)> = self.mcp.db(|db| db.query_row("SELECT enabled, disabled_tools FROM mcp_servers WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()).ok().flatten();
        let Some((enabled, off)) = row else { return Answer::json(404, r#"{"error":"no such server"}"#) };
        if enabled == 0 {
            return out(200, &json!({"tools": [], "status": "off"}));
        }
        let off: Vec<String> = serde_json::from_str::<Value>(off.as_deref().unwrap_or("[]")).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(js::string_of).collect();
        let e = self.mcp.ensure(id).await;
        let connected = e.get("ok") == Some(&Value::Bool(true));
        let tools: Vec<Value> = e
            .get("tools")
            .and_then(Value::as_array)
            .map(|ts| {
                ts.iter()
                    .map(|t| {
                        let name = t.get("name").map(js::string_of).unwrap_or_default();
                        json!({"name": name, "description": first_truthy(t, &["description"]), "enabled": !off.contains(&name)})
                    })
                    .collect()
            })
            .unwrap_or_default();
        out(200, &json!({"tools": tools, "status": if connected { "connected" } else { "error" }}))
    }

    async fn state(&self, parsed: Result<Value, String>) -> Answer {
        let Ok(b) = parsed else { return Answer::refused(400, true, &parsed.err().unwrap_or_default()) };
        let id = text(&b, "id");
        let on = b.get("enabled").is_some_and(js::truthy);
        let sql = if on { "UPDATE mcp_servers SET enabled=1, agent_pending=NULL WHERE id=?" } else { "UPDATE mcp_servers SET enabled=0 WHERE id=?" };
        match self.mcp.db(|db| db.execute(sql, [&id])) {
            Ok(0) => Answer::refused(400, true, "no such server"),
            Ok(_) => {
                if on {
                    let (mcp, id) = (self.mcp.clone(), id.clone());
                    tokio::spawn(async move {
                        mcp.ensure(&id).await;
                    });
                } else {
                    self.mcp.invalidate(&id).await;
                }
                ok()
            }
            Err(e) => Answer::refused(400, true, &e),
        }
    }

    fn tool_state(&self, parsed: Result<Value, String>) -> Answer {
        let Ok(b) = parsed else { return Answer::refused(400, true, &parsed.err().unwrap_or_default()) };
        let id = text(&b, "id");
        let row: Option<(String, Option<String>)> = self.mcp.db(|db| db.query_row("SELECT id, disabled_tools FROM mcp_servers WHERE id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?))).optional()).ok().flatten();
        let Some((id, off)) = row else { return Answer::refused(400, true, "no such server") };
        let mut off: Vec<String> = serde_json::from_str::<Value>(off.as_deref().unwrap_or("[]")).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(js::string_of).collect();
        off.dedup();
        let tool = b.get("tool").map(js::string_of).unwrap_or_else(|| "undefined".into());
        if b.get("enabled").is_some_and(js::truthy) {
            off.retain(|t| *t != tool);
        } else if !off.contains(&tool) {
            off.push(tool);
        }
        match self.mcp.db(|db| db.execute("UPDATE mcp_servers SET disabled_tools=? WHERE id=?", rusqlite::params![js::stringify(&json!(off)), id])) {
            Ok(_) => ok(),
            Err(e) => Answer::refused(400, true, &e),
        }
    }

    /// `mcpAppCall`: a shipped mini app calling its server.
    async fn app_call(&self, server: &str, tool: &str, args: Value) -> Value {
        let row: Option<(String, Option<String>, Option<String>)> =
            self.mcp.db(|db| db.query_row("SELECT id, name, disabled_tools FROM mcp_servers WHERE id=? AND enabled=1", [server], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()).ok().flatten();
        let Some((id, name, off)) = row else { return json!({"ok": false, "error": "no such server"}) };
        let off: Vec<String> = serde_json::from_str::<Value>(off.as_deref().unwrap_or("[]")).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(js::string_of).collect();
        if off.iter().any(|t| t == tool) {
            let who = name.filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
            return json!({"ok": false, "error": format!("{tool} is switched off for {who}; turn it on in Plugins to let apps call it")});
        }
        self.call_json(&id, tool, args).await
    }

    /// `callToolJson`: the result as data, or `{ok: false, error}`.
    async fn call_json(&self, id: &str, tool: &str, args: Value) -> Value {
        let e = self.mcp.ensure(id).await;
        if e.get("ok") != Some(&Value::Bool(true)) {
            let err = e.get("err").map(js::string_of).filter(|s| !s.is_empty()).unwrap_or_else(|| "failed to start".into());
            return json!({"ok": false, "error": format!("{id} is not connected ({err})")});
        }
        match self.mcp.request(id, "tools/call", json!({"name": tool, "arguments": args}), APP_CALL).await {
            Ok(r) => tool_json(&r),
            Err(e) => json!({"ok": false, "error": e.message}),
        }
    }

    /// `mcpResolve`: the tool the server's manifest names for a command or
    /// scope, with the chat's attached tokens of that server.
    pub async fn resolve(&self, b: &Value) -> Value {
        let server = text(b, "server");
        let row: Option<String> = self.mcp.db(|db| db.query_row("SELECT id FROM mcp_servers WHERE id=? AND enabled=1", [&server], |r| r.get(0)).optional()).ok().flatten();
        let Some(id) = row else { return json!({"ok": false, "error": "no such server"}) };
        let name = b.get("name").map(js::string_of).unwrap_or_else(|| "undefined".into());
        let m = self.mcp.manifest(&id).await;
        let mut all: Vec<Value> = m.get("commands").and_then(Value::as_array).cloned().unwrap_or_default();
        all.extend(m.get("scopes").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter(|x| x.get("name").is_some_and(Value::is_string)));
        let tool = all.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(&name) && x.get("resolve").is_some_and(js::truthy)).and_then(|x| x.get("resolve")).map(js::string_of);
        let Some(tool) = tool else { return json!({"ok": false, "error": format!("{id} has no /{name}")}) };
        let thread = text(b, "thread");
        let attached: Vec<String> = if thread.is_empty() {
            Vec::new()
        } else {
            self.mcp
                .db(|db| {
                    let t: Option<String> = db.query_row("SELECT id FROM threads WHERE id=? OR uid=?", [&thread, &thread], |r| r.get(0)).optional()?;
                    let Some(t) = t else { return Ok(Vec::new()) };
                    let mut st = db.prepare("SELECT token FROM attachments WHERE thread=? AND server=? ORDER BY created_at")?;
                    let rows = st.query_map([&t, &id], |r| r.get::<_, String>(0))?;
                    rows.collect()
                })
                .unwrap_or_default()
        };
        let args = b.get("args").filter(|a| !a.is_null()).map(js::string_of).unwrap_or_default();
        let mut call = json!({"name": name, "args": args, "complete": b.get("complete").is_some_and(js::truthy)});
        if !attached.is_empty() {
            call["context"] = json!({"attached": attached});
        }
        let r = self.call_json(&id, &tool, call).await;
        if r.get("ok") == Some(&Value::Bool(true)) {
            return r.get("result").cloned().unwrap_or(Value::Null);
        }
        r
    }

    /// `POST /mcp/account {id, label?, disconnect?}`.
    async fn account(&self, b: &Value) -> Answer {
        let id = text(b, "id");
        if !self.exists(&id) {
            return Answer::refused(400, true, "no such server");
        }
        if b.get("label").is_some() {
            let label = b.get("label").filter(|l| js::truthy(l)).map(js::string_of).unwrap_or_default();
            let label = js::slice_utf16(label.trim(), 40);
            if label.eq_ignore_ascii_case("grok") {
                return Answer::refused(400, true, "Grok is a reserved account label");
            }
            let label = (!label.is_empty()).then_some(label);
            if let Err(e) = self.mcp.db(|db| db.execute("UPDATE mcp_servers SET account=? WHERE id=?", rusqlite::params![label, id])) {
                return Answer::refused(400, true, &e);
            }
        }
        if b.get("disconnect").is_some_and(js::truthy) {
            if let Err(e) = self.mcp.db(|db| db.execute("UPDATE mcp_servers SET auth=NULL WHERE id=?", [&id])) {
                return Answer::refused(400, true, &e);
            }
            self.mcp.invalidate(&id).await;
        }
        ok()
    }

    /// `GET /oauth/callback/<id>?code=…`: the flow McpHost started.
    async fn callback(&self, url: &str) -> Answer {
        let (path, q) = url.split_once('?').unwrap_or((url, ""));
        let raw = path.strip_prefix("/oauth/callback/").unwrap_or_default();
        let id = url::form_urlencoded::parse(format!("x={}", raw.replace('+', "%2B")).as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default();
        let param = |k: &str| url::form_urlencoded::parse(q.as_bytes()).find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
        if let Some(host) = self.plugins.as_ref().filter(|h| h.account_pending(&id)) {
            let (result, why) = host.account_callback(&id, param("code").as_deref(), param("state").as_deref(), param("error").as_deref()).await;
            return match result {
                "connected" => html(200, "Signed in"),
                "refused" => html(400, "Authentication failed"),
                _ => {
                    if !why.is_empty() {
                        eprintln!("  v1 {id}: sign-in failed: {}", js::slice_utf16(&why, 200));
                    }
                    html(200, "Sign-in failed")
                }
            };
        }
        let r = self.mcp.auth_finish(&id, param("code").as_deref(), param("state").as_deref(), param("error").as_deref()).await;
        match r.get("result").and_then(Value::as_str) {
            Some("connected") => html(200, "Connected"),
            Some("refused") => html(400, "Authentication failed"),
            _ => {
                let why = r.get("error").map(js::string_of).unwrap_or_default();
                if !why.is_empty() {
                    eprintln!("  mcp {id}: token exchange failed: {}", js::slice_utf16(&why, 200));
                }
                html(200, "Authentication failed")
            }
        }
    }

    /// `POST /mcp/open {url}`.
    async fn open(&self, url: &str) -> Answer {
        let app = app_link(url);
        if !app && !chat_link(url) && view_path(url).is_none() {
            return out(400, &json!({"ok": false, "error": "only buwiz://app/<server>/<app>, buwiz://chat/new?draft= and buwiz://view?path=<absolute path> links"}));
        }
        let target = if app { self.with_pass(url) } else { url.to_owned() };
        let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        match tokio::process::Command::new(cmd).arg(&target).stdin(std::process::Stdio::null()).output().await {
            Ok(o) if o.status.success() => ok(),
            Ok(o) => out(200, &json!({"ok": false, "error": format!("Command failed: {cmd} {target}\n{}", String::from_utf8_lossy(&o.stderr))})),
            Err(e) => out(200, &json!({"ok": false, "error": format!("spawn {cmd} {e}")})),
        }
    }

    /// open-pass.mjs's `withPass`: `url` with a fresh one-time pass.
    fn with_pass(&self, url: &str) -> String {
        let mut passes = self.passes.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        passes.retain(|_, until| *until > now);
        let pass = buwiz_protocol::keys::random_id();
        passes.insert(pass.clone(), now + PASS_FOR);
        format!("{url}{}buwiz_pass={pass}", if url.contains('?') { "&" } else { "?" })
    }

    /// open-pass.mjs's `redeem`: once, while fresh.
    fn redeem(&self, pass: &str) -> bool {
        let until = self.passes.lock().unwrap_or_else(|e| e.into_inner()).remove(pass);
        until.is_some_and(|u| u > Instant::now())
    }
}

/// `r.a || r.b || ""` over string fields.
fn first_truthy(r: &Value, keys: &[&str]) -> String {
    keys.iter().find_map(|k| r.get(*k).filter(|v| js::truthy(v)).map(js::string_of)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_checked_as_market_mjs_checks_them() {
        assert!(app_link("buwiz://app/apps/hello"));
        assert!(app_link("buwiz://app/apps/hello.v2?x=1&y=2"));
        assert!(!app_link("buwiz://app/Apps/hello"));
        assert!(!app_link("buwiz://app/apps/hello/more"));
        assert!(!app_link("buwiz://app/apps/hello?x= 1"));
        assert!(chat_link("buwiz://chat/new?draft=hi%20there"));
        assert!(!chat_link("buwiz://chat/new?draft=hi there"));
        assert_eq!(view_path("buwiz://view?path=/tmp/x.pdf").as_deref(), Some("/tmp/x.pdf"));
        assert_eq!(view_path("buwiz://view?path=%2Ftmp%2Fa%20b.pdf").as_deref(), Some("/tmp/a b.pdf"));
        assert_eq!(view_path("buwiz://view?path=relative"), None);
        assert_eq!(view_path("https://x/view?path=/a"), None);
    }

    #[test]
    fn tool_results_become_app_data_as_call_tool_json_makes_them() {
        assert_eq!(tool_json(&json!({"content": [], "structuredContent": {"result": 3}})), json!({"ok": true, "result": 3}));
        assert_eq!(tool_json(&json!({"content": [], "structuredContent": {"a": 1}})), json!({"ok": true, "result": {"a": 1}}));
        assert_eq!(tool_json(&json!({"content": [{"type": "text", "text": "{\"b\":2}"}]})), json!({"ok": true, "result": {"b": 2}}));
        assert_eq!(tool_json(&json!({"content": [{"type": "text", "text": "plain"}]})), json!({"ok": true, "result": "plain"}));
        assert_eq!(tool_json(&json!({"content": [], "isError": true})), json!({"ok": false, "error": "tool failed"}));
    }
}
