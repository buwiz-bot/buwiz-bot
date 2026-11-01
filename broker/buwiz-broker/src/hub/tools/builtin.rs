//! The tools that need nothing but the network, the shell or the chat
//! (tools.mjs `toolFetchUrl`, `toolShell`, `toolQuerySource`,
//! `toolWebSearch`, `toolReact`).

use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use url::Url;

use super::{Out, Run};
use crate::helper::http;
use crate::js;

/// `fetch(url, {signal: AbortSignal.timeout(ms), redirect: "follow"})`:
/// the final answer; `Err` as the thrown error's message.
async fn fetch_follow(url: &str, headers: &[(&str, &str)], ms: u64) -> Result<http::Resp, String> {
    let mut u = Url::parse(url).map_err(|_| format!("Failed to parse URL from {url}"))?;
    let go = async {
        for _ in 0..=20 {
            let mut req = http::Req::get(u.clone());
            for (k, v) in headers {
                req = req.header(k, v);
            }
            let res = http::send(&req).await.map_err(|_| "fetch failed".to_owned())?;
            match http::redirect_target(&res) {
                Some(next) => {
                    res.cancel();
                    u = next;
                }
                None => return Ok(res),
            }
        }
        Err("fetch failed".to_owned())
    };
    tokio::time::timeout(Duration::from_millis(ms), go).await.map_err(|_| "The operation was aborted due to timeout".to_owned())?
}

/// `s.replace(/<tag[\s\S]*?<\/tag>/gi, "")`.
fn drop_blocks(s: &str, tag: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = String::new();
    let mut at = 0;
    while let Some(p) = lower[at..].find(&open) {
        let start = at + p;
        let Some(q) = lower[start + open.len()..].find(&close) else { break };
        out.push_str(&s[at..start]);
        at = start + open.len() + q + close.len();
    }
    out.push_str(&s[at..]);
    out
}

