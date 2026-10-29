//! rox-broker in front of a stand-in Node (tests/fixture-node.mjs): what it
//! answers itself, what it forwards unchanged, streams, restarts, shutdown,
//! and a second instance on a taken port.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Broker {
    child: Child,
    port: u16,
    pids: PathBuf,
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

fn start_on(port: u16, pids: &PathBuf) -> Child {
    start_with(port, pids, &[])
}

fn start_with(port: u16, pids: &PathBuf, env: &[(&str, &str)]) -> Child {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture-node.mjs");
    Command::new(env!("CARGO_BIN_EXE_rox-broker"))
        .args(["--port", &port.to_string(), "--node", "node", "--node-script"])
        .arg(&fixture)
        .env("FIXTURE_PID_FILE", pids)
        .env("FIXTURE_CHILD_FILE", pids.with_extension("children"))
        .envs(env.iter().copied())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

fn start(name: &str) -> Broker {
    start_env(name, &[])
}

fn start_env(name: &str, env: &[(&str, &str)]) -> Broker {
    let port = free_port();
    let pids = std::env::temp_dir().join(format!("rox-broker-test-{name}-{}-{port}.pids", std::process::id()));
    let _ = std::fs::remove_file(&pids);
    let _ = std::fs::remove_file(pids.with_extension("children"));
    let broker = Broker { child: start_with(port, &pids, env), port, pids };
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
    panic!("rox-broker on {port} never became healthy");
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

fn pids(b: &Broker) -> Vec<i32> {
    std::fs::read_to_string(&b.pids).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).collect()
}

fn children(b: &Broker) -> Vec<i32> {
    std::fs::read_to_string(b.pids.with_extension("children")).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).collect()
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn it_answers_health_and_meta_itself() {
    let b = start("meta");
    let (status, body) = get(b.port, "/health").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("\"broker\":\"rox-broker\""), "{body}");
    let (status, body) = get(b.port, "/meta").unwrap();
    assert_eq!(status, 200);
    let meta: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(meta["broker"], "rox-broker");
    assert_eq!(meta["ported"], serde_json::json!(["state", "prefs", "keymap", "timeclock", "skills"]));
    assert!(meta["node_version"].as_str().unwrap().starts_with('v'), "{body}");
}

#[test]
fn forward_all_sends_the_ported_routes_to_node_again() {
    let b = start_env("forward-all", &[("ROX_BROKER_FORWARD_ALL", "1")]);
    let meta: serde_json::Value = serde_json::from_str(&get(b.port, "/meta").unwrap().1).unwrap();
    assert_eq!(meta["ported"], serde_json::json!([]));
    // the fixture answers 404 "no" to what it doesn't know
    assert_eq!(get(b.port, "/prefs").unwrap(), (404, "no".to_owned()));
}

#[test]
fn it_forwards_method_path_query_headers_and_raw_body() {
    let b = start("echo");
    let raw = b"{\"v\":1,  \"text\":\"spacing kept\"}\n";
    let (status, body) = request(
        b.port,
        "POST",
        "/echo/x?thread=t5&n=1",
        &[("X-Rox-Host", "host-token"), ("X-Rox-Signature", "t=1,v1=ab"), ("Content-Type", "application/json")],
        raw,
    )
    .unwrap();
    assert_eq!(status, 200);
    let got: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(got["method"], "POST");
    assert_eq!(got["url"], "/echo/x?thread=t5&n=1");
    assert_eq!(got["host"], format!("127.0.0.1:{}", b.port));
    assert_eq!(got["rox_host"], "host-token");
    assert_eq!(got["signature"], "t=1,v1=ab");
    assert_eq!(got["body"].as_str().unwrap(), String::from_utf8_lossy(raw));
    assert_eq!(get(b.port, "/nothing-here").unwrap().0, 404);
}

#[test]
fn events_stream_through_as_they_are_written() {
    let b = start("sse");
    let mut s = TcpStream::connect(("127.0.0.1", b.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(format!("GET /sse HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", b.port).as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut first = None;
    let mut second = None;
    let start = Instant::now();
    let mut line = String::new();
    while r.read_line(&mut line).unwrap_or(0) > 0 {
        if line.starts_with("data: 1") {
            first = Some(start.elapsed());
        }
        if line.starts_with("data: 2") {
            second = Some(start.elapsed());
        }
        line.clear();
    }
    let (first, second) = (first.expect("first event"), second.expect("second event"));
    assert!(second - first >= Duration::from_millis(200), "events arrived together: {first:?} {second:?}");
}

#[test]
fn node_comes_back_after_it_exits() {
    let b = start("restart");
    let before = pids(&b);
    assert_eq!(before.len(), 1);
    let _ = get(b.port, "/exit");
    let deadline = Instant::now() + Duration::from_secs(5);
    while pids(&b).len() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(pids(&b).len(), 2, "node was not started again within 5 s");
    wait_healthy(b.port, Duration::from_secs(5));
    assert_eq!(get(b.port, "/echo").unwrap().0, 200);
}

#[test]
fn stopping_the_front_door_leaves_no_node_and_no_child_of_it() {
    let mut b = start("stop");
    let node = pids(&b)[0];
    let deadline = Instant::now() + Duration::from_secs(5);
    while children(&b).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let child = children(&b)[0];
    assert!(alive(node) && alive(child));
    unsafe { libc::kill(b.child.id() as i32, libc::SIGTERM) };
    let status = b.child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert!(!alive(node), "node {node} outlived the front door");
    assert!(!alive(child), "node's child {child} outlived the front door");
}

#[test]
fn a_child_of_a_node_that_died_goes_with_it() {
    let b = start("orphan");
    let deadline = Instant::now() + Duration::from_secs(5);
    while children(&b).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let first = children(&b)[0];
    let _ = get(b.port, "/exit");
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(first) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(first), "the dead node's child {first} was left running");
    wait_healthy(b.port, Duration::from_secs(10));
}

#[test]
fn a_second_front_door_on_a_taken_port_exits_without_starting_node() {
    let b = start("second");
    let mut second = start_on(b.port, &b.pids);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = second.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the second front door kept running");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(1));
    assert_eq!(pids(&b).len(), 1, "the second front door started a node");
}
