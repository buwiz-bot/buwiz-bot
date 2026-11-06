//! Who may call a route answered here: server/src/guard.mjs, the same
//! checks Node runs before any of its routes.
//!
//! A request whose `Origin` is not on the list comes from a web page and is
//! refused (403); a body sent with a body method must be JSON (415).
//!
//! `Origin: null` is never on it, on any route. That's every sandboxed frame on every website, and
//! a preview's own page (`leaf/files.rs`).

use hyper::header::{HeaderMap, CONTENT_LENGTH, CONTENT_TYPE, ORIGIN, TRANSFER_ENCODING};
use hyper::{Method, StatusCode};

/// What `BUWIZ_ALLOWED_ORIGINS` (comma-separated) adds. `null` isn't an origin anyone can be
/// allowed: naming it would let in every sandboxed frame there is, so it's dropped.
pub fn extra_origins(extra: Option<&str>) -> Vec<String> {
    extra.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty() && *s != "null").map(str::to_owned).collect()
}

/// The origins allowed to call: the front door's own, plus `extra`.
pub fn allowed_origins(port: u16, extra: Option<&str>) -> Vec<String> {
    let mut out = vec![format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
    out.extend(extra_origins(extra));
    out
}

/// Why a request is refused — (status, error) — or `None`.
pub fn refusal(method: &Method, headers: &HeaderMap, allowed: &[String]) -> Option<(StatusCode, String)> {
    if let Some(origin) = headers.get(ORIGIN) {
        let origin = String::from_utf8_lossy(origin.as_bytes());
        if !allowed.iter().any(|a| *a == origin) {
            let shown: String = origin.chars().take(100).collect();
            return Some((StatusCode::FORBIDDEN, format!("requests from origin {shown} are refused: this server answers Buwiz Bot and its apps, not web pages")));
        }
    }
    let body_method = matches!(*method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE);
    let has_body = headers.contains_key(TRANSFER_ENCODING)
        || headers.get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok()).is_some_and(|n| n > 0.0);
    let json = headers
        .get(CONTENT_TYPE)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"))
        .unwrap_or(false);
    if body_method && has_body && !json {
        return Some((StatusCode::UNSUPPORTED_MEDIA_TYPE, "send the request body as Content-Type: application/json".to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    fn headers(list: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in list {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn guard_matches_node() {
        let allowed = allowed_origins(8787, Some(" https://a.example ,"));
        assert_eq!(allowed, ["http://127.0.0.1:8787", "http://localhost:8787", "https://a.example"]);
        assert_eq!(refusal(&Method::GET, &headers(&[]), &allowed), None);
        assert_eq!(refusal(&Method::GET, &headers(&[("origin", "https://a.example")]), &allowed), None);
        assert_eq!(refusal(&Method::GET, &headers(&[("origin", "https://evil.example")]), &allowed).unwrap().0, StatusCode::FORBIDDEN);
        assert_eq!(refusal(&Method::POST, &headers(&[]), &allowed), None);
        assert_eq!(refusal(&Method::POST, &headers(&[("content-length", "2"), ("content-type", "Application/JSON; charset=utf-8")]), &allowed), None);
        assert_eq!(refusal(&Method::POST, &headers(&[("content-length", "2"), ("content-type", "text/plain")]), &allowed).unwrap().0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(refusal(&Method::POST, &headers(&[("transfer-encoding", "chunked")]), &allowed).unwrap().0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(refusal(&Method::GET, &headers(&[("content-length", "2")]), &allowed), None);
    }

    #[test]
    fn null_stays_refused_when_the_environment_names_it() {
        let allowed = allowed_origins(8787, Some("null, https://a.example, null "));
        assert_eq!(allowed, ["http://127.0.0.1:8787", "http://localhost:8787", "https://a.example"]);
        assert_eq!(refusal(&Method::GET, &headers(&[("origin", "null")]), &allowed).unwrap().0, StatusCode::FORBIDDEN);
    }
}
