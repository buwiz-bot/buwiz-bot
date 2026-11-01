//! HTTP against the Grok app server (`server/app-server.mjs`, default
//! `127.0.0.1:8787`): the same endpoints the TSX app calls — `/chat`,
//! `/turns`, `/status`, `/prefs`, `/threads`, `/models`, `/catalog`,
//! `/state/:app`, `/artifacts`, `/mcp/servers`, `/skills`.
//!
//! The address comes from `ROX_BACKEND`, read once, so a test Rox can talk
//! to a test broker on another port. The broker's host-internal routes
//! (`/v1/host/*`, the mini app protocol's) also need the token the broker
//! writes at start ([`host_token`]): only a process that can read it may
//! open sessions.

use nsh_core::prelude::*;

/// The app server when `ROX_BACKEND` names none.
pub const DEFAULT_GEN: &str = "http://127.0.0.1:8787";

/// The generation server the TSX app binds to: `ROX_BACKEND`, read once,
/// else [`DEFAULT_GEN`].
pub fn backend() -> &'static str {
    static GEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    GEN.get_or_init(|| backend_from(std::env::var("ROX_BACKEND").ok().as_deref()))
}

/// `ROX_BACKEND`'s value as the server's origin: an `http(s)://` URL
/// without its trailing `/`. Anything else is a typo, and talking to the
/// default instead would reach the live server: logged, and the default
/// is used only when the variable is unset or empty.
pub(crate) fn backend_from(env: Option<&str>) -> String {
    let Some(v) = env.map(str::trim).filter(|v| !v.is_empty()) else {
        return DEFAULT_GEN.to_owned();
    };
    let v = v.trim_end_matches('/');
    let rest = v.strip_prefix("http://").or_else(|| v.strip_prefix("https://")).unwrap_or_default();
    if rest.is_empty() || rest.contains(['/', '?', '#', ' ']) {
        log::error!("ROX_BACKEND {v:?} is not an http origin; using an unreachable one");
        return "http://127.0.0.1:0".to_owned();
    }
    v.to_owned()
}

/// Where the broker keeps the host token: `ROX_HOST_TOKEN_FILE`, else
/// `~/Library/Application Support/Grok Shell/host-token`.
pub(crate) fn host_token_path(env: Option<&str>, home: &str) -> std::path::PathBuf {
    match env.map(str::trim).filter(|v| !v.is_empty()) {
        Some(p) => p.into(),
        None => std::path::Path::new(home).join("Library/Application Support/Grok Shell/host-token"),
    }
}

/// The broker's host token (`X-Buwiz-Host`), read at each request: the
/// broker makes a new one when it restarts. `None` when there is none.
/// Never logged.
pub(crate) fn host_token() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    token_in(&host_token_path(std::env::var("ROX_HOST_TOKEN_FILE").ok().as_deref(), &home))
}

/// The token in `path`, trimmed; `None` when the file is missing or empty.
pub(crate) fn token_in(path: &std::path::Path) -> Option<String> {
    let token = std::fs::read_to_string(path).ok()?;
    Some(token.trim().to_owned()).filter(|t| !t.is_empty())
}

/// Whether `path` is one of the broker's host-internal routes, which carry
/// the host token.
pub(crate) fn is_host_route(path: &str) -> bool {
    path.starts_with("/v1/host/")
}

fn req(method: &str, path: &str) -> HttpRequest {
    let url = if path.starts_with("http") {
        path.to_owned()
    } else {
        format!("{}{path}", backend())
    };
    let mut r = HttpRequest::new(method, url);
    if is_host_route(path)
        && let Some(token) = host_token()
    {
        r.headers.push(("X-Buwiz-Host".to_owned(), token));
    }
    r
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

/// [`post`], but an error status's JSON body comes back too (an endpoint
/// that answers 400 `{ok:false, error}`).
pub async fn post_any(path: &str, body: serde_json::Value) -> Option<serde_json::Value> {
    let mut r = req("POST", path);
    r.headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    r.body = nsh_core::platform::HttpBody::Bytes(body.to_string().into_bytes());
    let res = fetch(r).await.ok()?;
    serde_json::from_str(&res.text()).ok()
}

/// [`post_any`] with the status: `(status, JSON body or null)`; `None`
/// only when the broker can't be reached.
pub async fn post_status(path: &str, body: serde_json::Value) -> Option<(u16, serde_json::Value)> {
    let mut r = req("POST", path);
    r.headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    r.body = nsh_core::platform::HttpBody::Bytes(body.to_string().into_bytes());
    let res = fetch(r).await.ok()?;
    Some((res.head.status, serde_json::from_str(&res.text()).unwrap_or(serde_json::Value::Null)))
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

/// `decodeURIComponent` (and `+` as a space, as a query value may carry).
pub fn urldec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_backend_defaults_to_the_live_port_and_takes_rox_backend() {
        assert_eq!(super::backend_from(None), "http://127.0.0.1:8787");
        assert_eq!(super::backend_from(Some("")), "http://127.0.0.1:8787");
        assert_eq!(super::backend_from(Some("  ")), "http://127.0.0.1:8787");
        assert_eq!(super::backend_from(Some("http://127.0.0.1:8797")), "http://127.0.0.1:8797");
        assert_eq!(super::backend_from(Some("http://127.0.0.1:8797/")), "http://127.0.0.1:8797");
        assert_eq!(super::backend_from(Some("https://broker.local")), "https://broker.local");
        // a typo never falls back to the live server
        for bad in ["127.0.0.1:8797", "ftp://x", "http://", "http://h/x", "http://h?x"] {
            assert_eq!(super::backend_from(Some(bad)), "http://127.0.0.1:0", "{bad}");
        }
    }

    #[test]
    fn the_host_token_file_defaults_to_grok_shells_data_dir() {
        assert_eq!(
            super::host_token_path(None, "/Users/u"),
            std::path::PathBuf::from("/Users/u/Library/Application Support/Grok Shell/host-token")
        );
        assert_eq!(super::host_token_path(Some(" "), "/Users/u"), super::host_token_path(None, "/Users/u"));
        assert_eq!(
            super::host_token_path(Some("/tmp/x/host-token"), "/Users/u"),
            std::path::PathBuf::from("/tmp/x/host-token")
        );
        assert!(super::is_host_route("/v1/host/plugins"));
        assert!(!super::is_host_route("/v1/hostx"));
        assert!(!super::is_host_route("/mcp/call"));
    }

    #[test]
    fn a_missing_or_empty_token_file_is_no_token() {
        let dir = std::env::temp_dir().join(format!("rox-host-token-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("host-token");
        assert_eq!(super::token_in(&file), None);
        std::fs::write(&file, " \n").unwrap();
        assert_eq!(super::token_in(&file), None);
        std::fs::write(&file, "tok\n").unwrap();
        assert_eq!(super::token_in(&file).as_deref(), Some("tok"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
