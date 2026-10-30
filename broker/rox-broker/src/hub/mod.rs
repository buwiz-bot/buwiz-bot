//! The hub (plan phase 5): the brain's routes answered in Rust behind
//! `ROX_BROKER_HUB=1` (D30), with Node kept for the office routes
//! (`ROX_NODE_ROLE=office`). `routes` says which routes are the hub's; the
//! `Registry` maps method and path to the handler a track ported; a hub
//! route with no handler answers 501 `not_ported` and never reaches Node.
//!
//! Each track adds its routes, tools, seams and startup work from its own
//! files through one `install` call below; this file, `types`, `ctx`,
//! `traits` and `routes` are the shared skeleton.

pub mod agent;
pub mod agents;
pub mod ctx;
pub mod jobs;
pub mod library;
pub mod market;
pub mod model;
pub mod pipeline;
pub mod routes;
pub mod store;
pub mod tools;
pub mod routines;
pub mod traits;
pub mod types;

use std::future::Future;
use std::sync::{Arc, Mutex};

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{HeaderValue, ORIGIN, VARY};
use hyper::{Method, Request, StatusCode};
use serde_json::json;

use self::ctx::HubCtx;
use self::types::{BoxFuture, HubRequest, HubResponse, Pattern, Verb};

pub use self::types::not_ported;

/// A route's handler.
pub type Handler = Arc<dyn Fn(Arc<HubCtx>, HubRequest) -> BoxFuture<'static, HubResponse> + Send + Sync>;

/// Work a track starts once the hub is up (timers, recovery at start).
pub type StartHook = Box<dyn FnOnce(Arc<HubCtx>) + Send>;

/// The ported routes, the webhook receivers and the startup hooks.
#[derive(Default)]
pub struct Registry {
    routes: Vec<(Verb, Pattern, Handler)>,
    start: Mutex<Vec<StartHook>>,
    agent_hooks: Option<Handler>,
    routine_hooks: Option<Handler>,
}

fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(Arc<HubCtx>, HubRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HubResponse> + Send + 'static,
{
    Arc::new(move |ctx, req| Box::pin(f(ctx, req)))
}

impl Registry {
    /// Answers `verb` on `pattern` with `f`. The first registered match wins.
    pub fn route<F, Fut>(&mut self, verb: Verb, pattern: Pattern, f: F)
    where
        F: Fn(Arc<HubCtx>, HubRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HubResponse> + Send + 'static,
    {
        self.routes.push((verb, pattern, handler(f)));
    }

    /// Runs `f` once, when the hub starts.
    pub fn on_start(&mut self, f: impl FnOnce(Arc<HubCtx>) + Send + 'static) {
        self.start.get_mut().unwrap_or_else(|e| e.into_inner()).push(Box::new(f));
    }

    /// `POST /v1/hooks/ah_…/<secret>`: an agent's webhook (track E).
    pub fn agent_hooks<F, Fut>(&mut self, f: F)
    where
        F: Fn(Arc<HubCtx>, HubRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HubResponse> + Send + 'static,
    {
        self.agent_hooks = Some(handler(f));
    }

    /// `POST /v1/hooks/<instance>/<secret>`: a routine's webhook (track D).
    pub fn routine_hooks<F, Fut>(&mut self, f: F)
    where
        F: Fn(Arc<HubCtx>, HubRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HubResponse> + Send + 'static,
    {
        self.routine_hooks = Some(handler(f));
    }

    /// The handler for a request and what its pattern's `*` matched.
    pub fn find(&self, method: &Method, path: &str) -> Option<(Handler, Vec<String>)> {
        self.routes.iter().find_map(|(verb, pattern, h)| if verb.allows(method) { pattern.matches(path).map(|params| (h.clone(), params)) } else { None })
    }
}

/// What each track adds. The skeleton adds nothing; a track adds one line
/// here calling its own `install(routes, ctx)`.
fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    // track A first: it loads the store the others' starts read
    routes::chat::install(routes, ctx);
    pipeline::install(ctx);
    // 5.7b: after the store's start, as app-server.mjs runs its migration
    routes::apps::install(routes, ctx);
    model::install(routes, ctx);
    agent::install(routes, ctx);
    market::install(routes, ctx);
    routes::leftovers::install(routes, ctx);
    // before routines: its /v1/host/ catch-all would answer the agents' routes
    agents::install(routes, ctx);
    routines::install(routes, ctx);
}

pub struct Hub {
    ctx: Arc<HubCtx>,
    routes: Registry,
    allowed: Vec<String>,
}

impl Hub {
    /// The hub with every track's routes; `allowed` the origins the guard
    /// lets through.
    pub fn new(ctx: HubCtx, allowed: Vec<String>) -> Self {
        Self::with(ctx, allowed, install)
    }

    /// The hub with the routes `install` adds (tests).
    pub fn with(mut ctx: HubCtx, allowed: Vec<String>, install: impl FnOnce(&mut Registry, &mut HubCtx)) -> Self {
        let mut routes = Registry::default();
        install(&mut routes, &mut ctx);
        Self { ctx: Arc::new(ctx), routes, allowed }
    }

    pub fn ctx(&self) -> &Arc<HubCtx> {
        &self.ctx
    }

    /// Runs the startup hooks, once.
    pub fn start(&self) {
        let hooks = std::mem::take(&mut *self.routes.start.lock().unwrap_or_else(|e| e.into_inner()));
        for hook in hooks {
            hook(self.ctx.clone());
        }
    }

    /// A request the front door classed as the hub's.
    pub async fn serve(&self, req: Request<Incoming>) -> HubResponse {
        let (parts, body) = req.into_parts();
        let body = body.collect().await.map(|b| b.to_bytes()).unwrap_or_default();
        let url = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/").to_owned();
        self.answer(HubRequest { method: parts.method, url, headers: parts.headers, body, params: Vec::new() }).await
    }

    /// The guard Node runs before any route, then the route's handler, the
    /// webhook dispatcher, or 501; with Node's CORS headers on the answer.
    pub async fn answer(&self, mut req: HubRequest) -> HubResponse {
        if let Some((status, error)) = crate::guard::refusal(&req.method, &req.headers, &self.allowed) {
            return types::json(status, &json!({ "ok": false, "error": error }));
        }
        let origin = req.headers.get(ORIGIN).cloned();
        let path = req.path().to_owned();
        let mut res = if path.starts_with("/v1/hooks/") {
            self.hook(req).await
        } else {
            match self.routes.find(&req.method, &path) {
                Some((h, params)) => {
                    req.params = params;
                    h(self.ctx.clone(), req).await
                }
                None => not_ported(),
            }
        };
        if let Some(origin) = origin {
            let h = res.headers_mut();
            h.insert("access-control-allow-origin", origin);
            h.insert("access-control-allow-headers", HeaderValue::from_static("content-type"));
            h.insert(VARY, HeaderValue::from_static("Origin"));
        }
        res
    }

    /// `/v1/hooks/<id>/<secret>` (v1/routes.mjs `hookRoute`): an `ah_` id
    /// is an agent's webhook, any other a routine instance's. A path of
    /// another shape, or not a POST, is Node's 404.
    async fn hook(&self, mut req: HubRequest) -> HubResponse {
        let Some(params) = Pattern::Segments("/v1/hooks/*/*").matches(req.path()).filter(|_| req.method == Method::POST) else {
            return types::json(StatusCode::NOT_FOUND, &json!({ "ok": false, "code": "not_found", "message": "no such hook", "retryable": false }));
        };
        let to = if params[0].starts_with("ah_") { &self.routes.agent_hooks } else { &self.routes.routine_hooks };
        req.params = params;
        match to {
            Some(h) => h(self.ctx.clone(), req).await,
            None => not_ported(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hyper::header::HeaderMap;

    fn req(method: Method, url: &str) -> HubRequest {
        HubRequest { method, url: url.to_owned(), headers: HeaderMap::new(), body: Bytes::new(), params: Vec::new() }
    }

    async fn text(res: HubResponse) -> (u16, String) {
        let status = res.status().as_u16();
        (status, String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned())
    }

    fn hub(install: impl FnOnce(&mut Registry, &mut HubCtx)) -> Hub {
        let ctx = HubCtx::new(std::env::temp_dir().join("rox-hub-test-unused"), traits::Clock::manual());
        Hub::with(ctx, vec!["http://127.0.0.1:8798".to_owned()], install)
    }

    fn says(text: &'static str) -> impl Fn(Arc<HubCtx>, HubRequest) -> BoxFuture<'static, HubResponse> + Send + Sync {
        move |_, r| Box::pin(async move { types::json(StatusCode::OK, &json!({ "says": text, "params": r.params })) })
    }

    #[tokio::test]
    async fn hub_registry_answers_what_a_track_registered_and_501_otherwise() {
        let hub = hub(|routes, _| {
            routes.route(Verb::Get, Pattern::Exact("/threads"), says("threads"));
            routes.route(Verb::Post, Pattern::Segments("/v1/host/plugins/*/enable"), says("enable"));
        });
        assert_eq!(text(hub.answer(req(Method::GET, "/threads")).await).await, (200, r#"{"says":"threads","params":[]}"#.to_owned()));
        assert_eq!(text(hub.answer(req(Method::POST, "/v1/host/plugins/acme/enable")).await).await, (200, r#"{"says":"enable","params":["acme"]}"#.to_owned()));
        // registered for GET only; another hub route; a path no track has
        for (m, url) in [(Method::POST, "/threads"), (Method::GET, "/turns"), (Method::POST, "/v1/host/plugins/acme/disable")] {
            assert_eq!(text(hub.answer(req(m, url)).await).await, (501, r#"{"ok":false,"code":"not_ported"}"#.to_owned()));
        }
    }

    #[tokio::test]
    async fn hub_hooks_go_to_agents_or_routines_by_their_id() {
        let none = hub(|_, _| {});
        assert_eq!(text(none.answer(req(Method::POST, "/v1/hooks/ah_1/s")).await).await.0, 501);
        assert_eq!(text(none.answer(req(Method::POST, "/v1/hooks/ri_1/s")).await).await.0, 501);
        let both = hub(|routes, _| {
            routes.agent_hooks(says("agents"));
            routes.routine_hooks(says("routines"));
        });
        assert_eq!(text(both.answer(req(Method::POST, "/v1/hooks/ah_1/s3")).await).await, (200, r#"{"says":"agents","params":["ah_1","s3"]}"#.to_owned()));
        assert_eq!(text(both.answer(req(Method::POST, "/v1/hooks/ri_1/s3?x=1")).await).await, (200, r#"{"says":"routines","params":["ri_1","s3"]}"#.to_owned()));
        let no = r#"{"ok":false,"code":"not_found","message":"no such hook","retryable":false}"#.to_owned();
        assert_eq!(text(both.answer(req(Method::GET, "/v1/hooks/ah_1/s3")).await).await, (404, no.clone()));
        assert_eq!(text(both.answer(req(Method::POST, "/v1/hooks/ah_1")).await).await, (404, no));
    }

    #[tokio::test]
    async fn hub_guard_refuses_a_foreign_origin_and_answers_cors_to_an_allowed_one() {
        let hub = hub(|_, _| {});
        let mut r = req(Method::GET, "/threads");
        r.headers.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert_eq!(text(hub.answer(r).await).await.0, 403);
        let mut r = req(Method::GET, "/threads");
        r.headers.insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:8798"));
        let res = hub.answer(r).await;
        assert_eq!(res.headers()["access-control-allow-origin"], "http://127.0.0.1:8798");
        assert_eq!(text(res).await.0, 501);
    }

    #[test]
    fn hub_start_hooks_run_once_and_tools_register_by_name() {
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = ran.clone();
        let hub = hub(move |routes, ctx| {
            routes.on_start(move |_| {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            });
            ctx.tools.register("echo", |_: Arc<HubCtx>, _: types::TurnCtx, input: serde_json::Value| async move { types::ToolResult { content: input, is_error: false } });
        });
        hub.start();
        hub.start();
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(hub.ctx().tools.names().collect::<Vec<_>>(), ["echo"]);
        assert!(hub.ctx().tools.get("nope").is_none());
    }
}
