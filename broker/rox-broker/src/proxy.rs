//! The front door: `/health`, `/meta` and the routes ported to Rust
//! (`leaf`) here, everything else forwarded to Node with its method, path, query, headers and body as they came, and
//! the answer streamed back as it arrives. No timeout: an event stream or a
//! long poll stays open as long as Node keeps it.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;

use crate::helper::line::Line;
use crate::hub::{routes, Hub};
use crate::leaf::{self, Leaf};
use crate::node::Node;

pub type Body = BoxBody<Bytes, hyper::Error>;

pub struct FrontDoor {
    pub node: Arc<Node>,
    /// the routes answered in Rust; `None` forwards them too
    leaf: Option<Leaf>,
    /// the plugin host's API for Node, when this broker hosts v1 plugins
    line: Option<Arc<Line>>,
    /// the /mcp routes, when this broker hosts the MCP servers (4.3)
    mcp: Option<leaf::mcp::McpRoutes>,
    /// the brain's routes, with `ROX_BROKER_HUB=1` (plan D30)
    hub: Option<Arc<Hub>>,
    client: Client<HttpConnector, Incoming>,
}

impl FrontDoor {
    pub fn new(node: Arc<Node>, leaf: Option<Leaf>) -> Self {
        Self { node, leaf, line: None, mcp: None, hub: None, client: Client::builder(TokioExecutor::new()).build_http() }
    }

    pub fn with_line(mut self, line: Arc<Line>) -> Self {
        self.line = Some(line);
        self
    }

    pub fn with_mcp(mut self, routes: leaf::mcp::McpRoutes) -> Self {
        self.mcp = Some(routes);
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
            p if p.starts_with("/v1/line/") => match &self.line {
                Some(line) => (line.serve(req).await, "rust"),
                None => (json(StatusCode::NOT_FOUND, serde_json::json!({ "ok": false, "error": "this broker hosts no v1 plugins" })), "rust"),
            },
            p if self.mcp.is_some() && leaf::mcp::route(&method, req.uri().path_and_query().map(|q| q.as_str()).unwrap_or(p)) => match &self.mcp {
                Some(m) => (m.serve(req).await, "rust"),
                None => (self.forward(req).await, "node"),
            },
            _ => match (&self.leaf, leaf::route(&method, req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/"))) {
                (Some(leaf), Some(route)) => (leaf.serve(route, req).await, "rust"),
                // with the hub on, a hub route never reaches Node, nor does
                // a Rust route whose part is switched off: Node holds no
                // state for either (its office role)
                _ => match (&self.hub, routes::classify(&method, req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/")).map(|r| r.owner)) {
                    (Some(hub), Some(routes::Owner::Hub(_) | routes::Owner::Rust)) => (hub.serve(req).await, "hub"),
                    // a route no broker has (the table holds all of Node's):
                    // Node's 404, which its office role would answer 501
                    (Some(hub), None) if method != Method::OPTIONS => (hub.serve(req).await, "hub"),
                    _ => (self.forward(req).await, "node"),
                },
            },
        };
        println!("rox-broker {method} {shown} {} {}ms {side}", res.status().as_u16(), started.elapsed().as_millis());
        Ok(res)
    }

    fn health(&self) -> Response<Body> {
        match self.node.port() {
            Some(_) => json(StatusCode::OK, serde_json::json!({ "ok": true, "broker": "rox-broker" })),
            None => json(StatusCode::SERVICE_UNAVAILABLE, serde_json::json!({ "ok": false, "broker": "rox-broker", "node": "starting" })),
        }
    }

    fn meta(&self) -> Response<Body> {
        json(
            StatusCode::OK,
            serde_json::json!({
                "broker": "rox-broker",
                "version": env!("CARGO_PKG_VERSION"),
                "node_version": self.node.version.get().cloned().unwrap_or_default(),
                // route groups answered in Rust (phase 2 on)
                "ported": if self.leaf.is_some() { leaf::PORTED } else { &[] },
            }),
        )
    }

    async fn forward(&self, req: Request<Incoming>) -> Response<Body> {
        let Some(port) = self.node.port() else {
            return json(StatusCode::BAD_GATEWAY, serde_json::json!({ "ok": false, "error": "the broker is starting" }));
        };
        let (mut parts, body) = req.into_parts();
        let path = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/").to_owned();
        parts.uri = match format!("http://127.0.0.1:{port}{path}").parse::<Uri>() {
            Ok(uri) => uri,
            Err(_) => return json(StatusCode::BAD_REQUEST, serde_json::json!({ "ok": false, "error": "a malformed path" })),
        };
        parts.version = hyper::Version::HTTP_11;
        strip_hop_by_hop(&mut parts.headers);
        match self.client.request(Request::from_parts(parts, body)).await {
            Ok(res) => {
                let (mut parts, body) = res.into_parts();
                strip_hop_by_hop(&mut parts.headers);
                Response::from_parts(parts, body.boxed())
            }
            Err(e) => {
                eprintln!("rox-broker: node did not answer {path}: {e}");
                json(StatusCode::BAD_GATEWAY, serde_json::json!({ "ok": false, "error": "the broker's node side did not answer" }))
            }
        }
    }
}

/// Headers that describe one connection, not the message (RFC 9110 7.6.1),
/// and those `Connection` names. `Host` stays: Node builds URLs from it.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(hyper::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in ["connection", "keep-alive", "proxy-connection", "te", "trailer", "transfer-encoding", "upgrade"] {
        headers.remove(name);
    }
}

fn json(status: StatusCode, value: serde_json::Value) -> Response<Body> {
    let mut res = Response::new(Full::new(Bytes::from(value.to_string())).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_headers_stay_on_their_hop() {
        let mut h = HeaderMap::new();
        h.insert("connection", HeaderValue::from_static("keep-alive, x-private"));
        h.insert("x-private", HeaderValue::from_static("1"));
        h.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        h.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        h.insert("host", HeaderValue::from_static("127.0.0.1:8787"));
        h.insert("x-rox-host", HeaderValue::from_static("token"));
        h.insert("x-rox-signature", HeaderValue::from_static("t=1,v1=ab"));
        strip_hop_by_hop(&mut h);
        let left: Vec<&str> = h.keys().map(|k| k.as_str()).collect();
        assert_eq!(left, ["host", "x-rox-host", "x-rox-signature"]);
    }
}
