//! The local-commands policy routes the app's Settings → Computer page
//! reads and edits. Transcribed from opengrok-server
//! `crates/opengrok-server/src/local_exec.rs` (`get_policy`, `set_mode`,
//! `add_rule`, `remove_rule`, `policy_listing`): same paths, bodies and
//! answers, minus the account (one person per broker). `machineId` names
//! the machine; absent or empty, it is this broker's own Mac
//! (`policy::local_machine_id`).
//!
//! - `GET /local-exec/policy?machineId=` → `{machineId, mode, allow, deny, inert}`
//! - `PUT /local-exec/policy` `{machineId, mode}` → 204
//! - `POST /local-exec/policy/rule` `{machineId, kind, pattern}` → 204
//! - `DELETE /local-exec/policy/rule` `{machineId, kind, pattern}` → 204
//!
//! Bad input is a 422 with `{error, code}` and changes nothing. The one
//! check a rule must pass is `standing_rule_refusal`: the store writes rows,
//! never policy.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};

use super::chat::parse;
use crate::hub::ctx::HubCtx;
use crate::hub::policy::local_exec::{standing_rule_refusal, LocalExecMode, LocalExecPolicy};
use crate::hub::policy::{self, machine_or_local};
use crate::hub::types::{json as answer, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;

/// opengrok-server `local_exec.rs` `VALID_MODES` / `VALID_KINDS`.
const VALID_MODES: &[&str] = &["never", "ask", "bypass"];
const VALID_KINDS: &[&str] = &["allow", "deny"];

/// `PUT /local-exec/policy` body (opengrok-server `local_exec.rs` `SetMode`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetMode {
    #[serde(default)]
    machine_id: Option<String>,
    mode: String,
}

/// `POST`/`DELETE /local-exec/policy/rule` body (opengrok-server `local_exec.rs` `RuleBody`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleBody {
    #[serde(default)]
    machine_id: Option<String>,
    kind: String,
    pattern: String,
}

pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    if let Err(e) = policy::install(&ctx.db) {
        eprintln!("rox-broker local-exec policy tables: {e}");
    }
    routes.route(Verb::Any, Pattern::Exact("/local-exec/policy"), policy_route);
    routes.route(Verb::Any, Pattern::Exact("/local-exec/policy/rule"), rule_route);
}

/// `GET /local-exec/policy` → `policy_listing` (opengrok-server `local_exec.rs`):
/// `inert` names the stored allow rules the gate would never match, with
/// why, so the page can show them instead of hiding them.
pub fn policy_listing(machine: &str, policy: &LocalExecPolicy) -> Value {
    let inert: Vec<Value> = policy.allow.iter().filter_map(|pattern| standing_rule_refusal("allow", pattern).map(|reason| json!({"pattern": pattern, "reason": reason}))).collect();
    json!({
        "machineId": machine,
        "mode": policy.mode.as_stored(),
        "allow": policy.allow,
        "deny": policy.deny,
        "inert": inert,
    })
}

/// opengrok-server `local_exec.rs` `coded`: `{error, code}` with the status.
fn coded(status: StatusCode, error: &str, code: &str) -> HubResponse {
    answer(status, &json!({ "error": error, "code": code }))
}

fn unprocessable(error: &str) -> HubResponse {
    coded(StatusCode::UNPROCESSABLE_ENTITY, error, "unprocessable")
}

/// A write this API could not make: a 500 that says which (opengrok-server `failed`).
fn failed(what: &str) -> HubResponse {
    coded(StatusCode::INTERNAL_SERVER_ERROR, what, "store")
}

fn no_content() -> HubResponse {
    let mut res = answer(StatusCode::NO_CONTENT, &Value::Null);
    *res.body_mut() = Full::new(Bytes::new()).map_err(|never| match never {}).boxed();
    res
}

fn method_not_allowed() -> HubResponse {
    coded(StatusCode::METHOD_NOT_ALLOWED, "method not allowed", "method_not_allowed")
}

