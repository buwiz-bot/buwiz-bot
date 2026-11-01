//! A bot's tool ceiling on opengrok-server's routes (`agui/routes.rs`,
//! `agui/ceiling.rs`), with a `Bot.id` in the `{coworker_id}` slot:
//! `GET`/`PUT /coworkers/{bot}/ceiling`, `GET /coworkers/{bot}/tools`
//! (what the next turn is offered) and `PUT /coworkers/{bot}/tool-mode`.
//! An unknown bot is the hub's 404; a PUT naming a `version` other than
//! the one stored is opengrok's 409 (`ceiling-changed`), so two cards
//! cannot overwrite each other's switches.


use hyper::StatusCode;
use serde_json::{json, Value};

use super::host::read_body;
use crate::hub::ctx::HubCtx;
use crate::hub::store::{Bot, Ceiling};
use crate::hub::tools::ceiling;
use crate::hub::types::{json as answer, HubRequest, HubResponse, Pattern::Segments as S, Verb};
use crate::hub::Registry;
use crate::js;

pub fn install(routes: &mut Registry) {
    routes.route(Verb::Get, S("/coworkers/*/ceiling"), |ctx, req| async move { get_ceiling(&ctx, &req) });
    routes.route(Verb::Put, S("/coworkers/*/ceiling"), |ctx, req| async move { put_ceiling(&ctx, &req) });
    routes.route(Verb::Get, S("/coworkers/*/tools"), |ctx, req| async move { get_tools(&ctx, &req) });
    routes.route(Verb::Put, S("/coworkers/*/tool-mode"), |ctx, req| async move { put_tool_mode(&ctx, &req) });
}

/// opengrok-server `agui/ceiling.rs` `reply`: `{"tools": rows, "version"}`.
fn reply(ctx: &HubCtx, bot: &Bot) -> HubResponse {
    answer(StatusCode::OK, &json!({ "tools": ceiling::rows(ctx, bot), "version": bot.tools.version }))
}

/// The hub's 404 (v1 error shape): no bot has this id.
fn no_bot() -> HubResponse {
    answer(StatusCode::NOT_FOUND, &json!({ "ok": false, "code": "not_found", "message": "no such bot", "retryable": false }))
}

/// opengrok-server `health.rs` `refusal`: `{"error": why}` with its status.
fn refusal(status: StatusCode, why: &str) -> HubResponse {
    answer(status, &json!({ "error": why }))
}

fn bot_of(ctx: &HubCtx, req: &HubRequest) -> Option<Bot> {
    let id = req.params.first()?;
    ctx.store.read(|s| s.bot(id).cloned())
}

/// `GET /coworkers/{bot}/ceiling` (`agui/ceiling.rs` `get_ceiling`).
fn get_ceiling(ctx: &HubCtx, req: &HubRequest) -> HubResponse {
    match bot_of(ctx, req) {
        Some(b) => reply(ctx, &b),
        None => no_bot(),
    }
}

/// `GET /coworkers/{bot}/tools` (`agui/routes/computer.rs` `list_tools`):
/// `{"tools": [{name, description, kind, mode}]}`, the tools a turn of this
/// bot is offered — the known rows the ceiling lets through. `mode` is
/// `always`; approvals are another slice's.
fn get_tools(ctx: &HubCtx, req: &HubRequest) -> HubResponse {
    let Some(b) = bot_of(ctx, req) else { return no_bot() };
    let tools: Vec<Value> = ceiling::known(ctx, &b)
        .into_iter()
        .filter(|k| k.on && b.tools.allows(&k.name))
        .map(|k| {
            let mut row = json!({ "name": k.name, "description": k.description, "kind": k.kind, "mode": "always" });
            if let Some(l) = k.label {
                row["label"] = json!(l);
            }
            row
        })
        .collect();
    answer(StatusCode::OK, &json!({ "tools": tools }))
}

/// The most rows a ceiling holds: every known row plus the unknown ones
/// a PUT may keep; past this a write is a mistake or an attack, not a
/// Tools card.
const ROWS_MAX: usize = 1024;

/// Why a write did not land.
enum Unsaved {
    /// `agui/ceiling.rs`: the write names a version that has moved (409).
    Stale,
    /// The bot was removed between the read and the write.
    Gone,
    /// app.db did not take the write.
    Db(String),
}

