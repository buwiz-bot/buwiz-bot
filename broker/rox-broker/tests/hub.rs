//! The hub switch (`ROX_BROKER_HUB=1`, plan D30) in front of a stand-in
//! Node (tests/fixture-node.mjs, logging every request it gets): with the
//! hub on, a hub route no track has ported answers 501 and Node never sees
//! it, and Node runs in its office role; with it off, the same request is
//! forwarded as today. No route reaches Node with the hub on (phase 7); a
//! path no broker has is Node's 404 from the hub. With
//! `ROX_BROKER_NO_NODE=1` too, Node is never started.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Broker {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Drop for Broker {
    fn drop(&mut self) {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log);
        let _ = std::fs::remove_file(self.log.with_extension("role"));
    }
}

impl Broker {
    /// The requests Node got, without the supervisor's health checks.
    fn seen(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log).unwrap_or_default().lines().filter(|l| *l != "GET /health").map(str::to_owned).collect()
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start(name: &str, hub: bool) -> Broker {
    start_with(name, hub, false)
}

fn start_with(name: &str, hub: bool, no_node: bool) -> Broker {
    let port = free_port();
    let log = std::env::temp_dir().join(format!("rox-broker-hub-{name}-{}-{port}.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let data = std::env::temp_dir().join(format!("rox-broker-hub-{name}-{}-{port}", std::process::id()));
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture-node.mjs");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rox-broker"));
    cmd.args(["--port", &port.to_string(), "--node", "node", "--node-script"])
        .arg(&fixture)
        .arg("--data")
        .arg(&data)
        .env("FIXTURE_LOG", &log)
        .env("FIXTURE_ROLE_FILE", log.with_extension("role"))
        .env_remove("ROX_BROKER_HUB")
        .env_remove("ROX_BROKER_NO_NODE")
        .env_remove("ROX_NODE_ROLE")
        // the hub's /models and image routes call the model gateway: at a
        // closed port, never the real opencodex (localhost:8080), and no
        // connections file from this user's home
        .env("OPENCODEX_URL", "http://127.0.0.1:9")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
        .env("GEN_BASE_URL", "http://127.0.0.1:9")
        .env_remove("ANTHROPIC_API_KEY")
        .env("HOME", &data)
        .stdout(std::process::Stdio::null());
    if hub {
        cmd.env("ROX_BROKER_HUB", "1");
    }
    if no_node {
        cmd.env("ROX_BROKER_NO_NODE", "1");
    }
    let broker = Broker { child: cmd.spawn().unwrap(), port, log };
    let deadline = Instant::now() + Duration::from_secs(15);
    while request(port, "GET", "/health", b"").map(|(s, _)| s != 200).unwrap_or(true) {
        assert!(Instant::now() < deadline, "rox-broker on {port} never became healthy");
        std::thread::sleep(Duration::from_millis(100));
    }
    broker
}

/// One request, `Connection: close`; → (status, body). Bodies are JSON.
fn request(port: u16, method: &str, path: &str, body: &[u8]) -> std::io::Result<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len());
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    Ok((status, rest.to_owned()))
}

const NOT_PORTED: &str = r#"{"ok":false,"code":"not_ported"}"#;

