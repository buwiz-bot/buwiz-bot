//! Routes answered in Rust (plan phase 2): leaves that hold nothing in
//! memory on either side, so Node and Rust can share their tables row by
//! row (decision D7). Each one answers as Node's handler does, byte for
//! byte where a client could tell (server/test/leaf-routes.test.mjs).

mod keymap;
mod prefs;
mod skills;
mod state;
mod timeclock;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderValue, CONTENT_TYPE, ORIGIN, VARY};
use hyper::{Method, Request, Response, StatusCode};
use rusqlite::Connection;

use crate::proxy::Body;

/// The route groups `/meta` lists as ported.
pub const PORTED: &[&str] = &["state", "prefs", "keymap", "timeclock", "skills"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Route {
    StateGet,
    StatePost,
    Prefs,
    Keymap,
    Timeclock,
    ClockIn,
    ClockOut,
    Skills,
}

/// The route a request takes here, matched as Node matches it (`req.url`,
/// query included); `None` goes to Node. OPTIONS always goes to Node, which
/// answers it before any route.
pub fn route(method: &Method, url: &str) -> Option<Route> {
    let (get, post) = (method == Method::GET, method == Method::POST);
    match url {
        _ if method == Method::OPTIONS => None,
        "/prefs" => Some(Route::Prefs),
        "/keymap" => Some(Route::Keymap),
        "/data/timeclock" if get => Some(Route::Timeclock),
        "/data/timeclock/in" if post => Some(Route::ClockIn),
        "/data/timeclock/out" if post => Some(Route::ClockOut),
        "/skills" if get => Some(Route::Skills),
        _ if get && url.starts_with("/state") => Some(Route::StateGet),
        _ if post && url.starts_with("/state") => Some(Route::StatePost),
        _ => None,
    }
}

/// An answer before CORS: status, whether Node set `content-type:
/// application/json` on it, and the body.
pub struct Answer {
    status: u16,
    json: bool,
    body: String,
}

impl Answer {
    fn json(status: u16, body: impl Into<String>) -> Self {
        Self { status, json: true, body: body.into() }
    }

    /// A body Node sends without setting a content type.
    fn bare(status: u16, body: impl Into<String>) -> Self {
        Self { status, json: false, body: body.into() }
    }

    /// `{ok: false, error}`.
    fn refused(status: u16, json: bool, error: &str) -> Self {
        Self { status, json, body: format!("{{\"ok\":false,\"error\":{}}}", crate::js::quote(error)) }
    }

    /// What Node's dispatcher sends when a route throws.
    fn thrown(error: &str) -> Self {
        Self::json(500, format!("{{\"error\":{}}}", crate::js::quote(error)))
    }
}

pub struct Leaf {
    data: PathBuf,
    allowed: Vec<String>,
    db: Mutex<Option<Connection>>,
}

impl Leaf {
    /// `data` is Node's data dir (`GROK_DATA_DIR`); `allowed` the origins
    /// the guard lets through.
    pub fn new(data: PathBuf, allowed: Vec<String>) -> Self {
        Self { data, allowed, db: Mutex::new(None) }
    }

    pub async fn serve(&self, route: Route, req: Request<Incoming>) -> Response<Body> {
        let origin = req.headers().get(ORIGIN).cloned();
        if let Some((status, error)) = crate::guard::refusal(req.method(), req.headers(), &self.allowed) {
            return respond(Answer::refused(status.as_u16(), true, &error), None);
        }
        let method = req.method().clone();
        let url = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/").to_owned();
        let body = match req.into_body().collect().await {
            Ok(b) => String::from_utf8_lossy(&b.to_bytes()).into_owned(),
            Err(_) => String::new(),
        };
        let answer = tokio::task::block_in_place(|| self.answer(route, &method, &url, &body));
        respond(answer, origin)
    }

    fn answer(&self, route: Route, method: &Method, url: &str, body: &str) -> Answer {
        match route {
            Route::StateGet => state::get(self, url),
            Route::StatePost => state::post(self, url, body),
            Route::Prefs if method == Method::GET => prefs::get(self),
            Route::Prefs => prefs::post(self, body),
            Route::Keymap if method == Method::GET => keymap::get(self),
            Route::Keymap => keymap::post(self, body),
            Route::Timeclock => timeclock::get(self),
            Route::ClockIn => timeclock::clock_in(self),
            Route::ClockOut => timeclock::clock_out(self),
            Route::Skills => skills::get(self),
        }
    }

    /// app.db, opened on first use: Node's file, journal mode left as Node
    /// set it, and a busy timeout so neither side fails on the other's lock.
    fn db<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        let mut slot = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            let conn = Connection::open(self.data.join("generated").join("app.db")).map_err(|e| e.to_string())?;
            conn.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
            // the tables Rust writes, as Node creates them (state.mjs)
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS kv(name TEXT PRIMARY KEY, data TEXT);
                 CREATE TABLE IF NOT EXISTS timeclock(
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   clock_in TEXT NOT NULL, clock_out TEXT);",
            )
            .map_err(|e| e.to_string())?;
            *slot = Some(conn);
        }
        f(slot.as_ref().expect("opened above")).map_err(|e| e.to_string())
    }
}

