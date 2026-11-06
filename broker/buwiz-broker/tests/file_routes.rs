//! The file routes through the real front door: `Leaf::serve`, the guard and the hub, in a broker of
//! its own, as a web page would ask.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Broker(Child, PathBuf, u16);

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn start() -> Broker {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let root = std::env::temp_dir().join(format!("buwiz-file-routes-{}-{port}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (assets, data) = (root.join("server"), root.join("data"));
    std::fs::create_dir_all(assets.join("office")).unwrap();
    for folder in ["files/m1", "assets"] {
        std::fs::create_dir_all(data.join("generated").join(folder)).unwrap();
    }
    write(&data.join("generated/files/m1/page.html"), "<script>x</script>");
    write(&data.join("generated/files/m1/a.js"), "x");
    let child = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"))
        .args(["--port", &port.to_string(), "--no-node", "--assets"])
        .arg(&assets)
        .arg("--data")
        .arg(&data)
        .env("BUWIZ_BROKER_HUB", "1")
        .env("BUWIZ_BROKER_DATA_DIR", &data)
        .env("BUWIZ_NO_KEYCHAIN", "1")
        .env("OPENCODEX_URL", "http://127.0.0.1:9")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
        .env("GEN_BASE_URL", "http://127.0.0.1:9")
        .env_remove("ANTHROPIC_API_KEY")
        .env("HOME", &data)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let broker = Broker(child, root, port);
    let deadline = Instant::now() + Duration::from_secs(60);
    while get(&broker, "/health", "").0 != 200 {
        assert!(Instant::now() < deadline, "it never answered");
        std::thread::sleep(Duration::from_millis(100));
    }
    broker
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

/// (status, head).
fn get(b: &Broker, path: &str, extra: &str) -> (u16, String) {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", b.2)) else { return (0, String::new()) };
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{extra}Connection: close\r\n\r\n", b.2).as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_lowercase();
    let head = text.split("\r\n\r\n").next().unwrap_or_default().to_owned();
    (head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), head)
}

#[test]
fn a_page_gets_the_policy_and_an_empty_origin_gets_nothing() {
    let b = start();
    let (status, head) = get(&b, "/file/raw/m1/page.html", "");
    assert_eq!(status, 200, "{head}");
    assert!(head.contains("content-security-policy: sandbox\r\n") && head.contains("x-content-type-options: nosniff"), "{head}");
    let (status, head) = get(&b, "/file/preview/m1/page.html", "");
    assert_eq!(status, 200, "{head}");
    assert!(head.contains("content-security-policy: sandbox allow-scripts\r\n"), "{head}");

    // a preview's script, or any sandboxed frame on any website: the guard turns it away everywhere,
    // its own files included, and says nothing a page could read
    for path in ["/file/preview/m1/a.js", "/file/preview/m1/page.html", "/file/raw/m1/a.js", "/turns", "/threads", "/prefs", "/state"] {
        let (status, head) = get(&b, path, "Origin: null\r\n");
        assert_eq!(status, 403, "{path}: {head}");
        assert!(!head.contains("access-control-allow-origin"), "{path}: {head}");
    }
    // the broker's own origin still reads a file, as the office viewer does
    let own = format!("Origin: http://127.0.0.1:{}\r\n", b.2);
    let (status, head) = get(&b, "/file/raw/m1/a.js", &own);
    assert_eq!(status, 200, "{head}");
    assert!(head.contains("access-control-allow-origin"), "{head}");
}
