//! The broker as Rox.app carries it (plan 7.1): `--no-node` with the hub
//! on, its static files from `--assets` (the bundle's `broker/server`,
//! `node_modules` beside it) instead of Node's server dir.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// This test's port (plan 7, track F2's).
const PORT: u16 = 8800;

struct Broker(Child, std::path::PathBuf);

impl Drop for Broker {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn request(port: u16, path: &str) -> std::io::Result<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Ok((head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), rest.to_owned()))
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn a_bundled_broker_runs_without_node_and_serves_its_assets() {
    let root = std::env::temp_dir().join(format!("buwiz-broker-bundle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (assets, data) = (root.join("broker/server"), root.join("data"));
    write(&assets.join("office/viewer.html"), "<!doctype html><title>viewer fixture</title>");
    std::fs::create_dir_all(&data).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"))
        // a Node that can't start: --no-node must never try it
        .args(["--port", &PORT.to_string(), "--no-node", "--node", "/nonexistent/node", "--node-script", "/nonexistent/server/app-server.mjs", "--assets"])
        .arg(&assets)
        .arg("--data")
        .arg(&data)
        .env("BUWIZ_BROKER_HUB", "1")
        .env_remove("BUWIZ_BROKER_V1_HOST")
        .env_remove("ROX_BROKER_V1_HOST")
        .env_remove("BUWIZ_BROKER_MCP_HOST")
        .env_remove("ROX_BROKER_MCP_HOST")
        // the model gateway at a closed port, never the real opencodex
        .env("OPENCODEX_URL", "http://127.0.0.1:9")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
        .env("GEN_BASE_URL", "http://127.0.0.1:9")
        .env_remove("ANTHROPIC_API_KEY")
        .env("HOME", &data)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _broker = Broker(child, root.clone());
    let deadline = Instant::now() + Duration::from_secs(15);
    let health = loop {
        match request(PORT, "/health") {
            Ok(answer) => break answer,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("buwiz-broker on {PORT} never answered: {e}"),
        }
    };
    // up with no Node to wait for
    assert_eq!(health.0, 200, "{}", health.1);
    assert!(health.1.contains(r#""ok":true"#), "{}", health.1);
    let (status, body) = request(PORT, "/office/").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("viewer fixture"), "{body}");
    // the editor's page isn't in these assets: the error names the assets dir
    let (status, body) = request(PORT, "/office/editor/x.docx").unwrap();
    assert_eq!(status, 500);
    assert!(body.contains("broker/server/office/editor/editor.html"), "{body}");
}
