//! The host-internal API Rox calls (`/v1/host/*`, v1/routes.mjs; plan
//! 5.5h). Every request carries `X-Buwiz-Host: <token>`; the token is 32
//! random bytes written to `<data dir>/host-token` (0600) when the hub
//! starts, after the port is ours, and a request without it is answered
//! 401 before anything else. The Origin and JSON guard has run already.
//! Bodies are JSON objects of at most 2 MiB; refusals are `{ok: false,
//! code, message, retryable}` with the status map below. Plugin actions
//! and sessions are the plugin host's (`helper::plugins`,
//! `helper::sessions`); the routine routes are `hub::routines::v1`'s.
//! An install may take settings from another plugin (`env_from`), and
//! `notices/settle` answers a mini app's install or uninstall card.
//! A mini app's accounts (2.1 §4): `GET /v1/host/plugins/<id>/accounts`,
//! `POST /v1/host/plugins/<id>/accounts/<account>/{signin,signout,remove}`;
//! a sign-in settles the chat's `account_required` cards for it
//! (`connected`); the user's Not now settles them `declined`.
//! `POST /v1/hooks/<instance>/<secret>`, a routine's webhook, takes no
//! token: the secret is the key.
//!
//! The agents' `/v1/host/agents/*` routes (5.6, track E) answer 501 here
//! until that track registers them; it checks the token with
//! [`authorized`].

use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Frame;
use hyper::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use hyper::{Method, Response, StatusCode};
use serde_json::{json, Value};

use crate::helper::plugins::Host;
use crate::hub::ctx::HubCtx;
use crate::hub::routines::store_host::settle_notices;
use crate::hub::routines::v1::V1;
use crate::hub::routines::Routines;
use crate::hub::types::{self, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;
use crate::v1::V1Error;

const BODY_MAX: usize = 2 * 1024 * 1024;

/// The agents' host routes (track E's).
const AGENT_ROUTES: &[&str] = &[
    "/v1/host/agents/archive",
    "/v1/host/agents/resume",
    "/v1/host/agents/accepts",
    "/v1/host/agents/endpoints/create",
    "/v1/host/agents/endpoints/list",
    "/v1/host/agents/endpoints/rotate",
    "/v1/host/agents/endpoints/delete",
];

/// The token of the hub this process runs, for routes other tracks
/// register under `/v1/host/` (the agents').
static TOKEN: Mutex<Option<String>> = Mutex::new(None);

/// A fresh token in `<data dir>/host-token` (0600), replacing the last
/// one (`initHostToken`).
pub fn init_token(rs: &Routines, data: &Path) {
    let token = rox_v1::keys::b64(&rox_v1::keys::random::<32>());
    let file = data.join("host-token");
    if let Err(e) = write_token(&file, &token) {
        eprintln!("rox-broker: the host token {}: {e}", file.display());
    }
    *rs.token.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.clone());
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(token);
}

fn write_token(file: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(file)?;
    f.write_all(token.as_bytes())?;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
}

