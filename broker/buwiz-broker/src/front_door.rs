//! The front door: `/health`, `/meta`, the routes of `leaf`, the line, the
//! MCP routes and (with `BUWIZ_BROKER_HUB=1`) the hub's routes, all in Rust.
//! A path none of them has gets the hub's 404, or without the hub a JSON 404.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Method, Request, Response, StatusCode};

use crate::helper::line::Line;
use crate::hub::Hub;
use crate::leaf::{self, Leaf};

pub type Body = BoxBody<Bytes, hyper::Error>;

pub struct FrontDoor {
    /// the routes ported first: state, prefs, keymap, skills, files
    leaf: Leaf,
    /// the plugin host's line, when this broker hosts v1 plugins or MCP servers
    line: Option<Arc<Line>>,
    /// the /mcp routes, when this broker hosts the MCP servers
    mcp: Option<leaf::mcp::McpRoutes>,
    /// the brain's routes, with `BUWIZ_BROKER_HUB=1`
    hub: Option<Arc<Hub>>,
    /// the v1 plugin host whose account sign-ins come back on
    /// `/oauth/callback/account:…`, MCP host or not
    accounts: Option<Arc<crate::helper::plugins::Host>>,
}

impl FrontDoor {
    pub fn new(leaf: Leaf) -> Self {
        Self { leaf, line: None, mcp: None, hub: None, accounts: None }
    }

    pub fn with_line(mut self, line: Arc<Line>) -> Self {
        self.line = Some(line);
        self
    }

    pub fn with_mcp(mut self, routes: leaf::mcp::McpRoutes) -> Self {
        self.mcp = Some(routes);
        self
    }

    /// The plugin host a mini app account's sign-in callback reaches.
    pub fn with_accounts(mut self, host: Arc<crate::helper::plugins::Host>) -> Self {
        self.accounts = Some(host);
        self
    }

    pub fn with_hub(mut self, hub: Arc<Hub>) -> Self {
        self.hub = Some(hub);
        self
    }

    pub async fn handle(&self, req: Request<Incoming>) -> Result<Response<Body>, Infallible> {
        let started = Instant::now();
        let method = req.method().clone();
        let shown = crate::redact(req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/"));
        let (res, side) = match req.uri().path() {
            "/health" => (self.health(), "rust"),
            "/meta" => (self.meta(), "rust"),
            // a mini app account's sign-in: the plugin host's, whatever else is hosted
            p if method == Method::GET && p.starts_with("/oauth/callback/account:") && self.accounts.is_some() => match &self.accounts {
                Some(host) => (leaf::mcp::serve_account_callback(host, req.uri().path_and_query().map(|q| q.as_str()).unwrap_or(p)).await, "rust"),
                None => (self.no_route(), "rust"),
            },
            p if p.starts_with("/v1/line/") => match &self.line {
                Some(line) => (line.serve(req).await, "rust"),
                None => (json(StatusCode::NOT_FOUND, serde_json::json!({ "ok": false, "error": "this broker hosts no v1 plugins" })), "rust"),
            },
            p if self.mcp.is_some() && leaf::mcp::route(&method, req.uri().path_and_query().map(|q| q.as_str()).unwrap_or(p)) => match &self.mcp {
                Some(m) => (m.serve(req).await, "rust"),
                None => (self.no_route(), "rust"),
            },
            _ => match leaf::route(&method, req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/")) {
                Some(route) => (self.leaf.serve(route, req).await, "rust"),
                // the hub answers everything else, a path no broker has (its
                // 404) and a preflight (its 204) included
                None => match &self.hub {
                    Some(hub) => (hub.serve(req).await, "hub"),
                    None => (self.no_route(), "rust"),
                },
            },
        };
        println!("buwiz-broker {method} {shown} {} {}ms {side}", res.status().as_u16(), started.elapsed().as_millis());
        Ok(res)
    }

    fn health(&self) -> Response<Body> {
        json(StatusCode::OK, serde_json::json!({ "ok": true, "broker": "buwiz-broker" }))
    }

    fn meta(&self) -> Response<Body> {
        json(
            StatusCode::OK,
            serde_json::json!({
                "broker": "buwiz-broker",
                "version": env!("CARGO_PKG_VERSION"),
                // route groups answered by the leaf
                "ported": leaf::PORTED,
            }),
        )
    }

    /// A route nothing here answers: this broker runs without the hub.
    fn no_route(&self) -> Response<Body> {
        json(StatusCode::NOT_FOUND, serde_json::json!({ "ok": false, "error": "no such route (this broker runs without BUWIZ_BROKER_HUB=1)" }))
    }
}

fn json(status: StatusCode, value: serde_json::Value) -> Response<Body> {
    let mut res = Response::new(Full::new(Bytes::from(value.to_string())).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}
