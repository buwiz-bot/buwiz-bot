//! How a card's answer leaves Rox: nativechat's REST bodies, sent as they
//! are, and its rules for reading the reply. Today's server has none of
//! these routes, so a missing route (or no server at all) paints the card
//! as answered here, says "Not connected to an agent server yet", and logs
//! the body it would have sent — every flow clicks through offline.

use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::client::{self, urlenc};

/// What came back from a send, read the way a card needs it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Sent {
    /// 2xx: the body (`Null` when there was none).
    Done(Value),
    /// The route is not there (404, not a missing form entry) or the
    /// server is not up: nothing was answered, and nothing can be yet.
    Offline,
    /// Refused or failed, in the server's own words.
    Refused(String),
}

/// The reply to `POST /ag-ui/user-form/submit|dismiss` (nativechat
/// `user_form_action_from_http`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UserFormReply {
    /// 403 `{error, message}`: the server's sentence; the card stays open.
    Refused(String),
    /// 404 `{"error":"form entry missing"}`.
    MissingEntry,
    /// Any other 404, or no server: painted locally, toasted, logged.
    MissingRoute,
    /// 200 with no `formResolution`: painted "Not filled".
    Empty,
    /// `{alreadyAnswered:true}`.
    AlreadyAnswered,
    /// 200 carrying the settled card (`formResolution`, or a Computer
    /// handoff's `handoffEntryId`): the whole body.
    Settled(Value),
    /// Another failure (5xx, …): its sentence; the card stays open.
    Failed(String),
}

/// One request: `(status, body)`, the body as JSON or, when it is not
/// JSON, as its text. `None` when the server could not be reached. The
/// Settings › Computer page reads it raw: a 501 `not_ported` there means
/// "not on this broker yet", not a refusal.
pub(crate) async fn call(method: &str, path: &str, body: Option<&Value>) -> Option<(u16, Value)> {
    let url = format!("{}{path}", client::backend());
    let mut r = HttpRequest::new(method, url);
    if let Some(body) = body {
        r.headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        r.body = nsh_core::platform::HttpBody::Bytes(body.to_string().into_bytes());
    }
    let res = fetch(r).await.ok()?;
    let text = res.text();
    let value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    Some((res.status(), value))
}

/// Sends and reads the reply as [`classify`] does; a miss logs the body it
/// would have sent.
async fn send(method: &str, path: &str, body: Value) -> Sent {
    let sent = match call(method, path, Some(&body)).await {
        Some((status, reply)) => classify(status, &reply),
        None => Sent::Offline,
    };
    if sent == Sent::Offline {
        log_unsent(method, path, &body);
    }
    sent
}

/// A reply in a card's terms: 2xx done; 404 a missing route (this server
/// has none of these); anything else refused in the server's words.
pub(crate) fn classify(status: u16, body: &Value) -> Sent {
    match status {
        200..=299 => Sent::Done(body.clone()),
        404 if !is_form_entry_missing(body) => Sent::Offline,
        _ => Sent::Refused(
            refusal_sentence(body).unwrap_or_else(|| format!("The server answered {status}; nothing was changed.")),
        ),
    }
}

/// The toast a missing route shows, once per answer.
pub(crate) fn not_connected() {
    crate::toast::show("Not connected to an agent server yet");
}

/// What would have gone out, for the log — never a typed value: a form's
/// `values` are logged by field name only.
fn log_unsent(method: &str, path: &str, body: &Value) {
    let mut shown = body.clone();
    if let Some(values) = shown.get_mut("values").and_then(Value::as_object_mut) {
        for v in values.values_mut() {
            *v = Value::String("«redacted»".to_owned());
        }
    }
    log::info!("rox cards: no agent server for {method} {path}; would send {shown}");
}

// ---------------------------------------------------------------------------
// Approvals
// ---------------------------------------------------------------------------

/// `POST /ag-ui/runs/{runId}/answer {call_id, approved}`.
pub(crate) fn answer_body(call_id: &str, approved: bool) -> Value {
    json!({ "call_id": call_id, "approved": approved })
}

