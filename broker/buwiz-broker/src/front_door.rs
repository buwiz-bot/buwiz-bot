//! The front door: `/health`, `/meta`, the routes of `leaf`, the MCP routes
//! and the hub's routes. A path none of the others has gets the hub's 404.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Method, Request, Response, StatusCode};

use crate::hub::Hub;
use crate::leaf::{self, Leaf};

pub type Body = BoxBody<Bytes, hyper::Error>;

pub struct FrontDoor {
    /// the routes ported first: state, prefs, keymap, skills, files
    leaf: Leaf,
    /// the /mcp routes, when this broker hosts the MCP servers
    mcp: Option<leaf::mcp::McpRoutes>,
    /// the brain's routes, and the answer to everything else
    hub: Arc<Hub>,
    /// the v1 plugin host whose account sign-ins come back on
    /// `/oauth/callback/account:…`, MCP host or not
    accounts: Option<Arc<crate::helper::plugins::Host>>,
}

impl FrontDoor {
    pub fn new(leaf: Leaf, hub: Arc<Hub>) -> Self {
        Self { leaf, mcp: None, hub, accounts: None }
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

    pub async fn handle(&self, req: Request<Incoming>) -> Result<Response<Body>, Infallible> {
        let started = Instant::now();
        let method = req.method().clone();
        let shown = crate::redact(req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/"));
        let path = req.uri().path().to_owned();
        let target = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/").to_owned();
        // a mini app account's sign-in: the plugin host's, whatever else is hosted
        let accounts = self.accounts.as_ref().filter(|_| method == Method::GET && path.starts_with("/oauth/callback/account:"));
        let mcp = self.mcp.as_ref().filter(|_| leaf::mcp::route(&method, &target));
        // every route, /health and the sign-in callbacks too, answers only a loopback Host
        let (res, side) = if let Some((status, error)) = crate::guard::host_refusal(req.headers(), self.leaf.allowed()) {
            (json(status, serde_json::json!({ "ok": false, "error": error })), "rust")
        } else if path == "/health" {
            (self.health(), "rust")
        } else if path == "/meta" {
            (self.meta(), "rust")
        } else if let Some(host) = accounts {
            (leaf::mcp::serve_account_callback(host, &target).await, "rust")
        } else if let Some(m) = mcp {
            (m.serve(req).await, "rust")
        } else if let Some(route) = leaf::route(&method, &target) {
            (self.leaf.serve(route, req).await, "rust")
        } else {
            // the hub answers everything else, a path no broker has (its
            // 404) and a preflight (its 204) included
            (self.hub.serve(req).await, "hub")
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
}

fn json(status: StatusCode, value: serde_json::Value) -> Response<Body> {
    let mut res = Response::new(Full::new(Bytes::from(value.to_string())).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}