fn same(got: &[u8], want: &[u8]) -> bool {
    !want.is_empty() && got.len() == want.len() && got.iter().zip(want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Whether `headers` carry this process's host token (constant time).
pub fn authorized(headers: &HeaderMap) -> bool {
    let want = TOKEN.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    same(headers.get("x-buwiz-host").map(|v| v.as_bytes()).unwrap_or_default(), want.as_bytes())
}

fn authorized_by(rs: &Routines, headers: &HeaderMap) -> bool {
    let want = rs.token.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    same(headers.get("x-buwiz-host").map(|v| v.as_bytes()).unwrap_or_default(), want.as_bytes())
}

/// v1/routes.mjs's `STATUS`: a refusal's HTTP status.
pub fn status_of(code: &str) -> StatusCode {
    match code {
        "disabled" => StatusCode::CONFLICT,
        "not_found" => StatusCode::NOT_FOUND,
        "plugin_unavailable" | "plugin_restarted" => StatusCode::SERVICE_UNAVAILABLE,
        "timeout" => StatusCode::GATEWAY_TIMEOUT,
        "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        "internal" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    }
}

fn refusal(status: StatusCode, code: &str, message: impl Into<String>) -> HubResponse {
    types::json(status, &V1Error::new(code, message).body())
}

/// `readBody`: the body as a JSON object (`{}` when empty); `None` past
/// 2 MiB or when it is no object.
pub fn read_body(body: &[u8]) -> Option<Value> {
    if body.len() > BODY_MAX {
        return None;
    }
    if body.is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str::<Value>(&String::from_utf8_lossy(body)).ok().filter(Value::is_object)
}

/// A route's answer: 200 with it, or its refusal with the status map.
fn sent(path: &str, out: Result<Value, V1Error>) -> HubResponse {
    match out {
        Ok(mut v) => {
            // the plugin host's answers carry the line's snapshot
            if let Some(o) = v.as_object_mut() {
                o.remove("runtime");
            }
            types::json(StatusCode::OK, &v)
        }
        Err(e) => {
            if e.code == "internal" {
                eprintln!("  v1 {path}: {}", js::slice_utf16(&e.message, 200));
            }
            types::json(status_of(&e.code), &e.body())
        }
    }
}

pub fn install(routes: &mut Registry, rs: Arc<Routines>) {
    routes.on_start(settle_on_sign_in);
    let host = rs.clone();
    routes.route(Verb::Any, Pattern::Prefix("/v1/host/"), move |ctx, req| {
        let rs = host.clone();
        async move { answer(rs, ctx, req).await }
    });
    routes.routine_hooks(move |ctx, req| {
        let rs = rs.clone();
        async move { hook(rs, ctx, req) }
    });
}

async fn answer(rs: Arc<Routines>, ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    if !authorized_by(&rs, &req.headers) {
        return refusal(StatusCode::UNAUTHORIZED, "unauthorized", "X-Buwiz-Host is missing or wrong");
    }
    let path = req.path().to_owned();
    if req.method == Method::GET && path == "/v1/host/plugins" {
        return types::json(StatusCode::OK, &json!({"ok": true, "plugins": rs.v1.list(&ctx.db)}));
    }
    if req.method == Method::GET && path == "/v1/host/events" {
        return events(&ctx);
    }
    if let Some(raw) = path.strip_prefix("/v1/host/plugins/").and_then(|r| r.strip_suffix("/accounts")).filter(|r| req.method == Method::GET && !r.is_empty() && !r.contains('/')) {
        let out = match (js::decode_uri_component(raw), ctx.plugins.as_ref()) {
            (Err(_), _) => Err(V1Error::new("internal", "URI malformed")),
            (_, None) => Err(V1Error::new("plugin_unavailable", "this broker hosts no v1 plugins")),
            (Ok(id), Some(host)) => match host.row(&id) {
                Some(_) => Ok(json!({"ok": true, "id": id, "accounts": host.accounts_view(&id)})),
                None => Err(V1Error::new("not_found", format!("{} is not installed", js::slice_utf16(&id, 80)))),
            },
        };
        return sent(&path, out);
    }
    if req.method != Method::POST {
        return refusal(StatusCode::METHOD_NOT_ALLOWED, "not_supported", format!("{} {path}", req.method));
    }
    if AGENT_ROUTES.contains(&path.as_str()) {
        return types::not_ported();
    }
    let Some(route) = Route::of(&path) else { return refusal(StatusCode::NOT_FOUND, "not_found", format!("no route {path}")) };
    let Some(body) = read_body(&req.body) else { return refusal(StatusCode::BAD_REQUEST, "invalid_args", "the body must be a JSON object (at most 2 MiB)") };
    sent(&path, route.run(&rs, &ctx, &body).await)
}

/// v1/routes.mjs's `POSTS` and `ACTIONS`.
enum Route {
    Post(&'static str),
    Action(String, &'static str),
    /// `/v1/host/plugins/<id>/accounts/<account>/<op>`
    Account(String, String, &'static str),
}

const POSTS: &[&str] = &[
    "/v1/host/plugins/inspect",
    "/v1/host/plugins/install",
    "/v1/host/notices/settle",
    "/v1/host/apps/source",
    "/v1/host/sessions/open",
    "/v1/host/sessions/close",
    "/v1/host/call",
    "/v1/host/tickets/title",
    "/v1/host/resolve",
    "/v1/host/routines/approve",
    "/v1/host/routines/enable",
    "/v1/host/routines/disable",
    "/v1/host/routines/run",
    "/v1/host/routines/update",
    "/v1/host/routines/delete",
    "/v1/host/routines/hook",
];

impl Route {
    fn of(path: &str) -> Option<Route> {
        if let Some(p) = POSTS.iter().find(|p| **p == path) {
            return Some(Route::Post(p));
        }
        // /^\/v1\/host\/plugins\/([^/]+)\/(enable|disable|uninstall|tools)$/
        let rest = path.strip_prefix("/v1/host/plugins/")?;
        if let [id, "accounts", account, op] = rest.split('/').collect::<Vec<_>>().as_slice() {
            let op = ["signin", "signout", "remove"].into_iter().find(|o| o == op)?;
            return (!id.is_empty() && !account.is_empty()).then(|| Route::Account((*id).to_owned(), (*account).to_owned(), op));
        }
        let (id, action) = rest.split_once('/')?;
        let action = ["enable", "disable", "uninstall", "tools"].into_iter().find(|a| *a == action)?;
        (!id.is_empty()).then(|| Route::Action(id.to_owned(), action))
    }

    async fn run(&self, rs: &Arc<Routines>, ctx: &Arc<HubCtx>, b: &Value) -> Result<Value, V1Error> {
        let no_host = || V1Error::new("plugin_unavailable", "this broker hosts no v1 plugins");
        let v1 = &rs.v1;
        match self {
            Route::Post(path) => {
                if let Some(op) = path.strip_prefix("/v1/host/routines/") {
                    return match op {
                        "approve" => v1.approve(ctx, b),
                        "enable" => v1.enable(ctx, b),
                        "disable" => v1.disable(ctx, b),
                        "run" => v1.run(ctx, b).await,
                        "update" => v1.update(ctx, b),
                        "delete" => v1.remove(ctx, b),
                        _ => v1.hook(ctx, b),
                    };
                }
                if *path == "/v1/host/notices/settle" {
                    return settle(ctx, b);
                }
                if let Some(op) = path.strip_prefix("/v1/host/plugins/") {
                    let host = ctx.plugins.as_ref().ok_or_else(no_host)?;
                    return if op == "inspect" { host.inspect(b).await } else { install_plugin(ctx, host, b).await };
                }
                let ss = ctx.sessions.as_ref().ok_or_else(no_host)?;
                match *path {
                    "/v1/host/apps/source" => ss.app_source(b).await,
                    "/v1/host/sessions/open" => ss.open(b).await,
                    "/v1/host/sessions/close" => ss.close(b).await,
                    "/v1/host/call" => ss.call(b).await,
                    "/v1/host/tickets/title" => ss.ticket_title(b).await,
                    _ => ss.resolve(b).await,
                }
            }
            Route::Action(raw, action) => {
                let id = js::decode_uri_component(raw).map_err(|_| V1Error::new("internal", "URI malformed"))?;
                let host = ctx.plugins.as_ref().ok_or_else(no_host)?;
                match *action {
                    "enable" => host.enable(&id).await,
                    "disable" => host.disable(&id).await,
                    "uninstall" => {
                        let out = host.uninstall(&id).await?;
                        // 13.6: its routines go with it
                        V1::forget(&ctx.db, &id);
                        Ok(out)
                    }
                    _ => host.set_tool(&id, b.get("tool"), b.get("disabled")),
                }
            }
            Route::Account(raw, account, op) => {
                let (Ok(id), Ok(account)) = (js::decode_uri_component(raw), js::decode_uri_component(account)) else { return V1Error::err("internal", "URI malformed") };
                let host = ctx.plugins.as_ref().ok_or_else(no_host)?;
                match *op {
                    "signin" => host.account_signin(&id, &account, b).await,
                    "signout" => host.account_signout(&id, &account, b).await,
                    _ => host.account_remove(&id, &account, b).await,
                }
            }
        }
    }
}

/// The install body's `env_from: "<installed id>"`: each key of
/// `env_from_keys` the body's `env` lacks is copied from that plugin's
/// env. The values stay in the broker (never logged, never answered).
pub fn with_env_from(host: &Host, b: &Value) -> Result<Value, V1Error> {
    let from = match b.get("env_from") {
        None | Some(Value::Null) => return Ok(b.clone()),
        Some(Value::String(f)) if !f.is_empty() => f,
        Some(_) => return V1Error::err("invalid_args", "env_from must be an installed plugin id"),
    };
    let Some(keys) = b.get("env_from_keys").and_then(Value::as_array).filter(|k| k.iter().all(Value::is_string)) else {
        return V1Error::err("invalid_args", "env_from_keys must be an array of names");
    };
    let Some(row) = host.row(from) else { return V1Error::err("not_found", format!("{} is not installed", js::slice_utf16(from, 80))) };
    let theirs = crate::helper::plugins::parse(&row.env, json!({}));
    let mut out = b.clone();
    let o = out.as_object_mut().expect("the body is an object");
    let mut env = match o.get("env") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return V1Error::err("invalid_args", "env must map names to strings"),
    };
    for k in keys.iter().filter_map(Value::as_str) {
        if env.contains_key(k) {
            continue;
        }
        match theirs.get(k).and_then(Value::as_str) {
            Some(v) => {
                env.insert(k.to_owned(), json!(v));
            }
            None => return V1Error::err("invalid_args", format!("{from} has no {}", js::slice_utf16(k, 80))),
        }
    }
    o.insert("env".into(), Value::Object(env));
    o.shift_remove("env_from");
    o.shift_remove("env_from_keys");
    Ok(out)
}

/// `POST /v1/host/plugins/install` with `env_from`; once it installed,
/// every pending `app_install` card for the id reads `installed`.
pub async fn install_plugin(ctx: &HubCtx, host: &Host, b: &Value) -> Result<Value, V1Error> {
    let out = host.install(&with_env_from(host, b)?).await?;
    if let Some(id) = out.get("id").and_then(Value::as_str) {
        settle_notices(ctx, None, "app_install", id, "installed");
    }
    Ok(out)
}

/// `POST /v1/host/notices/settle {chat, kind, id, state}`: the user's
/// answer on an install or uninstall card.
pub fn settle(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let str_of = |k: &str| b.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let (Some(chat), Some(kind), Some(id), Some(state)) = (str_of("chat"), str_of("kind"), str_of("id"), str_of("state")) else {
        return V1Error::err("invalid_args", "send {chat, kind, id, state}");
    };
    let states: &[&str] = match kind {
        "app_install" => &["declined"],
        "app_uninstall" => &["uninstalled", "cancelled"],
        "account_required" => &["declined"],
        _ => return V1Error::err("invalid_args", "kind must be app_install, app_uninstall or account_required"),
    };
    if !states.contains(&state) {
        return V1Error::err("invalid_args", format!("{kind} settles as {}", states.join(" or ")));
    }
    if ctx.store.read(|s| s.thread(chat).is_none()) {
        return V1Error::err("not_found", format!("no chat {}", js::slice_utf16(chat, 80)));
    }
    let n = settle_notices(ctx, Some(chat), kind, id, state);
    Ok(json!({"ok": true, "settled": n}))
}

/// A signed-in account settles its pending `account_required` cards (the
/// plugin's, for that account) as `connected`.
fn settle_on_sign_in(ctx: Arc<HubCtx>) {
    let Some(mut events) = ctx.plugins.as_ref().map(|h| h.account_events()) else { return };
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(e) if e.get("state").and_then(Value::as_str) == Some("connected") => {
                    settle_signed_in(&ctx, &js::string_of(&e["plugin"]), &js::string_of(&e["account"]));
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return,
            }
        }
    });
}

/// Every chat's pending `account_required` card of `plugin` and `account`
/// reads `connected`; → how many.
pub fn settle_signed_in(ctx: &HubCtx, plugin: &str, account: &str) -> usize {
    let n = ctx
        .store
        .write(|s| {
            let mut n = 0;
            for t in s.list.iter_mut() {
                for u in &mut t.turns {
                    let Some(x) = u.notice.as_mut() else { continue };
                    if x["kind"] == "account_required" && x["id"] == json!(plugin) && x["account"] == json!(account) && x["state"] == "pending" {
                        x["state"] = json!("connected");
                        n += 1;
                    }
                }
            }
            n
        })
        .unwrap_or(0);
    if n > 0 {
        ctx.store.write_index();
    }
    n
}

/// `POST /v1/hooks/<instance>/<secret>`: a webhook instance's run, 202
/// once it started (`hookRoute`).
fn hook(rs: Arc<Routines>, ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let body = read_body(&req.body).unwrap_or_else(|| json!({}));
    let ids = (js::decode_uri_component(&req.params[0]), js::decode_uri_component(&req.params[1]));
    let out = match ids {
        (Ok(id), Ok(secret)) => rs.v1.hook_call(&ctx, &id, &secret, &body),
        _ => Err(V1Error::new("internal", "URI malformed")),
    };
    match out {
        Ok(v) => types::json(StatusCode::ACCEPTED, &v),
        Err(e) => types::json(status_of(&e.code), &e.body()),
    }
}

/// `GET /v1/host/events`: plugin state and generation changes as SSE, a
/// comment line first and a ping every 25 s.
fn events(ctx: &HubCtx) -> HubResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    let sub = ctx.plugins.as_ref().map(|h| h.events());
    tokio::spawn(async move {
        if tx.send(Bytes::from_static(b": rox broker events\n\n")).await.is_err() {
            return;
        }
        let mut beat = tokio::time::interval(Duration::from_secs(25));
        beat.tick().await;
        let mut sub = sub;
        loop {
            let out = tokio::select! {
                got = async { match sub.as_mut() { Some(s) => s.recv().await, None => std::future::pending().await } } => match got {
                    Ok(s) => {
                        let e = json!({"id": s.get("id"), "state": s.get("state"), "code": s.get("code"), "generation": s.get("generation"), "enabled": s.get("enabled")});
                        Bytes::from(format!("event: plugin\ndata: {}\n\n", js::stringify(&e)))
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                },
                _ = beat.tick() => Bytes::from_static(b": ping\n\n"),
                _ = tx.closed() => return,
            };
            if tx.send(out).await.is_err() {
                return;
            }
        }
    });
    let mut res = Response::new(Channel(rx).boxed());
    let h = res.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    h.insert("cache-control", HeaderValue::from_static("no-cache"));
    res
}

/// A response body fed from a channel (the event stream).
struct Channel(tokio::sync::mpsc::Receiver<Bytes>);

impl hyper::body::Body for Channel {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        self.0.poll_recv(cx).map(|chunk| chunk.map(|b| Ok(Frame::data(b))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_routes_name_the_plugin_the_account_and_the_op() {
        let of = |p: &str| match Route::of(p) {
            Some(Route::Account(id, account, op)) => Some((id, account, op)),
            _ => None,
        };
        assert_eq!(of("/v1/host/plugins/app.buwiz.gws/accounts/google/signin"), Some(("app.buwiz.gws".into(), "google".into(), "signin")));
        assert_eq!(of("/v1/host/plugins/app.buwiz.gws/accounts/google/signout").map(|r| r.2), Some("signout"));
        assert_eq!(of("/v1/host/plugins/app.buwiz.gws/accounts/google/remove").map(|r| r.2), Some("remove"));
        assert!(Route::of("/v1/host/plugins/app.buwiz.gws/accounts/google/delete").is_none());
        assert!(Route::of("/v1/host/plugins//accounts/google/signin").is_none());
        assert!(matches!(Route::of("/v1/host/plugins/app.buwiz.gws/enable"), Some(Route::Action(_, "enable"))));
    }
}
