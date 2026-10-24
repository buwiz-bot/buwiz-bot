//! HTTP against the Grok app server (`server/app-server.mjs`, default
//! `127.0.0.1:8787`): the same endpoints the TSX app calls — `/chat`,
//! `/turns`, `/status`, `/prefs`, `/threads`, `/models`, `/catalog`,
//! `/state/:app`, `/artifacts`, `/mcp/servers`, `/skills`.

use nsh_core::ui::prelude::*;

/// The generation server the TSX app binds to.
pub const GEN: &str = "http://127.0.0.1:8787";

fn req(method: &str, path: &str) -> HttpRequest {
    let url = if path.starts_with("http") {
        path.to_owned()
    } else {
        format!("{GEN}{path}")
    };
    HttpRequest::new(method, url)
}

/// GET `path`, parsed as JSON. `None` on any transport or parse error —
/// the shell treats the server as best-effort (it may be down).
pub async fn get(path: &str) -> Option<serde_json::Value> {
    let res = fetch(req("GET", path)).await.ok()?;
    if !res.ok() {
        return None;
    }
    serde_json::from_str(&res.text()).ok()
}

/// POST `path` with a JSON body; returns the reply parsed as JSON.
pub async fn post(path: &str, body: serde_json::Value) -> Option<serde_json::Value> {
    let mut r = req("POST", path);
    r.headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    r.body = nsh_core::platform::HttpBody::Bytes(body.to_string().into_bytes());
    let res = fetch(r).await.ok()?;
    if !res.ok() {
        return None;
    }
    serde_json::from_str(&res.text()).ok()
}

/// DELETE `path`.
pub async fn del(path: &str) -> Option<serde_json::Value> {
    let res = fetch(req("DELETE", path)).await.ok()?;
    if !res.ok() {
        return None;
    }
    serde_json::from_str(&res.text()).ok()
}

/// Fire-and-forget POST used by mutations (rename, delete, react…).
/// Detached: these run from press handlers that close their own layer
/// (emoji pill, context menu) right after calling, and a task spawned
/// under the caller's owner dies with it before the post can send.
pub fn post_bg(path: String, body: serde_json::Value) {
    Owner::detached().run(|| {
        spawn(async move {
            let _ = post(&path, body).await;
        });
    });
}

/// `encodeURIComponent` — a path segment or query value.
pub fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