/// The JSON body as `T`, or why it is not one (a 400 for the caller to send).
fn body<T: for<'de> Deserialize<'de>>(req: &HubRequest) -> Result<T, String> {
    let v = parse(&req.body)?;
    serde_json::from_value(v).map_err(|e| format!("bad body: {e}"))
}

fn bad_request(why: &str) -> HubResponse {
    coded(StatusCode::BAD_REQUEST, why, "bad_request")
}

async fn policy_route(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    match req.method {
        Method::GET => {
            let machine = machine_or_local(req.query("machineId").as_deref());
            answer(StatusCode::OK, &policy_listing(&machine, &policy::load(&ctx.db, &machine)))
        }
        Method::PUT => {
            let set: SetMode = match body(&req) {
                Ok(b) => b,
                Err(why) => return bad_request(&why),
            };
            if !VALID_MODES.contains(&set.mode.as_str()) {
                return unprocessable("mode must be never, ask or bypass");
            }
            let machine = machine_or_local(set.machine_id.as_deref());
            match policy::set_mode(&ctx.db, &machine, LocalExecMode::from_stored(&set.mode), ctx.clock.now() as i64) {
                Ok(()) => no_content(),
                Err(_) => failed("could not save the mode"),
            }
        }
        _ => method_not_allowed(),
    }
}

