//! ```mermaid fences drawn by Pele (`pele/serve.mjs`, MIT) as native SVG
//! images: one small Node helper renders every diagram in well under a
//! millisecond, where each fence used to be a WKWebView (its own web
//! content process) loading mermaid.js — the red spikes on the frame
//! monitor. The helper starts on the first diagram and quits with Rox.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nsh_core::ui::prelude::*;

use crate::live::store;

/// A rendered diagram: an SVG `data:` URI and its natural size (points).
#[derive(Clone, PartialEq)]
pub(crate) struct Diagram {
    pub uri: String,
    pub w: f32,
    pub h: f32,
}

/// The diagram for `code` in the dark or light palette: `None` while it
/// renders, then the image or why it could not be drawn (no Node, a
/// parse error) — the caller falls back to the web view then.
pub(crate) fn diagram(code: &str, dark: bool) -> Signal<Option<Result<Diagram, String>>> {
    let theme = if dark { "dark" } else { "light" };
    let key = format!("{theme}\0{code}");
    let st = store();
    if let Some(sig) = st.diagrams.borrow().get(&key) {
        return *sig;
    }
    let sig = st.in_store(|| signal(None));
    st.diagrams.borrow_mut().insert(key, sig);
    let code = code.to_owned();
    Owner::detached().run(|| {
        spawn(async move {
            let result = match base_url() {
                Err(e) => Err(e),
                Ok(base) => request(&base, &code, theme).await,
            };
            if let Err(e) = &result {
                log::warn!("mermaid: {e}");
            }
            sig.set(Some(result));
        });
    });
    sig
}

async fn request(base: &str, code: &str, theme: &str) -> Result<Diagram, String> {
    let v = crate::client::post(
        &format!("{base}/render"),
        serde_json::json!({"code": code, "theme": theme}),
    )
    .await
    .ok_or("pele helper did not answer")?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_owned());
    }
    let uri = v.get("uri").and_then(|u| u.as_str()).unwrap_or_default();
    let w = v.get("width").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
    let h = v.get("height").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
    if uri.is_empty() || w <= 0.0 || h <= 0.0 {
        return Err("empty diagram".to_owned());
    }
    Ok(Diagram { uri: uri.to_owned(), w, h })
}

/// The helper's base URL, starting it on first use.
fn base_url() -> Result<String, String> {
    let st = store();
    if let Some((url, child)) = st.pele.borrow_mut().as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return Ok(url.clone());
        }
    }
    let (url, child) = start()?;
    *st.pele.borrow_mut() = Some((url.clone(), child));
    Ok(url)
}

/// `node <dir>/serve.mjs`, reading the `PORT <n>` line it prints first.
fn start() -> Result<(String, Child), String> {
    let script = std::env::var("ROX_PELE_DIR")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/pele").to_owned())
        + "/serve.mjs";
    let mut child = Command::new(node()?)
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot start node {script}: {e}"))?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "pele helper did not start".to_owned())?;
    let port = line
        .trim()
        .strip_prefix("PORT ")
        .ok_or_else(|| format!("pele helper said {line:?}"))?;
    Ok((format!("http://127.0.0.1:{port}"), child))
}

/// Node: `ROX_NODE`, else the usual install places (an app launched from
/// the Dock has a minimal PATH), else `PATH`.
fn node() -> Result<String, String> {
    if let Ok(n) = std::env::var("ROX_NODE") {
        return Ok(n);
    }
    for p in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        if std::path::Path::new(p).exists() {
            return Ok(p.to_owned());
        }
    }
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|d| format!("{d}/node"))
        .find(|p| std::path::Path::new(p).exists())
        .ok_or_else(|| "node not found (set ROX_NODE)".to_owned())
}