/// Store `rows` as the ceiling of the bot `id` at the next version, if
/// `version` (what the PUT read, when it says) is still the stored one;
/// the bot as saved. The check and the write share the store's lock, so
/// two PUTs that read one version cannot both land.
fn save(ctx: &HubCtx, id: &str, version: Option<i64>, rows: Vec<Value>) -> Result<Bot, Unsaved> {
    ctx.store
        .write(|s| {
            let b = s.bots.iter_mut().find(|b| b.id == id).ok_or(Unsaved::Gone)?;
            if version.is_some_and(|v| v != b.tools.version) {
                return Err(Unsaved::Stale);
            }
            b.tools = Ceiling { rows, version: b.tools.version + 1 };
            Ok(b.clone())
        })
        .unwrap_or_else(|e| Err(Unsaved::Db(e)))
}

fn unsaved(why: Unsaved) -> HubResponse {
    match why {
        Unsaved::Stale => answer(StatusCode::CONFLICT, &json!({ "error": "the tools changed since you looked", "code": "ceiling-changed" })),
        Unsaved::Gone => no_bot(),
        Unsaved::Db(e) => refusal(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `PUT /coworkers/{bot}/ceiling` (`agui/ceiling.rs` `put_ceiling`): the
/// body names what stays on — `{"enabled": [names], "version"}` as
/// opengrok's does, or `{"tools": [rows], "version"}` echoing a GET with
/// `enabled` flipped. Rows naming tools this broker does not know are
/// kept as sent. Answers as the GET does.
fn put_ceiling(ctx: &HubCtx, req: &HubRequest) -> HubResponse {
    let Some(b) = bot_of(ctx, req) else { return no_bot() };
    let Some(body) = read_body(&req.body) else {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, "send {\"enabled\": [names]} or {\"tools\": [rows]}");
    };
    let version = body.get("version").and_then(Value::as_i64);
    let enabled: Option<Vec<String>> = body.get("enabled").and_then(Value::as_array).map(|a| a.iter().map(js::string_of).collect());
    let sent: Vec<Value> = body.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
    if enabled.is_none() && body.get("tools").and_then(Value::as_array).is_none() {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, "send {\"enabled\": [names]} or {\"tools\": [rows]}");
    }
    if version.is_some_and(|v| v != b.tools.version) {
        return unsaved(Unsaved::Stale);
    }
    let rows = ceiling::settle(ctx, &b, &sent, enabled.as_deref());
    if rows.len() > ROWS_MAX {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, &format!("too many tool rows (at most {ROWS_MAX})"));
    }
    match save(ctx, &b.id, version, rows) {
        Ok(saved) => reply(ctx, &saved),
        Err(why) => unsaved(why),
    }
}