pub(crate) async fn answer_run(run_id: &str, call_id: &str, approved: bool) -> Sent {
    let path = format!("/ag-ui/runs/{}/answer", urlenc(run_id));
    send("POST", &path, answer_body(call_id, approved)).await
}

/// This Mac's id for its local-exec rules: `ROX_MACHINE_ID`, until Rox
/// enrols a daemon of its own.
pub(crate) fn machine_id() -> String {
    std::env::var("ROX_MACHINE_ID").unwrap_or_default()
}

/// `POST /local-exec/policy/rule {machineId, kind: allow|deny, pattern}`.
pub(crate) fn rule_body(kind: &str, pattern: &str) -> Value {
    json!({ "machineId": machine_id(), "kind": kind, "pattern": pattern })
}

pub(crate) async fn write_rule(kind: &str, pattern: &str) -> Sent {
    send("POST", "/local-exec/policy/rule", rule_body(kind, pattern)).await
}

/// `DELETE /local-exec/policy/rule {machineId, kind, pattern}` (the same
/// body as the add; opengrok-server `local_exec.rs` `remove_rule`).
pub(crate) async fn remove_rule(kind: &str, pattern: &str) -> Option<(u16, Value)> {
    call("DELETE", "/local-exec/policy/rule", Some(&rule_body(kind, pattern))).await
}

/// `PUT /local-exec/policy {machineId, mode: never|ask|bypass}`
/// (opengrok-server `local_exec.rs` `SetMode`).
pub(crate) fn mode_body(mode: &str) -> Value {
    json!({ "machineId": machine_id(), "mode": mode })
}

pub(crate) async fn write_mode(mode: &str) -> Option<(u16, Value)> {
    call("PUT", "/local-exec/policy", Some(&mode_body(mode))).await
}

/// `GET /local-exec/policy?machine=<id>` (opengrok-server `local_exec.rs`
/// `MachineQuery`, answered by `policy_listing`).
pub(crate) async fn read_policy() -> Option<(u16, Value)> {
    let path = format!("/local-exec/policy?machine={}", urlenc(&machine_id()));
    call("GET", &path, None).await
}

/// `GET /ag-ui/host-settings`: the host record with `egressTunnelEnabled`
/// and `egressTunnelAvailable` (opengrok-server `agui/routes/host.rs`
/// `host_settings_reply`).
pub(crate) async fn read_host_settings() -> Option<(u16, Value)> {
    call("GET", "/ag-ui/host-settings", None).await
}

/// `PUT /ag-ui/host-settings {egressTunnelEnabled}`: a partial record,
/// answered with the whole one (opengrok-server `agui/routes/host.rs`
/// `patch_host_settings`, mounted as `put` in `agui/routes.rs`).
pub(crate) async fn write_host_settings(patch: Value) -> Option<(u16, Value)> {
    call("PUT", "/ag-ui/host-settings", Some(&patch)).await
}

/// `PUT /coworkers/{id}/computer/egress-policy {mode}`.
pub(crate) async fn write_egress(coworker: &str, mode: &str) -> Sent {
    let path = format!("/coworkers/{}/computer/egress-policy", urlenc(coworker));
    send("PUT", &path, json!({ "mode": mode })).await
}

/// `PUT /coworkers/{id}/pins/{connector} {connectionId}`: a plugin-needs
/// card's "Remember for <bot>".
pub(crate) async fn pin_account(coworker: &str, connector: &str, connection: &str) -> Sent {
    let path = format!("/coworkers/{}/pins/{}", urlenc(coworker), urlenc(connector));
    send("PUT", &path, json!({ "connectionId": connection })).await
}

// ---------------------------------------------------------------------------
// User forms (the bodies and reply rules C2's card uses)
// ---------------------------------------------------------------------------

