//! The agents' HTTP side (v1/routes.mjs; plan 5.6, track E):
//!
//! - `POST /v1/host/agents/archive|resume|accepts|card` and
//!   `/v1/host/agents/endpoints/create|list|rotate|delete`: the host
//!   token first, then a JSON object body, then `{ok: true, …}` or the
//!   refusal with the host routes' status map. Other methods on them fall
//!   through to `host.rs`'s catch-all (Node's 405).
//! - `POST /v1/hooks/ah_<id>/<secret>`: a signed message from outside, read
//!   as its raw bytes (past 512 KB, a string one byte too long); 202.
//! - `/v1/bridge/*`: rox-bridge, a Claude Code session's way in, with its
//!   endpoint's token as `Authorization: Bearer`.

use std::sync::Arc;

use hyper::{Method, StatusCode};
use serde_json::{json, Map, Value};

use crate::hub::agents::external::{ExternalError, BODY_MAX};
use crate::hub::agents::{str_or_empty, AgentError, AgentsHub};
use crate::hub::ctx::HubCtx;
use crate::hub::policy::{self, Change};
use crate::hub::routes::host::{authorized, read_body, status_of};
use crate::hub::tools::miniapps::post_in;
use crate::hub::types::{self, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;
use crate::v1::V1Error;

const HOST: &[&str] = &[
    "/v1/host/agents/archive",
    "/v1/host/agents/resume",
    "/v1/host/agents/accepts",
    "/v1/host/agents/card",
    "/v1/host/agents/endpoints/create",
    "/v1/host/agents/endpoints/list",
    "/v1/host/agents/endpoints/rotate",
    "/v1/host/agents/endpoints/delete",
];

pub fn install(routes: &mut Registry, hub: Arc<AgentsHub>) {
    for path in HOST {
        let hub = hub.clone();
        routes.route(Verb::Post, Pattern::Exact(path), move |ctx, req| {
            let hub = hub.clone();
            async move { host(hub, ctx, req, path) }
        });
    }
    let h = hub.clone();
    routes.route(Verb::Any, Pattern::Prefix("/v1/bridge/"), move |ctx, req| {
        let hub = h.clone();
        async move { bridge(hub, ctx, req).await }
    });
    routes.agent_hooks(move |ctx, req| {
        let hub = hub.clone();
        async move { hook(hub, ctx, req) }
    });
}

fn refusal(code: &str, message: impl Into<String>) -> HubResponse {
    let e = V1Error::new(code, message);
    types::json(status_of(&e.code), &e.body())
}

/// An agents or endpoints call as a host route: `{ok: true, …}`, or its
/// refusal as a V1Error (`agentsCall`, `externalCall`).
fn host(hub: Arc<AgentsHub>, ctx: Arc<HubCtx>, req: HubRequest, path: &str) -> HubResponse {
    if !authorized(&req.headers) {
        return types::json(StatusCode::UNAUTHORIZED, &V1Error::new("unauthorized", "X-Buwiz-Host is missing or wrong").body());
    }
    let Some(b) = read_body(&req.body) else { return refusal("invalid_args", "the body must be a JSON object (at most 2 MiB)") };
    let a = &hub.agents;
    let x = &hub.externals;
    let done = |extra: Result<Map<String, Value>, ExternalError>| -> Result<Value, (String, String)> {
        let mut o = Map::new();
        o.insert("ok".into(), json!(true));
        o.extend(extra.map_err(|e| (e.code, e.message))?);
        Ok(Value::Object(o))
    };
    let agents = |r: Result<(), AgentError>| r.map(|_| json!({"ok": true})).map_err(|e| (e.code, e.message));
    let out = match path.strip_prefix("/v1/host/agents/").unwrap_or("") {
        "archive" => agents(hub.write(&ctx, |s, _| a.archive(s, b.get("conversation"), b.get("archived") != Some(&Value::Bool(false))))),
        "resume" => agents(hub.write(&ctx, |s, _| a.resume(s, b.get("conversation")))),
        "accepts" => host_accepts(&ctx, &b),
        "card" => host_card(&ctx, &b),
        "endpoints/create" => done(x.create(&b, |bot| ctx.store.read(|s| a.party(s, bot).is_some()))),
        "endpoints/list" => done(x.list(b.get("bot")).map(|l| Map::from_iter([("endpoints".to_owned(), l)]))),
        "endpoints/rotate" => done(x.rotate(b.get("id"))),
        _ => done(hub.remove_endpoint(&ctx, b.get("id")).map(|_| Map::new())),
    };
    match out {
        Ok(v) => types::json(StatusCode::OK, &v),
        Err((code, message)) => {
            if code == "internal" {
                eprintln!("  v1 {path}: {}", js::slice_utf16(&message, 200));
            }
            refusal(&code, message)
        }
    }
}

/// `POST /v1/host/agents/accepts {bot, accepts}`, "Accept messages from": "all", "none", or
/// a list of agent ids; → `{ok, text, undo}` (`undo` null when it changed nothing). The one
/// place a [`Change::Accepts`] is made: a person's, through this token-locked route.
pub(crate) fn host_accepts(ctx: &HubCtx, b: &Value) -> Result<Value, (String, String)> {
    let bot = str_or_empty(b.get("bot"));
    if ctx.store.read(|s| s.bot(&bot).is_none()) {
        return Err(("invalid_args".into(), "no such agent".into()));
    }
    let to = match b.get("accepts") {
        Some(Value::String(a)) if a == "all" || a == "none" => b["accepts"].clone(),
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => b["accepts"].clone(),
        _ => return Err(("invalid_args".into(), "accepts is \"all\", \"none\" or a list of agent ids".into())),
    };
    // `from` is read as it applies
    policy::applied(ctx, &Change::Accepts { bot, to, from: Value::Null }).map_err(|e| (e.code, e.message))
}

/// `POST /v1/host/agents/card {chat, bot, said?}`: a `messages` notice for `bot` in `chat`,
/// `said` the typed line it answers. The card holds no state: the app draws it from what is
/// true now, so an old one in the scrollback is never wrong.
pub(crate) fn host_card(ctx: &HubCtx, b: &Value) -> Result<Value, (String, String)> {
    let (chat, bot) = (str_or_empty(b.get("chat")), str_or_empty(b.get("bot")));
    let Some(name) = ctx.store.read(|s| s.bot(&bot).map(|b| b.name.clone())) else {
        return Err(("invalid_args".into(), "no such agent".into()));
    };
    let said = js::slice_utf16(&str_or_empty(b.get("said")), 200);
    if !post_in(ctx, &chat, &json!({"kind": "messages", "bot": bot, "botName": name, "said": said})) {
        return Err(("not_found".into(), "no such chat".into()));
    }
    Ok(json!({"ok": true}))
}

/// `auth.replace(/^Bearer\s+/i, "")`.
fn bearer(auth: &str) -> String {
    if auth.len() >= 6 && auth.is_char_boundary(6) && auth[..6].eq_ignore_ascii_case("bearer") {
        let rest = &auth[6..];
        let token = rest.trim_start_matches(js::is_space);
        if token.len() != rest.len() {
            return token.to_owned();
        }
    }
    auth.to_owned()
}

/// A bridge refusal: `{ok: false, code, message, retryable: false}`.
fn bridge_refusal(status: u16, code: &str, message: &str) -> HubResponse {
    types::json(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), &json!({"ok": false, "code": code, "message": js::slice_utf16(message, 300), "retryable": false}))
}

