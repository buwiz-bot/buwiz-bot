//! A real SSH server and a real TLS terminator, on loopback, for the tests that treat a machine
//! here as a remote one. Each lives in a folder of its own and goes when dropped.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// `/usr/sbin/sshd`, as this login, on `127.0.0.1`. Its sessions have `home` for a home, so
/// nothing they make is in the real one. Only `key` logs in.
pub struct Sshd {
    pub port: u16,
    pub home: PathBuf,
    /// a private key it takes
    pub key: PathBuf,
    /// its host key, as `<type> <key>`
    pub pin: String,
    pub dir: PathBuf,
    child: Child,
}

fn keygen(path: &Path) {
    let done = Command::new("/usr/bin/ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(path).status().unwrap();
    assert!(done.success(), "ssh-keygen failed");
}

impl Sshd {
    pub fn start(dir: &Path) -> Self {
        Self::on(dir, 0)
    }

    /// On `port`, or a free one for 0: a test restarts it there to see a connection come back.
    pub fn on(dir: &Path, port: u16) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        let (host_key, key, home) = (dir.join("host_key"), dir.join("client_key"), dir.join("home"));
        std::fs::create_dir_all(&home).unwrap();
        if !host_key.exists() {
            keygen(&host_key);
            keygen(&key);
        }
        let port = if port == 0 { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() } else { port };
        let authorized = dir.join("authorized_keys");
        std::fs::copy(dir.join("client_key.pub"), &authorized).unwrap();
        let config = dir.join("sshd_config");
        std::fs::write(
            &config,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {h}\nAuthorizedKeysFile {a}\nPidFile {p}\nUsePAM no\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowTcpForwarding local\nX11Forwarding no\nSetEnv HOME={home}\n",
                h = host_key.display(),
                a = authorized.display(),
                p = dir.join("sshd.pid").display(),
                home = home.display(),
            ),
        )
        .unwrap();
        let log = std::fs::File::create(dir.join("sshd.log")).unwrap();
        let mut child = Command::new("/usr/sbin/sshd").args(["-D", "-e", "-f"]).arg(&config).stdin(Stdio::null()).stdout(Stdio::null()).stderr(log).spawn().unwrap();
        for _ in 0..200 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                let text = std::fs::read_to_string(dir.join("host_key.pub")).unwrap();
                let pin = text.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                return Self { port, home, key, pin, dir: dir.to_owned(), child };
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("sshd never listened: {}", std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default());
    }

    /// What this login's name is, for `ssh://`.
    pub fn user() -> String {
        std::env::var("USER").unwrap()
    }

    pub fn url(&self) -> String {
        format!("ssh://{}@127.0.0.1:{}", Self::user(), self.port)
    }

    pub fn stop(&mut self) {
        // the sessions it forked outlive it, each its own process
        let _ = Command::new("/usr/bin/pkill").args(["-P", &self.child.id().to_string()]).status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Carries a loopback `listen` port to `to`, as the machine's own loopback would have it.
pub async fn forward(listen: u16, to: String) {
    let listener = TcpListener::bind(("127.0.0.1", listen)).await.unwrap_or_else(|e| panic!("port {listen}: {e}"));
    tokio::spawn(async move {
        while let Ok((mut inbound, _)) = listener.accept().await {
            let to = to.clone();
            tokio::spawn(async move {
                if let Ok(mut out) = TcpStream::connect(&to).await {
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut out).await;
                }
            });
        }
    });
}

/// A self-signed certificate and its key, as DER, by `openssl`.
pub fn certificate(dir: &Path, name: &str) -> (Vec<u8>, Vec<u8>) {
    std::fs::create_dir_all(dir).unwrap();
    let (cert, key) = (dir.join(format!("{name}.der")), dir.join(format!("{name}.key.der")));
    let pem = dir.join(format!("{name}.key.pem"));
    let ran = |args: &[&str]| assert!(Command::new("/usr/bin/openssl").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "openssl {args:?}");
    let subject = format!("/CN={name}");
    ran(&["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", &subject, "-addext", "basicConstraints=CA:FALSE", "-keyout", pem.to_str().unwrap(), "-out", cert.to_str().unwrap(), "-outform", "DER"]);
    ran(&["pkcs8", "-topk8", "-nocrypt", "-in", pem.to_str().unwrap(), "-out", key.to_str().unwrap(), "-outform", "DER"]);
    (std::fs::read(cert).unwrap(), std::fs::read(key).unwrap())
}

/// A certificate authority's root, and a certificate and key it issued for each of `names`, as
/// DER, by `openssl`: what the public roots would vouch for.
pub fn vouched_certificate(dir: &Path, names: &[String]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    std::fs::create_dir_all(dir).unwrap();
    let at = |f: &str| dir.join(f).to_str().unwrap().to_owned();
    let ran = |args: &[&str]| assert!(Command::new("/usr/bin/openssl").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "openssl {args:?}");
    let sans: Vec<String> = names.iter().map(|n| format!("DNS:{n}")).collect();
    std::fs::write(at("ext"), format!("basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName={}\n", sans.join(","))).unwrap();
    ran(&["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=c3t root", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign", "-keyout", &at("ca.key"), "-out", &at("ca.pem")]);
    ran(&["x509", "-in", &at("ca.pem"), "-outform", "DER", "-out", &at("ca.der")]);
    ran(&["req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=c3t box", "-keyout", &at("leaf.key"), "-out", &at("leaf.csr")]);
    ran(&["x509", "-req", "-in", &at("leaf.csr"), "-CA", &at("ca.pem"), "-CAkey", &at("ca.key"), "-CAcreateserial", "-days", "2", "-extfile", &at("ext"), "-outform", "DER", "-out", &at("leaf.der")]);
    ran(&["pkcs8", "-topk8", "-nocrypt", "-in", &at("leaf.key"), "-out", &at("leaf.key.der"), "-outform", "DER"]);
    (std::fs::read(at("ca.der")).unwrap(), std::fs::read(at("leaf.der")).unwrap(), std::fs::read(at("leaf.key.der")).unwrap())
}

/// A TLS proxy on loopback as a person would run one: it goes by the name each connection asks
/// for, and hands the bytes to that service's port. A viewer goes by its own Host, so for a
/// `desktop-` name it sets that first, as `header_up Host {upstream_hostport}` does in Caddy (one
/// request a connection is all the gate sends). `routes` is name → address. Returns where it listens.
pub async fn terminator(certificate: Vec<u8>, key: Vec<u8>, routes: HashMap<String, String>) -> SocketAddr {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![CertificateDer::from(certificate)], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key))).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, routes) = (acceptor.clone(), routes.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                let name = tls.get_ref().1.server_name().map(str::to_owned).unwrap_or_default();
                let Some(to) = routes.get(&name) else { return };
                let Ok(mut out) = TcpStream::connect(to).await else { return };
                if name.starts_with("desktop-") && !with_host(&mut tls, &mut out, to).await {
                    return;
                }
                let _ = tokio::io::copy_bidirectional(&mut tls, &mut out).await;
            });
        }
    });
    addr
}

/// Passes one request's head on, with `host` for its Host.
async fn with_host(from: &mut (impl AsyncRead + Unpin), to: &mut TcpStream, host: &str) -> bool {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        if !matches!(from.read(&mut byte).await, Ok(1)) {
            return false;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let lines: Vec<String> = text.split("\r\n").map(|l| if l.to_ascii_lowercase().starts_with("host:") { format!("Host: {host}") } else { l.to_owned() }).collect();
    to.write_all(lines.join("\r\n").as_bytes()).await.is_ok()
}
