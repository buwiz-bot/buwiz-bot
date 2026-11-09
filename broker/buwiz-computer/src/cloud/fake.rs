#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A fake HTTPS platform for tests: it speaks TLS with the test CA (`e2b/testdata`), so a provider
//! that checks names and roots can be tested without the network. Shared by every cloud provider.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// The test CA, DER. It is not in the public roots: a provider must be given it.
pub const CA: &[u8] = include_bytes!("../e2b/testdata/ca.der");
const LEAF: &[u8] = include_bytes!("../e2b/testdata/leaf.der");
const KEY: &[u8] = include_bytes!("../e2b/testdata/leaf.key.der");

/// One request the fake received.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    /// the request target, query and all
    pub path: String,
    /// the `Host` header
    pub host: String,
    /// names lowercased
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `METHOD path`, as the platform's API saw it.
    pub fn call(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

/// status (0 = never answer), headers, body
pub type Answer = (u16, Vec<(String, String)>, Vec<u8>);

pub fn ok(status: u16, body: &str) -> Answer {
    (
        status,
        vec![("Content-Type".to_owned(), "application/json".to_owned())],
        body.as_bytes().to_vec(),
    )
}

pub fn empty(status: u16) -> Answer {
    (status, vec![], vec![])
}

pub struct Fake {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// What the platform's own API got, in order.
    pub fn api_calls(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter(|r| r.host == "api.e2b.app")
            .map(Seen::call)
            .collect()
    }
}

/// A TLS server on loopback that answers each request with `route`, and records it.
pub async fn serve(route: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let chain = vec![
        CertificateDer::from(LEAF.to_vec()),
        CertificateDer::from(CA.to_vec()),
    ];
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            chain,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let (route, log) = (Arc::new(route), seen.clone());
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, route, log) = (acceptor.clone(), route.clone(), log.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Some(request) = read_request(&mut tls).await else {
                    return;
                };
                let answer = route(&request);
                log.lock().unwrap_or_else(|e| e.into_inner()).push(request);
                write_answer(&mut tls, answer).await;
            });
        }
    });
    Fake { addr, seen }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn read_request<S: AsyncReadExt + Unpin>(stream: &mut S) -> Option<Seen> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at + 4;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let host = headers
        .iter()
        .find(|(k, _)| k == "host")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Some(Seen {
        method,
        path,
        host,
        headers,
        body,
    })
}

async fn write_answer<S: AsyncWriteExt + Unpin>(stream: &mut S, (status, headers, body): Answer) {
    if status == 0 {
        tokio::time::sleep(Duration::from_secs(5)).await;
        return;
    }
    let mut head = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.shutdown().await;
}
