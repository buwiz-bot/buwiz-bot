//! The `linear` source: the user's Linear issues, through Linear's GraphQL (what
//! `/data/linear/*` answered before widgets called sources). The key comes from
//! `LINEAR_API_KEY` or `secret get`; a test steers both with a [`LinearApi`] in
//! `ctx.ext`.

use std::sync::Mutex;

use serde_json::{json, Map, Value};

use super::reminders::local;
use crate::helper::http;
use crate::hub::ctx::HubCtx;
use crate::hub::jsval::{field, get, parse_error, trim, truthy};
use crate::hub::traits::LocalCommand;
use crate::js;

const LINEAR_URL: &str = "https://api.linear.app/graphql";
const L_FIELDS: &str = "nodes { id identifier title priority url updatedAt state { id name } assignee { displayName } team { key } }";
const Q_STATES: &str = "{ teams(first: 10) { nodes { key states { nodes { id name type position } } } } }";
const Q_VIEWER: &str = "{ viewer { id } }";
const Q_UPDATE: &str = "mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success } }";

/// In `ctx.ext`: Linear's endpoint and key, instead of the real endpoint and the
/// environment's and keychain's key (tests; `key: None` is a Linear with no key).
#[derive(Clone)]
pub struct LinearApi {
    pub url: String,
    pub key: Option<String>,
}

/// The key once found.
static LINEAR_KEY: Mutex<Option<String>> = Mutex::new(None);
/// When a lookup last found no key. A miss is looked up again (the person may add a key
/// while the broker runs), but not before [`MISS_FOR`] has passed: every listing of the
/// sources asks, and a lookup starts a process.
static LINEAR_MISS: Mutex<Option<std::time::Instant>> = Mutex::new(None);
const MISS_FOR: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether a lookup that found no key at `missed` still stands at `now`.
fn miss_stands(missed: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    missed.is_some_and(|at| now.saturating_duration_since(at) < MISS_FOR)
}

/// `getLinearKey()`: `LINEAR_API_KEY`, else `secret get LINEAR_API_KEY`.
pub async fn key(ctx: &HubCtx) -> Option<String> {
    if let Some(api) = ctx.ext.get::<LinearApi>() {
        return api.key.clone();
    }
    if let Some(k) = LINEAR_KEY.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(k);
    }
    let mut key = std::env::var("LINEAR_API_KEY").ok().filter(|k| !k.is_empty());
    if key.is_none() && miss_stands(*LINEAR_MISS.lock().unwrap_or_else(|e| e.into_inner()), std::time::Instant::now()) {
        return None;
    }
    if key.is_none() {
        let cmd = LocalCommand { program: "secret".into(), args: vec!["get".into(), "LINEAR_API_KEY".into()], ..Default::default() };
        if let Ok(out) = local(ctx).run(cmd).await {
            if out.code == Some(0) {
                key = Some(trim(&String::from_utf8_lossy(&out.stdout)).to_owned()).filter(|k| !k.is_empty());
            }
        }
    }
    match &key {
        Some(k) => *LINEAR_KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(k.clone()),
        None => *LINEAR_MISS.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now()),
    }
    key
}

/// `lgql(query, variables)`: the response's JSON, `{_nokey: true}` with no
/// key, or the message fetch or `r.json()` rejects with.
async fn lgql(ctx: &HubCtx, query: &str, variables: Option<Value>) -> Result<Value, String> {
    let Some(key) = key(ctx).await else { return Ok(json!({ "_nokey": true })) };
    let body = match variables {
        Some(v) => json!({ "query": query, "variables": v }),
        None => json!({ "query": query }),
    };
    let url = ctx.ext.get::<LinearApi>().map(|a| a.url.as_str()).unwrap_or(LINEAR_URL);
    let url = url::Url::parse(url).map_err(|e| e.to_string())?;
    let req = http::Req::post(url, js::stringify(&body).into_bytes()).header("Authorization", &key).header("Content-Type", "application/json");
    let res = http::fetch(req).await.map_err(|_| "fetch failed".to_owned())?;
    let text = res.text().await.map_err(|_| "terminated".to_owned())?;
    serde_json::from_str(&text).map_err(|e| parse_error(&text, &e))
}

/// `throw new Error(String(d.errors[0]?.message))`, caught as
/// `String(e?.message || e)`.
fn graphql_error(d: &Value) -> Option<String> {
    let errors = get(Some(d), "errors");
    if !truthy(errors) {
        return None;
    }
    let first = errors.and_then(|e| e.as_array()).and_then(|a| a.first());
    let msg = get(first, "message").map(js::string_of).unwrap_or_else(|| "undefined".to_owned());
    Some(if msg.is_empty() { "Error".to_owned() } else { msg })
}

/// `a.b?.c || fallback`.
fn or(v: Option<&Value>, fallback: Value) -> Value {
    if truthy(v) {
        v.cloned().expect("truthy")
    } else {
        fallback
    }
}