/// `/v1/bridge/*` (`bridgeRoute`).
async fn bridge(hub: Arc<AgentsHub>, ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let path = js::pathname(&req.url);
    let auth = req.headers.get("authorization").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    let r = match hub.externals.bridge(&bearer(&auth)) {
        Ok(r) => r,
        Err(e) => return bridge_refusal(e.status, &e.code, &e.message),
    };
    let me = ctx.store.read(|s| hub.agents.party(s, &format!("ext:{}", r.id)));
    let body = if req.method == Method::POST { read_body(&req.body).unwrap_or_else(|| json!({})) } else { json!({}) };
    let agent_err = |e: AgentError| if e.code == "internal" { bridge_refusal(500, "internal", &e.message) } else { bridge_refusal(400, &e.code, &e.message) };
    match path.as_str() {
        "/v1/bridge/agents" => {
            let agents = ctx.store.read(|s| s.bot(&r.bot).map(|b| json!([{"id": b.id, "name": b.name, "description": crate::hub::agents::description(b)}])));
            types::json(StatusCode::OK, &json!({"ok": true, "agents": agents.unwrap_or_else(|| json!([]))}))
        }
        "/v1/bridge/send" if req.method == Method::POST => {
            let id = body.get("id").cloned();
            match hub.write(&ctx, |s, out| hub.agents.send_from(s, &r.id, &body, id, out)) {
                Ok(sent) => {
                    let mut o = Map::new();
                    o.insert("ok".into(), json!(true));
                    o.extend(sent.as_object().cloned().unwrap_or_default());
                    types::json(StatusCode::ACCEPTED, &Value::Object(o))
                }
                Err(e) => agent_err(e),
            }
        }
        "/v1/bridge/receive" if req.method == Method::POST => {
            let got = hub.externals.receive(&r, &body).await;
            let mut o = Map::new();
            o.insert("ok".into(), json!(true));
            o.extend(got.as_object().cloned().unwrap_or_default());
            types::json(StatusCode::OK, &Value::Object(o))
        }
        "/v1/bridge/conversations" => {
            let id = me.map(|p| p.id).unwrap_or_default();
            let list = ctx.store.read(|s| hub.agents.conversations(s, &id, &body));
            types::json(StatusCode::OK, &json!({"ok": true, "conversations": list}))
        }
        _ => bridge_refusal(404, "not_found", &format!("no route {path}")),
    }
}

/// `POST /v1/hooks/ah_…/<secret>`: an agent's webhook, a signed message
/// read as its raw bytes; 202 with `{ok, id, conversation}`.
fn hook(hub: Arc<AgentsHub>, ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let raw = if req.body.len() > BODY_MAX { "x".repeat(BODY_MAX + 1) } else { String::from_utf8_lossy(&req.body).into_owned() };
    let (Ok(id), Ok(secret)) = (js::decode_uri_component(&req.params[0]), js::decode_uri_component(&req.params[1])) else {
        return types::json(StatusCode::INTERNAL_SERVER_ERROR, &json!({"ok": false, "code": "internal", "message": "URI malformed", "retryable": false}));
    };
    let header = req.headers.get("x-buwiz-signature").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    match hub.externals.incoming(&hub, &ctx, &id, &secret, &raw, header.as_deref()) {
        Ok(answer) => {
            let mut o = Map::new();
            o.insert("ok".into(), json!(true));
            o.extend(answer.as_object().cloned().unwrap_or_default());
            types::json(StatusCode::ACCEPTED, &Value::Object(o))
        }
        Err(e) => types::json(StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), &json!({"ok": false, "code": e.code, "message": e.message, "retryable": e.status == 429})),
    }
}
