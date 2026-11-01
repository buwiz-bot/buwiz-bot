//! An in-test stand-in for opencodex: a loopback HTTP server that answers
//! each request with the next scripted answer (whole, in chunks a few
//! milliseconds apart, cut off mid-body, or refused), recording what it
//! got; and a `LocalTools` that plays sips, qlmanage, mv and secret.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::hub::traits::{LocalCommand, LocalOutput, LocalTools};
use crate::hub::types::BoxFuture;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// lowercased names, as sent
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

#[derive(Clone, Debug)]
pub enum Answer {
    /// status, headers, and the body in chunks sent apart
    Chunks(u16, Vec<(String, String)>, Vec<Vec<u8>>),
    /// a 200 event stream whose connection drops after `bytes`
    Die(Vec<u8>),
}

impl Answer {
    pub fn json(status: u16, v: &Value) -> Self {
        Answer::Chunks(status, vec![("content-type".into(), "application/json".into())], vec![crate::js::stringify(v).into_bytes()])
    }

    pub fn status(status: u16, v: &Value, extra: &[(&str, &str)]) -> Self {
        let mut h = vec![("content-type".to_owned(), "application/json".to_owned())];
        h.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Answer::Chunks(status, h, vec![crate::js::stringify(v).into_bytes()])
    }

    /// An event stream, cut into pieces of `size` bytes.
    pub fn sse(text: &str, size: usize) -> Self {
        Answer::Chunks(200, vec![("content-type".into(), "text/event-stream".into())], text.as_bytes().chunks(size.max(1)).map(<[u8]>::to_vec).collect())
    }
}

/// Events as an SSE body (`event: <type>\ndata: <json>\n\n`).
pub fn sse_text(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap_or(""), crate::js::stringify(e))).collect()
}

pub struct Fake {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    answers: Arc<Mutex<VecDeque<Answer>>>,
}

impl Fake {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let answers: Arc<Mutex<VecDeque<Answer>>> = Arc::default();
        let (reqs, ans) = (requests.clone(), answers.clone());
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (reqs, ans) = (reqs.clone(), ans.clone());
                tokio::spawn(async move {
                    let _ = serve(sock, reqs, ans).await;
                });
            }
        });
        Self { url, requests, answers }
    }

    pub fn push(&self, a: Answer) {
        self.answers.lock().unwrap().push_back(a);
    }

    pub fn taken(&self) -> Vec<Recorded> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

async fn serve(mut sock: tokio::net::TcpStream, reqs: Arc<Mutex<Vec<Recorded>>>, ans: Arc<Mutex<VecDeque<Answer>>>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let first: Vec<&str> = lines.next().unwrap_or("").split(' ').collect();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    reqs.lock().unwrap().push(Recorded { method: first.first().unwrap_or(&"").to_string(), path: first.get(1).unwrap_or(&"").to_string(), headers, body: String::from_utf8_lossy(&body).into_owned() });
    let answer = ans.lock().unwrap().pop_front().unwrap_or_else(|| Answer::json(500, &serde_json::json!({"error": {"message": "the fake has no answer"}})));
    match answer {
        Answer::Chunks(status, headers, chunks) => {
            let mut head = format!("HTTP/1.1 {status} X\r\nconnection: close\r\ntransfer-encoding: chunked\r\n");
            for (k, v) in headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str("\r\n");
            sock.write_all(head.as_bytes()).await?;
            for c in chunks {
                sock.write_all(format!("{:x}\r\n", c.len()).as_bytes()).await?;
                sock.write_all(&c).await?;
                sock.write_all(b"\r\n").await?;
                sock.flush().await?;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            sock.write_all(b"0\r\n\r\n").await?;
        }
        Answer::Die(bytes) => {
            sock.write_all(b"HTTP/1.1 200 X\r\nconnection: close\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n").await?;
            if !bytes.is_empty() {
                sock.write_all(format!("{:x}\r\n", bytes.len()).as_bytes()).await?;
                sock.write_all(&bytes).await?;
                sock.write_all(b"\r\n").await?;
            }
            sock.flush().await?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    sock.shutdown().await
}

/// sips (copies `<in>` to `--out`), qlmanage (`<dir>/<file>.png`), mv,
/// and secret (fails: no key), each call recorded.
#[derive(Default)]
pub struct Local {
    pub calls: Mutex<Vec<Vec<String>>>,
}

pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

impl LocalTools for Local {
    fn run(&self, cmd: LocalCommand) -> BoxFuture<'static, Result<LocalOutput, String>> {
        let mut call = vec![cmd.program.clone()];
        call.extend(cmd.args.clone());
        self.calls.lock().unwrap().push(call);
        let a = cmd.args;
        let code = match cmd.program.as_str() {
            "sips" => {
                let o = a.iter().position(|x| x == "--out").unwrap();
                std::fs::copy(&a[o - 1], &a[o + 1]).map(|_| 0).unwrap_or(1)
            }
            "qlmanage" => {
                let o = a.iter().position(|x| x == "-o").unwrap();
                let name = std::path::Path::new(&a[o + 2]).file_name().unwrap().to_string_lossy().into_owned();
                std::fs::write(std::path::Path::new(&a[o + 1]).join(format!("{name}.png")), PNG).map(|_| 0).unwrap_or(1)
            }
            "mv" => std::fs::rename(&a[0], &a[1]).map(|_| 0).unwrap_or(1),
            _ => 1,
        };
        Box::pin(async move { Ok(LocalOutput { code: Some(code), stdout: Vec::new(), stderr: Vec::new() }) })
    }
}