/// `POST /ag-ui/user-form/submit {entryId, agentId, values, savedLogin?,
/// savedLoginId?, username?}`. `values` exist only here, on their way out.
pub(crate) fn submit_body(
    entry_id: &str,
    agent_id: &str,
    values: serde_json::Map<String, Value>,
    saved_login_id: Option<&str>,
    username: Option<&str>,
) -> Value {
    let mut body = json!({ "entryId": entry_id, "agentId": agent_id, "values": Value::Object(values) });
    if let Some(id) = saved_login_id.filter(|id| !id.is_empty()) {
        body["savedLogin"] = Value::Bool(true);
        body["savedLoginId"] = Value::String(id.to_owned());
    }
    if let Some(name) = username.filter(|n| !n.is_empty()) {
        body["username"] = Value::String(name.to_owned());
    }
    body
}

/// `POST /ag-ui/user-form/dismiss {entryId, agentId, mode}`, `mode`
/// `dismissed` or `escalated`.
pub(crate) fn dismiss_body(entry_id: &str, agent_id: &str, mode: &str) -> Value {
    json!({ "entryId": entry_id, "agentId": agent_id, "mode": mode })
}

pub(crate) async fn submit_user_form(body: Value) -> UserFormReply {
    user_form_send("/ag-ui/user-form/submit", body).await
}

pub(crate) async fn dismiss_user_form(body: Value) -> UserFormReply {
    user_form_send("/ag-ui/user-form/dismiss", body).await
}

/// A user-form send; a missing route toasts and logs (values redacted).
async fn user_form_send(path: &str, body: Value) -> UserFormReply {
    let reply = match call("POST", path, Some(&body)).await {
        Some((status, reply)) => user_form_action_from_http(status, &reply),
        None => UserFormReply::MissingRoute,
    };
    if reply == UserFormReply::MissingRoute {
        log_unsent("POST", path, &body);
        not_connected();
    }
    reply
}

/// nativechat `user_form_action_from_http`.
pub(crate) fn user_form_action_from_http(status: u16, body: &Value) -> UserFormReply {
    if status == 403 {
        return UserFormReply::Refused(
            refusal_sentence(body).unwrap_or_else(|| "The server would not fill this form on that computer.".to_owned()),
        );
    }
    if status == 404 {
        return if is_form_entry_missing(body) { UserFormReply::MissingEntry } else { UserFormReply::MissingRoute };
    }
    if !(200..300).contains(&status) {
        return UserFormReply::Failed(
            refusal_sentence(body).unwrap_or_else(|| format!("The server answered {status}; the form is still open.")),
        );
    }
    if body.is_null() {
        return UserFormReply::Empty;
    }
    let resolved = |v: &Value| {
        v.get("formResolution").is_some_and(|r| r.as_str().is_some_and(|s| !s.is_empty()) || r.is_object())
            || v.get("handoffEntryId").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
    };
    if resolved(body) || body.get("value").is_some_and(resolved) {
        return UserFormReply::Settled(body.clone());
    }
    if body.get("alreadyAnswered").and_then(Value::as_bool).unwrap_or(false) {
        return UserFormReply::AlreadyAnswered;
    }
    UserFormReply::Empty
}

/// The 404 body a gone card answers with.
pub(crate) fn is_form_entry_missing(body: &Value) -> bool {
    body.get("error").and_then(Value::as_str).is_some_and(|e| e.eq_ignore_ascii_case("form entry missing"))
}

/// The sentence a refusal carries (nativechat `refusal_words`): `message`
/// beside a code under `error`, or `error` itself; a text body is its text.
pub(crate) fn refusal_sentence(body: &Value) -> Option<String> {
    if let Some(text) = body.as_str() {
        return Some(text.trim().to_owned()).filter(|t| !t.is_empty());
    }
    let field = |k: &str| body.get(k).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let (error, message) = (field("error"), field("message"));
    if field("code").is_some() {
        return error.or(message).map(str::to_owned);
    }
    match error {
        Some(code) if is_error_code(code) => Some(message.unwrap_or(code).to_owned()),
        error => error.or(message).map(str::to_owned),
    }
}

/// `shared-computer`, `form_entry_missing`: a code, not a sentence.
fn is_error_code(text: &str) -> bool {
    !text.contains(' ') && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A refusal fit to end a sentence with: one line of plain text.
pub(crate) fn plain_reason(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty() && !t.contains('\n') && !t.starts_with(['{', '[', '<'])).then(|| t.trim_end_matches('.').to_owned() + ".")
}