#[test]
fn hub_on_answers_unported_hub_routes_501_and_node_never_sees_them() {
    let b = start("on", true);
    // ported or not, the hub answers: the stand-in Node answers 404 "no"
    // to all of these, and its log stays empty (the registry's own test
    // checks that an unregistered route is 501 not_ported)
    for (method, path, body) in [
        ("GET", "/models", ""),
        ("GET", "/artifacts", ""),
        ("GET", "/routines", ""),
        ("POST", "/app/meta", "{}"),
        ("POST", "/v1/host/plugins/acme/enable", "{}"),
        ("GET", "/v1/host/plugins", ""),
        ("POST", "/v1/hooks/ah_x/secret", "{}"),
        ("POST", "/v1/hooks/ri_x/secret", "{}"),
    ] {
        let answer = request(b.port, method, path, body.as_bytes()).unwrap();
        assert_ne!(answer, (404, "no".to_owned()), "{method} {path} reached Node");
        assert!(answer.0 != 501 || answer.1 == NOT_PORTED, "{method} {path}: {answer:?}");
    }
    // the host routes and the routine webhooks are answered (5.5h): the
    // token first, an unknown hook not found
    let unauthorized = r#"{"ok":false,"code":"unauthorized","message":"X-Rox-Host is missing or wrong","retryable":false}"#;
    for (method, path, body) in [("POST", "/v1/host/plugins/acme/enable", "{}"), ("GET", "/v1/host/plugins", "")] {
        assert_eq!(request(b.port, method, path, body.as_bytes()).unwrap(), (401, unauthorized.to_owned()), "{method} {path}");
    }
    assert_eq!(request(b.port, "POST", "/v1/hooks/ri_x/secret", b"{}").unwrap(), (404, r#"{"ok":false,"code":"not_found","message":"no such hook","retryable":false}"#.to_owned()));
    // a malformed hook path is Node's 404, answered by the dispatcher
    assert_eq!(request(b.port, "GET", "/v1/hooks/ri_x/secret", b"").unwrap().0, 404);
    assert_eq!(b.seen(), Vec::<String>::new(), "Node saw a hub route");
    // Node runs in its office role
    assert_eq!(std::fs::read_to_string(b.log.with_extension("role")).unwrap(), "office\n");
}

#[test]
fn hub_on_answers_every_route_and_unknown_paths_404() {
    let b = start("office", true);
    // the office routes are Rust's since 6.1: /office/* and /file/render
    // for an office file never reach Node
    assert_eq!(request(b.port, "GET", "/office/caps", b"").unwrap().0, 200);
    let (status, missing) = request(b.port, "GET", "/file/render?path=a.docx", b"").unwrap();
    assert_eq!((status, missing.as_str()), (200, r#"{"name":"a.docx","ext":"docx","size":0,"kind":"docx","variant":"docx","error":"missing"}"#));
    // every other kind since phase 7, and GET /
    let (status, missing) = request(b.port, "GET", "/file/render?path=a.md", b"").unwrap();
    assert_eq!((status, missing.as_str()), (200, r#"{"name":"a.md","ext":"md","size":0,"kind":"markdown","variant":"generic","error":"missing"}"#));
    let (status, root) = request(b.port, "GET", "/", b"").unwrap();
    assert!(status == 200 && root.starts_with("app-server ok — "), "{status} {root}");
    // deleted in 6.2 (no caller): no entry, so the hub's own 404
    assert_eq!(request(b.port, "POST", "/highlight", b"{}").unwrap(), (404, "not found".to_owned()));
    // the hub's since 6.2 (/mermaid*) and 6.3 (/popout)
    assert_eq!(request(b.port, "POST", "/popout", br#"{"name":"R1"}"#).unwrap(), (200, r#"{"ok":false,"error":"no such module"}"#.to_owned()));
    assert_eq!(request(b.port, "GET", "/mermaid", b"").unwrap(), (404, "not found".to_owned()));
    // a path no broker has: Node's last resort, from the hub (Node's office
    // role would answer it 501)
    assert_eq!(request(b.port, "POST", "/echo?x=1", b"{\"a\":1}").unwrap(), (404, "not found".to_owned()));
    assert_eq!(b.seen(), Vec::<String>::new(), "Node saw a request");
}

#[test]
fn no_node_starts_no_node_and_answers_preflights() {
    let b = start_with("no-node", true, true);
    let (status, root) = request(b.port, "GET", "/", b"").unwrap();
    assert!(status == 200 && root.starts_with("app-server ok — "), "{status} {root}");
    assert_eq!(request(b.port, "OPTIONS", "/threads", b"").unwrap(), (204, String::new()));
    assert_eq!(request(b.port, "GET", "/echo", b"").unwrap(), (404, "not found".to_owned()));
    let (_, meta) = request(b.port, "GET", "/meta", b"").unwrap();
    assert!(meta.contains("\"node_version\":\"\""), "{meta}");
    // the stand-in Node writes its role file when it starts: it never did
    assert!(!b.log.with_extension("role").exists(), "Node was started");
    assert_eq!(b.seen(), Vec::<String>::new());
}

#[test]
fn no_node_needs_the_hub() {
    // the env var and the flag (the broker inside Rox.app) alike
    let env = Command::new(env!("CARGO_BIN_EXE_rox-broker")).args(["--port", &free_port().to_string()]).env_remove("ROX_BROKER_HUB").env("ROX_BROKER_NO_NODE", "1").output().unwrap();
    let flag = Command::new(env!("CARGO_BIN_EXE_rox-broker")).args(["--port", &free_port().to_string(), "--no-node"]).env_remove("ROX_BROKER_HUB").env_remove("ROX_BROKER_NO_NODE").output().unwrap();
    for out in [env, flag] {
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("(ROX_BROKER_NO_NODE=1) needs ROX_BROKER_HUB=1"));
    }
}

#[test]
fn hub_off_forwards_hub_routes_unchanged() {
    let b = start("off", false);
    assert_eq!(request(b.port, "GET", "/threads", b"").unwrap(), (404, "no".to_owned()));
    assert_eq!(request(b.port, "POST", "/v1/hooks/ah_x/secret", b"{}").unwrap(), (404, "no".to_owned()));
    assert_eq!(request(b.port, "GET", "/office/caps", b"").unwrap(), (404, "no".to_owned()));
    assert_eq!(request(b.port, "GET", "/role", b"").unwrap(), (200, String::new()));
    assert_eq!(b.seen(), ["GET /threads", "POST /v1/hooks/ah_x/secret", "GET /office/caps", "GET /role"]);
}
