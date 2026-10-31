//! v0 routine templates servers announce (routines.mjs "templates servers
//! announce"): a connected server's `buwiz://manifest` may carry `routines:
//! [{id, name, description, every_s, needs, query: {tool, field, args?},
//! prompt}]`. `needs: "gmail"` makes the check a Gmail search through
//! whichever connected server has a search tool; `query` is a cheap tool
//! on the announcing server whose result's `field` is that search. The
//! user's Google address (`meta` `google_email`, `google_email_seen`).

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::host::RoutineHost;
use super::mcp::{McpView, Server};
use super::text::{is_email, is_obj, mcp_name, trim};
use super::v0::clamp_every;
use crate::hub::ctx::Db;
use crate::js;

/// A Gmail search tool: `query` its search argument, `ids` the message
/// ids in its result, `email` its account argument, `read` the tool the
/// agent reads messages with.
#[derive(Clone, Debug)]
pub struct Adapter {
    pub tool: String,
    pub query: String,
    pub args: Value,
    pub ids: String,
    pub email: Option<String>,
    pub read: Option<String>,
    pub schema: Value,
}

fn gmail() -> Vec<Adapter> {
    vec![
        // workspace-mcp (google-workspace): text, one "Message ID: <id>" line each
        Adapter {
            tool: "search_gmail_messages".into(),
            query: "query".into(),
            args: json!({"page_size": 25}),
            ids: r"/Message ID: (\S+)/".into(),
            email: Some("user_google_email".into()),
            read: Some("get_gmail_messages_content_batch".into()),
            schema: Value::Null,
        },
        // Google's Gmail MCP: JSON threads or messages
        Adapter {
            tool: "search_threads".into(),
            query: "query".into(),
            args: json!({}),
            ids: r#"/"(?:id|threadId)"\s*:\s*"([^"]+)"/"#.into(),
            email: None,
            read: Some("get_thread".into()),
            schema: Value::Null,
        },
    ]
}

/// `/^(search|list)_?\w*(mail|message|thread)/i`.
fn mail_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("search").or_else(|| lower.strip_prefix("list")) else { return false };
    let word: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    ["mail", "message", "thread"].iter().any(|k| word.contains(k))
}

/// `gmailAdapter(tools)`: the server's Gmail search, else any search/list
/// tool over mail, messages or threads that takes a query.
pub fn gmail_adapter(tools: &[Value]) -> Option<Adapter> {
    for a in gmail() {
        if let Some(t) = tools.iter().find(|t| t.get("name").and_then(Value::as_str) == Some(a.tool.as_str())) {
            return Some(Adapter { schema: t.get("inputSchema").cloned().unwrap_or(Value::Null), ..a });
        }
    }
    for t in tools {
        let props = t.get("inputSchema").and_then(|s| s.get("properties"));
        let q = ["query", "q"].into_iter().find(|k| props.and_then(|p| p.get(*k)).is_some_and(js::truthy));
        let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
        if let Some(q) = q.filter(|_| mail_tool(name)) {
            return Some(Adapter {
                tool: name.to_owned(),
                query: q.to_owned(),
                args: json!({}),
                ids: r#"/"(?:id|threadId)"\s*:\s*"([^"]+)"/"#.into(),
                email: None,
                read: None,
                schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
            });
        }
    }
    None
}

/// The connected server that can search Gmail (`gmailSource`), or why none.
pub enum Source {
    Found(Server, Adapter),
    None(String),
}

/// Settled within 15 s, else the fallback (`within`).
const WITHIN: Duration = Duration::from_secs(15);

pub async fn gmail_source(mcp: &dyn McpView) -> Source {
    let mut reason = "No Gmail app is connected — connect Google Workspace or Gmail in Connect Apps".to_owned();
    for row in mcp.servers() {
        let tools = match tokio::time::timeout(WITHIN, mcp.ensure(&row.id)).await {
            Ok(Ok(tools)) => tools,
            _ => continue,
        };
        let Some(adapter) = gmail_adapter(&tools) else { continue };
        let st = mcp.status(&row.id);
        let status = st.get("status").and_then(Value::as_str).unwrap_or_default();
        if status != "connected" {
            let who = if row.name.is_empty() { row.id.clone() } else { row.name.clone() };
            let why = if status == "needs-auth" {
                "needs sign-in (Connect Apps › Authenticate)".to_owned()
            } else {
                st.get("err").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_else(|| "isn't working".to_owned())
            };
            reason = format!("{who} can search Gmail but {why}");
            continue;
        }
        return Source::Found(row, adapter);
    }
    Source::None(reason)
}