/// `s.replace(/<[^>]+>/g, " ")`.
fn tags_to_space(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find('<') {
        let after = &rest[p + 1..];
        match after.find('>') {
            Some(q) if q > 0 => {
                out.push_str(&rest[..p]);
                out.push(' ');
                rest = &after[q + 1..];
            }
            _ => {
                out.push_str(&rest[..p + 1]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `fetch_url`: the page's readable text.
pub async fn fetch_url(input: &Value) -> Result<Out, String> {
    let url = input.get("url").filter(|u| js::truthy(u)).map(js::string_of).unwrap_or_default();
    let r = fetch_follow(&url, &[("user-agent", "Mozilla/5.0 (buwiz-bot)")], 20000).await?;
    if !r.ok() {
        return Ok(Out::err(format!("fetch failed: {}", r.status)));
    }
    let html = r.text().await.map_err(|_| "terminated".to_owned())?;
    let text = tags_to_space(&drop_blocks(&drop_blocks(&html, "script"), "style")).replace("&nbsp;", " ").replace("&amp;", "&");
    let text = js::slice_utf16(&js::squeeze(&text), 6000);
    Ok(Out::ok(if text.is_empty() { "(empty page)".into() } else { text }))
}

/// `s.trimEnd()`.
fn trim_end(s: &str) -> &str {
    s.trim_end_matches(js::is_space)
}

/// `shell_run`: `zsh -lc <command>` on this Mac, capped.
pub async fn shell_run(input: &Value) -> Out {
    let command = input.get("command").filter(|c| js::truthy(c)).map(js::string_of).unwrap_or_default();
    let command = command.trim_matches(js::is_space).to_owned();
    if command.is_empty() {
        return Out::err("shell_run needs a command".into());
    }
    let n = crate::hub::store::json::js_number(input.get("timeout_ms"));
    let ms = (if n.is_nan() || n == 0.0 { 30000.0 } else { n }).clamp(1000.0, 180000.0);
    let cwd = match input.get("cwd").filter(|c| js::truthy(c)) {
        Some(c) => js::string_of(c),
        None => std::env::var("HOME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "/".into()),
    };
    let mut cmd = tokio::process::Command::new("/bin/zsh");
    cmd.args(["-lc", &command]).current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let (code, stdout, stderr) = match cmd.spawn() {
        Err(e) => (Some(if e.kind() == std::io::ErrorKind::NotFound { "ENOENT".to_owned() } else { e.to_string() }), Vec::new(), Vec::new()),
        Ok(child) => match tokio::time::timeout(Duration::from_millis(ms as u64), child.wait_with_output()).await {
            Err(_) => (Some("timeout".to_owned()), Vec::new(), Vec::new()),
            Ok(Err(e)) => (Some(e.to_string()), Vec::new(), Vec::new()),
            Ok(Ok(o)) => (if o.status.success() { None } else { Some(o.status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())) }, o.stdout, o.stderr),
        },
    };
    let (stdout, stderr) = (String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr));
    let mut out = String::new();
    if !stdout.is_empty() {
        out.push_str(trim_end(&stdout));
    }
    if !stderr.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(trim_end(&stderr));
    }
    if let Some(code) = &code {
        out.push_str(&format!("{}(exit {code})", if out.is_empty() { "" } else { "\n" }));
    }
    let out = if out.is_empty() { "(no output)".to_owned() } else { out };
    Out { content: json!(js::slice_utf16(&out, 50000)), is_error: Some(code.is_some()) }
}

/// `query_source`: one of the app's `/data` sources, through this broker's
/// front door (its public port, D34; never a fixed 8787).
pub async fn query_source(input: &Value) -> Out {
    let src = input.get("source").filter(|s| js::truthy(s)).map(js::string_of).unwrap_or_default();
    let port = crate::hub::market::node_paths().1;
    let url = if src == "linear" { format!("http://127.0.0.1:{port}/data/linear/issues") } else { format!("http://127.0.0.1:{port}/data/{src}") };
    let got: Result<Value, String> = async {
        let r = fetch_follow(&url, &[], 25000).await?;
        let text = r.text().await.map_err(|_| "terminated".to_owned())?;
        serde_json::from_str::<Value>(&text).map_err(|e| format!("Unexpected token in JSON: {e}"))
    }
    .await;
    match got {
        Ok(j) => Out::ok(js::slice_utf16(&js::stringify(&j), 12000)),
        Err(e) => Out::err(format!("source error: {e}")),
    }
}

/// `encodeURIComponent`.
fn encode(s: &str) -> String {
    crate::hub::library::botctx::encode_uri_component(s)
}

/// The client-side `web_search` (DuckDuckGo's HTML page): only offered once
/// the model refused the server-side search.
pub async fn web_search(input: &Value) -> Result<Out, String> {
    let q = encode(&input.get("query").filter(|q| js::truthy(q)).map(js::string_of).unwrap_or_default());
    let r = fetch_follow(&format!("https://html.duckduckgo.com/html/?q={q}"), &[("user-agent", "Mozilla/5.0 (buwiz-bot)")], 20000).await?;
    if !r.ok() {
        return Ok(Out::err(format!("search failed: {}", r.status)));
    }
    let html = r.text().await.map_err(|_| "terminated".to_owned())?;
    let mut results = Vec::new();
    let mut at = 0;
    while results.len() < 8 {
        let Some(p) = html[at..].find("<a") else { break };
        let start = at + p;
        at = start + 2;
        let Some(end) = html[start..].find('>').map(|n| start + n) else { break };
        let tag = &html[start + 2..end];
        let Some(c) = tag.find("class=\"result__a\"") else { continue };
        let after = &tag[c + "class=\"result__a\"".len()..];
        let Some(h) = after.rfind("href=\"").filter(|h| after[h + 6..].find('"').is_some_and(|n| n > 0)) else { continue };
        let href = &after[h + 6..h + 6 + after[h + 6..].find('"').unwrap_or(0)];
        let Some(close) = html[end + 1..].find("</a>") else { continue };
        let title = tags_to_space_none(&html[end + 1..end + 1 + close]);
        at = end + 1 + close + 4;
        let href = match href.rfind("uddg=") {
            Some(u) => &href[u + 5..],
            None => href,
        };
        let href = href.split('&').next().unwrap_or("");
        let href = js::decode_uri_component(href).map_err(|_| "URI malformed".to_owned())?;
        results.push(format!("{}\n{href}", title.trim_matches(js::is_space)));
    }
    Ok(Out::ok(if results.is_empty() { "(no results)".into() } else { results.join("\n\n") }))
}

/// `s.replace(/<[^>]+>/g, "")`.
fn tags_to_space_none(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find('<') {
        let after = &rest[p + 1..];
        match after.find('>') {
            Some(q) if q > 0 => {
                out.push_str(&rest[..p]);
                rest = &after[q + 1..];
            }
            _ => {
                out.push_str(&rest[..p + 1]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `react`: the agent's tapback on a message, as `/react` writes it.
pub fn react(run: &Run, input: &Value) -> Result<Out, String> {
    let emoji: String = input.get("emoji").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_default().chars().take(16).collect();
    let index = input.get("index").and_then(Value::as_f64).filter(|f| f.fract() == 0.0 && f.is_finite());
    let done = run.ctx.store.write(|s| {
        let t = s.thread_mut(&run.turn.thread).ok_or_else(|| "Cannot read properties of null (reading 'turns')".to_owned())?;
        let n = t.turns.len() as i64;
        let mut i = index.map(|f| f as i64).unwrap_or(-1);
        if i < 0 || i >= n {
            i = n - 1;
            while i >= 0 && t.turns[i as usize].role != "user" {
                i -= 1;
            }
        }
        if i < 0 {
            return Ok::<_, String>(None);
        }
        t.turns[i as usize].reaction = (!emoji.is_empty()).then(|| emoji.clone());
        Ok(Some(i))
    })?;
    match done {
        Err(e) => Err(e),
        Ok(None) => Ok(Out::err("no message to react to".into())),
        Ok(Some(i)) => {
            run.ctx.store.write_index();
            Ok(Out::ok(if emoji.is_empty() { format!("cleared reaction on turn {i}") } else { format!("reacted {emoji} to turn {i}") }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_strips_html_as_fetch_url_does() {
        let html = "<html><SCRIPT>x()</script><style a>b</STYLE><p>Hi&nbsp;there &amp; <b>you</b></p>";
        assert_eq!(js::squeeze(&tags_to_space(&drop_blocks(&drop_blocks(html, "script"), "style")).replace("&nbsp;", " ").replace("&amp;", "&")), "Hi there & you");
        assert_eq!(tags_to_space_none("a <b>bold</b> <> x"), "a bold <> x");
    }

    #[tokio::test]
    async fn builtin_shell_run_reports_output_and_exit() {
        let r = shell_run(&json!({"command": "echo out; echo err >&2; exit 3", "cwd": "/"})).await;
        assert_eq!(r, Out { content: json!("out\nerr\n(exit 3)"), is_error: Some(true) });
        let r = shell_run(&json!({"command": "true", "cwd": "/"})).await;
        assert_eq!(r, Out { content: json!("(no output)"), is_error: Some(false) });
        assert_eq!(shell_run(&json!({"command": "  "})).await, Out::err("shell_run needs a command".into()));
    }
}
