//! What the hub's routes and tools exchange: a path pattern, the request a
//! handler gets, its answer, a turn's context and a tool's result.

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use hyper::{Method, Response, StatusCode};
use serde_json::Value;

use super::jobs::JobId;
use crate::proxy::Body;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The methods a route answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
    Any,
}

impl Verb {
    pub fn allows(self, method: &Method) -> bool {
        match self {
            Verb::Get => method == Method::GET,
            Verb::Post => method == Method::POST,
            Verb::Any => true,
        }
    }
}

/// A path pattern, matched against the path without its query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// the whole path
    Exact(&'static str),
    /// the path starts with it (Node's `req.url.startsWith`)
    Prefix(&'static str),
    /// `/`-separated segments, `*` one non-empty segment (Node's `([^/]+)`)
    Segments(&'static str),
}

impl Pattern {
    /// The segments `*` matched (none for the other kinds), or `None`.
    pub fn matches(self, path: &str) -> Option<Vec<String>> {
        match self {
            Pattern::Exact(p) => (path == p).then(Vec::new),
            Pattern::Prefix(p) => path.starts_with(p).then(Vec::new),
            Pattern::Segments(p) => {
                let (want, got): (Vec<&str>, Vec<&str>) = (p.split('/').collect(), path.split('/').collect());
                if want.len() != got.len() {
                    return None;
                }
                let mut params = Vec::new();
                for (w, g) in want.iter().zip(&got) {
                    match *w {
                        "*" if !g.is_empty() => params.push((*g).to_owned()),
                        "*" => return None,
                        w if w == *g => {}
                        _ => return None,
                    }
                }
                Some(params)
            }
        }
    }
}

/// A request as a hub handler gets it, its body read in full.
pub struct HubRequest {
    pub method: Method,
    /// the path and query as sent (Node's `req.url`)
    pub url: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    /// what the route pattern's `*` segments matched, in order
    pub params: Vec<String>,
}

impl HubRequest {
    /// The path, without the query.
    pub fn path(&self) -> &str {
        self.url.split('?').next().unwrap_or("")
    }

    /// A query parameter, as `URLSearchParams.get` reads it.
    pub fn query(&self, name: &str) -> Option<String> {
        crate::js::search_param(&self.url, name)
    }
}

pub type HubResponse = Response<Body>;

/// A JSON answer, serialized as `JSON.stringify` would.
pub fn json(status: StatusCode, value: &Value) -> HubResponse {
    let mut res = Response::new(Full::new(Bytes::from(crate::js::stringify(value))).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}

/// What a hub route no track has ported yet answers (plan D30).
pub fn not_ported() -> HubResponse {
    json(StatusCode::NOT_IMPLEMENTED, &serde_json::json!({ "ok": false, "code": "not_ported" }))
}

/// A thread's model choice (Node's `modelCtx`): opencodex's model, its
/// effort, whether to take its `--fast` variant, and the bot whose thread
/// it is (status labels credit it: `labelOf`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelChoice {
    pub model: String,
    pub effort: String,
    pub fast: bool,
    pub name: Option<String>,
}

/// What a turn runs under, passed explicitly where Node's rides
/// AsyncLocalStorage (`modelCtx`, `actingBot`) and the job object.
#[derive(Clone, Debug, Default)]
pub struct TurnCtx {
    /// the thread's `t<n>` id
    pub thread: String,
    pub job: Option<JobId>,
    /// the bot a delivery turn answers as (Node's `actingBot`)
    pub acting_bot: Option<String>,
    pub model: Option<ModelChoice>,
    /// a v1 routine's surface, when the turn may use only its tools (Node's
    /// `job.v1Surface`: `{plugin, routine, instance, params, reads, writes}`)
    pub surface: Option<serde_json::Value>,
    /// the routine a turn runs for (Node's `job.routine`: a v0 routine's
    /// id, or `<plugin>/<routine>`); its turn gets no routine tools
    pub routine: Option<String>,
}

/// A tool's answer as the model gets it (Node's `{content, is_error}`).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: Value,
    pub is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_patterns_match_as_nodes_routes_do() {
        assert_eq!(Pattern::Exact("/threads").matches("/threads"), Some(vec![]));
        assert_eq!(Pattern::Exact("/threads").matches("/threads/"), None);
        assert_eq!(Pattern::Prefix("/module").matches("/module"), Some(vec![]));
        assert_eq!(Pattern::Prefix("/module").matches("/modules/x"), Some(vec![]));
        assert_eq!(Pattern::Prefix("/module").matches("/mod"), None);
        let action = Pattern::Segments("/v1/host/plugins/*/enable");
        assert_eq!(action.matches("/v1/host/plugins/acme.books/enable"), Some(vec!["acme.books".to_owned()]));
        assert_eq!(action.matches("/v1/host/plugins//enable"), None);
        assert_eq!(action.matches("/v1/host/plugins/a/b/enable"), None);
        assert_eq!(action.matches("/v1/host/plugins/a/disable"), None);
        assert!(Verb::Any.allows(&Method::DELETE) && Verb::Get.allows(&Method::GET) && !Verb::Post.allows(&Method::GET));
    }
}