pub fn meta_get(db: &Db, k: &str) -> Option<String> {
    db.with(|db| db.query_row("SELECT v FROM meta WHERE k=?", [k], |r| r.get::<_, Option<String>>(0))).ok().flatten()
}

pub fn meta_set(db: &Db, k: &str, v: &str) {
    let _ = db.with(|db| db.execute("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)", [k, v]));
}

/// The user's Google address for a Gmail server's account argument
/// (`googleEmail`): the one the user gave, the server's
/// USER_GOOGLE_EMAIL, a Google plugin's setup value, the connector's
/// account label, the last one a tool call used, or a Drive file a chat
/// recorded; "" when none is known.
pub fn google_email(db: &Db, host: &dyn RoutineHost, row: Option<&Server>) -> String {
    let env = row.map(|r| super::plugins::parse(&r.env, json!({}))).unwrap_or(json!({}));
    let settings: Vec<Value> = db
        .with(|db| {
            let mut st = db.prepare("SELECT vals FROM plugin_settings")?;
            let rows = st.query_map([], |r| r.get::<_, Option<String>>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|v| super::plugins::parse(v.as_deref().unwrap_or(""), json!({})))
        .collect();
    let mut found: Vec<Value> = vec![meta_get(db, "google_email").map(Value::from).unwrap_or(Value::Null), env.get("USER_GOOGLE_EMAIL").cloned().unwrap_or(Value::Null)];
    for v in &settings {
        found.push(v.get("GOOGLE_ACCOUNT_EMAIL").cloned().unwrap_or(Value::Null));
        found.push(v.get("USER_GOOGLE_EMAIL").cloned().unwrap_or(Value::Null));
    }
    found.push(row.and_then(|r| r.account.clone()).map(Value::from).unwrap_or(Value::Null));
    found.push(meta_get(db, "google_email_seen").map(Value::from).unwrap_or(Value::Null));
    let hit = found.iter().find(|v| is_email(Some(v))).and_then(Value::as_str).map(str::to_owned).or_else(|| host.linked_account(&|a| is_email(Some(&json!(a)))));
    hit.map(|h| trim(&h).to_owned()).unwrap_or_default()
}

/// A tool call that named the user's Google address (`noteGoogleEmail`):
/// the fallback `google_email` remembers.
pub fn note_google_email(db: &Db, input: &Value) {
    let Some(e) = input.get("user_google_email").filter(|e| is_email(Some(e))).and_then(Value::as_str) else { return };
    let e = trim(e);
    if meta_get(db, "google_email_seen").as_deref() != Some(e) {
        meta_set(db, "google_email_seen", e);
    }
}

/// A manifest's templates, laxly: the ones with an id and a name.
#[derive(Clone, Debug)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    pub every_s: f64,
    pub needs: Option<String>,
    pub query: Option<Value>,
    pub prompt: String,
}

pub fn templates_of(m: &Value) -> Vec<Template> {
    m.get("routines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|x| x.is_object() && x.get("id").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) && x.get("name").and_then(Value::as_str).is_some_and(|s| !s.is_empty()))
        .map(|x| {
            let str_of = |k: &str| x.get(k).and_then(Value::as_str).map(str::to_owned);
            let every = x.get("every_s").filter(|v| js::truthy(v)).cloned().unwrap_or(json!(3600));
            Template {
                id: str_of("id").unwrap_or_default(),
                name: js::slice_utf16(&str_of("name").unwrap_or_default(), 80),
                description: str_of("description").unwrap_or_default(),
                every_s: clamp_every(Some(&every)),
                needs: str_of("needs"),
                query: x.get("query").filter(|q| q.is_object() && q.get("tool").is_some_and(Value::is_string)).cloned(),
                prompt: str_of("prompt").unwrap_or_default(),
            }
        })
        .collect()
}

/// Why a template can't be enabled now ("" when it can), given the Gmail source.
pub fn unavailable(t: &Template, source: Option<&Source>) -> String {
    match t.needs.as_deref() {
        Some(n) if n != "gmail" => return format!("Buwiz Bot can't provide \"{n}\" yet"),
        Some(_) if t.query.is_none() => return "The template names no query tool".to_owned(),
        Some(_) => {
            if let Some(Source::None(why)) = source {
                return why.clone();
            }
        }
        None if trim(&t.prompt).is_empty() => return "The template has nothing to do".to_owned(),
        None => {}
    }
    String::new()
}

/// Does the Gmail tool require the account argument, with none known?
pub fn needs_email(db: &Db, host: &dyn RoutineHost, source: Option<&Source>) -> bool {
    let Some(Source::Found(row, a)) = source else { return false };
    let Some(email) = &a.email else { return false };
    let required = a.schema.get("required").and_then(Value::as_array).is_some_and(|r| r.iter().any(|x| x.as_str() == Some(email.as_str())));
    required && google_email(db, host, Some(row)).is_empty()
}

/// GET /routines/templates: every connected server's templates.
pub async fn list(db: &Db, host: &dyn RoutineHost, mcp: &dyn McpView) -> Vec<Value> {
    let rows = mcp.servers();
    let mut found = Vec::new();
    for row in rows {
        let m = tokio::time::timeout(WITHIN, mcp.manifest(&row.id)).await.unwrap_or(json!({}));
        found.push((row, templates_of(&m)));
    }
    if !found.iter().any(|(_, t)| !t.is_empty()) {
        return Vec::new();
    }
    let source = if found.iter().any(|(_, t)| t.iter().any(|t| t.needs.as_deref() == Some("gmail"))) { Some(gmail_source(mcp).await) } else { None };
    let ask = needs_email(db, host, source.as_ref());
    let mut out = Vec::new();
    for (row, tpls) in found {
        for t in tpls {
            let reason = unavailable(&t, source.as_ref());
            let mut o = Map::new();
            o.insert("server".into(), json!(row.id));
            o.insert("id".into(), json!(t.id));
            o.insert("name".into(), json!(t.name));
            o.insert("description".into(), json!(t.description));
            o.insert("every_s".into(), super::text::num(t.every_s));
            o.insert("needs".into(), json!(t.needs));
            o.insert("available".into(), json!(reason.is_empty()));
            if !reason.is_empty() {
                o.insert("reason".into(), json!(reason));
            }
            if t.needs.as_deref() == Some("gmail") && ask {
                o.insert("needs_email".into(), json!(true));
            }
            out.push(Value::Object(o));
        }
    }
    out
}

/// The check and prompt a Gmail template makes (`routineFromTemplate`'s
/// middle): the search through `row`'s adapter, its query from the
/// announcing server's tool each tick.
pub async fn gmail_check(db: &Db, host: &dyn RoutineHost, mcp: &dyn McpView, announcer: &str, t: &Template, row: &Server, a: &Adapter) -> (Value, String) {
    let email = match &a.email {
        Some(e) if a.schema.get("properties").and_then(|p| p.get(e)).is_some_and(js::truthy) => google_email(db, host, Some(row)),
        _ => String::new(),
    };
    let q = t.query.clone().unwrap_or(json!({}));
    let mut args = a.args.as_object().cloned().unwrap_or_default();
    if let (false, Some(e)) = (email.is_empty(), &a.email) {
        args.insert(e.clone(), json!(email));
    }
    let field = q.get("field").and_then(Value::as_str).filter(|f| !f.is_empty()).unwrap_or("query");
    let check = json!({
        "server": row.id, "tool": a.tool, "args": args, "ids": a.ids,
        "args_from": {"server": announcer, "tool": q.get("tool"), "args": if is_obj(q.get("args")) { q["args"].clone() } else { json!({}) }, "field": field, "into": a.query},
    });
    let tools = mcp.ensure(&row.id).await.unwrap_or_default();
    let reader = a.read.as_deref().filter(|r| tools.iter().any(|x| x.get("name").and_then(Value::as_str) == Some(*r)));
    let read_with = match reader {
        Some(r) => mcp_name(&row.id, r),
        None => format!("{}'s Gmail tools", row.id),
    };
    let account = match (&a.email, email.is_empty()) {
        (Some(e), false) => format!(" ({e}: {email})"),
        _ => String::new(),
    };
    let head = format!("Each new item is a Gmail message from {}'s {}. Read them with {read_with}{account}.", row.id, a.tool);
    let prompt = [head, t.prompt.clone()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");
    (check, prompt)
}
