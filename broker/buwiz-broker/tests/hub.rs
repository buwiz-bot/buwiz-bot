//! The hub switch (`BUWIZ_BROKER_HUB=1`): with the hub on, it answers the
//! brain's routes (a route no track has ported answers 501) and a path no
//! broker has gets its 404 and a preflight its 204; with it off, those
//! requests get the front door's own 404. `--no-node` is accepted and changes
//! nothing.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Broker {
    child: Child,
    port: u16,
    data: PathBuf,
}

impl Drop for Broker {
    fn drop(&mut self) {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start(name: &str, hub: bool) -> Broker {
    start_with(name, hub, false)
}

fn start_with(name: &str, hub: bool, no_node: bool) -> Broker {
    start_env(name, hub, no_node, &[])
}

fn start_env(name: &str, hub: bool, no_node: bool, env: &[(&str, &str)]) -> Broker {
    let port = free_port();
    let data = std::env::temp_dir().join(format!("buwiz-broker-hub-{name}-{}-{port}", std::process::id()));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"));
    cmd.args(["--port", &port.to_string()])
        .arg("--data")
        .arg(&data)
        .env_remove("BUWIZ_BROKER_HUB")
        .env_remove("ROX_BROKER_HUB")
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
        cmd.env("BUWIZ_BROKER_HUB", "1");
    }
    // the app passes it; it changes nothing
    if no_node {
        cmd.arg("--no-node");
    }
    cmd.env_remove("BUWIZ_BROKER_V1_HOST").env_remove("BUWIZ_BROKER_MCP_HOST").env("BUWIZ_TRUST_FILE", data.join("trust.json")).envs(env.iter().copied());
    let broker = Broker { child: cmd.spawn().unwrap(), port, data };
    let deadline = Instant::now() + Duration::from_secs(15);
    while request(port, "GET", "/health", b"").map(|(s, _)| s != 200).unwrap_or(true) {
        assert!(Instant::now() < deadline, "buwiz-broker on {port} never became healthy");
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
fn hub_on_answers_unported_hub_routes_501() {
    let b = start("on", true);
    // ported or not, the hub answers (the registry's own test checks that an
    // unregistered route is 501 not_ported)
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
        assert!(answer.0 != 501 || answer.1 == NOT_PORTED, "{method} {path}: {answer:?}");
    }
    // the host routes and the routine webhooks are answered (5.5h): the
    // token first, an unknown hook not found
    let unauthorized = r#"{"ok":false,"code":"unauthorized","message":"X-Buwiz-Host is missing or wrong","retryable":false}"#;
    for (method, path, body) in [("POST", "/v1/host/plugins/acme/enable", "{}"), ("GET", "/v1/host/plugins", "")] {
        assert_eq!(request(b.port, method, path, body.as_bytes()).unwrap(), (401, unauthorized.to_owned()), "{method} {path}");
    }
    assert_eq!(request(b.port, "POST", "/v1/hooks/ri_x/secret", b"{}").unwrap(), (404, r#"{"ok":false,"code":"not_found","message":"no such hook","retryable":false}"#.to_owned()));
    // a malformed hook path is a 404, answered by the dispatcher
    assert_eq!(request(b.port, "GET", "/v1/hooks/ri_x/secret", b"").unwrap().0, 404);
}

#[test]
fn hub_on_answers_every_route_and_unknown_paths_404() {
    let b = start("office", true);
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
    // the hub's since 6.3 (/popout)
    assert_eq!(request(b.port, "POST", "/popout", br#"{"name":"R1"}"#).unwrap(), (200, r#"{"ok":false,"error":"no such module"}"#.to_owned()));
    // deleted: Rox draws every diagram natively
    assert_eq!(request(b.port, "GET", "/mermaid", b"").unwrap(), (404, "not found".to_owned()));
    // a path no broker has: the hub's last resort
    assert_eq!(request(b.port, "POST", "/echo?x=1", b"{\"a\":1}").unwrap(), (404, "not found".to_owned()));
}

#[test]
fn the_no_node_flag_changes_nothing_and_the_hub_answers_preflights() {
    let b = start_with("no-node", true, true);
    let (status, root) = request(b.port, "GET", "/", b"").unwrap();
    assert!(status == 200 && root.starts_with("app-server ok — "), "{status} {root}");
    assert_eq!(request(b.port, "OPTIONS", "/threads", b"").unwrap(), (204, String::new()));
    assert_eq!(request(b.port, "GET", "/echo", b"").unwrap(), (404, "not found".to_owned()));
    let (_, meta) = request(b.port, "GET", "/meta", b"").unwrap();
    assert!(!meta.contains("node_version"), "{meta}");
}

/// Without the hub the same requests get the front door's own 404: it never
/// hands one to anything else.
#[test]
fn hub_off_answers_hub_routes_with_its_own_404() {
    let b = start("off", false);
    for (method, path) in [("GET", "/threads"), ("POST", "/v1/hooks/ah_x/secret"), ("GET", "/office/caps"), ("GET", "/role")] {
        let (status, body) = request(b.port, method, path, b"{}").unwrap();
        assert_eq!(status, 404, "{method} {path}");
        assert!(body.contains("no such route"), "{method} {path}: {body}");
    }
}

#[test]
fn hub_proves_itself_with_its_host_token_and_asks_for_none() {
    let b = start("prove", true);
    // written once the hub starts
    let deadline = Instant::now() + Duration::from_secs(10);
    let token = loop {
        match std::fs::read_to_string(b.data.join("host-token")) {
            Ok(t) if !t.trim().is_empty() => break t,
            _ => assert!(Instant::now() < deadline, "no host token"),
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let nonce = buwiz_protocol::keys::b64(&[42u8; 32]);
    // no X-Buwiz-Host on the request: the answer is the HMAC under the token
    let (status, body) = request(b.port, "POST", "/v1/host/prove", format!(r#"{{"nonce":"{nonce}"}}"#).as_bytes()).unwrap();
    assert_eq!(status, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let proof = v["proof"].as_str().unwrap();
    assert!(buwiz_protocol::proof::proof_matches(token.trim(), &nonce, proof));
    assert_eq!(proof, buwiz_protocol::proof::broker_proof(token.trim(), &nonce));
    assert!(!buwiz_protocol::proof::proof_matches("another token", &nonce, proof));
    // another nonce, another proof
    let other = buwiz_protocol::keys::b64(&[43u8; 32]);
    let (_, body) = request(b.port, "POST", "/v1/host/prove", format!(r#"{{"nonce":"{other}"}}"#).as_bytes()).unwrap();
    assert!(!body.contains(proof));
    // the nonce's bounds and form; a GET is not the route
    for bad in [r#"{}"#.to_owned(), r#"{"nonce":42}"#.to_owned(), format!(r#"{{"nonce":"{}"}}"#, "a".repeat(21)), format!(r#"{{"nonce":"{}"}}"#, "a".repeat(129)), r#"{"nonce":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaa="}"#.to_owned(), "not json".to_owned()] {
        let (status, body) = request(b.port, "POST", "/v1/host/prove", bad.as_bytes()).unwrap();
        assert_eq!(status, 400, "{bad}: {body}");
        assert!(!body.contains("proof"), "{bad}");
    }
    assert_eq!(request(b.port, "GET", "/v1/host/prove", b"").unwrap().0, 405);
    // the other host routes still want the token
    assert_eq!(request(b.port, "GET", "/v1/host/plugins", b"").unwrap().0, 401);
    let _ = std::fs::remove_dir_all(&b.data);
}

/// A mini app account's sign-in callback reaches the v1 plugin host with
/// the MCP host off: the line's (`BUWIZ_BROKER_V1_HOST=1`) or the hub's
/// own. A flow it never started is refused with its page, not the hub's
/// 501.
#[test]
fn an_account_callback_reaches_the_plugin_host_without_the_mcp_host() {
    let path = "/oauth/callback/account:app.buwiz.fixture%2Fgoogle%2Fdefault?code=c&state=s";
    for (name, env) in [("acct-line", &[("BUWIZ_BROKER_V1_HOST", "1"), ("BUWIZ_BROKER_MCP_HOST", "0")][..]), ("acct-hub", &[][..])] {
        let keys = format!("file:{}", std::env::temp_dir().join(format!("buwiz-broker-hub-{name}-keys-{}", std::process::id())).display());
        let env: Vec<(&str, &str)> = env.iter().copied().chain([("BUWIZ_KEYSTORE", keys.as_str())]).collect();
        let b = start_env(name, true, true, &env);
        let (status, body) = request(b.port, "GET", path, b"").unwrap();
        assert_eq!(status, 400, "{name}: {body}");
        assert!(body.contains("Authentication failed") && !body.contains("not_ported"), "{name}: {body}");
    }
}
