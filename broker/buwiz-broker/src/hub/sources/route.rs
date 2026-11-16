//! `POST /w/<name>/<ticket>/call`: a widget's one way to a data source. No host token: the
//! ticket in the path is the proof. Every answer is HTTP 200 with JSON, so widget code has
//! one shape to read, but a wrong ticket (403):
//!
//! ```json
//! { "ok": true, "result": … }
//! { "ok": false, "error": { "code": "…", "message": "…", "ask": "<card id, with not_granted>" } }
//! ```
//!
//! The checks, in order: the ticket; the limits (8 calls in flight and 20 new calls a second
//! per widget, counted before anything is looked up, so a widget that calls in a loop costs
//! no more refused or waiting than allowed); the source is listed for the widget's Bot; the
//! action exists; the source is ready; what the person allowed ([`super::grants`]); then
//! the call.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use hyper::StatusCode;
use serde_json::{json, Value};

use super::grants::{self, Verdict};
use super::ticket::{is_widget, valid};
use super::{CallError, Caller};
use crate::hub::ctx::HubCtx;
use crate::hub::types::{self, HubRequest, HubResponse};

/// At most this many calls of one widget at once.
pub const IN_FLIGHT: usize = 8;
/// At most this many new calls of one widget in a second.
pub const PER_SECOND: usize = 20;
/// The arguments of a call, as JSON text.
pub const ARGS_MAX: usize = 256 * 1024;

#[derive(Default)]
struct Meter {
    in_flight: usize,
    recent: VecDeque<f64>,
}

fn meters() -> &'static Mutex<HashMap<String, Meter>> {
    static METERS: OnceLock<Mutex<HashMap<String, Meter>>> = OnceLock::new();
    METERS.get_or_init(Default::default)
}

/// A call under way: its place among the widget's calls, given back when it ends.
struct Slot(String);

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(m) = meters().lock().unwrap_or_else(|e| e.into_inner()).get_mut(&self.0) {
            m.in_flight = m.in_flight.saturating_sub(1);
        }
    }
}

/// Takes a place for a new call of `widget` (in this data dir), or says it is over a limit.
fn admit(ctx: &HubCtx, widget: &str) -> Result<Slot, CallError> {
    let key = format!("{}|{widget}", ctx.data.display());
    let now = ctx.clock.now();
    let mut all = meters().lock().unwrap_or_else(|e| e.into_inner());
    let m = all.entry(key.clone()).or_default();
    while m.recent.front().is_some_and(|t| now - t >= 1000.0) {
        m.recent.pop_front();
    }
    if m.in_flight >= IN_FLIGHT {
        return Err(CallError::new("rate_limited", format!("more than {IN_FLIGHT} calls of this widget are under way")));
    }
    if m.recent.len() >= PER_SECOND {
        return Err(CallError::new("rate_limited", format!("more than {PER_SECOND} calls a second")));
    }
    m.recent.push_back(now);
    m.in_flight += 1;
    Ok(Slot(key))
}

fn failed(status: StatusCode, e: &CallError, ask: Option<&str>) -> HubResponse {
    let mut error = json!({"code": e.code, "message": e.message});
    if let Some(id) = ask {
        error["ask"] = json!(id);
    }
    types::json(status, &json!({"ok": false, "error": error}))
}

/// The call's body: `{source, action, args?}`.
fn parse(body: &[u8]) -> Result<(String, String, Value), CallError> {
    let bad = |m: &str| CallError::new("bad_args", m);
    let v: Value = serde_json::from_slice(body).map_err(|_| bad("send {source, action, args?} as a JSON object"))?;
    let name = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    let (Some(source), Some(action)) = (name("source"), name("action")) else { return Err(bad("send {source, action, args?}: both are names")) };
    let args = match v.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(a) if a.is_object() => a.clone(),
        Some(_) => return Err(bad("args must be an object")),
    };
    if crate::js::stringify(&args).len() > ARGS_MAX {
        return Err(bad("args are over 256 KiB"));
    }
    Ok((source, action, args))
}

pub async fn call(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    // /w/<name>/<ticket>/call: any other shape is no route; an empty ticket is a bad one
    let path = req.path().to_owned();
    let (name, ticket) = match path.strip_prefix("/w/").map(|p| p.split('/').collect::<Vec<_>>()).as_deref() {
        Some([n, t, "call"]) => (*n, *t),
        _ => return types::json(StatusCode::NOT_FOUND, &json!({"ok": false, "code": "not_found", "message": "no such route", "retryable": false})),
    };
    // 1. the ticket: before the body is looked at
    if !is_widget(name) || !valid(&ctx, name, ticket) {
        return failed(StatusCode::FORBIDDEN, &CallError::new("bad_ticket", "this link does not belong to the widget"), None);
    }
    let (source, action, args) = match parse(&req.body) {
        Ok(p) => p,
        Err(e) => return failed(StatusCode::OK, &e, None),
    };
    // 2. the limits: a call that will be refused, or wait for an answer, counts too
    let _slot = match admit(&ctx, name) {
        Ok(s) => s,
        Err(e) => return failed(StatusCode::OK, &e, None),
    };
    let home = grants::home(&ctx, name);
    // 3-5. the source is listed for the Bot, has the action, is ready
    let r = match super::resolve(&ctx, &home.owner, &source, &action).await {
        Ok(r) => r,
        Err(e) => return failed(StatusCode::OK, &e, None),
    };
    // 6. what the person allowed
    match grants::verdict(grants::row(&ctx, name, &r.source.name).as_ref(), &r.action) {
        Verdict::Allowed => {}
        Verdict::Refused(why) => return failed(StatusCode::OK, &CallError::new("refused", why), None),
        Verdict::Ask => {
            return match grants::ask(&ctx, &home, name, &r) {
                Ok(id) => failed(StatusCode::OK, &CallError::new("not_granted", format!("waiting for the person's OK to let this widget use {}", r.source.title)), Some(&id)),
                Err(e) => failed(StatusCode::OK, &e, None),
            };
        }
    }
    // 7. the call
    match super::run(&ctx, Caller::Widget(name), &r, &args).await {
        Ok(result) => types::json(StatusCode::OK, &json!({"ok": true, "result": result})),
        Err(e) => failed(StatusCode::OK, &e, None),
    }
}