/// The answer as Node sends it, with Node's CORS headers when the request
/// had an (allowed) Origin.
fn respond(answer: Answer, origin: Option<HeaderValue>) -> Response<Body> {
    let mut res = Response::new(Full::new(Bytes::from(answer.body)).map_err(|never| match never {}).boxed());
    *res.status_mut() = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let h = res.headers_mut();
    if answer.json {
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    if let Some(origin) = origin {
        h.insert("access-control-allow-origin", origin);
        h.insert("access-control-allow-headers", HeaderValue::from_static("content-type"));
        h.insert(VARY, HeaderValue::from_static("Origin"));
    }
    res
}

/// A column's value as node:sqlite hands it to JavaScript.
fn sql_json(v: rusqlite::types::ValueRef<'_>) -> serde_json::Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::json!(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
        ValueRef::Text(t) => serde_json::Value::String(String::from_utf8_lossy(t).into_owned()),
        // a Uint8Array: JSON.stringify writes it as an object of its bytes
        ValueRef::Blob(b) => serde_json::Value::Object(b.iter().enumerate().map(|(i, byte)| (i.to_string(), serde_json::json!(byte))).collect()),
    }
}

/// A JSON body as Node's handlers parse it: `JSON.parse(body || "{}")`.
fn parse_body(body: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(if body.is_empty() { "{}" } else { body }).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_match_as_node_matches_them() {
        let (get, post) = (Method::GET, Method::POST);
        assert_eq!(route(&get, "/state/x"), Some(Route::StateGet));
        assert_eq!(route(&get, "/state?name=x"), Some(Route::StateGet));
        assert_eq!(route(&post, "/statefoo"), Some(Route::StatePost));
        assert_eq!(route(&Method::PUT, "/state/x"), None);
        assert_eq!(route(&Method::OPTIONS, "/prefs"), None);
        assert_eq!(route(&Method::PUT, "/prefs"), Some(Route::Prefs));
        assert_eq!(route(&get, "/prefs?x=1"), None);
        assert_eq!(route(&post, "/keymap"), Some(Route::Keymap));
        assert_eq!(route(&get, "/data/timeclock"), Some(Route::Timeclock));
        assert_eq!(route(&get, "/data/timeclock/in"), None);
        assert_eq!(route(&post, "/data/timeclock/out"), Some(Route::ClockOut));
        assert_eq!(route(&get, "/skills"), Some(Route::Skills));
        assert_eq!(route(&post, "/skills"), None);
        assert_eq!(route(&get, "/threads"), None);
    }
}
