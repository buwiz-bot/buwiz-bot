//! The HTTP client remote MCP servers and their OAuth run on, as Node's
//! `fetch` does for the SDK (1.32): http and https (rustls, Mozilla's roots
//! from webpki-roots, as Node's bundled store; the Keychain is never read),
//! no proxy, `accept: */*` unless a request names one. Redirects follow the
//! SDK's `fetchWithinOrigin` (shared/transport.js): at most 5, only within
//! the origin (http to https on default ports counts), only when the method
//! survives (GET, or a 307/308), and never one that adds credentials.
//! Anything else comes back as the redirect itself.

use std::sync::OnceLock;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, Request, Response};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use url::Url;

type Https = hyper_rustls::HttpsConnector<HttpConnector>;

const MAX_REDIRECTS: usize = 5;
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];
/// How long a connection may take to open (fetch has no request timeout;
/// the MCP layer times its own requests).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn client() -> &'static Client<Https, Full<Bytes>> {
    static CLIENT: OnceLock<Client<Https, Full<Bytes>>> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        http.set_connect_timeout(Some(CONNECT_TIMEOUT));
        let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("rustls protocol versions")
            .with_root_certificates(rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() })
            .with_no_client_auth();
        let https = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls).https_or_http().enable_http1().wrap_connector(http);
        Client::builder(TokioExecutor::new()).build(https)
    })
}

/// One request: headers in order (a later one replaces an earlier one of
/// the same name, as `new Headers({...})` does).
#[derive(Debug, Clone)]
pub struct Req {
    pub method: Method,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl Req {
    pub fn get(url: Url) -> Self {
        Self { method: Method::GET, url, headers: Vec::new(), body: None }
    }

    pub fn post(url: Url, body: Vec<u8>) -> Self {
        Self { method: Method::POST, url, headers: Vec::new(), body: Some(body) }
    }

    /// Sets `name` (case-insensitive), replacing any earlier value.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.set(name, value);
        self
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.headers.push((name.to_owned(), value.to_owned()));
    }

    pub fn has(&self, name: &str) -> bool {
        self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
    }
}

/// A response whose body has not been read yet.
pub struct Resp {
    pub status: u16,
    pub headers: HeaderMap,
    pub url: Url,
    body: Incoming,
}

impl Resp {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
    }

    pub async fn bytes(self) -> Result<Vec<u8>, String> {
        self.body.collect().await.map(|b| b.to_bytes().to_vec()).map_err(|e| e.to_string())
    }

    pub async fn text(self) -> Result<String, String> {
        self.bytes().await.map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The next chunk of a streamed body (`None` at its end).
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, String> {
        loop {
            match self.body.frame().await {
                None => return Ok(None),
                Some(Err(e)) => return Err(e.to_string()),
                Some(Ok(f)) => {
                    if let Ok(data) = f.into_data() {
                        return Ok(Some(data));
                    }
                }
            }
        }
    }

    /// Drops the body without reading it (fetch's `body.cancel()`).
    pub fn cancel(self) {}
}

/// The error fetch rejects with: `fetch failed` and the cause.
fn failed(e: impl std::fmt::Display) -> String {
    format!("fetch failed: {e}")
}

/// One request, no redirects followed.
pub async fn send(req: &Req) -> Result<Resp, String> {
    let mut b = Request::builder().method(req.method.clone()).uri(req.url.as_str());
    let headers = b.headers_mut().expect("a fresh builder");
    if !req.has("accept") {
        headers.insert(hyper::header::ACCEPT, HeaderValue::from_static("*/*"));
    }
    for (k, v) in &req.headers {
        let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.to_ascii_lowercase().as_bytes()), HeaderValue::from_str(v)) else {
            return Err(failed(format!("invalid header {k}")));
        };
        headers.insert(name, value);
    }
    let host = match (req.url.host_str(), req.url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => return Err(failed("no host")),
    };
    headers.insert(hyper::header::HOST, HeaderValue::from_str(&host).map_err(failed)?);
    let body = Full::new(Bytes::from(req.body.clone().unwrap_or_default()));
    let request = b.body(body).map_err(failed)?;
    let res: Response<Incoming> = client().request(request).await.map_err(|e| failed(source_of(&e)))?;
    let (parts, body) = res.into_parts();
    Ok(Resp { status: parts.status.as_u16(), headers: parts.headers, url: req.url.clone(), body })
}

/// The innermost cause, as Node prints a fetch failure's `cause`.
fn source_of(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

/// `isWithinOrigin`: the same scheme, host and port, or http to https on
/// the default ports.
pub fn within_origin(from: &Url, to: &Url) -> bool {
    if from.scheme() == to.scheme() && from.host_str() == to.host_str() && from.port() == to.port() {
        return true;
    }
    from.host_str() == to.host_str() && from.scheme() == "http" && from.port().is_none() && to.scheme() == "https" && to.port().is_none()
}

/// The redirect a response asks for, if it is one.
pub fn redirect_target(res: &Resp) -> Option<Url> {
    if !REDIRECTS.contains(&res.status) {
        return None;
    }
    res.header("location").and_then(|l| res.url.join(&l).ok())
}

/// `fetchWithinOrigin(fetch)(url, init)`.
pub async fn fetch(req: Req) -> Result<Resp, String> {
    let mut req = req;
    let mut res = send(&req).await?;
    for _ in 0..MAX_REDIRECTS {
        let Some(target) = redirect_target(&res) else { break };
        let keeps = res.status == 307 || res.status == 308 || req.method == Method::GET;
        let adds_userinfo = (!target.username().is_empty() || target.password().is_some())
            && (target.username() != req.url.username() || target.password() != req.url.password());
        if !keeps || adds_userinfo || !within_origin(&req.url, &target) {
            break;
        }
        res.cancel();
        req.url = target;
        res = send(&req).await?;
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_origin_takes_the_same_origin_and_an_http_upgrade_only() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(within_origin(&u("http://a.test/x"), &u("http://a.test/y")));
        assert!(within_origin(&u("http://a.test/x"), &u("https://a.test/y")));
        assert!(!within_origin(&u("https://a.test/x"), &u("http://a.test/y")));
        assert!(!within_origin(&u("http://a.test:8080/x"), &u("https://a.test/y")));
        assert!(!within_origin(&u("http://a.test/x"), &u("http://b.test/x")));
    }

    #[test]
    fn a_later_header_replaces_an_earlier_one_whatever_its_case() {
        let r = Req::get(Url::parse("http://a.test/").unwrap()).header("Accept", "x").header("accept", "y");
        assert_eq!(r.headers, vec![("accept".to_owned(), "y".to_owned())]);
    }
}
