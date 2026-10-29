//! The Node broker behind the front door: started on a free loopback port,
//! watched, and restarted with backoff when it exits. Node leads a process
//! group of its own, which its plugin and MCP children join: stopping Node
//! stops the group, so no child outlives it (Node itself leaves them
//! running on SIGTERM; a Google Workspace plugin did, once per restart).

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;

#[derive(Default)]
pub struct Node {
    port: AtomicU16,
    ready: AtomicBool,
    pid: AtomicI32,
    stopping: AtomicBool,
    pub version: std::sync::OnceLock<String>,
}

impl Node {
    /// The port Node answers on, once it is up.
    pub fn port(&self) -> Option<u16> {
        self.ready.load(Ordering::SeqCst).then(|| self.port.load(Ordering::SeqCst))
    }

    /// Asks Node and its children to stop and keeps them stopped. Returns
    /// once the group is gone.
    pub async fn stop(&self, grace: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid > 0 {
            stop_group(pid, grace).await;
        }
    }
}

/// SIGTERM to process group `pgid`, SIGKILL after `grace` to whatever is
/// left, then wait until none is.
async fn stop_group(pgid: i32, grace: Duration) {
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let deadline = Instant::now() + grace;
    while group_alive(pgid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if group_alive(pgid) {
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
        for _ in 0..100 {
            if !group_alive(pgid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn group_alive(pgid: i32) -> bool {
    // the leader counts until the supervisor's wait reaps it
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// A port nothing listens on now, on loopback.
fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// `GROK_ALLOWED_ORIGINS` for Node: what the environment gives, plus the
/// front door's own origins. Node allows its own port's origins by itself,
/// and behind the front door its port is a private one.
pub fn allowed_origins(env: Option<&str>, public_port: u16) -> String {
    let mut out: Vec<String> = env.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
    for origin in [format!("http://127.0.0.1:{public_port}"), format!("http://localhost:{public_port}")] {
        if !out.contains(&origin) {
            out.push(origin);
        }
    }
    out.join(",")
}

/// `node --version`, for `/meta`.
pub fn version_of(node: &std::path::Path) -> String {
    std::process::Command::new(node)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Runs Node until the front door stops: start, wait for `/health`, watch,
/// restart with backoff (0.5 s doubling to 10 s, back to 0.5 s after a
/// minute up).
pub async fn supervise(config: Config, node: Arc<Node>) {
    let mut backoff = Duration::from_millis(500);
    while !node.stopping.load(Ordering::SeqCst) {
        let port = match free_port() {
            Ok(port) => port,
            Err(e) => {
                eprintln!("rox-broker: no free port for node: {e}");
                tokio::time::sleep(backoff).await;
                continue;
            }
        };
        let mut cmd = tokio::process::Command::new(&config.node);
        cmd.process_group(0).arg(&config.node_script)
            .env("PORT", port.to_string())
            .env("GROK_ALLOWED_ORIGINS", allowed_origins(std::env::var("GROK_ALLOWED_ORIGINS").ok().as_deref(), config.port))
            .stdin(std::process::Stdio::null());
        if let Some(data) = &config.data {
            cmd.env("GROK_DATA_DIR", data);
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                eprintln!("rox-broker: could not start node ({}): {e}", config.node.display());
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
        };
        let started = Instant::now();
        node.pid.store(child.id().map(|p| p as i32).unwrap_or(0), Ordering::SeqCst);
        node.port.store(port, Ordering::SeqCst);
        let watch = {
            let node = node.clone();
            tokio::spawn(async move {
                for _ in 0..600 {
                    if healthy(port).await {
                        node.ready.store(true, Ordering::SeqCst);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
        };
        let pgid = node.pid.load(Ordering::SeqCst);
        let status = child.wait().await;
        watch.abort();
        node.ready.store(false, Ordering::SeqCst);
        if node.stopping.load(Ordering::SeqCst) {
            break;
        }
        node.pid.store(0, Ordering::SeqCst);
        // Node went down on its own: its children go with it before the next
        if pgid > 0 {
            stop_group(pgid, Duration::from_secs(2)).await;
        }
        eprintln!("rox-broker: node exited ({}); restarting", status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()));
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_millis(500);
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(10));
    }
}

/// Whether Node answers `GET /health` with 200 on `port`.
async fn healthy(port: u16) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(Ok(mut s)) = tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(("127.0.0.1", port))).await else {
        return false;
    };
    if s.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").await.is_err() {
        return false;
    }
    let mut head = [0u8; 12];
    matches!(tokio::time::timeout(Duration::from_secs(2), s.read_exact(&mut head)).await, Ok(Ok(_))) && head.starts_with(b"HTTP/1.1 200")
}

#[cfg(test)]
mod tests {
    use super::allowed_origins;

    #[test]
    fn node_allows_the_front_doors_origin_and_keeps_the_environments() {
        assert_eq!(allowed_origins(None, 8787), "http://127.0.0.1:8787,http://localhost:8787");
        assert_eq!(allowed_origins(Some(""), 8787), "http://127.0.0.1:8787,http://localhost:8787");
        assert_eq!(
            allowed_origins(Some("https://rox.example, http://127.0.0.1:8787"), 8787),
            "https://rox.example,http://127.0.0.1:8787,http://localhost:8787"
        );
    }
}
