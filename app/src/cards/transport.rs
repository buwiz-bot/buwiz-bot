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
/// JSON, as its text. `None` when the server could not be reached.
async fn call(method: &str, path: &str, body: Option<&Value>) -> Option<(u16, Value)> {
    call_as(method, path, body, None).await
}

/// [`call`], with the host token as `X-Buwiz-Host` when `token` is one:
/// only for a broker that has just proven it holds it.
async fn call_as(method: &str, path: &str, body: Option<&Value>, token: Option<&str>) -> Option<(u16, Value)> {
    let url = format!("{}{path}", client::backend());
    let mut r = HttpRequest::new(method, url);
    if let Some(token) = token {
        r.headers.push(("X-Buwiz-Host".to_owned(), token.to_owned()));
    }
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

/// This Mac's id for its local-exec rules: `BUWIZ_MACHINE_ID`, until Rox
/// enrols a daemon of its own.
pub(crate) fn machine_id() -> String {
    buwiz_protocol::env::var("BUWIZ_MACHINE_ID").unwrap_or_default()
}

/// `POST /local-exec/policy/rule {machineId, kind: allow|deny, pattern}`.
pub(crate) fn rule_body(kind: &str, pattern: &str) -> Value {
    json!({ "machineId": machine_id(), "kind": kind, "pattern": pattern })
}

pub(crate) async fn write_rule(kind: &str, pattern: &str) -> Sent {
    send("POST", "/local-exec/policy/rule", rule_body(kind, pattern)).await
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
    user_form_send(SUBMIT_PATH, body).await
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

// ---------------------------------------------------------------------------
// Saved logins: only to a broker that proves itself (GHSA-m7pv-qvp8-4mf2)
// ---------------------------------------------------------------------------

/// The card's sentence when a saved login stayed in the app.
pub(crate) const UNPROVEN: &str = "Buwiz Bot couldn't verify its broker, so the saved login wasn't sent.";

/// The broker's proof route (`buwiz-broker` `hub/routes/host.rs`): no token
/// on the request, an HMAC of the nonce under the token in the answer.
pub(crate) const PROVE_PATH: &str = "/v1/host/prove";

const SUBMIT_PATH: &str = "/ag-ui/user-form/submit";

/// A submit whose `values` came from the vault (a saved login's password
/// or name, a code minted from a saved seed): sent, with `X-Buwiz-Host`,
/// only once the server on the broker's port has proven it holds the host
/// token in the 0600 file. Otherwise nothing leaves the app and the card
/// says why. Values the person typed go by [`submit_user_form`].
pub(crate) async fn submit_saved_login(body: Value) -> UserFormReply {
    let nonce = buwiz_protocol::keys::b64(&buwiz_protocol::keys::random::<32>());
    let reply = send_proven(client::host_token(), &nonce, body, |path, body, token| async move {
        call_as("POST", path, Some(&body), token.as_deref()).await
    })
    .await;
    if reply == UserFormReply::MissingRoute {
        log::info!("rox cards: the broker has no {SUBMIT_PATH}; a saved login went to it and was not used");
        not_connected();
    }
    reply
}

/// [`submit_saved_login`] over `post(path, body, token)`, which answers
/// `(status, body)` or `None` when nothing could be reached: the proof
/// first (no token on it), then the submit with the token, or nothing.
/// Logs never carry a value.
pub(crate) async fn send_proven<F, Fut>(token: Option<String>, nonce: &str, body: Value, mut post: F) -> UserFormReply
where
    F: FnMut(&'static str, Value, Option<String>) -> Fut,
    Fut: std::future::Future<Output = Option<(u16, Value)>>,
{
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        log::warn!("rox cards: no host token to check the broker with; a saved login was not sent");
        return UserFormReply::Refused(UNPROVEN.to_owned());
    };
    let proven = match post(PROVE_PATH, json!({ "nonce": nonce }), None).await {
        Some((200, reply)) => reply
            .get("proof")
            .and_then(Value::as_str)
            .is_some_and(|proof| buwiz_protocol::proof::proof_matches(&token, nonce, proof)),
        Some((status, _)) => {
            log::warn!("rox cards: the broker answered {PROVE_PATH} {status}; a saved login was not sent");
            false
        }
        None => false,
    };
    if !proven {
        log::warn!("rox cards: the broker did not prove itself; a saved login was not sent");
        return UserFormReply::Refused(UNPROVEN.to_owned());
    }
    match post(SUBMIT_PATH, body, Some(token)).await {
        Some((status, reply)) => user_form_action_from_http(status, &reply),
        None => UserFormReply::MissingRoute,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Runs a future whose every await is ready (the fake transport's).
    fn ready<T>(f: impl std::future::Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => panic!("the fake transport never waits"),
        }
    }

    /// A broker that answers the proof with `prove` (given the nonce) and
    /// the submit 200 settled; it records each path and whether it carried
    /// the token, never a body.
    fn run(token: Option<&str>, prove: impl Fn(&str) -> Option<(u16, Value)>) -> (UserFormReply, Vec<(String, Option<String>)>) {
        let seen = RefCell::new(Vec::new());
        let nonce = buwiz_protocol::keys::b64(&[9u8; 32]);
        let body = submit_body("e_1", "cw_1", serde_json::Map::from_iter([("password".to_owned(), json!("s3cret"))]), Some("sl_1"), None);
        let reply = ready(send_proven(token.map(str::to_owned), &nonce, body, |path, body, token| {
            seen.borrow_mut().push((path.to_owned(), token));
            let out = if path == PROVE_PATH {
                assert_eq!(body["nonce"], json!(nonce));
                prove(&nonce)
            } else {
                Some((200, json!({ "formResolution": "submitted" })))
            };
            std::future::ready(out)
        }));
        (reply, seen.into_inner())
    }

    #[test]
    fn a_saved_login_goes_with_the_token_to_a_broker_that_proves_it_holds_it() {
        let (reply, seen) = run(Some("tok"), |n| Some((200, json!({ "ok": true, "proof": buwiz_protocol::proof::broker_proof("tok", n) }))));
        assert!(matches!(reply, UserFormReply::Settled(_)), "{reply:?}");
        assert_eq!(seen, [(PROVE_PATH.to_owned(), None), (SUBMIT_PATH.to_owned(), Some("tok".to_owned()))], "no token on the proof");
    }

    #[test]
    fn a_saved_login_stays_in_the_app_when_the_broker_does_not_prove_itself() {
        let unproven = UserFormReply::Refused(UNPROVEN.to_owned());
        let only_proof = vec![(PROVE_PATH.to_owned(), None)];
        // a proof under another token, a wrong-shaped one, Node's 401, a 404, no server
        type Answer = Box<dyn Fn(&str) -> Option<(u16, Value)>>;
        let answers: Vec<Answer> = vec![
            Box::new(|n| Some((200, json!({ "proof": buwiz_protocol::proof::broker_proof("other", n) })))),
            Box::new(|_| Some((200, json!({ "proof": 42 })))),
            Box::new(|_| Some((200, json!({ "formResolution": "submitted" })))),
            Box::new(|n| Some((401, json!({ "proof": buwiz_protocol::proof::broker_proof("tok", n) })))),
            Box::new(|_| Some((404, json!("no")))),
            Box::new(|_| None),
        ];
        for (i, prove) in answers.into_iter().enumerate() {
            let (reply, seen) = run(Some("tok"), prove);
            assert_eq!(reply, unproven, "answer {i}");
            assert_eq!(seen, only_proof, "answer {i}: the submit was sent");
        }
        // no token file: nothing at all is sent
        for token in [None, Some("")] {
            let (reply, seen) = run(token, |_| panic!("asked for a proof without a token"));
            assert_eq!(reply, unproven);
            assert!(seen.is_empty(), "{seen:?}");
        }
        assert_eq!(UNPROVEN, "Buwiz Bot couldn't verify its broker, so the saved login wasn't sent.");
    }
}