/// `PUT /coworkers/{bot}/tool-mode` (`agui/ceiling.rs` `put_tool_mode`):
/// `{"tool", "mode"}`, mode `always`, `ask` or `never`; `never` switches the
/// row off, the others on (`ask` means on here: asking is the approval
/// slice's). Answers `{"tool", "mode"}`.
fn put_tool_mode(ctx: &HubCtx, req: &HubRequest) -> HubResponse {
    let Some(b) = bot_of(ctx, req) else { return no_bot() };
    let Some(body) = read_body(&req.body) else { return refusal(StatusCode::UNPROCESSABLE_ENTITY, "send {\"tool\", \"mode\"}") };
    let tool = body.get("tool").and_then(Value::as_str).unwrap_or_default().to_owned();
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or_default().to_owned();
    if !matches!(mode.as_str(), "always" | "ask" | "never") {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, "mode is always, ask or never");
    }
    let version = body.get("version").and_then(Value::as_i64);
    if version.is_some_and(|v| v != b.tools.version) {
        return unsaved(Unsaved::Stale);
    }
    let known = ceiling::known(ctx, &b);
    if !known.iter().any(|k| k.name == tool) && b.tools.row(&tool).is_none() {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, &format!("this Bot has no tool named {tool}"));
    }
    let on = mode != "never";
    let mut sent: Vec<Value> = ceiling::rows(ctx, &b);
    for r in sent.iter_mut() {
        if r.get("name").and_then(Value::as_str) == Some(tool.as_str()) {
            r["enabled"] = json!(on);
        }
    }
    let rows = ceiling::settle(ctx, &b, &sent, None);
    match save(ctx, &b.id, version, rows) {
        Ok(_) => answer(StatusCode::OK, &json!({ "tool": tool, "mode": mode })),
        Err(why) => unsaved(why),
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use hyper::header::HeaderMap;
    use hyper::Method;
    use serde_json::{json, Value};

    use super::{save, Unsaved, ROWS_MAX};
    use crate::hub::store::{Bot, Ceiling};
    use crate::hub::tools::ceiling;
    use crate::hub::traits::Clock;
    use crate::hub::types::HubRequest;
    use crate::hub::{Hub, HubCtx};

    fn hub(label: &str) -> Hub {
        let dir = std::env::temp_dir().join(format!("rox-ceiling-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
        let ctx = HubCtx::new(dir, Clock::manual());
        ctx.store.load().unwrap();
        ctx.store
            .write(|s| {
                s.bots.push(Bot::new("ann", "Ann", "star", "#FF309B", None));
            })
            .unwrap();
        ctx.db
            .with(|c| {
                c.execute("INSERT INTO skills(id,name,description,body,enabled,created_at) VALUES('tone','Tone Guide','how to talk','Be brief.',1,1)", [])?;
                c.execute("INSERT INTO mcp_servers(id,name,enabled) VALUES('acme','Acme',1)", [])?;
                c.execute("INSERT INTO mcp_servers(id,name,enabled) VALUES('dark','Dark',0)", [])
            })
            .unwrap();
        Hub::with(ctx, Vec::new(), |routes, _| super::install(routes))
    }

    async fn send(hub: &Hub, method: Method, url: &str, body: Value) -> (u16, Value) {
        let body = if body.is_null() { Bytes::new() } else { Bytes::from(body.to_string()) };
        let res = hub.answer(HubRequest { method, url: url.to_owned(), headers: HeaderMap::new(), body, params: Vec::new() }).await;
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn row<'a>(v: &'a Value, name: &str) -> &'a Value {
        v["tools"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or(&Value::Null)
    }

    #[tokio::test]
    async fn ceiling_get_lists_every_builtin_server_and_skill_on_at_version_0() {
        let hub = hub("get");
        let (status, v) = send(&hub, Method::GET, "/coworkers/ann/ceiling", Value::Null).await;
        assert_eq!(status, 200);
        assert_eq!(v["version"], 0);
        assert_eq!(row(&v, "shell_run")["kind"], "builtin");
        assert_eq!(row(&v, "shell_run")["enabled"], true);
        assert!(row(&v, "shell_run")["description"].as_str().is_some_and(|d| !d.is_empty()));
        for name in ["routine_create", "memory_add", "agent_send"] {
            assert_eq!(row(&v, name)["kind"], "builtin", "{name}");
        }
        assert_eq!(row(&v, "acme"), &json!({"name": "acme", "kind": "plugin", "description": "", "enabled": true, "label": "Acme"}));
        assert_eq!(row(&v, "skill:tone"), &json!({"name": "skill:tone", "kind": "skill", "description": "how to talk", "enabled": true, "label": "Tone Guide"}));
        assert_eq!(row(&v, "dark")["available"], false, "a server the global switch holds off is listed, marked unavailable");
        let (status, v) = send(&hub, Method::GET, "/coworkers/nobody/ceiling", Value::Null).await;
        assert_eq!((status, v), (404, json!({"ok": false, "code": "not_found", "message": "no such bot", "retryable": false})));
    }

    #[tokio::test]
    async fn ceiling_put_switches_rows_keeps_unknown_ones_and_refuses_a_stale_version() {
        let hub = hub("put");
        let (status, v) = send(&hub, Method::PUT, "/coworkers/ann/ceiling", json!({"enabled": ["fetch_url", "acme", "skill:tone", "ghost"], "version": 0})).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["version"], 1);
        assert_eq!(row(&v, "shell_run")["enabled"], false);
        assert_eq!(row(&v, "fetch_url")["enabled"], true);
        assert_eq!(row(&v, "acme")["enabled"], true);
        assert_eq!(row(&v, "ghost"), &json!({"name": "ghost", "kind": "plugin", "enabled": true, "available": false}), "an unknown name is kept, not dropped");
        // it persisted
        let saved = hub.ctx().store.read(|s| s.bot("ann").map(|b| b.tools.clone())).unwrap();
        assert_eq!(saved.version, 1);
        assert!(!saved.allows("shell_run") && saved.allows("ghost"));
        // a stale version: 409 in opengrok's shape, nothing written
        let (status, v) = send(&hub, Method::PUT, "/coworkers/ann/ceiling", json!({"enabled": ["shell_run"], "version": 0})).await;
        assert_eq!((status, v), (409, json!({"error": "the tools changed since you looked", "code": "ceiling-changed"})));
        assert_eq!(hub.ctx().store.read(|s| s.bot("ann").map(|b| b.tools.version)), Some(1));
        // rows as a GET answered, with enabled flipped, and an unknown row with its own fields
        let (status, v) = send(&hub, Method::PUT, "/coworkers/ann/ceiling", json!({"tools": [{"name": "shell_run", "kind": "builtin", "enabled": true}, {"name": "zed", "kind": "plugin", "enabled": true, "connector": "zed-cloud"}], "version": 1})).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["version"], 2);
        assert_eq!(row(&v, "shell_run")["enabled"], true);
        assert_eq!(row(&v, "fetch_url")["enabled"], true, "a row the write left out keeps its switch");
        assert_eq!(row(&v, "zed")["connector"], "zed-cloud");
        assert_eq!(row(&v, "ghost")["enabled"], true, "a stored unknown row survives a write that did not name it");
        // bad bodies and no bot
        assert_eq!(send(&hub, Method::PUT, "/coworkers/ann/ceiling", json!({"version": 2})).await.0, 422);
        assert_eq!(send(&hub, Method::PUT, "/coworkers/nobody/ceiling", json!({"enabled": []})).await.0, 404);
        // more rows than any Tools card sends
        let flood: Vec<Value> = (0..ROWS_MAX + 1).map(|i| json!({"name": format!("x{i}"), "enabled": true})).collect();
        assert_eq!(send(&hub, Method::PUT, "/coworkers/ann/ceiling", json!({"tools": flood})).await.0, 422);
        assert_eq!(hub.ctx().store.read(|s| s.bot("ann").map(|b| b.tools.version)), Some(2), "nothing written");
        // two writes that read one version: the check sits under the store's lock, so only the first lands
        let ann = hub.ctx().store.read(|s| s.bot("ann").cloned()).unwrap();
        let rows = ceiling::settle(hub.ctx(), &ann, &[], Some(&["fetch_url".to_owned()]));
        assert!(save(hub.ctx(), "ann", Some(2), rows.clone()).is_ok());
        assert!(matches!(save(hub.ctx(), "ann", Some(2), rows), Err(Unsaved::Stale)));
        assert!(matches!(save(hub.ctx(), "nobody", None, Vec::new()), Err(Unsaved::Gone)));
    }

    #[tokio::test]
    async fn ceiling_tools_lists_what_the_next_turn_is_offered() {
        let hub = hub("tools");
        let (status, v) = send(&hub, Method::GET, "/coworkers/ann/tools", Value::Null).await;
        assert_eq!(status, 200);
        assert_eq!(row(&v, "shell_run"), &json!({"name": "shell_run", "description": row(&v, "shell_run")["description"], "kind": "builtin", "mode": "always"}));
        assert_eq!(row(&v, "acme")["mode"], "always");
        assert_eq!(row(&v, "dark"), &Value::Null, "a server the global switch holds off is not offered: ceiling ∩ global");
        assert_eq!(send(&hub, Method::PUT, "/coworkers/ann/ceiling", json!({"enabled": ["fetch_url"]})).await.0, 200);
        let (_, v) = send(&hub, Method::GET, "/coworkers/ann/tools", Value::Null).await;
        let names: Vec<&str> = v["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["fetch_url"]);
        assert_eq!(send(&hub, Method::GET, "/coworkers/nobody/tools", Value::Null).await.0, 404);
    }

    #[tokio::test]
    async fn ceiling_tool_mode_never_switches_a_row_off_and_always_back_on() {
        let hub = hub("mode");
        let (status, v) = send(&hub, Method::PUT, "/coworkers/ann/tool-mode", json!({"tool": "shell_run", "mode": "never"})).await;
        assert_eq!((status, v), (200, json!({"tool": "shell_run", "mode": "never"})));
        let (_, v) = send(&hub, Method::GET, "/coworkers/ann/ceiling", Value::Null).await;
        assert_eq!(v["version"], 1);
        assert_eq!(row(&v, "shell_run")["enabled"], false);
        assert_eq!(row(&v, "fetch_url")["enabled"], true, "the other rows stay on");
        let (status, _) = send(&hub, Method::PUT, "/coworkers/ann/tool-mode", json!({"tool": "shell_run", "mode": "ask"})).await;
        assert_eq!(status, 200);
        assert!(hub.ctx().store.read(|s| s.bot("ann").map(|b| b.tools.allows("shell_run"))).unwrap());
        assert_eq!(send(&hub, Method::PUT, "/coworkers/ann/tool-mode", json!({"tool": "shell_run", "mode": "sometimes"})).await.0, 422);
        assert_eq!(send(&hub, Method::PUT, "/coworkers/ann/tool-mode", json!({"tool": "nope", "mode": "never"})).await.0, 422);
        assert_eq!(send(&hub, Method::PUT, "/coworkers/ann/tool-mode", json!({"tool": "shell_run", "mode": "never", "version": 0})).await.0, 409);
        assert_eq!(send(&hub, Method::PUT, "/coworkers/nobody/tool-mode", json!({"tool": "shell_run", "mode": "never"})).await.0, 404);
        let _ = Ceiling::default();
    }
}
