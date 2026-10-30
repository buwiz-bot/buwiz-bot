//! Mermaid in a web view (plan 6.2; routes/files.mjs): Rox draws a
//! ```mermaid fence natively and falls back to this page when its helper
//! can't (app/src/files.rs `mermaid_url`, inline and full-screen).
//! `GET /mermaid?d=<base64url>&theme=dark|light&mode=inline|full` is the
//! page; `GET /mermaid.js` serves the npm package's mermaid.min.js from
//! the server's node_modules, which phase 7 must ship beside the broker.

use std::path::PathBuf;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use serde_json::json;

use super::chat::answer;
use super::popout::fill;
use crate::hub::ctx::HubCtx;
use crate::hub::types::{HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;

/// The page's text, copied from routes/files.mjs, its five `${…}`s numbered.
const PAGE: &str = include_str!("mermaid_page.html");

pub fn install(routes: &mut Registry, _ctx: &mut HubCtx) {
    routes.route(Verb::Get, Pattern::Exact("/mermaid.js"), |_, req| async move { script(&req) });
    routes.route(Verb::Get, Pattern::Exact("/mermaid"), |_, req| async move { page(&req) });
}

/// Node's answer to a path no route took.
fn not_found() -> HubResponse {
    answer(404, "not found".into(), false)
}

fn typed(status: StatusCode, ct: &'static str, body: impl Into<Bytes>) -> HubResponse {
    let mut res = Response::new(Full::new(body.into()).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(ct));
    res
}

/// `serverFile("../node_modules/mermaid/dist/mermaid.min.js")`.
fn script_path() -> PathBuf {
    let src = crate::hub::market::node_paths().0;
    let server = src.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    server.join("node_modules").join("mermaid").join("dist").join("mermaid.min.js")
}

/// `GET /mermaid.js`, exactly (a query is Node's 404).
fn script(req: &HubRequest) -> HubResponse {
    if req.url != "/mermaid.js" {
        return not_found();
    }
    let path = script_path();
    match std::fs::read(&path) {
        Ok(bytes) => typed(StatusCode::OK, "text/javascript", bytes),
        // readFileSync threw after the content type was set
        Err(_) => typed(StatusCode::INTERNAL_SERVER_ERROR, "text/javascript", js::stringify(&json!({"error": format!("ENOENT: no such file or directory, open '{}'", path.display())}))),
    }
}

/// `Buffer.from(s, "base64url")`: either alphabet, other characters
/// skipped, stopping at the first `=`, a lone last character dropped.
fn base64url(s: &str) -> Vec<u8> {
    let mut sextets = Vec::with_capacity(s.len());
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        sextets.push(v as u32);
    }
    let mut out = Vec::with_capacity(sextets.len() * 3 / 4);
    for group in sextets.chunks(4) {
        let n = group.iter().enumerate().fold(0u32, |n, (i, v)| n | (v << (18 - 6 * i)));
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..group.len().saturating_sub(1)]);
    }
    out
}

/// `GET /mermaid?…`: the page drawing `d` (a request without the `?` is
/// Node's 404).
fn page(req: &HubRequest) -> HubResponse {
    if !req.url.starts_with("/mermaid?") {
        return not_found();
    }
    let code = String::from_utf8_lossy(&base64url(&req.query("d").unwrap_or_default())).into_owned();
    let dark = req.query("theme").as_deref() == Some("dark");
    let full = req.query("mode").as_deref() == Some("full");
    let esc = code.replace('&', "&amp;").replace('<', "&lt;");
    let html = fill(
        PAGE,
        &[
            ("0", if full { "center" } else { "0 0" }.into()),
            ("1", if full { format!("padding:28px;background:{};border-radius:12px", if dark { "#262626" } else { "#fff" }) } else { String::new() }),
            ("2", if full { "body{display:flex;align-items:center;justify-content:center;height:100vh}" } else { "" }.into()),
            ("3", esc),
            ("4", js::quote(if dark { "dark" } else { "neutral" })),
        ],
    );
    typed(StatusCode::OK, "text/html; charset=utf-8", html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mermaid_base64url_reads_as_node_buffer_does() {
        for (s, want) in [("Z3JhcGg", "graph"), ("Z3 Jh!cGg", "graph"), ("Z3Jh=cGg", "gra"), ("Zg==", "f"), ("Z", ""), ("Zm9", "fo"), ("Zm9v YmFy", "foobar")] {
            assert_eq!(base64url(s), want.as_bytes(), "{s}");
        }
        assert_eq!(base64url("Z3J+/hcGg"), base64url("Z3J-_hcGg"));
        assert_eq!(base64url("Z3J-_hcGg"), b"gr~\xfe\x17\x06");
    }
}
