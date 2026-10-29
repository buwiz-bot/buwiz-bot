//! The line: the plugin host's API for Node (`/v1/line/*`), which keeps
//! the Rox-facing routes, the plugin list, routines and sessions and calls
//! here for everything that touches a plugin process
//! (`server/src/v1/plugins-line.mjs`). Every request carries
//! `X-Rox-Line: <token>`; the token is made by whoever starts this host and
//! handed to Node in its environment. Bodies are JSON; refusals are the
//! spec's `{ok: false, code, message, retryable}`.
//!
//! | route | body | answer |
//! |---|---|---|
//! | `GET events` | | SSE: a `snapshot` of every plugin, `ready`, then a `plugin` event per change |
//! | `POST plugins/inspect`, `plugins/install` | the host API's body | its answer |
//! | `POST plugins/<id>/enable\|disable\|uninstall\|tools\|fail` | the host API's body; `fail`: `{code, message}` | its answer, plus `runtime` |
//! | `POST call` | `{plugin, tool, args, meta?, timeout_ms?}` | `{ok, result}` (the raw tools/call result) |
//! | `POST reserved` | `{plugin, name, args}` | `{ok, value}` (`null` when the plugin has no such tool) |
//! | `POST resource` | `{plugin, uri}` | `{ok, result}` |
//! | `POST start`, `stop` | | `{ok}` |
//! | `POST clock` | `{now?, fire?}` | `{ok, fired}`; a manual clock only (tests) |
//!
//! Answers that change a plugin carry its `runtime` snapshot, so Node's
//! view is current when the call returns, before the event arrives.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::{Frame, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};

use super::plugins::Host;
use crate::js;
use crate::proxy::Body;
use crate::v1::{get, tpl, V1Error};

const BODY_MAX: usize = 2 * 1024 * 1024;

pub struct Line {
    host: Arc<Host>,
    token: String,
}

impl Line {
    pub fn new(host: Arc<Host>, token: String) -> Self {
        Self { host, token }
    }

    pub fn host(&self) -> &Arc<Host> {
        &self.host
    }

