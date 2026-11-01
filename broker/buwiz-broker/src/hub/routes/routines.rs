//! The v0 routines Rox shows in a chat's Details pane (routes/routines.mjs):
//! the list, the on/off switch and interval, Delete, Run now, and the
//! templates connected servers suggest. Node matches `req.url` exactly, so
//! a query string makes any of them Node's final 404 ("not found").

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Response, StatusCode};
use serde_json::{json, Map, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::routines::{templates, Routines};
use crate::hub::types::{self, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;

/// Node's answer when no route took a request: 404 "not found".
fn not_found() -> HubResponse {
    let mut res = Response::new(Full::new(Bytes::from_static(b"not found")).map_err(|never| match never {}).boxed());
    *res.status_mut() = StatusCode::NOT_FOUND;
    res
}

pub fn install(routes: &mut Registry, rs: Arc<Routines>) {
    let list = rs.clone();
    routes.route(Verb::Get, Pattern::Exact("/routines"), move |ctx, req| {
        let rs = list.clone();
        async move {
            if req.url != "/routines" {
                return not_found();
            }
            types::json(StatusCode::OK, &json!({"routines": rs.v0.list(&ctx.db)}))
        }
    });
    let tpl = rs.clone();
    routes.route(Verb::Get, Pattern::Exact("/routines/templates"), move |ctx, req| {
        let rs = tpl.clone();
        async move {
            if req.url != "/routines/templates" {
                return not_found();
            }
            types::json(StatusCode::OK, &json!(templates::list(&ctx.db, rs.host.as_ref(), rs.v0.mcp.as_ref()).await))
        }
    });
    // any method: another one is Node's final 404, not the office role's 501
    for path in ["/routine/update", "/routine/delete", "/routine/run", "/routine/from-template"] {
        let rs = rs.clone();
        routes.route(Verb::Any, Pattern::Exact(path), move |ctx, req| {
            let rs = rs.clone();
            async move { change(rs, ctx, req).await }
        });
    }
}

/// POST /routine/update {id, enabled?, every_s?, prompt?, name?} → {ok,
/// routine}; /routine/delete {id} → {ok}; /routine/run {id} → {ok,
/// routine} once the tick is done; /routine/from-template {server, id,
/// thread, user_google_email?} → {ok, routine}, or {ok: false, error,
/// need: "user_google_email"}. A refusal is 400 {ok: false, error}.
async fn change(rs: Arc<Routines>, ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    if req.method != Method::POST || !["/routine/update", "/routine/delete", "/routine/run", "/routine/from-template"].contains(&req.url.as_str()) {
        return not_found();
    }
    let text = String::from_utf8_lossy(&req.body);
    let j: Value = match serde_json::from_str(if text.is_empty() { "{}" } else { &text }) {
        Ok(j) => j,
        Err(e) => return refused(&format!("{e}"), None),
    };
    if j.is_null() {
        return refused("Cannot read properties of null (reading 'id')", None);
    }
    let done = match req.url.as_str() {
        "/routine/delete" => rs.v0.delete(&ctx, j.get("id")).map(|()| None),
        "/routine/run" => rs.v0.run(&ctx, j.get("id")).await.map(Some),
        "/routine/from-template" => rs.v0.from_template(&ctx, &j).await.map(Some),
        _ => rs.v0.update(&ctx, &j).map(Some),
    };
    match done {
        Ok(None) => types::json(StatusCode::OK, &json!({"ok": true})),
        Ok(Some(routine)) => types::json(StatusCode::OK, &json!({"ok": true, "routine": routine})),
        Err(e) => refused(&e.error, e.need),
    }
}

fn refused(error: &str, need: Option<&str>) -> HubResponse {
    let mut out = Map::new();
    out.insert("ok".into(), json!(false));
    out.insert("error".into(), json!(error));
    if let Some(n) = need {
        out.insert("need".into(), json!(n));
    }
    types::json(StatusCode::BAD_REQUEST, &Value::Object(out))
}