async fn rule_route(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    if req.method != Method::POST && req.method != Method::DELETE {
        return method_not_allowed();
    }
    let rule: RuleBody = match body(&req) {
        Ok(b) => b,
        Err(why) => return bad_request(&why),
    };
    if !VALID_KINDS.contains(&rule.kind.as_str()) {
        return unprocessable("kind must be allow or deny");
    }
    let pattern = rule.pattern.trim();
    if pattern.is_empty() {
        return unprocessable("pattern must not be empty");
    }
    let machine = machine_or_local(rule.machine_id.as_deref());
    if req.method == Method::DELETE {
        return match policy::remove_rule(&ctx.db, &machine, &rule.kind, pattern) {
            Ok(()) => no_content(),
            Err(_) => failed("could not remove the rule"),
        };
    }
    if let Some(why) = standing_rule_refusal(&rule.kind, pattern) {
        return unprocessable(why);
    }
    match policy::add_rule(&ctx.db, &machine, &rule.kind, pattern, ctx.clock.now() as i64) {
        Ok(()) => no_content(),
        Err(_) => failed("could not add the rule"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::traits::Clock;
    use crate::hub::Hub;
    use hyper::header::{HeaderMap, HeaderValue, CONTENT_TYPE};

    fn hub(label: &str) -> Hub {
        let dir = std::env::temp_dir().join(format!("rox-lxp-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
        let ctx = HubCtx::new(dir, Clock::manual());
        ctx.store.load().unwrap();
        Hub::with(ctx, Vec::new(), install)
    }

    async fn call(hub: &Hub, method: Method, url: &str, body: &str) -> (u16, Value) {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let req = HubRequest { method, url: url.into(), headers, body: Bytes::from(body.to_owned()), params: Vec::new() };
        let res = hub.answer(req).await;
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    #[tokio::test]
    async fn local_exec_policy_routes_read_set_and_edit_one_machine() {
        let hub = hub("rw");
        let (s, v) = call(&hub, Method::GET, "/local-exec/policy?machineId=m1", "").await;
        assert_eq!((s, v), (200, json!({"machineId": "m1", "mode": "bypass", "allow": [], "deny": [], "inert": []})));

        let (s, v) = call(&hub, Method::PUT, "/local-exec/policy", r#"{"machineId":"m1","mode":"ask"}"#).await;
        assert_eq!((s, v), (204, Value::Null));
        let (s, _) = call(&hub, Method::POST, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"allow","pattern":" git status "}"#).await;
        assert_eq!(s, 204);
        let (s, _) = call(&hub, Method::POST, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"deny","pattern":"rm"}"#).await;
        assert_eq!(s, 204);
        let (s, v) = call(&hub, Method::GET, "/local-exec/policy?machineId=m1", "").await;
        assert_eq!((s, v), (200, json!({"machineId": "m1", "mode": "ask", "allow": ["git status"], "deny": ["rm"], "inert": []})));
        // another machine is untouched
        let (_, v) = call(&hub, Method::GET, "/local-exec/policy?machineId=m2", "").await;
        assert_eq!(v["mode"], "bypass");
        assert_eq!(v["deny"], json!([]));

        let (s, _) = call(&hub, Method::DELETE, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"deny","pattern":"rm"}"#).await;
        assert_eq!(s, 204);
        let (_, v) = call(&hub, Method::GET, "/local-exec/policy?machineId=m1", "").await;
        assert_eq!(v["deny"], json!([]));
        // a machine the body does not name is this broker's own
        let (s, _) = call(&hub, Method::PUT, "/local-exec/policy", r#"{"mode":"never"}"#).await;
        assert_eq!(s, 204);
        let (_, v) = call(&hub, Method::GET, "/local-exec/policy", "").await;
        assert_eq!((v["machineId"].as_str(), v["mode"].as_str()), (Some(policy::local_machine_id().as_str()), Some("never")));
    }

    #[tokio::test]
    async fn local_exec_policy_routes_refuse_bad_input_and_change_nothing() {
        let hub = hub("bad");
        let (s, v) = call(&hub, Method::PUT, "/local-exec/policy", r#"{"machineId":"m1","mode":"open"}"#).await;
        assert_eq!(s, 422, "{v}");
        assert_eq!(v["code"], "unprocessable");
        let (s, _) = call(&hub, Method::PUT, "/local-exec/policy", r#"{"machineId":"m1"}"#).await;
        assert_eq!(s, 400);
        let (s, _) = call(&hub, Method::PUT, "/local-exec/policy", "not json").await;
        assert_eq!(s, 400);
        let (s, v) = call(&hub, Method::POST, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"maybe","pattern":"ls"}"#).await;
        assert_eq!((s, v["error"].as_str()), (422, Some("kind must be allow or deny")));
        let (s, v) = call(&hub, Method::POST, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"allow","pattern":"  "}"#).await;
        assert_eq!((s, v["error"].as_str()), (422, Some("pattern must not be empty")));
        let (s, v) = call(&hub, Method::POST, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"allow","pattern":"sudo rm"}"#).await;
        assert_eq!((s, v["error"].as_str()), (422, Some("sudo cannot be a standing allow")));
        let (s, _) = call(&hub, Method::POST, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"allow","pattern":"cd src && cargo test"}"#).await;
        assert_eq!(s, 422);
        // deny is the safety net: never refused
        let (s, _) = call(&hub, Method::POST, "/local-exec/policy/rule", r#"{"machineId":"m1","kind":"deny","pattern":"sudo"}"#).await;
        assert_eq!(s, 204);
        let (s, _) = call(&hub, Method::PATCH, "/local-exec/policy", "{}").await;
        assert_eq!(s, 405);
        let (s, _) = call(&hub, Method::GET, "/local-exec/policy/rule", "").await;
        assert_eq!(s, 405);
        let (s, v) = call(&hub, Method::GET, "/local-exec/policy?machineId=m1", "").await;
        assert_eq!((s, v), (200, json!({"machineId": "m1", "mode": "bypass", "allow": [], "deny": ["sudo"], "inert": []})));
    }

    #[test]
    fn local_exec_policy_listing_names_inert_allows() {
        let p = LocalExecPolicy { mode: LocalExecMode::Ask, allow: vec!["ls".into(), "a && b".into()], deny: vec![], session_allow: vec!["git".into()] };
        let v = policy_listing("m1", &p);
        assert_eq!(v["allow"], json!(["ls", "a && b"]));
        assert_eq!(v["inert"][0]["pattern"], "a && b");
        assert!(v["inert"][0]["reason"].as_str().unwrap_or("").contains("one plain command"));
        assert!(v.get("sessionAllow").is_none() && v.get("session_allow").is_none());
    }
}
