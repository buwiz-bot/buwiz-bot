//! A box's services as the providers' tests need them: the front tab it reports, and a note of
//! every call that reaches it, so a test can say which of them a request caused. Loopback only.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub(crate) struct FakeGuest {
    tab: Mutex<Option<String>>,
    calls: Mutex<Vec<String>>,
}

impl FakeGuest {
    /// A box whose front tab shows `tab`, when it says one.
    pub(crate) fn showing(tab: Option<&str>) -> Arc<Self> {
        Arc::new(Self { tab: Mutex::new(tab.map(str::to_owned)), calls: Mutex::new(Vec::new()) })
    }

    /// Each call so far, as `METHOD /path body`.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// The calls that did something to the browser: not the reading of the front tab.
    pub(crate) fn browser_calls(&self) -> Vec<String> {
        self.calls().into_iter().filter(|c| !c.starts_with("GET ")).collect()
    }

    /// Notes `request` (a whole HTTP request) and gives the body of its answer.
    pub(crate) fn answer(&self, request: &str) -> String {
        let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
        let line = head.lines().next().unwrap_or_default();
        let mut parts = line.split_whitespace();
        let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        let path = target.split('?').next().unwrap_or_default();
        self.calls.lock().unwrap().push(format!("{method} {path} {body}").trim_end().to_owned());
        if path == "/v1/chrome/active-tab" {
            return serde_json::json!({"url": *self.tab.lock().unwrap(), "focus": "none", "seq": 1}).to_string();
        }
        match path {
            "/v1/info" => r#"{"capabilities":{"viewer":{"kind":"voxl"}}}"#.to_owned(),
            "/v1/screens" => "[]".to_owned(),
            _ => "{}".to_owned(),
        }
    }

    /// Serves it on a loopback port: the address, as `http://…`.
    pub(crate) async fn serve(self: &Arc<Self>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let this = self.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let this = this.clone();
                tokio::spawn(async move { this.serve_one(socket).await });
            }
        });
        addr
    }

    /// One request on `socket`, answered.
    pub(crate) async fn serve_one(&self, mut socket: TcpStream) {
        let request = read_request(&mut socket).await;
        let body = self.answer(&request);
        let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        let _ = socket.write_all(reply.as_bytes()).await;
    }
}

/// A whole request: its head, and as much body as it says it has.
pub(crate) async fn read_request(socket: &mut TcpStream) -> String {
    let mut got = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        got.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&got).into_owned();
        if let Some(at) = text.find("\r\n\r\n") {
            let length: usize = text[..at].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse().ok())).unwrap_or(0);
            if got.len() >= at + 4 + length {
                return text;
            }
        }
        if n == 0 {
            return text;
        }
    }
}