    fn authorized(&self, req: &Request<Incoming>) -> bool {
        let got = req.headers().get("x-rox-line").map(|v| v.as_bytes()).unwrap_or_default();
        let want = self.token.as_bytes();
        // constant time over the token's length
        !want.is_empty() && got.len() == want.len() && got.iter().zip(want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }

    pub async fn serve(&self, req: Request<Incoming>) -> Response<Body> {
        if !self.authorized(&req) {
            return answer(StatusCode::UNAUTHORIZED, &V1Error::new("unauthorized", "X-Rox-Line is missing or wrong").body());
        }
        let path = req.uri().path().strip_prefix("/v1/line/").unwrap_or("").to_owned();
        if req.method() == Method::GET && path == "events" {
            return self.events();
        }
        if req.method() != Method::POST {
            return answer(StatusCode::METHOD_NOT_ALLOWED, &V1Error::new("not_supported", format!("{} /v1/line/{path}", req.method())).body());
        }
        let body = match req.into_body().collect().await {
            Ok(b) => b.to_bytes(),
            Err(_) => Bytes::new(),
        };
        let body: Value = match body.len() {
            0 => json!({}),
            n if n > BODY_MAX => Value::Null,
            _ => serde_json::from_slice(&body).unwrap_or(Value::Null),
        };
        if !body.is_object() {
            return answer(StatusCode::BAD_REQUEST, &V1Error::new("invalid_args", "the body must be a JSON object (at most 2 MiB)").body());
        }
        match self.run(&path, &body).await {
            Ok(v) => answer(StatusCode::OK, &v),
            Err(e) => {
                if e.code == "internal" {
                    eprintln!("  v1 line {path}: {}", js::slice_utf16(&e.message, 200));
                }
                answer(status_of(&e.code), &e.body())
            }
        }
    }

    async fn run(&self, path: &str, b: &Value) -> Result<Value, V1Error> {
        let h = &self.host;
        let str_of = |k: &str| get(b, k).and_then(Value::as_str).unwrap_or_default().to_owned();
        if let Some(rest) = path.strip_prefix("plugins/") {
            match rest {
                "inspect" => return h.inspect(b).await,
                "install" => {
                    let mut r = h.install(b).await?;
                    if let Some(id) = get(&r, "id").and_then(Value::as_str).map(str::to_owned) {
                        r["runtime"] = h.snapshot(&id);
                    }
                    return Ok(r);
                }
                _ => {}
            }
            let Some((id, action)) = rest.rsplit_once('/') else { return V1Error::err("not_found", format!("no route /v1/line/{path}")) };
            let id = js::decode_uri_component(id).map_err(|_| V1Error::new("invalid_args", "a malformed plugin id"))?;
            let mut r = match action {
                "enable" => h.enable(&id).await?,
                "disable" => h.disable(&id).await?,
                "uninstall" => h.uninstall(&id).await?,
                "tools" => h.set_tool(&id, get(b, "tool"), get(b, "disabled"))?,
                "fail" => h.fail(&id, &str_of("code"), &str_of("message")).await?,
                _ => return V1Error::err("not_found", format!("no route /v1/line/{path}")),
            };
            if get(&r, "runtime").is_none() {
                r["runtime"] = h.snapshot(&id);
            }
            return Ok(r);
        }
        match path {
            "call" => {
                let timeout = get(b, "timeout_ms").and_then(Value::as_f64).filter(|t| *t > 0.0).map(|t| Duration::from_millis(t as u64));
                let r = h.call_tool(&str_of("plugin"), &tpl(get(b, "tool")), get(b, "args").unwrap_or(&json!({})), get(b, "meta").filter(|m| js::truthy(m)), timeout).await?;
                Ok(json!({"ok": true, "result": r}))
            }
            "reserved" => Ok(json!({"ok": true, "value": h.call_reserved(&str_of("plugin"), &str_of("name"), get(b, "args").cloned().unwrap_or(json!({}))).await?})),
            "resource" => Ok(json!({"ok": true, "result": h.read_resource(&str_of("plugin"), &str_of("uri")).await?})),
            "start" => {
                h.start_all().await;
                Ok(json!({"ok": true}))
            }
            "stop" => {
                h.stop_all().await;
                Ok(json!({"ok": true}))
            }
            "clock" => {
                let now = get(b, "now").and_then(Value::as_f64);
                let manual = matches!(h.clock, super::plugins::Clock::Manual(_));
                if !manual {
                    return V1Error::err("not_supported", "this host runs on the system clock");
                }
                Ok(json!({"ok": true, "fired": h.clock.manual_step(now, get(b, "fire") == Some(&Value::Bool(true)))}))
            }
            _ => V1Error::err("not_found", format!("no route /v1/line/{path}")),
        }
    }

    /// Every plugin's snapshot, then each change, as SSE; a ping every 25 s.
    fn events(&self) -> Response<Body> {
        let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(64);
        let host = self.host.clone();
        let mut sub = host.events();
        tokio::spawn(async move {
            let event = |name: &str, v: &Value| Bytes::from(format!("event: {name}\ndata: {}\n\n", js::stringify(v)));
            let all = |host: &Host| host.snapshots().iter().map(|v| event("snapshot", v)).collect::<Vec<_>>();
            let mut first = all(&host);
            first.insert(0, Bytes::from_static(b": rox line events\n\n"));
            first.push(event("ready", &json!({})));
            for e in first {
                if tx.send(e).await.is_err() {
                    return;
                }
            }
            let mut beat = tokio::time::interval(Duration::from_secs(25));
            beat.tick().await;
            loop {
                let out = tokio::select! {
                    got = sub.recv() => match got {
                        Ok(v) => vec![event("plugin", &v)],
                        // fell behind: everything again, as changes
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => host.snapshots().iter().map(|v| event("plugin", v)).collect(),
                        Err(_) => return,
                    },
                    _ = beat.tick() => vec![Bytes::from_static(b": ping\n\n")],
                };
                for b in out {
                    if tx.send(b).await.is_err() {
                        return;
                    }
                }
            }
        });
        let mut res = Response::new(Channel(rx).boxed());
        let h = res.headers_mut();
        h.insert("content-type", "text/event-stream".parse().expect("static"));
        h.insert("cache-control", "no-cache".parse().expect("static"));
        res
    }
}

/// v1/routes.mjs's statuses for refusals.
fn status_of(code: &str) -> StatusCode {
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

fn answer(status: StatusCode, v: &Value) -> Response<Body> {
    let mut res = Response::new(Full::new(Bytes::from(js::stringify(v))).map_err(|never: Infallible| match never {}).boxed());
    *res.status_mut() = status;
    res.headers_mut().insert("content-type", "application/json".parse().expect("static"));
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
