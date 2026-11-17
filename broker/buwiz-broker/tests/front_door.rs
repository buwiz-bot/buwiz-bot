//! buwiz-broker's front door as a process: what it answers itself (`/health`,
//! `/meta`), what it says at start, the arguments it takes (`--no-node` is
//! accepted and does nothing; `--node` and `--node-script` are gone), shutdown,
//! and a second instance on a taken port.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Broker {
    child: Child,
    port: u16,
    /// what the broker wrote to stderr
    err: PathBuf,
}

impl Drop for Broker {
    fn drop(&mut self) {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.err);
    }
}

impl Broker {
    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap_or_default()
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A broker on `port` with `args` after `--port`; its stderr goes to `err`.
fn spawn(port: u16, args: &[&str], env: &[(&str, &str)], err: &PathBuf) -> Child {
    Command::new(env!("CARGO_BIN_EXE_buwiz-broker"))
        .args(["--port", &port.to_string()])
        .args(args)
        .env_remove("BUWIZ_BROKER_HUB")
        .env_remove("ROX_BROKER_HUB")
        .env_remove("BUWIZ_TRUST_FILE")
        .env_remove("ROX_TRUST_FILE")
        .env_remove("BUWIZ_BROKER_V1_HOST")
        .env_remove("BUWIZ_BROKER_MCP_HOST")
        .envs(env.iter().copied())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(err).unwrap())
        .spawn()
        .unwrap()
}

fn start(name: &str) -> Broker {
    start_args(name, &[])
}

fn start_args(name: &str, args: &[&str]) -> Broker {
    start_env(name, args, &[])
}

fn start_env(name: &str, args: &[&str], env: &[(&str, &str)]) -> Broker {
    let port = free_port();
    let err = std::env::temp_dir().join(format!("buwiz-broker-test-{name}-{}-{port}.err", std::process::id()));
    let data = std::env::temp_dir().join(format!("buwiz-broker-test-{name}-{}-{port}.data", std::process::id()));
    let args: Vec<&str> = ["--data", data.to_str().unwrap()].into_iter().chain(args.iter().copied()).collect();
    let broker = Broker { child: spawn(port, &args, env, &err), port, err };
    wait_healthy(broker.port, Duration::from_secs(15));
    broker
}

fn wait_healthy(port: u16, within: Duration) {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if get(port, "/health").map(|(s, _)| s == 200).unwrap_or(false) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("buwiz-broker on {port} never became healthy");
}

/// One request, `Connection: close`; → (status, body), chunked decoded.
fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> std::io::Result<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { unchunk(rest) } else { rest.to_owned() };
    Ok((status, body))
}

fn get(port: u16, path: &str) -> std::io::Result<(u16, String)> {
    request(port, "GET", path, &[], b"")
}

fn unchunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&rest[..n]);
        s = &rest[n + 2..];
    }
    out
}

#[test]
fn it_answers_health_and_meta_itself() {
    let b = start("meta");
    let (status, body) = get(b.port, "/health").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("\"broker\":\"buwiz-broker\""), "{body}");
    let (status, body) = get(b.port, "/meta").unwrap();
    assert_eq!(status, 200);
    let meta: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(meta["broker"], "buwiz-broker");
    assert_eq!(meta["ported"], serde_json::json!(["state", "prefs", "keymap", "skills", "files"]));
    assert!(meta.get("node_version").is_none(), "{body}");
}

/// Without the hub nothing answers a path the front door has no entry for:
/// its own 404 (a preflight too); a ported route (`/prefs`) is still the leaf's.
#[test]
fn without_the_hub_a_path_nothing_answers_is_a_404() {
    let b = start("no-route");
    let (status, body) = get(b.port, "/nothing-here").unwrap();
    assert_eq!(status, 404);
    assert!(body.contains("no such route"), "{body}");
    assert_eq!(request(b.port, "OPTIONS", "/threads", &[], b"").unwrap().0, 404);
    // a ported route reaches the leaf, whatever it answers for a data dir with nothing in it
    let (_, body) = get(b.port, "/prefs").unwrap();
    assert!(!body.contains("no such route"), "{body}");
}

#[test]
fn without_assets_it_says_once_what_is_off_and_serves() {
    let b = start("no-assets");
    let log = b.stderr();
    let line = "buwiz-broker: started without --assets, so off: the plugin catalog, the office pages and seeds, the plugin trust list";
    assert_eq!(log.matches(line).count(), 1, "{log}");
    assert_eq!(log.matches("without --assets").count(), 1, "{log}");
    assert_eq!(get(b.port, "/health").unwrap().0, 200);
}

#[test]
fn a_named_trust_file_is_not_listed_as_off() {
    let trust = std::env::temp_dir().join(format!("buwiz-broker-test-trust-{}.json", std::process::id()));
    let b = start_env("trust-named", &[], &[("BUWIZ_TRUST_FILE", trust.to_str().unwrap())]);
    let log = b.stderr();
    assert!(log.contains("so off: the plugin catalog, the office pages and seeds\n"), "{log}");
    assert!(!log.contains("trust list"), "{log}");
}

#[test]
fn with_assets_it_says_nothing_is_off() {
    let assets = std::env::temp_dir().join(format!("buwiz-broker-test-assets-{}", std::process::id()));
    std::fs::create_dir_all(&assets).unwrap();
    let b = start_args("assets", &["--assets", assets.to_str().unwrap()]);
    assert!(!b.stderr().contains("without --assets"), "{}", b.stderr());
    assert_eq!(get(b.port, "/health").unwrap().0, 200);
    let _ = std::fs::remove_dir_all(&assets);
}

/// The app still passes `--no-node`: accepted, and it changes nothing.
#[test]
fn no_node_is_accepted_and_changes_nothing() {
    let b = start_args("no-node", &["--no-node"]);
    assert_eq!(get(b.port, "/health").unwrap().0, 200);
    let log = b.stderr();
    assert!(!log.to_lowercase().contains("unknown argument"), "{log}");
    assert_eq!(log.matches("without --assets").count(), 1, "{log}");
}

#[test]
fn the_node_flags_are_unknown_arguments() {
    for flag in [&["--node", "node"][..], &["--node-script", "app-server.mjs"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_buwiz-broker")).args(["--port", &free_port().to_string()]).args(flag).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(&format!("unknown argument {}", flag[0])), "{err}");
    }
}

#[test]
fn stopping_it_exits_cleanly() {
    let mut b = start("stop");
    unsafe { libc::kill(b.child.id() as i32, libc::SIGTERM) };
    let status = b.child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert!(b.stderr().contains("buwiz-broker: stopped"), "{}", b.stderr());
}

#[test]
fn a_second_front_door_on_a_taken_port_exits() {
    let b = start("second");
    let err = std::env::temp_dir().join(format!("buwiz-broker-test-second-{}-{}.err", std::process::id(), b.port));
    let mut second = spawn(b.port, &[], &[], &err);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = second.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the second front door kept running");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(1));
    assert!(std::fs::read_to_string(&err).unwrap_or_default().contains("is taken"));
    let _ = std::fs::remove_file(&err);
}
