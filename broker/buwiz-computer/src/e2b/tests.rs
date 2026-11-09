//! The E2B provider against a fake platform that speaks TLS with a test CA: its names are
//! `*.e2b.app`, its certificate is one the tests' root vouches for, and the box behind its relays
//! answers as a box. Nothing here reaches E2B.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper::Method;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::E2bProvider;
use crate::cloud::Api;
use crate::{Machine, Power, Problem, Provider, ProviderKind, Reach, Secret, Sleep, Spec, Woke};

const CA: &[u8] = include_bytes!("testdata/ca.der");
const LEAF: &[u8] = include_bytes!("testdata/leaf.der");
const KEY: &[u8] = include_bytes!("testdata/leaf.key.der");

const CREATED: &str = r#"{"sandboxID":"sbx-1","envdAccessToken":"envd-1","templateID":"tpl","clientID":"c","envdVersion":"0.2.0"}"#;

/// One request the fake received.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    /// the request target, query and all
    path: String,
    /// the `Host` header
    host: String,
    /// names lowercased
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `METHOD path`, as the platform's API saw it.
    fn call(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

/// status (0 = never answer), headers, body
type Answer = (u16, Vec<(String, String)>, Vec<u8>);

fn ok(status: u16, body: &str) -> Answer {
    (
        status,
        vec![("Content-Type".to_owned(), "application/json".to_owned())],
        body.as_bytes().to_vec(),
    )
}

fn empty(status: u16) -> Answer {
    (status, vec![], vec![])
}

struct Fake {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// What the platform's own API got, in order.
    fn api_calls(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter(|r| r.host == "api.e2b.app")
            .map(Seen::call)
            .collect()
    }
}

/// A TLS server on loopback that answers each request with `route`, and records it.
async fn serve(route: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> Fake {
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

/// The platform as it answers a sandbox that exists, and the box behind its names.
fn platform(req: &Seen) -> Answer {
    let api = req.host == "api.e2b.app";
    let envd = req.host == "sandbox.e2b.app";
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/v2/sandboxes") if api => ok(201, CREATED),
        ("POST", p) if envd && p.starts_with("/files?") => ok(200, "[]"),
        ("POST", "/sandboxes/sbx-1/pause") if api => empty(204),
        ("POST", "/v2/sandboxes/sbx-1/connect") if api => ok(201, "{}"),
        ("GET", "/sandboxes/sbx-1") if api => ok(200, r#"{"state":"paused"}"#),
        ("DELETE", "/sandboxes/sbx-1") if api => empty(204),
        ("GET", "/v1/info") => ok(200, r#"{"capabilities":{"viewer":{"kind":"voxl"}}}"#),
        ("GET", "/v1/ready") => ok(200, "{}"),
        ("GET", "/v1/screens") => ok(200, "[]"),
        _ => ok(404, "{}"),
    }
}

fn provider(addr: SocketAddr) -> E2bProvider {
    E2bProvider::new(Some(Secret::new("key-1")), Some("tpl".to_owned()))
        .with_api(Api::with_test(addr, Some(CA.to_vec())))
}

fn spec(name: &str) -> Spec {
    Spec {
        id: "c1".to_owned(),
        dir: std::env::temp_dir().join(format!("e2b-test-{name}")),
    }
}

/// A saved record for sandbox `id`, as create would have left it.
fn machine(id: &str, spec: &Spec) -> Machine {
    Machine {
        provider: ProviderKind::E2b,
        reach: Reach::Sandbox { id: id.to_owned() },
        image: Some("tpl".to_owned()),
        token: Secret::new("tok-e2b"),
        viewer_password: Secret::new("pw-e2b"),
        dir: spec.dir.clone(),
        link: None,
        seen: None,
    }
}

#[tokio::test]
async fn the_lifecycle_is_one_sandbox_and_its_secrets_go_in_in_order() {
    let fake = serve(platform).await;
    let p = provider(fake.addr);
    let made = p.create(&spec("lifecycle")).await.unwrap();
    assert_eq!(
        made.reach,
        Reach::Sandbox {
            id: "sbx-1".to_owned()
        }
    );
    assert_eq!(p.power(&made).await.unwrap(), Power::Asleep);
    assert_eq!(p.wake(&made).await.unwrap(), Woke::Resumed);
    assert_eq!(p.power(&made).await.unwrap(), Power::Awake);
    p.sleep(&made, Sleep::Light).await.unwrap();
    assert_eq!(p.power(&made).await.unwrap(), Power::Asleep);
    assert_eq!(p.wake(&made).await.unwrap(), Woke::Resumed);
    p.destroy(&made).await.unwrap();

    assert_eq!(
        fake.api_calls(),
        vec![
            "POST /v2/sandboxes",
            "POST /sandboxes/sbx-1/pause",
            "GET /sandboxes/sbx-1",
            "POST /v2/sandboxes/sbx-1/connect",
            "POST /sandboxes/sbx-1/pause",
            "GET /sandboxes/sbx-1",
            "POST /v2/sandboxes/sbx-1/connect",
            "DELETE /sandboxes/sbx-1",
        ]
    );
    let requests = fake.requests();
    // the key goes only to the platform's API; the envd token only to the sandbox's files
    assert!(requests
        .iter()
        .filter(|r| r.header("x-api-key").is_some())
        .all(|r| r.host == "api.e2b.app" && r.header("x-api-key") == Some("key-1")));
    let uploads: Vec<&Seen> = requests
        .iter()
        .filter(|r| r.host == "sandbox.e2b.app")
        .collect();
    let paths: Vec<&str> = uploads.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/files?path=/run/buwiz/token&username=box",
            "/files?path=/run/buwiz/viewer&username=box",
            "/files?path=/run/buwiz/go&username=box"
        ]
    );
    assert!(uploads
        .iter()
        .all(|r| r.header("x-access-token") == Some("envd-1")
            && r.header("e2b-sandbox-id") == Some("sbx-1")
            && r.header("e2b-sandbox-port") == Some("49983")));
    assert_eq!(uploads[0].body, made.token.expose().as_bytes());
    assert_eq!(uploads[1].body, made.viewer_password.expose().as_bytes());
    assert!(uploads[2].body.is_empty(), "go is a trigger, not a secret");
    // the box got its bearer, and only after the handshake that checked its name
    let box_calls: Vec<&Seen> = requests.iter().filter(|r| r.path == "/v1/info").collect();
    assert!(!box_calls.is_empty());
    assert!(box_calls
        .iter()
        .all(|r| r.header("authorization") == Some(&format!("Bearer {}", made.token.expose()))));
}

#[tokio::test]
async fn a_missing_key_or_template_is_a_sentence_and_no_call_is_made() {
    let no_key = E2bProvider::new(None, Some("tpl".to_owned()));
    let sentence = no_key.check().await.unwrap_err().sentence();
    assert!(sentence.contains("E2B_API_KEY"), "{sentence}");
    let no_template = E2bProvider::new(Some(Secret::new("key-1")), None);
    let sentence = no_template.check().await.unwrap_err().sentence();
    assert!(sentence.contains("BUWIZ_E2B_TEMPLATE"), "{sentence}");
    assert!(no_key.create(&spec("nokey")).await.is_err());
}

#[tokio::test]
async fn nothing_is_sent_to_a_host_the_roots_dont_vouch_for() {
    // the fake's certificate is not in the public roots, and this provider trusts no extra root
    let fake = serve(platform).await;
    let p = E2bProvider::new(Some(Secret::new("key-1")), Some("tpl".to_owned()))
        .with_api(Api::with_test(fake.addr, None));
    let err = p.create(&spec("untrusted")).await.unwrap_err();
    assert!(matches!(err, Problem::Failed(_)), "{err:?}");
    assert!(!err.sentence().contains("key-1"));
    assert!(
        fake.requests().is_empty(),
        "a request was written before the name checked out"
    );
}

#[tokio::test]
async fn power_reads_the_platform_when_no_link_is_held() {
    for (status, body, want) in [
        (200, r#"{"state":"running"}"#, Power::Asleep),
        (200, r#"{"state":"paused"}"#, Power::Asleep),
        (404, "{}", Power::Absent),
        (
            200,
            r#"{"state":"killing"}"#,
            Power::Broken("killing".to_owned()),
        ),
    ] {
        let fake = serve(move |_| ok(status, body)).await;
        let p = provider(fake.addr);
        let record = machine("sbx-1", &spec("power"));
        assert_eq!(p.power(&record).await.unwrap(), want, "{status} {body}");
    }
}

#[tokio::test]
async fn a_failed_create_kills_the_sandbox_and_says_why_without_the_key() {
    // the files won't take the secrets: the sandbox is killed, and no sentence says the key
    let fake = serve(|req| {
        if req.host == "sandbox.e2b.app" {
            ok(500, "{}")
        } else {
            platform(req)
        }
    })
    .await;
    let err = provider(fake.addr)
        .create(&spec("failed"))
        .await
        .unwrap_err();
    assert!(
        err.sentence().contains("so the computer was stopped"),
        "{}",
        err.sentence()
    );
    assert!(!err.sentence().contains("key-1") && !err.sentence().contains("envd-1"));
    assert_eq!(
        fake.api_calls(),
        vec!["POST /v2/sandboxes", "DELETE /sandboxes/sbx-1"]
    );
}

#[tokio::test]
async fn a_status_is_a_sentence_and_a_missing_template_names_itself() {
    let fake = serve(|req| {
        if req.path == "/v2/sandboxes" {
            ok(404, "{}")
        } else {
            platform(req)
        }
    })
    .await;
    let err = provider(fake.addr)
        .create(&spec("template"))
        .await
        .unwrap_err();
    assert!(
        err.sentence().contains("no template named tpl"),
        "{}",
        err.sentence()
    );

    let fake = serve(|req| {
        if req.path == "/v2/sandboxes" {
            ok(401, r#"{"message":"key-1 is bad"}"#)
        } else {
            platform(req)
        }
    })
    .await;
    let err = provider(fake.addr).create(&spec("auth")).await.unwrap_err();
    assert!(
        err.sentence().contains("refused this broker's key"),
        "{}",
        err.sentence()
    );
    assert!(
        !err.sentence().contains("key-1"),
        "the platform's body is never shown"
    );
}

#[tokio::test]
async fn a_sandbox_that_is_gone_is_gone_on_wake_and_fine_to_destroy() {
    let fake = serve(|req| {
        if req.path == "/v2/sandboxes/sbx-1/connect" {
            ok(404, "{}")
        } else {
            platform(req)
        }
    })
    .await;
    let p = provider(fake.addr);
    let record = machine("sbx-1", &spec("gone"));
    assert_eq!(p.wake(&record).await.unwrap_err(), Problem::Gone);

    let fake = serve(|req| {
        if req.method == "DELETE" {
            empty(404)
        } else {
            platform(req)
        }
    })
    .await;
    provider(fake.addr)
        .destroy(&machine("sbx-1", &spec("gone2")))
        .await
        .unwrap();
}

#[tokio::test]
async fn sweep_stops_every_sandbox_the_computer_names_and_recover_makes_none() {
    let fake = serve(|req| {
        let api = req.host == "api.e2b.app";
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", p) if api && p.contains("nextToken=tok2") => {
                ok(200, r#"[{"sandboxID":"sbx-b"}]"#)
            }
            ("GET", p) if api && p.contains("metadata=buwiz-computer%3Dc1") => (
                200,
                vec![("X-Next-Token".to_owned(), "tok2".to_owned())],
                br#"[{"sandboxID":"sbx-a"}]"#.to_vec(),
            ),
            ("DELETE", _) if api => empty(204),
            _ => ok(404, "{}"),
        }
    })
    .await;
    let p = provider(fake.addr);
    p.sweep(
        "c1",
        Some(&Reach::Sandbox {
            id: "sbx-c".to_owned(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        fake.api_calls(),
        vec![
            "GET /v2/sandboxes?state=running,paused&limit=100&metadata=buwiz-computer%3Dc1",
            "GET /v2/sandboxes?state=running,paused&limit=100&metadata=buwiz-computer%3Dc1&nextToken=tok2",
            "DELETE /sandboxes/sbx-a",
            "DELETE /sandboxes/sbx-b",
            "DELETE /sandboxes/sbx-c",
        ]
    );

    let fake = serve(platform).await;
    assert!(provider(fake.addr)
        .recover(&spec("recover"), None)
        .await
        .unwrap()
        .is_none());
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn answers_are_limited_in_size_and_time_and_redirects_are_errors() {
    let fake = serve(|req| match req.path.as_str() {
        "/big" => (200, vec![], vec![b'x'; 2 * 1024 * 1024]),
        "/redirect" => (
            302,
            vec![("Location".to_owned(), "https://evil.example/".to_owned())],
            vec![],
        ),
        "/hold" => empty(0),
        _ => ok(200, "{}"),
    })
    .await;
    let api = Api::with_test(fake.addr, Some(CA.to_vec()));
    let patience = Duration::from_secs(3);
    assert!(api
        .call("api.e2b.app", Method::GET, "/big", &[], None, patience)
        .await
        .is_err());
    let redirect = api
        .call("api.e2b.app", Method::GET, "/redirect", &[], None, patience)
        .await
        .unwrap_err();
    assert!(
        redirect.sentence().contains("redirect"),
        "{}",
        redirect.sentence()
    );
    let slow = api
        .call(
            "api.e2b.app",
            Method::GET,
            "/hold",
            &[],
            None,
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
    assert!(slow.sentence().contains("too long"), "{}", slow.sentence());
    assert!(api
        .call("api.e2b.app", Method::GET, "/small", &[], None, patience)
        .await
        .is_ok());
}
