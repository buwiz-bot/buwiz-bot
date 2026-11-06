//! A machine here treated as a remote one, through a real `sshd` and a real TLS handshake:
//! `cargo test -p buwiz-computer --features mock --test real_remote -- --ignored`. Ignored in a
//! plain run, which may have no `sshd` or Docker. Asked for, it never skips.
//!
//! Its containers start with `c3t-`. Nothing listens on anything but loopback.

#![cfg(feature = "mock")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use buwiz_computer::loopback::{self, Sshd};
use buwiz_computer::remote::tls::fingerprint;
use buwiz_computer::remote::{Asked, Probed};
use buwiz_computer::{DockerProvider, Gate, Input, Machine, Mode, Power, Provider, RemoteProvider, Screen, Secret, Sleep, Spec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn temp(name: &str) -> PathBuf {
    // sshd wants its folders on a short path it can read
    let d = std::env::temp_dir().join(format!("c3t-{name}-{}-{}", std::process::id(), buwiz_computer::random_hex(3).unwrap()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn plain(sshd: &Sshd, dir: &Path) -> Machine {
    Asked { url: sshd.url(), key_file: Some(sshd.key.clone()), pin: Some(sshd.pin.clone()), ..Asked::default() }.machine(dir).unwrap()
}

fn screen(bot: &str) -> Screen {
    Screen { display: String::new(), viewer_port: 0, folder: format!("agents/{bot}"), profile: None }
}

/// The file listing of a tree, for seeing it's unchanged.
fn listing(dir: &Path) -> String {
    let out = std::process::Command::new("/usr/bin/find").arg(dir).output().unwrap();
    let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_owned).collect();
    lines.sort();
    lines.join("\n")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn a_plain_ssh_host_runs_commands_and_files_and_has_no_screen() {
    let dir = temp("plain");
    let sshd = Sshd::start(&dir.join("far"));
    let (computer, provider) = (dir.join("computer"), RemoteProvider::new());
    std::fs::create_dir_all(&computer).unwrap();

    // nothing is trusted until a person says: first what the far end offers, with no login
    let mut asked = Asked { url: sshd.url(), key_file: Some(sshd.key.clone()), ..Asked::default() };
    let offered = provider.probe(&asked.machine(&computer).unwrap()).await.unwrap();
    let Probed::Trust { pin, fingerprint, .. } = offered else { panic!("{offered:?}") };
    assert_eq!(pin, sshd.pin);
    let shown = std::process::Command::new("/usr/bin/ssh-keygen").arg("-lf").arg(sshd.dir.join("host_key.pub")).output().unwrap();
    assert!(String::from_utf8_lossy(&shown.stdout).contains(&fingerprint), "the fingerprint a person compares");
    asked.pin = Some(pin);
    let m = asked.machine(&computer).unwrap();
    assert!(matches!(provider.probe(&m).await.unwrap(), Probed::Ready { boxed: false, system: Some(_) }));
    assert_eq!(provider.power(&m).await.unwrap(), Power::Asleep, "a test leaves nothing open");

    provider.wake(&m).await.unwrap();
    assert_eq!(provider.power(&m).await.unwrap(), Power::Awake);
    provider.guard(&m).await.unwrap();
    assert!(!computer.join("ssh/key").exists(), "the key isn't left on disk once logged in");
    let bot = provider.screen_up(&m, "b_1").await.unwrap();
    assert_eq!(bot.folder, "agents/b_1");

    let out = provider.run(&m, &bot, "pwd; echo out; echo err >&2; exit 3", 10_000).await.unwrap();
    assert_eq!(out.exit_code, Some(3));
    assert!(out.stdout.trim_end().ends_with("out") && out.stderr.contains("err"), "{out:?}");
    assert!(out.stdout.contains("/buwiz-computer/agents/b_1"), "{}", out.stdout);
    assert!(sshd.home.join("buwiz-computer/agents/b_1").is_dir(), "its folder is in that login's home");

    // what a shell would break on is one word all the way
    let tricky = "printf '%s\\n' \"it's $((1+1)) `echo x`\" 'a  b'";
    assert_eq!(provider.run(&m, &bot, tricky, 10_000).await.unwrap().stdout, "it's 2 x\na  b\n");

    // files, as the tools ask: resolved first
    let real = provider.resolve(&m, "agents/b_1/notes/a.txt").await.unwrap();
    assert_eq!(real, "/workspace/agents/b_1/notes/a.txt");
    provider.write_file(&m, &real, "hello\n\0binary".as_bytes()).await.unwrap();
    assert_eq!(provider.read_file(&m, &real).await.unwrap(), "hello\n\0binary".as_bytes());
    assert!(provider.read_file(&m, "/workspace/agents/b_1/none").await.is_err());
    assert!(provider.read_file(&m, "/workspace/agents/b_1/notes").await.is_err(), "a folder");

    // a link out isn't followed, into a folder or to a file
    provider.run(&m, &bot, "mkdir -p /tmp/c3t-out; ln -s /tmp/c3t-out out; ln -s /etc/hosts hosts; ln -s /no/such dead", 10_000).await.unwrap();
    assert_eq!(provider.resolve(&m, "agents/b_1/out/x").await.unwrap().rsplit_once('/').unwrap().0, std::fs::canonicalize("/tmp/c3t-out").unwrap().to_str().unwrap());
    assert!(!provider.resolve(&m, "agents/b_1/hosts").await.unwrap().starts_with("/workspace/"));
    assert!(!provider.resolve(&m, "agents/b_1/dead").await.unwrap().starts_with("/workspace/"));
    let _ = std::fs::remove_dir_all("/tmp/c3t-out");

    // a file past the limit is refused, not cut: a cut read as whole gets written back
    let big = sshd.home.join("buwiz-computer/agents/b_1/big");
    std::fs::write(&big, vec![b'x'; 9 * 1024 * 1024]).unwrap();
    let said = provider.read_file(&m, "/workspace/agents/b_1/big").await.unwrap_err().sentence();
    assert!(said.contains("too big to read whole"), "{said}");

    // the screen isn't there, and says so
    for said in [
        provider.screenshot(&m, &bot).await.map(|_| ()),
        provider.input(&m, &bot, &Input::Click { x: 1, y: 1, button: 1, double: false }).await,
        provider.open_url(&m, &bot, "https://example.com").await,
        provider.record_start(&m, &bot).await.map(|_| ()),
    ] {
        assert!(matches!(said, Err(buwiz_computer::Problem::Unsupported(_))), "{said:?}");
    }
    assert_eq!(provider.viewer(&m, &bot).await.unwrap(), None);
    assert_eq!(provider.mark(&m, &bot).await, None);

    // a flood is cut, a hang is cut, and neither holds the call
    let flood = provider.run(&m, &bot, "head -c 5000000 /dev/zero | tr '\\0' x", 20_000).await.unwrap();
    assert!(flood.truncated && flood.stdout.len() <= 1024 * 1024, "{}", flood.stdout.len());
    let began = std::time::Instant::now();
    let slept = provider.run(&m, &bot, "sleep 30", 1000).await.unwrap();
    assert!(slept.timed_out && began.elapsed() < Duration::from_secs(8), "{slept:?}");

    // sleep hangs up and leaves the machine alone
    let before = listing(&sshd.home);
    provider.sleep(&m, Sleep::Deep).await.unwrap();
    assert_eq!(provider.power(&m).await.unwrap(), Power::Asleep);
    assert!(matches!(provider.run(&m, &bot, "true", 1000).await, Err(buwiz_computer::Problem::Asleep)));
    provider.wake(&m).await.unwrap();
    provider.destroy(&m).await.unwrap();
    assert_eq!(listing(&sshd.home), before, "disconnecting touched the far end");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn the_wrong_host_key_or_a_stranger_gets_nothing() {
    let dir = temp("pin");
    let sshd = Sshd::start(&dir.join("far"));
    let other = Sshd::start(&dir.join("other"));
    let provider = RemoteProvider::new();
    let computer = dir.join("computer");
    std::fs::create_dir_all(&computer).unwrap();

    // another machine's key pinned for this one: refused before any login
    let wrong = Asked { url: sshd.url(), key_file: Some(sshd.key.clone()), pin: Some(other.pin.clone()), ..Asked::default() }.machine(&computer).unwrap();
    let said = provider.wake(&wrong).await.unwrap_err().sentence();
    assert!(said.contains("isn't the one you trusted"), "{said}");
    assert_eq!(provider.power(&wrong).await.unwrap(), Power::Asleep);
    let log = std::fs::read_to_string(sshd.dir.join("sshd.log")).unwrap();
    assert!(!log.contains("Accepted"), "{log}");

    // the right host and a key it doesn't know
    let stranger = Asked { url: sshd.url(), key_file: Some(other.key.clone()), pin: Some(sshd.pin.clone()), ..Asked::default() }.machine(&computer).unwrap();
    assert!(provider.wake(&stranger).await.unwrap_err().sentence().contains("didn't accept"));

    // a dead port
    let dead = format!("ssh://{}@127.0.0.1:1", Sshd::user());
    let gone = Asked { url: dead, key_file: Some(sshd.key.clone()), pin: Some(sshd.pin.clone()), ..Asked::default() }.machine(&computer).unwrap();
    assert!(provider.wake(&gone).await.unwrap_err().sentence().contains("can't be reached"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn a_connection_that_drops_is_seen_and_a_click_brings_it_back() {
    let dir = temp("drop");
    let far = dir.join("far");
    let mut sshd = Sshd::start(&far);
    let provider = RemoteProvider::new();
    let computer = dir.join("computer");
    std::fs::create_dir_all(&computer).unwrap();
    let m = plain(&sshd, &computer);

    // a broker that went away left its master running: the next connection ends it, and is its own
    let gone_broker = RemoteProvider::new();
    gone_broker.wake(&m).await.unwrap();
    std::mem::forget(gone_broker);
    let masters = || String::from_utf8_lossy(&std::process::Command::new("/usr/bin/pgrep").args(["-f", "--", &format!("-p {} .*ControlMaster=yes", sshd.port)]).output().unwrap().stdout).lines().count();
    assert_eq!(masters(), 1);
    provider.wake(&m).await.unwrap();
    assert_eq!(masters(), 1, "the old master stayed");

    provider.screen_up(&m, "b_1").await.unwrap();
    let port = sshd.port;
    sshd.stop();
    for _ in 0..100 {
        if provider.power(&m).await.unwrap() == Power::Asleep {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(provider.power(&m).await.unwrap(), Power::Asleep, "a dead master still read as awake");
    assert!(provider.guard(&m).await.is_err());
    let sshd = Sshd::on(&far, port);
    provider.wake(&m).await.unwrap();
    assert_eq!(provider.run(&m, &screen("b_1"), "echo back", 5000).await.unwrap().stdout, "back\n");
    provider.destroy(&m).await.unwrap();
    drop(sshd);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs openssl"]
async fn a_certificate_the_public_roots_take_still_gets_no_token_before_a_yes() {
    use tokio::net::TcpListener;
    let dir = temp("vouched");
    let (exec, host) = ("exec.box.c3t.test".to_owned(), "host.box.c3t.test".to_owned());
    let (root, cert, key) = loopback::vouched_certificate(&dir, &[exec.clone(), host.clone()]);
    // what stands behind the proxy: each connection's bytes, sent here when it ends. It turns any
    // request away, so a call doesn't wait on it
    let behind = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let to = behind.local_addr().unwrap().to_string();
    let (ended, mut heard) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = behind.accept().await {
            let ended = ended.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let mut buf = [0u8; 2048];
                if let Ok(n @ 1..) = s.read(&mut buf).await {
                    got.extend_from_slice(&buf[..n]);
                    let _ = s.write_all(b"HTTP/1.1 401 No\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                }
                let _ = ended.send(String::from_utf8_lossy(&got).into_owned());
            });
        }
    });
    let at = loopback::terminator(cert.clone(), key, HashMap::from([(exec, to.clone()), (host, to)])).await;
    let provider = RemoteProvider::new().with_dial(at).with_root(root);
    let computer = dir.join("computer");
    std::fs::create_dir_all(&computer).unwrap();
    let mut asked = Asked { url: "https://box.c3t.test".into(), token: Some(Secret::new("the-token")), viewer_password: Some(Secret::new("the-password")), ..Asked::default() };
    let m = asked.machine(&computer).unwrap();
    let said = provider.probe(&m).await.unwrap();
    assert_eq!(said, Probed::Trust { fingerprint: fingerprint(&cert), pin: fingerprint(&cert), vouched: true });
    assert!(provider.wake(&m).await.is_err());
    // the handshake that found the roots take it got this far, and hung up with nothing said
    let first = tokio::time::timeout(Duration::from_secs(5), heard.recv()).await.expect("the proxy never reached what's behind it").unwrap();
    assert_eq!(first, "", "something went before a yes");
    while let Ok(more) = heard.try_recv() {
        assert_eq!(more, "", "something went before a yes");
    }

    // and the same way in does carry it, after a yes: so that silence was the code's
    asked.pin = Some(fingerprint(&cert));
    let m = asked.machine(&computer).unwrap();
    assert_eq!(provider.wake(&m).await.unwrap_err().sentence(), "The box refused this token.");
    let sent = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let text = heard.recv().await.unwrap();
            if !text.is_empty() {
                return text;
            }
        }
    });
    assert!(sent.await.expect("nothing arrived after a yes").contains("Bearer the-token"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A local box, its guest ports taken from Docker's publishing.
struct LocalBox {
    docker: DockerProvider,
    machine: Machine,
    ports: HashMap<u16, String>,
    dir: PathBuf,
}

impl LocalBox {
    async fn up(prefix: &str) -> Self {
        let image = std::env::var("BUWIZ_COMPUTER_IMAGE").unwrap_or_else(|_| "buwiz-computer:local".to_owned());
        let dir = temp(prefix);
        let docker = DockerProvider::new(image, prefix);
        docker.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
        let machine = docker.create(&Spec { id: format!("cmp_{prefix}"), dir: dir.clone() }).await.unwrap();
        docker.wake(&machine).await.unwrap();
        let (_, seen) = docker.look(&machine).await.unwrap();
        let ports = seen.seen.as_ref().unwrap().ports.clone();
        Self { docker, machine, ports, dir }
    }

    async fn gone(self, prefix: &str) {
        self.docker.destroy(&self.machine).await.unwrap();
        let _ = std::process::Command::new("docker").args(["network", "rm", &format!("{prefix}-net")]).output();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn viewer_answers(gate: &Gate, p: &RemoteProvider, m: &Machine, screen: &Screen) {
    let viewer = p.viewer(m, screen).await.unwrap().expect("a box has a viewer");
    assert!(viewer.far.is_some(), "a remote viewer goes by the far rules");
    let url = gate.open("c3t_a", viewer, Mode::Drive).unwrap();
    let (port, target) = url.strip_prefix("http://127.0.0.1:").unwrap().split_once('/').unwrap();
    let (port, target) = (port.parse::<u16>().unwrap(), format!("/{target}"));
    let entered = http(port, &target, "").await;
    let cookie = entered.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_owned();
    let clean = target.split('?').next().unwrap().to_owned();
    let page = http(port, &clean, &format!("Cookie: {cookie}\r\n")).await;
    assert!(page.starts_with("HTTP/1.1 200"), "{}", &page[..page.len().min(400)]);
    assert!(page.contains("Content-Security-Policy:"), "the gate's policy");
    assert!(page.to_lowercase().contains("<html"));
    // the socket goes through too, and input reaches the real screen
    let mut ws = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let hello = format!("GET {clean}ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {cookie}\r\nOrigin: http://127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n");
    ws.write_all(hello.as_bytes()).await.unwrap();
    let mut buf = [0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(10), ws.read(&mut buf)).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&buf[..n]));
}

async fn http(port: u16, target: &str, extra: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

/// Everything a Bot does on a box, through `p`, however it's connected.
async fn use_a_box(p: &RemoteProvider, m: &Machine) {
    let a = p.screen_up(m, "c3t_a").await.unwrap();
    assert!(a.display.starts_with(':') && (6902..=6905).contains(&a.viewer_port), "{a:?}");

    let shot = p.screenshot(m, &a).await.unwrap();
    assert!(shot.width > 0 && shot.for_model().0.starts_with(&[0xFF, 0xD8]));
    let since = p.mark(m, &a).await;
    assert!(since.is_some(), "the viewer's mark comes through");
    p.input(m, &a, &Input::Click { x: 40, y: 40, button: 1, double: false }).await.unwrap();
    p.settle(m, &a, buwiz_computer::WaitFor { since, ..Default::default() }).await.unwrap();

    let out = p.run(m, &a, "id -un; pwd", 10_000).await.unwrap();
    assert_eq!(out.exit_code, Some(0), "{out:?}");
    assert!(out.stdout.contains("/workspace/agents/c3t_a"), "{}", out.stdout);

    let path = format!("{}/notes/a.txt", a.folder);
    let real = p.resolve(m, &path).await.unwrap();
    assert_eq!(real, format!("/workspace/{path}"));
    p.write_file(m, &real, b"over the wire").await.unwrap();
    assert_eq!(p.read_file(m, &real).await.unwrap(), b"over the wire");

    let id = p.record_start(m, &a).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(p.record_stop(m, &a, id).await.unwrap().ends_with(".mp4"));

    p.open_url(m, &a, "http://127.0.0.1:1/").await.unwrap();

    let gate = Gate::start().await.unwrap();
    viewer_answers(&gate, p, m, &a).await;
    p.guard(m).await.unwrap();

    // a Bot leaving asks nothing of the far end: its screen and folder stay
    p.screen_down(m, "c3t_a").await.unwrap();
    assert!(p.screenshot(m, &a).await.is_ok(), "the screen stayed up");
}

/// That the box behind `m` is untouched by a disconnect: it answers Docker as before.
async fn still_there(local: &LocalBox, a: &Screen) {
    assert_eq!(local.docker.power(&local.machine).await.unwrap(), Power::Awake);
    assert_eq!(local.docker.read_file(&local.machine, &format!("/workspace/{}/notes/a.txt", a.folder)).await.unwrap(), b"over the wire");
    assert!(local.docker.screens(&local.machine).await.unwrap().iter().any(|b| b == "c3t_a"), "its screen was taken down");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd, Docker and the computer image"]
async fn a_real_box_works_over_real_ssh_and_a_disconnect_leaves_it_alone() {
    let dir = temp("box");
    let local = LocalBox::up("c3tb").await;
    // the box's ports on the far machine's loopback, as its compose file publishes them
    for port in [1337u16, 1340, 6901, 6902, 6903, 6904, 6905] {
        loopback::forward(port, local.ports[&port].clone()).await;
    }
    let sshd = Sshd::start(&dir.join("far"));
    let provider = RemoteProvider::new();
    let computer = dir.join("computer");
    std::fs::create_dir_all(&computer).unwrap();
    let asked = |token: &str| Asked { url: sshd.url(), token: Some(Secret::new(token)), viewer_password: Some(local.machine.viewer_password.clone()), key_file: Some(sshd.key.clone()), pin: Some(sshd.pin.clone()) };

    // a wrong token is told apart from no box
    let wrong = asked("not-the-token").machine(&computer).unwrap();
    assert_eq!(provider.probe(&wrong).await.unwrap_err().sentence(), "The box refused this token.");

    let m = asked(local.machine.token.expose()).machine(&computer).unwrap();
    assert_eq!(provider.probe(&m).await.unwrap(), Probed::Ready { boxed: true, system: None });
    provider.wake(&m).await.unwrap();
    assert_eq!(provider.power(&m).await.unwrap(), Power::Awake);
    use_a_box(&provider, &m).await;
    let a = provider.screen_up(&m, "c3t_a").await.unwrap();

    provider.sleep(&m, Sleep::Deep).await.unwrap();
    assert_eq!(provider.power(&m).await.unwrap(), Power::Asleep);
    still_there(&local, &a).await;

    // back on: found as it was
    provider.wake(&m).await.unwrap();
    assert_eq!(provider.screen_up(&m, "c3t_a").await.unwrap().display, a.display);
    provider.destroy(&m).await.unwrap();
    still_there(&local, &a).await;

    drop(sshd);
    local.gone("c3tb").await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs openssl, Docker and the computer image"]
async fn a_real_box_works_over_real_tls_with_a_pin_and_refuses_any_other() {
    let dir = temp("tls");
    let local = LocalBox::up("c3tt").await;
    let (cert, key) = loopback::certificate(&dir, "box.c3t.test");
    let mut routes: HashMap<String, String> = HashMap::new();
    routes.insert("exec.box.c3t.test".into(), local.ports[&1337].clone());
    routes.insert("host.box.c3t.test".into(), local.ports[&1340].clone());
    for port in 6901u16..=6905 {
        routes.insert(format!("desktop-{port}.box.c3t.test"), local.ports[&port].clone());
    }
    let at = loopback::terminator(cert.clone(), key, routes).await;
    let provider = RemoteProvider::new().with_dial(at).with_beat(Duration::from_secs(1));
    let computer = dir.join("computer");
    std::fs::create_dir_all(&computer).unwrap();
    let asked = |pin: Option<String>| Asked { url: "https://box.c3t.test".into(), token: Some(local.machine.token.clone()), viewer_password: Some(local.machine.viewer_password.clone()), pin, key_file: None };

    // no root vouches for it: nothing past the handshake, and a person gets its hash
    let unknown = asked(None).machine(&computer).unwrap();
    let Probed::Trust { fingerprint: shown, pin, vouched: false } = provider.probe(&unknown).await.unwrap() else { panic!() };
    assert_eq!(shown, fingerprint(&cert));
    assert!(provider.wake(&unknown).await.unwrap_err().sentence().contains("isn't trusted yet"));

    let wrong = asked(Some("0".repeat(64))).machine(&computer).unwrap();
    assert!(provider.wake(&wrong).await.unwrap_err().sentence().contains("isn't the one you trusted"));

    let m = asked(Some(pin)).machine(&computer).unwrap();
    assert_eq!(provider.probe(&m).await.unwrap(), Probed::Ready { boxed: true, system: None });
    provider.wake(&m).await.unwrap();
    use_a_box(&provider, &m).await;
    let a = provider.screen_up(&m, "c3t_a").await.unwrap();
    provider.sleep(&m, Sleep::Deep).await.unwrap();
    assert_eq!(provider.power(&m).await.unwrap(), Power::Asleep);
    still_there(&local, &a).await;

    // the box stops behind a proxy that's still up: it reads as not connected, with nobody asking
    provider.wake(&m).await.unwrap();
    local.docker.sleep(&local.machine, Sleep::Deep).await.unwrap();
    for _ in 0..200 {
        if provider.power(&m).await.unwrap() == Power::Asleep {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(provider.power(&m).await.unwrap(), Power::Asleep, "a dead box behind live relays read as awake");
    provider.destroy(&m).await.unwrap();

    local.gone("c3tt").await;
    let _ = std::fs::remove_dir_all(&dir);
}