/// An issue as the widget gets it (keys of missing members left out, as
/// JSON.stringify drops `undefined`).
fn issue(n: &Value) -> Value {
    let mut m = Map::new();
    let mut put = |k: &str, v: Option<&Value>| {
        if let Some(v) = v {
            m.insert(k.to_owned(), v.clone());
        }
    };
    put("uid", get(Some(n), "id"));
    put("id", get(Some(n), "identifier"));
    put("title", get(Some(n), "title"));
    m.insert("state".into(), or(get(get(Some(n), "state"), "name"), json!("")));
    m.insert("stateId".into(), or(get(get(Some(n), "state"), "id"), json!("")));
    if let Some(p) = get(Some(n), "priority") {
        m.insert("priority".into(), p.clone());
    }
    m.insert("assignee".into(), or(get(get(Some(n), "assignee"), "displayName"), Value::Null));
    m.insert("team".into(), or(get(get(Some(n), "team"), "key"), Value::Null));
    if let Some(u) = get(Some(n), "url") {
        m.insert("url".into(), u.clone());
    }
    Value::Object(m)
}

fn shape(nodes: &[Value], scope: &str) -> Value {
    json!({ "scope": scope, "issues": nodes.iter().map(issue).collect::<Vec<_>>() })
}

/// `x || []` of a nodes list; a list only.
fn nodes(v: Option<&Value>) -> Vec<Value> {
    v.and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

/// `issues`: assigned to me, else the workspace's latest. `Err` is Linear's or the
/// network's message (the old route's 502).
pub async fn issues(ctx: &HubCtx) -> Result<Value, String> {
    let filter = r#"filter: { state: { type: { nin: ["completed","canceled"] } } }"#;
    let d = lgql(ctx, &format!("{{ viewer {{ assignedIssues(first: 25, {filter}) {{ {L_FIELDS} }} }} }}"), None).await?;
    if truthy(get(Some(&d), "_nokey")) {
        return Ok(json!({ "error": "linear not configured — run `secret set LINEAR_API_KEY`" }));
    }
    if let Some(e) = graphql_error(&d) {
        return Err(e);
    }
    let assigned = nodes(get(get(get(get(Some(&d), "data"), "viewer"), "assignedIssues"), "nodes"));
    if !assigned.is_empty() {
        return Ok(shape(&assigned, "assigned"));
    }
    let d2 = lgql(ctx, &format!("{{ issues(first: 25, orderBy: updatedAt, {filter}) {{ {L_FIELDS} }} }}"), None).await?;
    if let Some(e) = graphql_error(&d2) {
        return Err(e);
    }
    Ok(shape(&nodes(get(get(get(Some(&d2), "data"), "issues"), "nodes")), "workspace"))
}

/// `states`: every team's workflow states, by position.
pub async fn states(ctx: &HubCtx) -> Result<Value, String> {
    let d = lgql(ctx, Q_STATES, None).await?;
    if truthy(get(Some(&d), "_nokey")) {
        return Ok(json!({ "error": "linear not configured" }));
    }
    if let Some(e) = graphql_error(&d) {
        return Err(e);
    }
    let position = |s: &Value| get(Some(s), "position").and_then(Value::as_f64).unwrap_or(f64::NAN);
    let teams: Vec<Value> = nodes(get(get(get(Some(&d), "data"), "teams"), "nodes"))
        .iter()
        .map(|t| {
            let mut states = nodes(get(get(Some(t), "states"), "nodes"));
            states.sort_by(|a, b| (position(a) - position(b)).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut m = Map::new();
            if let Some(k) = get(Some(t), "key") {
                m.insert("team".into(), k.clone());
            }
            let states = states
                .iter()
                .map(|s| {
                    let mut m = Map::new();
                    for k in ["id", "name", "type"] {
                        if let Some(v) = get(Some(s), k) {
                            m.insert(k.into(), v.clone());
                        }
                    }
                    Value::Object(m)
                })
                .collect();
            m.insert("states".into(), Value::Array(states));
            Value::Object(m)
        })
        .collect();
    Ok(json!({ "teams": teams }))
}

/// `assign {uid}`: the issue to the key's owner. Every answer but a `null` body's
/// TypeError (`Err`) is an `Ok`, an `{error}` included.
pub async fn assign(ctx: &HubCtx, b: &Value) -> Result<Value, String> {
    let uid = field(b, "uid")?.cloned();
    if !truthy(uid.as_ref()) {
        return Ok(json!({ "error": "uid?" }));
    }
    let run = async {
        let d = lgql(ctx, Q_VIEWER, None).await?;
        let vid = get(get(get(Some(&d), "data"), "viewer"), "id").filter(|v| js::truthy(v)).cloned().ok_or_else(|| "no viewer".to_owned())?;
        let d2 = lgql(ctx, Q_UPDATE, Some(json!({ "id": uid, "input": { "assigneeId": vid } }))).await?;
        match graphql_error(&d2) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    };
    Ok(match run.await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "error": e }),
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{miss_stands, MISS_FOR};

    #[test]
    fn a_lookup_that_found_no_key_is_not_repeated_at_once() {
        let at = Instant::now();
        assert!(!miss_stands(None, at), "never looked: look");
        assert!(miss_stands(Some(at), at) && miss_stands(Some(at), at + MISS_FOR - Duration::from_millis(1)));
        assert!(!miss_stands(Some(at), at + MISS_FOR), "after a while, look again");
    }
}
