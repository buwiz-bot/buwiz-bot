//! Against the real daemon and the real image: `cargo test -p buwiz-computer --test real_docker
//! -- --ignored`. Ignored in a plain run, which may have no Docker. Asked for, it never skips: no
//! Docker or no image is a failure that says so.
//!
//! Its containers start with `c2t-` or `c2j-`, and the only port on the Mac it tries is one it opened.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use buwiz_computer::job::{self, Seen};
use buwiz_computer::{DockerProvider, Gate, Input, Machine, Mode, Power, Provider, Screen, Sleep, Spec};
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

    let m = p.create(&Spec { id: "cmp_c2test".into(), dir: dir.clone(), network: Default::default() }).await.unwrap();
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

/// A job goes on past the command that started it, and past the broker that sent that command, as
/// the Bot's own user. It's frozen by a sleep, and a stop takes all of it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_job_outlives_its_command_as_the_bots_user() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2j-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image.clone(), "c2j");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let m = p.create(&Spec { id: "cmp_c2job".into(), dir: dir.clone(), network: Default::default() }).await.unwrap();
    let outcome = tokio::spawn({
        let m = m.clone();
        async move { jobs(&image, &m).await }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2j-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn seen(p: &DockerProvider, m: &Machine, s: &Screen, id: &str) -> Option<Seen> {
    job::read_look(&p.run(m, s, &job::look(id), 20_000).await.unwrap().stdout)
}

async fn start_job(p: &DockerProvider, m: &Machine, s: &Screen, id: &str, command: &str) {
    let path = p.resolve(m, &format!("{}/{}", s.folder, job::script_path(id))).await.unwrap();
    p.write_file(m, &path, job::script(id, command).as_bytes()).await.unwrap();
    let began = std::time::Instant::now();
    let out = p.run(m, s, &job::start(id), 20_000).await.unwrap();
    assert!(out.stdout.trim().parse::<u32>().is_ok(), "{out:?}");
    assert!(began.elapsed() < std::time::Duration::from_secs(5), "the start waited for the job");
}

async fn jobs(image: &str, m: &Machine) {
    let p = DockerProvider::new(image.to_owned(), "c2j");
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2j_a").await.unwrap();
    let mine = p.run(m, &a, "id -u", 20_000).await.unwrap().stdout;
    assert_ne!(mine.trim(), "0", "commands aren't root");

    start_job(&p, m, &a, "j_one", "id -u > who.txt; sleep 8; echo done > out.txt; exit 7").await;
    assert_eq!(seen(&p, m, &a, "j_one").await, Some(Seen::Running), "gone with the command that started it");

    // a new provider is a new broker: what it finds is the job, still going
    let next = DockerProvider::new(image.to_owned(), "c2j");
    next.guard(m).await.unwrap();
    assert_eq!(seen(&next, m, &a, "j_one").await, Some(Seen::Running), "gone with the broker");
    for _ in 0..60 {
        if seen(&next, m, &a, "j_one").await != Some(Seen::Running) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert_eq!(seen(&next, m, &a, "j_one").await, Some(Seen::Exited(7)));
    let file = |name: &str| format!("{}/{name}", a.folder);
    assert_eq!(String::from_utf8_lossy(&next.read_file(m, &next.resolve(m, &file("out.txt")).await.unwrap()).await.unwrap()).trim(), "done");
    assert_eq!(String::from_utf8_lossy(&next.read_file(m, &next.resolve(m, &file("who.txt")).await.unwrap()).await.unwrap()).trim(), mine.trim(), "as the Bot's user");

    // a sleep freezes it: it writes the time every 200 ms, so the freeze is a gap in what it wrote,
    // and it goes on after. A timer in it would run out while frozen, so that wouldn't show it
    start_job(&next, m, &a, "j_two", "sleep 300 & while :; do date +%s >> ticks.txt; sleep 0.2; done").await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    next.sleep(m, Sleep::Light).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    next.wake(m).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(seen(&next, m, &a, "j_two").await, Some(Seen::Running));
    let ticks = next.run(m, &a, "cat ticks.txt", 20_000).await.unwrap().stdout;
    let times: Vec<u64> = ticks.lines().map(|t| t.parse().unwrap()).collect();
    let gap = times.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(gap >= 4, "it ran on while asleep: the longest gap was {gap}s");
    assert!(times.last().unwrap() > &(times[0] + 5), "it didn't go on after the wake: {ticks}");

    // a Bot left while it slept: the words its stop owed reach the job after a wake, by the folder
    // alone, and a start nobody knows the end of comes to nothing even if it runs after them
    start_job(&next, m, &a, "j_left", "sleep 300").await;
    next.sleep(m, Sleep::Light).await.unwrap();
    next.wake(m).await.unwrap();
    assert_eq!(seen(&next, m, &a, "j_left").await, Some(Seen::Running), "it came back with the wake");
    let left = Screen { display: String::new(), viewer_port: 0, folder: a.folder.clone(), profile: None };
    assert_eq!(next.run(m, &left, &job::cancel("j_left"), 20_000).await.unwrap().exit_code, Some(0));
    assert_eq!(seen(&next, m, &a, "j_left").await, Some(Seen::Gone));
    assert_eq!(next.run(m, &left, &job::cancel("j_late"), 20_000).await.unwrap().exit_code, Some(0));
    start_job(&next, m, &a, "j_late", "echo ran > late.txt").await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(next.run(m, &a, "ls late.txt 2>/dev/null", 20_000).await.unwrap().stdout.trim(), "", "it ran after its cancel");
    assert_eq!(seen(&next, m, &a, "j_late").await, Some(Seen::Gone));

    // what a job leaves in the background is the job: `run.sh` has gone with its code, and the
    // group is what a look, the limit and a cancel go by
    start_job(&next, m, &a, "j_bg", "sleep 600 & echo started").await;
    for _ in 0..40 {
        if !next.run(m, &a, "cat .jobs/j_bg/exit 2>/dev/null", 20_000).await.unwrap().stdout.trim().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(next.run(m, &a, "cat .jobs/j_bg/exit", 20_000).await.unwrap().stdout.trim(), "0");
    assert_eq!(seen(&next, m, &a, "j_bg").await, Some(Seen::Running), "its child runs on");
    assert_eq!(next.run(m, &a, &job::cancel("j_bg"), 20_000).await.unwrap().exit_code, Some(0));
    assert_eq!(next.run(m, &a, "ps -eo args | grep -c '[s]leep 600'", 20_000).await.unwrap().stdout.trim(), "0", "the cancel missed it");
    assert_eq!(seen(&next, m, &a, "j_bg").await, Some(Seen::Exited(0)));

    // TERM ends the leader at once, and a child still gets its grace
    start_job(&next, m, &a, "j_grace", "sh -c 'trap \"sleep 1; echo graceful > g.txt; exit 0\" TERM; while :; do sleep 0.1; done' & wait").await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(next.run(m, &a, &job::stop("j_grace"), 20_000).await.unwrap().exit_code, Some(0));
    assert_eq!(next.run(m, &a, "cat g.txt", 20_000).await.unwrap().stdout.trim(), "graceful", "killed inside its grace");

    // after a restart its number isn't its own: a stale `boot` leaves that group alone
    start_job(&next, m, &a, "j_boot", "sleep 300").await;
    next.run(m, &a, "echo old > .jobs/j_boot/boot", 20_000).await.unwrap();
    assert_eq!(seen(&next, m, &a, "j_boot").await, Some(Seen::Gone));
    next.run(m, &a, "rm .jobs/j_boot/boot", 20_000).await.unwrap();
    assert_eq!(next.run(m, &a, &job::stop("j_boot"), 20_000).await.unwrap().exit_code, Some(0));

    // a deep sleep is a stop: a frozen job doesn't come back from it, and what's left says nothing
    // of how it ended, so that's the broker's to say at the sleep
    start_job(&next, m, &a, "j_deep", "sleep 300").await;
    next.sleep(m, Sleep::Light).await.unwrap();
    next.sleep(m, Sleep::Deep).await.unwrap();
    assert_eq!(next.power(m).await.unwrap(), Power::Asleep);
    next.wake(m).await.unwrap();
    let a = next.screen_up(m, "c2j_a").await.unwrap();
    assert_eq!(seen(&next, m, &a, "j_deep").await, Some(Seen::Gone), "it came back from a stop");

    // a stop takes what it started, and leaves no code
    next.run(m, &a, &job::stop("j_two"), 20_000).await.unwrap();
    assert_eq!(seen(&next, m, &a, "j_two").await, Some(Seen::Gone));
    let left = next.run(m, &a, "ps -eo args | grep -c '[s]leep 300'", 20_000).await.unwrap().stdout;
    assert_eq!(left.trim(), "0", "what it started went with it");
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
    // what could be left on a desktop's home, by something run there: it runs at the next start, and
    // the wall is already up, because the box's network is older than the box. The Mac's loopback
    // listens here.
    let plant = format!(
        r#"mkdir -p ~/.config/autostart; printf '%s\n' {autostart} > ~/.config/autostart/c2probe.desktop; printf '%s\n' {script} > ~/c2probe.py"#,
        autostart = "'[Desktop Entry]' 'Type=Application' 'Name=c2probe' 'Exec=python3 /home/box/desktops/c2t_a/c2probe.py'",
        script = shell_lines(&format!("import socket,time\nout=open('/home/box/desktops/c2t_a/c2probe.log','a')\nout.write('ran\\n')\nfor host,port in [('host.docker.internal',{port}),('192.168.65.254',{port}),('gateway.docker.internal',{port}),('example.com',443)]:\n    s=socket.socket();s.settimeout(3)\n    try:\n        s.connect((socket.gethostbyname(host),port));out.write(host+' REACHED\\n')\n    except Exception:\n        out.write(host+' blocked\\n')\nout.write('done\\n')\nout.close()\n")),
    );
    on_desktop(m, "c2t_a", &a, &plant);
    p.sleep(m, Sleep::Deep).await.unwrap();
    assert_eq!(p.power(m).await.unwrap(), Power::Asleep);
    p.wake(m).await.unwrap();
    let up = p.screen_up(m, "c2t_a").await.unwrap();
    assert_eq!(up.display, a.display, "a Bot keeps its screen's number");
    let mut log = String::new();
    for _ in 0..60 {
        log = on_desktop(m, "c2t_a", &a, "cat ~/c2probe.log 2>/dev/null");
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

/// `bot`'s two users, by the file only root writes.
fn users_of(m: &buwiz_computer::Machine, bot: &str) -> (u32, u32) {
    let index: u32 = docker(&["exec", "--user", "0", &name_of(m), "cat", &format!("/home/box/.box/bots/{bot}")]).parse().unwrap();
    (20000 + 2 * index, 20001 + 2 * index)
}

/// `script` as `bot`'s desktop user, with what that user has: the screen's cookie, its home and
/// the Bot's folder through the group. That's where a thing started by clicks runs.
fn on_desktop(m: &buwiz_computer::Machine, bot: &str, screen: &Screen, script: &str) -> String {
    let (agent, desktop) = users_of(m, bot);
    let n = screen.display.trim_start_matches(':');
    docker(&[
        "exec", "--user", "0", &name_of(m), "setpriv", &format!("--reuid={desktop}"), &format!("--regid={desktop}"), &format!("--groups={agent},1400"), "--inh-caps=-all",
        "env", "-i", "PATH=/usr/local/bin:/usr/bin:/bin", &format!("HOME=/home/box/desktops/{bot}"), &format!("DISPLAY=:{n}"), &format!("XAUTHORITY=/run/box/x/{n}/auth"), "sh", "-c", script,
    ])
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
    let a = p.create(&Spec { id: "cmp_c2ua".into(), dir: dirs[0].clone(), network: Default::default() }).await.unwrap();
    let b = p.create(&Spec { id: "cmp_c2ub".into(), dir: dirs[1].clone(), network: Default::default() }).await.unwrap();
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
    assert!(caps.contains("ALL") && !caps.contains("NET_ADMIN") && !caps.contains("NET_RAW"), "{caps}");

    // a peer on the same network, from any port and from an exempt one
    let peer = docker(&["inspect", &format!("{nb}-wall"), "--format", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}"]);
    assert!(!peer.is_empty());
    let out = p.run(a, &screen, &reach(&peer, 1337, 0), 10_000).await.unwrap();
    println!("to a peer: {}", out.stdout.trim());
    // refused here, by this box's own wall: a timeout would mean the packet left it
    assert!(out.stdout.contains("blocked OSError"), "{} {}", out.stdout, out.stderr);
    // and a command can't send from a port the wall lets through at all
    let out = p.run(a, &screen, &reach(&peer, 1337, 6905), 10_000).await.unwrap();
    assert!(out.stderr.contains("PermissionError") && !out.stdout.contains("REACHED"), "{} {}", out.stdout, out.stderr);

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
    let found = p.recover(&Spec { id: "cmp_c2ua".into(), dir: dir.clone(), network: Default::default() }, None).await.unwrap().expect("it's still there");
    assert_eq!(found.reach, a.reach);
    assert_ne!(found.token, a.token);
    // as a click does: what's running has the old secrets, so it stops first
    p.sleep(&found, Sleep::Deep).await.unwrap();
    p.wake(&found).await.unwrap();
    assert_eq!(p.read_file(&found, "kept.txt").await.unwrap(), b"both");

    // record and containers both lost: the files are found by name
    docker(&["rm", "-f", &na, &format!("{na}-wall")]);
    assert_eq!(p.recover(&Spec { id: "cmp_c2ua".into(), dir: dir.clone(), network: Default::default() }, None).await.unwrap(), None, "a label needs a container");
    let found = p.recover(&Spec { id: "cmp_c2ua".into(), dir, network: Default::default() }, Some(&a.reach)).await.unwrap().expect("its volumes are there");
    p.wake(&found).await.unwrap();
    assert_eq!(p.read_file(&found, "kept.txt").await.unwrap(), b"both", "the files were left behind");
    println!("record and containers lost: files kept");

    // only the home volume left: that's files, and a delete still finishes on real Docker's words
    docker(&["rm", "-f", &na, &format!("{na}-wall")]);
    docker(&["volume", "rm", &format!("{na}-workspace")]);
    let dir = a.dir.clone();
    let home_only = p.recover(&Spec { id: "cmp_c2ua".into(), dir, network: Default::default() }, Some(&a.reach)).await.unwrap();
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
    let _ = p;
    let out = docker(&["exec", "--user", "990", "-e", &format!("DISPLAY={}", screen.display), "-e", "XAUTHORITY=/run/box/x/svc.auth", &name_of(m), "sh", "-c", "xdotool getmouselocation --shell | head -2 | tr '\\n' ' '"]);
    assert!(out.contains("X="), "{out}");
    out
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
    let m = p.create(&Spec { id: "cmp_c2wtest".into(), dir: dir.clone(), network: Default::default() }).await.unwrap();
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
    assert!(gate.driving("c2w_a", quiet), "an open drive viewer counts as a person from the start");
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
const EARLY: &str = "import os,time\nout=open(os.path.expanduser('~/c2early.log'),'a')\nout.write('began %.3f\\n'%time.time());out.flush()\nend=time.time()+20\nhit=0\nseen=0\nn=0\nwhile time.time()<end:\n    n+=1\n    for p in ['/run/buwiz-computer/viewer','/run/buwiz-computer/token']:\n        try:\n            if os.path.exists(p):\n                seen+=1\n            d=open(p).read()\n            if d:\n                hit+=1\n                out.write('READ '+p+' '+str(len(d))+' after '+str(n)+'\\n');out.flush()\n        except Exception as e:\n            pass\n    time.sleep(0.002)\nout.write('done hits='+str(hit)+' seen='+str(seen)+' tries='+str(n)+'\\n');out.close()\n";

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
    let m = p.create(&Spec { id: "cmp_c2stest".into(), dir: dir.clone(), network: Default::default() }).await.unwrap();
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
    p.write_file(m, "agents/c2s_a/needles", needles.as_bytes()).await.unwrap();
    // the hunt must be able to find what's there: an ordinary process holds this, built at run time
    let holder = r#"setsid nohup python3 -c "x='c2s-control-'+'value-held-in-memory'.strip();import time;time.sleep(60)" >/dev/null 2>&1 &"#;
    assert_eq!(p.run(m, &a, holder, 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let quoted = HUNT.replace('\'', "'\\''");
    let out = p.run(m, &a, &format!("python3 -c '{quoted}' /workspace/agents/c2s_a/needles 6901 {}", a.viewer_port), 60_000).await.unwrap();
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

    // what a command leaves in its own home has nothing to start it
    let plant = "mkdir -p ~/.config/autostart; printf '%s\\n' '[Desktop Entry]' 'Type=Application' 'Name=c2own' 'Exec=touch /tmp/c2own' > ~/.config/autostart/c2own.desktop; printf 'touch /tmp/c2own\\n' | tee -a ~/.bashrc ~/.profile ~/.xsessionrc >/dev/null";
    assert_eq!(p.run(m, &a, plant, 10_000).await.unwrap().exit_code, Some(0));
    // planted to run at a start, in every place a desktop's start reads from the home that outlives it
    let plant = format!(
        r#"mkdir -p ~/.config/autostart; printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=c2early' 'Exec=python3 /home/box/desktops/c2s_a/c2early.py' > ~/.config/autostart/c2early.desktop; printf '%s\n' {script} > ~/c2early.py; printf 'python3 /home/box/desktops/c2s_a/c2early.py &\n' | tee -a ~/.bashrc ~/.profile ~/.xsessionrc >/dev/null"#,
        script = shell_lines(EARLY),
    );
    on_desktop(m, "c2s_a", &a, &plant);
    p.sleep(m, Sleep::Deep).await.unwrap();
    let began = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    p.wake(m).await.unwrap();
    let woke = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    println!("wake began {began:.3}, secrets gone by {woke:.3}");
    p.screen_up(m, "c2s_a").await.unwrap();
    let mut log = String::new();
    for _ in 0..60 {
        log = on_desktop(m, "c2s_a", &a, "cat ~/c2early.log 2>/dev/null");
        if log.contains("done") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    println!("planted at a start:\n{log}");
    assert!(log.contains("done"), "the planted reader never ran: {log}");
    assert!(!log.contains("READ"), "something planted read a staged secret at a start: {log}");
    assert_eq!(p.run(m, &a, "ls /tmp/c2own 2>/dev/null", 5000).await.unwrap().stdout, "", "a start ran what a command left");
}

/// A keylogger as a command could leave one: XInput2 raw key events on the root, over the X socket.
/// It offers `KL_COOKIE` when it has one, and says so when X turns it away.
const KEYLOGGER: &str = r#"
import os, socket, struct, sys, time
log = open(sys.argv[2], 'a', buffering=1)
s = socket.socket(socket.AF_UNIX); s.connect('/tmp/.X11-unix/X' + sys.argv[1].lstrip(':'))
cookie = bytes.fromhex(os.environ.get('KL_COOKIE', ''))
name = b'MIT-MAGIC-COOKIE-1' if cookie else b''
pad = lambda b: b + b'\0' * (-len(b) % 4)
s.sendall(struct.pack('<BxHHHHxx', 0x6c, 11, 0, len(name), len(cookie)) + pad(name) + pad(cookie))
def rd(n):
    b = b''
    while len(b) < n:
        c = s.recv(n - len(b))
        if not c:
            sys.exit(0)
        b += c
    return b
h = rd(8); d = rd(struct.unpack('<H', h[6:8])[0] * 4)
if h[0] != 1:
    log.write('refused\n')
    sys.exit(0)
root = struct.unpack('<I', d[32 + ((struct.unpack('<H', d[16:18])[0] + 3) & ~3) + 8 * d[21]:][:4])[0]
name = b'XInputExtension'; extra = (-len(name)) % 4
s.sendall(struct.pack('<BxHHxx', 98, 2 + (len(name) + extra) // 4, len(name)) + name + b'\0' * extra)
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

/// A screenshot loop, every way a command has: X's own tools on each screen, and every shared
/// memory segment in the box, the framebuffer of an Xvfb run with `-shmem` among them.
const PEEK: &str = r#"
import ctypes, os, subprocess, sys, time
libc = ctypes.CDLL(None, use_errno=True)
libc.shmat.restype = ctypes.c_void_p
libc.shmat.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
for round in range(3):
    for display in sys.argv[1:]:
        env = dict(os.environ, DISPLAY=display)
        shot = '/tmp/peek-%s.png' % os.getpid()
        got = subprocess.run(['import', '-window', 'root', shot], env=env, capture_output=True, timeout=10)
        if got.returncode == 0 and os.path.exists(shot) and os.path.getsize(shot):
            print('PICTURE', display)
        print('import', display, got.returncode)
    for line in open('/proc/sysvipc/shm').read().splitlines()[1:]:
        fields = line.split()
        at = libc.shmat(int(fields[1]), None, 0o10000)
        if at not in (None, ctypes.c_void_p(-1).value):
            data = ctypes.string_at(at, min(int(fields[3]), 1 << 20))
            print('SEGMENT', fields[1], 'mode', fields[2], 'bytes', sum(1 for b in data if b))
        else:
            print('segment', fields[1], 'denied')
    time.sleep(0.3)
"#;

/// From a command: everything it mustn't be able to do, each on one line, `DID ...` when it could.
const REACH_OUT: &str = r#"
import ctypes, os, signal, socket, sys
libc = ctypes.CDLL(None, use_errno=True)
PTRACE_SEIZE = 0x4206
me = os.getuid()
names = {'box-keeper', 'box-exec', 'box-host', 'voxl', 'Xvfb', 'xfce4-session', 'xfwm4', 'xfce4-panel', 'chromium', 'tini'}
seen = set()
for d in os.listdir('/proc'):
    if not d.isdigit():
        continue
    pid = int(d)
    try:
        comm = open('/proc/%d/comm' % pid).read().strip()
        owner = os.stat('/proc/%d' % pid).st_uid
    except OSError:
        continue
    if comm not in names:
        continue
    seen.add(comm)
    if owner == me:
        print('DID own', comm, pid)
    try:
        os.kill(pid, signal.SIGCONT)
        print('DID signal', comm, pid)
    except PermissionError:
        pass
    except ProcessLookupError:
        pass
    if libc.ptrace(PTRACE_SEIZE, pid, 0, 0) == 0:
        print('DID ptrace', comm, pid)
    for what in ('mem', 'environ', 'fd'):
        try:
            if what == 'fd':
                os.listdir('/proc/%d/fd' % pid)
            else:
                open('/proc/%d/%s' % (pid, what), 'rb').read(1)
            print('DID read', what, comm, pid)
        except OSError:
            pass
print('saw', ' '.join(sorted(seen)))
for path in sys.argv[1:]:
    try:
        if os.path.isdir(path):
            print('DID list', path, os.listdir(path)[:3])
        else:
            print('DID read', path, len(open(path, 'rb').read()))
    except OSError:
        pass
for port in [1337, 1340, 6901, 6902, 6903, 6904, 6905, 80, 3000]:
    s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        s.bind(('127.0.0.1', port)); s.listen(1); print('DID bind', port)
    except PermissionError:
        pass
    except OSError as e:
        print('DID bind-try', port, e)
    s.close()
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('127.0.0.1', 8123)); s.listen(1); print('own port ok')
try:
    open('/tmp/.X11-unix/X9', 'w'); print('DID plant a socket')
except OSError:
    pass
fields = dict(l.split(':', 1) for l in open('/proc/self/status').read().splitlines() if ':' in l)
print('uid', fields['Uid'].split()[0], 'caps', fields['CapEff'].strip(), fields['CapPrm'].strip(), fields['CapInh'].strip(), fields['CapAmb'].strip(), 'nnp', fields['NoNewPrivs'].strip())
print('env', sorted(k for k in os.environ if k.startswith(('BOX_', 'XAUTH', 'DISPLAY', 'VOXL', 'LD_'))))
"#;

/// A library that starts `HOOK_CMD` from inside whatever it's preloaded into.
const HOOK: &str = r#"
#include <stdlib.h>
#include <unistd.h>
__attribute__((constructor)) static void hook(void) {
    const char *cmd = getenv("HOOK_CMD");
    if (!cmd) return;
    unsetenv("HOOK_CMD");
    if (fork() == 0) {
        unsetenv("LD_PRELOAD");
        setsid();
        execl("/bin/sh", "sh", "-c", cmd, (char *)0);
        _exit(1);
    }
}
"#;

/// [`HOOK`], built for the box's machine by Docker, so the test needs no compiler of its own.
fn hook_library(dir: &std::path::Path) -> Vec<u8> {
    use std::io::Write;
    std::fs::write(dir.join("hook.c"), HOOK).unwrap();
    let recipe = "FROM rust:1.85-bookworm AS build\nCOPY hook.c /hook.c\nRUN gcc -shared -fPIC -o /hook.so /hook.c\nFROM scratch\nCOPY --from=build /hook.so /hook.so\n";
    let mut child = std::process::Command::new("docker").args(["build", "--quiet", "--output", &format!("type=local,dest={}", dir.join("out").display()), "-f", "-", &dir.display().to_string()]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(recipe.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success(), "the test's library wouldn't build");
    std::fs::read(dir.join("out/hook.so")).unwrap()
}

/// A person types: the key reaches the screen through the real gate, as a viewer sends it.
async fn type_key(ws: &mut TcpStream, code: u32) {
    ws.write_all(&command(&format!("k,{code},1"))).await.unwrap();
    ws.write_all(&command(&format!("k,{code},0"))).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
}

async fn with_computer<F, Fut>(prefix: &'static str, test: F)
where
    F: FnOnce(DockerProvider, buwiz_computer::Machine, PathBuf) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image.clone(), prefix);
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let m = p.create(&Spec { id: format!("cmp_{prefix}test"), dir: dir.clone(), network: Default::default() }).await.unwrap();
    let outcome = tokio::spawn({
        let (m, dir) = (m.clone(), dir.clone());
        async move { test(DockerProvider::new(image, prefix), m, dir).await }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", &format!("{prefix}-net")]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

/// The point of it all. A person types on a real screen through the real gate, with a keylogger, a
/// screenshot loop and an `LD_PRELOAD` into an allowed program, each started from `computer_run`.
/// Beside each, the same thing as the desktop's user, which has the cookie: it does see, so the
/// attack works and it's the wall that stops it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn what_a_command_starts_cant_see_a_person() {
    with_computer("c2p", |p, m, dir| async move { watched_person(&p, &m, &dir).await }).await;
}

async fn watched_person(p: &DockerProvider, m: &buwiz_computer::Machine, dir: &std::path::Path) {
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2p_a").await.unwrap();
    let b = p.screen_up(m, "c2p_b").await.unwrap();
    let hook = hook_library(dir);
    let folder = format!("/workspace/{}", a.folder);
    p.write_file(m, &format!("{folder}/kl.py"), KEYLOGGER.as_bytes()).await.unwrap();
    p.write_file(m, &format!("{folder}/peek.py"), PEEK.as_bytes()).await.unwrap();
    p.write_file(m, &format!("{folder}/hook.so"), &hook).await.unwrap();
    let read = async |file: &str| p.read_file(m, &format!("{folder}/{file}")).await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();

    // a browser on the screen: its window is shared memory, and what a person sees is in there
    p.open_url(m, &a, "http://127.0.0.1:1/").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let (_, desktop_a) = users_of(m, "c2p_a");
    let segments = docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", &format!("awk '$8 == {desktop_a} && $4 > 2000000' /proc/sysvipc/shm | wc -l")]);
    assert_ne!(segments, "0", "a browser window should be in shared memory, or the loop proves nothing");

    // from computer_run, on every screen, by every way in
    let displays = [":1".to_owned(), a.display.clone(), b.display.clone()];
    for (i, display) in displays.iter().enumerate() {
        let started = p.run(m, &a, &format!("setsid nohup python3 kl.py {display} keys-{i}.log >/dev/null 2>&1 &"), 5000).await.unwrap();
        assert_eq!(started.exit_code, Some(0), "{}", started.stderr);
    }
    let preload = format!("timeout 5 env HOOK_CMD='python3 {folder}/kl.py {} {folder}/keys-preload.log' LD_PRELOAD={folder}/hook.so /usr/bin/xfwm4 >/dev/null 2>&1; true", a.display);
    assert_eq!(p.run(m, &a, &preload, 10_000).await.unwrap().exit_code, Some(0));
    let peek = p.run(m, &a, &format!("python3 peek.py {}", displays.join(" ")), 60_000).await.unwrap();
    println!("the screenshot loop:\n{}\n{}", peek.stdout, peek.stderr);
    assert_eq!(peek.exit_code, Some(0), "{}", peek.stderr);

    // the same, as the desktop: it's what proves the rest
    let cookie = on_desktop(m, "c2p_a", &a, "xauth -i list | awk '{print $3}'");
    assert_eq!(cookie.len(), 32, "{cookie}");
    let quoted = KEYLOGGER.replace('\'', "'\\''");
    on_desktop(m, "c2p_a", &a, &format!("KL_COOKIE={cookie} setsid nohup python3 -c '{quoted}' {} $HOME/control.log >/dev/null 2>&1 &", a.display));
    on_desktop(m, "c2p_a", &a, &format!("cp {folder}/hook.so {folder}/kl.py $HOME/; KL_COOKIE={cookie} HOOK_CMD=\"python3 $HOME/kl.py {} $HOME/control-preload.log\" LD_PRELOAD=$HOME/hook.so timeout 5 /usr/bin/xfwm4 >/dev/null 2>&1; true", a.display));
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let gate = Gate::start().await.unwrap();
    let (mut ws, ..) = real_socket(&gate, "c2p_a", p.viewer(m, &a).await.unwrap().unwrap(), Mode::Drive).await;
    for code in [97, 98, 99] {
        type_key(&mut ws, code).await;
    }
    let control = on_desktop(m, "c2p_a", &a, "cat $HOME/control.log");
    println!("the desktop's keylogger: {control:?}");
    assert_eq!(control.matches("key ").count(), 3, "the keylogger must work, or this proves nothing: {control}");
    let through = on_desktop(m, "c2p_a", &a, "cat $HOME/control-preload.log");
    assert_eq!(through.matches("key ").count(), 3, "the library must work, or this proves nothing: {through}");
    let shot = on_desktop(m, "c2p_a", &a, "import -window root /tmp/ctl.png && test -s /tmp/ctl.png && echo PICTURE");
    assert!(shot.contains("PICTURE"), "the screenshot must work, or this proves nothing: {shot}");
    let (mut there, mut none) = (String::new(), 0);
    for i in 0..3 {
        let log = read(&format!("keys-{i}.log")).await;
        println!("{} from computer_run: {log:?}", displays[i]);
        none += usize::from(log.contains("refused"));
        there.push_str(&log);
    }
    let preloaded = read("keys-preload.log").await;
    println!("through LD_PRELOAD: {preloaded:?}");
    assert!(!there.contains("key ") && !preloaded.contains("key "), "a command saw a person's keys: {there} {preloaded}");
    assert!(!there.contains("ready") && !preloaded.contains("ready"), "a command got onto a screen: {there} {preloaded}");
    assert_eq!(none, 3, "each keylogger must have reached X and been turned away, or it tried nothing: {there}");
    assert!(preloaded.contains("refused"), "the library must have run: {preloaded}");
    assert!(!peek.stdout.contains("PICTURE"), "a command took a picture:\n{}", peek.stdout);
    assert!(!peek.stdout.contains("SEGMENT"), "a command read shared memory:\n{}", peek.stdout);
    assert!(peek.stdout.contains("import :1 "), "the loop must have run: {}", peek.stdout);
}

/// What a command can and can't touch, for two Bots on one computer, and what still works.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_commands_user_reaches_its_own_files_and_nothing_else() {
    with_computer("c2r", |p, m, _| async move { walled(&p, &m).await }).await;
}

async fn walled(p: &DockerProvider, m: &buwiz_computer::Machine) {
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2r_a").await.unwrap();
    let b = p.screen_up(m, "c2r_b").await.unwrap();
    p.open_url(m, &b, "http://127.0.0.1:1/").await.unwrap();
    p.open_url(m, &a, "http://127.0.0.1:1/").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let (na, nb) = (a.display.trim_start_matches(':'), b.display.trim_start_matches(':'));
    let targets = [
        "/run/box/x/svc.auth".to_owned(), format!("/run/box/x/{na}/auth"), format!("/run/box/x/{nb}/auth"), "/run/box/x/1/auth".to_owned(), "/run/box/keeper.sock".to_owned(),
        "/home/box/desktops/c2r_a".to_owned(), "/home/box/desktops/c2r_a/chrome-profile".to_owned(), "/home/box/desktops/c2r_b/chrome-profile".to_owned(), "/home/box/desktops/main".to_owned(), "/home/box/desktops/main/chrome-profile".to_owned(), "/run/box/x".to_owned(), "/run/box/log".to_owned(), "/run/box/log/voxl-1.log".to_owned(), format!("/run/box/x/{na}/run"), "/home/box/agents/c2r_b".to_owned(), "/workspace/agents/c2r_b".to_owned(), "/workspace/agents".to_owned(), "/home/box/agents".to_owned(),
    ];
    let quoted = REACH_OUT.replace('\'', "'\\''");
    let out = p.run(m, &a, &format!("python3 -c '{quoted}' {}", targets.join(" ")), 30_000).await.unwrap();
    println!("from a command:\n{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.exit_code, Some(0), "{}", out.stderr);
    let did: Vec<&str> = out.stdout.lines().filter(|l| l.starts_with("DID")).collect();
    assert!(did.is_empty(), "{did:#?}");
    let saw = out.stdout.lines().find(|l| l.starts_with("saw ")).unwrap();
    for name in ["box-exec", "box-host", "box-keeper", "chromium", "tini", "voxl", "Xvfb", "xfce4-session"] {
        assert!(saw.contains(name), "it never tried {name}: {saw}");
    }
    assert!(out.stdout.contains("own port ok"), "a port above the floor is the Bot's: {}", out.stdout);
    let (agent_a, desktop_a) = users_of(m, "c2r_a");
    let (agent_b, _) = users_of(m, "c2r_b");
    assert_ne!(agent_a, agent_b);
    let zero = "0000000000000000";
    assert!(out.stdout.contains(&format!("uid {agent_a} caps {zero} {zero} {zero} {zero} nnp 1")), "{}", out.stdout);
    assert!(out.stdout.contains("env []"), "{}", out.stdout);
    let id = p.run(m, &b, "id -u; id -G; pwd; echo $HOME", 5000).await.unwrap().stdout;
    assert_eq!(id.lines().collect::<Vec<_>>(), [agent_b.to_string().as_str(), agent_b.to_string().as_str(), "/workspace/agents/c2r_b", "/home/box/agents/c2r_b"], "{id}");

    // the agent's job: its files, by tool and by command, its home, the tools on its screen
    let file = "/workspace/agents/c2r_a/out/report.txt";
    p.write_file(m, file, b"from a tool").await.unwrap();
    let seen = p.run(m, &a, "cat out/report.txt; echo more >> out/report.txt; mkdir -p deep/er && echo x > deep/er/y; echo h > ~/h; cat ~/h; stat -c %u out/report.txt", 5000).await.unwrap();
    assert_eq!(seen.exit_code, Some(0), "{}", seen.stderr);
    assert!(seen.stdout.contains("from a tool") && seen.stdout.contains(&agent_a.to_string()), "{}", seen.stdout);
    assert_eq!(p.read_file(m, file).await.unwrap(), b"from a toolmore\n");
    let link = p.run(m, &a, "ln -s /workspace/agents/c2r_b/secret peek; ln -s /run/box/x/svc.auth svc; true", 5000).await.unwrap();
    assert_eq!(link.exit_code, Some(0));
    p.write_file(m, "/workspace/agents/c2r_b/secret", b"b's").await.unwrap();
    assert!(p.read_file(m, "/workspace/agents/c2r_a/peek").await.is_err(), "a link in a's folder reached b's file");
    assert!(p.read_file(m, "/workspace/agents/c2r_a/svc").await.is_err(), "a link reached the services' cookies");
    p.input(m, &a, &Input::Move { x: 77, y: 66 }).await.unwrap();
    assert!(pointer(p, m, &a).await.contains("X=77"), "the tools still reach the screen");
    assert!(p.screenshot(m, &a).await.unwrap().for_model().0.starts_with(&[0xFF, 0xD8]));
    let chromium = docker(&["exec", "--user", "0", &name_of(m), "pgrep", "-u", &desktop_a.to_string(), "-c", "chromium"]);
    assert_ne!(chromium, "0", "the Bot's browser runs as its desktop");
    assert_eq!(docker(&["exec", "--user", "0", &name_of(m), "pgrep", "-u", &agent_a.to_string(), "-c", "chromium"]), "0");

    // a recording and a download land in the Bot's folder, where it can read them
    let id = p.record_start(m, &a).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let path = p.record_stop(m, &a, id).await.unwrap();
    println!("recording at {path}");
    assert!(path.starts_with("agents/c2r_a/recordings/"), "{path}");
    let size = p.run(m, &a, &format!("stat -c '%u %s' /workspace/{path}"), 5000).await.unwrap().stdout;
    assert!(size.starts_with(&agent_a.to_string()) && !size.trim().ends_with(" 0"), "{size}");
    let serve = "import http.server as h\nclass H(h.BaseHTTPRequestHandler):\n    def do_GET(s):\n        got=s.path.endswith('got.txt')\n        s.send_response(200);s.send_header('Content-Length','4')\n        s.send_header('Content-Disposition','attachment; filename=got.txt') if got else s.send_header('Content-Type','text/html')\n        s.end_headers();s.wfile.write(b'got\\n')\nh.HTTPServer(('127.0.0.1',8124),H).serve_forever()\n";
    p.write_file(m, "/workspace/agents/c2r_a/srv.py", serve.as_bytes()).await.unwrap();
    assert_eq!(p.run(m, &a, "setsid nohup python3 srv.py >srv.log 2>&1 &", 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    p.open_url(m, &a, "http://127.0.0.1:8124/got.txt").await.unwrap();
    let mut got = String::new();
    for _ in 0..60 {
        got = p.run(m, &a, "cat got.txt 2>&1", 5000).await.unwrap().stdout;
        if got.trim() == "got" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert_eq!(got.trim(), "got", "a download the Bot can't read: {}", docker(&["exec", "--user", "0", &name_of(m), "ls", "-la", "/workspace/agents/c2r_a"]));

    // each screen's address comes from its own browser, through a recipe's own receipt
    let serve_b = p.open_url(m, &b, "http://127.0.0.1:8124/for-b").await;
    serve_b.unwrap();
    p.open_url(m, &a, "http://127.0.0.1:8124/for-a").await.unwrap();
    let receipt = |screen: &Screen| {
        docker(&["exec", "--user", "0", "-e", &format!("T={}", m.token.expose()), &name_of(m), "sh", "-c", &format!("curl -s -m 20 -H \"Authorization: Bearer $T\" -H 'content-type: application/json' -d '{{\"observe\":\"page\",\"steps\":[{{\"op\":\"click\",\"x\":640,\"y\":400}}]}}' 'http://127.0.0.1:1337/v1/cua/recipe?display=%3A{}'", screen.display.trim_start_matches(':'))])
    };
    let mut pages = (String::new(), String::new());
    for _ in 0..40 {
        pages = (receipt(&a), receipt(&b));
        if pages.0.contains("/for-a") && pages.1.contains("/for-b") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let urls = |receipt: &str| receipt.split(",\"screenshot\"").next().unwrap_or_default().to_owned();
    assert!(pages.0.contains("/for-a") && !urls(&pages.0).contains("/for-b"), "{}", urls(&pages.0));
    assert!(pages.1.contains("/for-b") && !urls(&pages.1).contains("/for-a"), "{}", urls(&pages.1));

    // a screen's report is only read as what it is: a link, a fifo or another's file says nothing
    let tab = |screen: &Screen| docker(&["exec", "--user", "0", "-e", &format!("T={}", m.token.expose()), &name_of(m), "sh", "-c", &format!("curl -s -m 5 -H \"Authorization: Bearer $T\" 'http://127.0.0.1:1340/v1/chrome/active-tab?display=%3A{}'", screen.display.trim_start_matches(':'))]);
    assert!(tab(&a).contains("/for-a"), "{}", tab(&a));
    let report = format!("/run/box/x/{na}/run/active-tab.json");
    docker(&["exec", "--user", "0", &name_of(m), "cp", &report, "/tmp/kept"]);
    let plant = |how: &str| docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", &format!("unlink {report}; {how}")]);
    plant(&format!("printf '{{\"url\":\"http://forged/\"}}' > /tmp/forged; chmod a+r /tmp/forged; ln -s /tmp/forged {report}"));
    assert!(tab(&a).contains("\"url\":null"), "a link was followed: {}", tab(&a));
    plant(&format!("printf '{{\"url\":\"http://forged/\"}}' > {report}"));
    assert!(tab(&a).contains("\"url\":null"), "another user's file was believed: {}", tab(&a));
    plant(&format!("mkfifo {report}"));
    assert!(tab(&a).contains("\"url\":null"), "a fifo: {}", tab(&a));
    docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", &format!("unlink {report}; cp /tmp/kept {report}; chown {desktop_a}:{desktop_a} {report}")]);
    assert!(tab(&a).contains("/for-a"), "{}", tab(&a));

    // nothing about X's folders is readable, and nothing was for a moment: nobody but root writes them
    let modes = docker(&["exec", "--user", "0", &name_of(m), "stat", "-c", "%a %U %G", "/run/box/x", "/run/box/x/svc.auth", &format!("/run/box/x/{na}"), &format!("/run/box/x/{na}/auth"), "/run/box/log"]);
    let desktop_name = format!("d{desktop_a}");
    assert_eq!(modes.lines().collect::<Vec<_>>(), ["711 root root", "640 root boxsvc", "711 root root", format!("640 root {desktop_name}").as_str(), "700 root root"], "{modes}");

    // the check from outside: yes while it's so, and no the moment it isn't
    let bots = ["c2r_a".to_owned(), "c2r_b".to_owned()];
    assert!(p.users_apart(m, &bots).await.unwrap(), "the check from outside said no");
    docker(&["exec", "--user", "0", &name_of(m), "chmod", "o+rx", "/workspace/agents/c2r_b"]);
    assert!(!p.users_apart(m, &bots).await.unwrap(), "an open folder passed");
    docker(&["exec", "--user", "0", &name_of(m), "chmod", "o-rx", "/workspace/agents/c2r_b"]);
    assert!(p.users_apart(m, &bots).await.unwrap());
    docker(&["exec", "-d", "--user", "0", &name_of(m), "setpriv", "--reuid=1000", "--regid=1000", "--groups=1400", "--inh-caps=-all", "Xvfb", ":70", "-ac", "-screen", "0", "64x64x24"]);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(!p.users_apart(m, &bots).await.unwrap(), "an open X server passed");
    docker(&["exec", "--user", "0", &name_of(m), "pkill", "-f", "Xvfb :70"]);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!p.users_apart(m, &["nobody".to_owned()]).await.unwrap_or(false), "a Bot with no user passed");
    // a cookie shared by group or by its folder, whichever way it's done
    let cookie = format!("/run/box/x/{nb}/auth");
    let as_root = |args: &[&str]| {
        let mut all = vec!["exec", "--user", "0", &*Box::leak(name_of(m).into_boxed_str())];
        all.extend_from_slice(args);
        docker(&all)
    };
    let group = as_root(&["stat", "-c", "%g", &cookie]);
    as_root(&["chgrp", "1400", &cookie]);
    assert!(!p.users_apart(m, &bots).await.unwrap(), "a cookie shared with every desktop passed");
    as_root(&["chgrp", &group, &cookie]);
    as_root(&["chmod", "0660", &cookie]);
    assert!(!p.users_apart(m, &bots).await.unwrap(), "a cookie its group can write passed");
    as_root(&["chmod", "0640", &cookie]);
    let folder = format!("/run/box/x/{nb}");
    as_root(&["chmod", "0771", &folder]);
    assert!(!p.users_apart(m, &bots).await.unwrap(), "a cookie's open folder passed");
    as_root(&["chmod", "0711", &folder]);
    assert!(p.users_apart(m, &bots).await.unwrap());
    // a copy of Xvfb with its door open: no path says it's X, the kernel's list of listeners does
    as_root(&["sh", "-c", "cp /usr/bin/Xvfb /tmp/not-x && setpriv --reuid=1000 --regid=1000 --groups=1400 --inh-caps=-all setsid nohup /tmp/not-x :71 -ac -screen 0 64x64x24 >/dev/null 2>&1 &"]);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(!p.users_apart(m, &bots).await.unwrap(), "a copied X server with -ac passed");
    as_root(&["pkill", "-f", "not-x :71"]);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    as_root(&["sh", "-c", "unlink /tmp/.X11-unix/X71; unlink /tmp/.X71-lock; true"]);
    assert!(p.users_apart(m, &bots).await.unwrap());
    // a real server told by its own desktop to take anybody: `xhost +`, which no argument shows
    let open_door = "import os,socket,struct,subprocess\nline=subprocess.check_output(['xauth','-i','list']).split()[2].decode()\ncookie=bytes.fromhex(line)\nname=b'MIT-MAGIC-COOKIE-1'\npad=lambda b:b+b'\\0'*(-len(b)%4)\ns=socket.socket(socket.AF_UNIX);s.connect('/tmp/.X11-unix/X'+os.environ['DISPLAY'].lstrip(':'))\ns.sendall(struct.pack('<BxHHHHxx',0x6c,11,0,len(name),len(cookie))+pad(name)+pad(cookie))\nh=s.recv(8);s.recv(struct.unpack('<H',h[6:8])[0]*4)\ns.sendall(struct.pack('<BBH',111,MODE,1))\ns.sendall(struct.pack('<BxHI',43,1,0)[:4])\ns.recv(32)\n";
    let as_quoted = |mode: u8| open_door.replace("MODE", &mode.to_string()).replace('\'', "'\\''");
    on_desktop(m, "c2r_a", &a, &format!("python3 -c '{}'", as_quoted(0)));
    assert!(!p.users_apart(m, &bots).await.unwrap(), "an X server open to anybody passed");
    on_desktop(m, "c2r_a", &a, &format!("python3 -c '{}'", as_quoted(1)));
    assert!(p.users_apart(m, &bots).await.unwrap(), "the door shut again");

    // a screen that's down: the name is anybody's, and nothing of the box's connects to it
    let bait = "import socket,time\ns=socket.socket(socket.AF_UNIX);s.bind('\\0/tmp/.X11-unix/X5');s.listen(5);s.settimeout(6)\ntry:\n    c,_=s.accept();print('HEARD',len(c.recv(256)))\nexcept socket.timeout:\n    print('heard nothing')\n";
    p.write_file(m, "/workspace/agents/c2r_a/bait.py", bait.as_bytes()).await.unwrap();
    assert_eq!(p.run(m, &a, "setsid nohup python3 bait.py > bait.log 2>&1 &", 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let down = Screen { display: ":5".into(), viewer_port: 6905, folder: "agents/c2r_a".into(), profile: None };
    let tried = p.screenshot(m, &down).await;
    println!("a screen that's down: {:?}", tried.as_ref().map(|_| "a picture"));
    assert!(tried.is_err(), "it took a picture of something");
    assert!(p.input(m, &down, &Input::Move { x: 1, y: 1 }).await.is_err());
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    let heard = p.run(m, &a, "cat bait.log", 5000).await.unwrap().stdout;
    assert!(heard.contains("heard nothing"), "a service connected to a squatter: {heard}");

    // a process holding a screen's name before it's up gets no screen, and no cookie
    let squat = "import socket,time\ns=socket.socket(socket.AF_UNIX);s.bind('\\0/tmp/.X11-unix/X4');s.listen(5);s.settimeout(8)\ntry:\n    c,_=s.accept();print('HEARD',len(c.recv(256)))\nexcept socket.timeout:\n    print('heard nothing')\n";
    p.write_file(m, "/workspace/agents/c2r_a/squat.py", squat.as_bytes()).await.unwrap();
    assert_eq!(p.run(m, &a, "setsid nohup python3 squat.py > squat.log 2>&1 &", 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let refused = p.screen_up(m, "c2r_c").await;
    println!("with its name taken: {refused:?}");
    assert!(refused.is_err(), "a screen came up behind a process holding its name");
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    let heard = p.run(m, &a, "cat squat.log", 5000).await.unwrap().stdout;
    assert!(heard.contains("heard nothing"), "{heard}");
    let c = p.screen_up(m, "c2r_c").await.expect("the name's free now");
    assert_eq!(c.display, ":4");

    // a Bot that left: its screen and commands go, and its numbers stay its own. A command in the
    // middle of something gets to finish it, and one that won't is ended all the same
    let tidy = "setsid nohup sh -c 'trap \"echo finished > /workspace/agents/c2r_c/term.log; exit 0\" TERM; while :; do sleep 0.1; done' >/dev/null 2>&1 & setsid nohup sh -c 'trap \"\" TERM; while :; do sleep 0.1; done' >/dev/null 2>&1 &";
    assert_eq!(p.run(m, &c, tidy, 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (agent_c, _) = users_of(m, "c2r_c");
    p.screen_down(m, "c2r_c").await.unwrap();
    assert_eq!(docker(&["exec", "--user", "0", &name_of(m), "pgrep", "-c", "-u", &agent_c.to_string()]), "0", "its commands went on");
    assert_eq!(docker(&["exec", "--user", "0", &name_of(m), "cat", "/workspace/agents/c2r_c/term.log"]), "finished", "a command was killed mid-write");
    assert_eq!(docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", "xauth -f /run/box/x/svc.auth list | grep -c ':4 '"]), "0", "the services still hold a stopped screen's cookie");
    let d = p.screen_up(m, "c2r_d").await.unwrap();
    assert_eq!(d.display, ":4", "the slot is free for another");
    assert_ne!(users_of(m, "c2r_d").0, agent_c, "a new Bot got an old Bot's user, and so its files");
}

/// What a hand-over clears: one Bot's desktop, of what isn't the desktop, wherever it hides. Commands
/// and the other Bot's desktop aren't its business.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn clearing_ends_what_was_left_on_a_desktop_and_nothing_else() {
    with_computer("c2k", |p, m, dir| async move { clear(&p, &m, &dir).await }).await;
}

async fn clear(p: &DockerProvider, m: &buwiz_computer::Machine, dir: &std::path::Path) {
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2k_a").await.unwrap();
    let b = p.screen_up(m, "c2k_b").await.unwrap();
    let (_, desktop_a) = users_of(m, "c2k_a");
    // something planted to start with the desktop, under its session: spared by nothing
    let plant = "mkdir -p ~/.config/autostart; printf '%s\\n' '[Desktop Entry]' 'Type=Application' 'Name=c2plant' 'Exec=python3 -c \"import time;time.sleep(600)\"' > ~/.config/autostart/c2plant.desktop";
    on_desktop(m, "c2k_a", &a, plant);
    p.sleep(m, Sleep::Deep).await.unwrap();
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2k_a").await.unwrap();
    let b_again = p.screen_up(m, "c2k_b").await.unwrap();
    assert_eq!(b_again.display, b.display);
    on_desktop(m, "c2k_a", &a, "rm ~/.config/autostart/c2plant.desktop");
    p.open_url(m, &a, "http://127.0.0.1:1/").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    // the open browser, as box-screen started it: box-chromium's loop, its launcher and Chromium
    let chain = format!("pgrep -u {desktop_a} -f '^/bin/sh /usr/local/bin/box-chromium '; pgrep -u {desktop_a} -x box-chrome-pipe; pgrep -u {desktop_a} -o -x chromium");
    let browser: Vec<String> = docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", &chain]).lines().map(str::to_owned).collect();
    println!("the open browser's loop, launcher and Chromium: {browser:?}");
    assert_eq!(browser.len(), 3, "the browser isn't one loop, one launcher and one Chromium, so this proves nothing: {browser:?}");

    // what was left: a keylogger, a forking loop, a copied interpreter, an allowed program with a
    // library of its own, a name that isn't text, an undumpable process, and a leader that exited
    let hook = hook_library(dir);
    p.write_file(m, "/workspace/agents/c2k_a/hook.so", &hook).await.unwrap();
    let quoted = KEYLOGGER.replace('\'', "'\\''");
    let cookie = on_desktop(m, "c2k_a", &a, "xauth -i list | awk '{print $3}'");
    on_desktop(m, "c2k_a", &a, &format!("KL_COOKIE={cookie} setsid nohup python3 -c '{quoted}' {} $HOME/keys.log >/dev/null 2>&1 &", a.display));
    on_desktop(m, "c2k_a", &a, "setsid nohup sh -c 'while true; do sleep 0.05 & done' >/dev/null 2>&1 & cp /usr/bin/python3 $HOME/innocent; setsid nohup $HOME/innocent -c 'import time;time.sleep(600)' >/dev/null 2>&1 &");
    on_desktop(m, "c2k_a", &a, "cp /workspace/agents/c2k_a/hook.so $HOME/hook.so; LD_PRELOAD=$HOME/hook.so setsid nohup /usr/bin/Xvfb :77 -screen 0 64x64x24 >/dev/null 2>&1 &");
    let hiding = "import ctypes,os,sys,threading,time\nlibc=ctypes.CDLL(None)\nif sys.argv[1]=='name':\n    libc.prctl(15,b'\\xff\\xfe\\x80bad',0,0,0)\nelif sys.argv[1]=='undumpable':\n    libc.prctl(4,0,0,0,0)\nopen(os.path.expanduser('~/'+sys.argv[1]+'.pid'),'w').write(str(os.getpid()))\nif sys.argv[1]=='zombie':\n    threading.Thread(target=time.sleep,args=(600,)).start()\n    time.sleep(0.2)\n    libc.pthread_exit(None)\ntime.sleep(600)\n";
    let quoted_hiding = hiding.replace('\'', "'\\''");
    for how in ["name", "undumpable", "zombie"] {
        on_desktop(m, "c2k_a", &a, &format!("setsid nohup python3 -c '{quoted_hiding}' {how} >/dev/null 2>&1 &"));
    }
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let zombie = on_desktop(m, "c2k_a", &a, "cat ~/zombie.pid");
    let state = docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", &format!("awk '{{print $3}}' /proc/{zombie}/stat; ls /proc/{zombie}/task | wc -l")]);
    println!("the exited leader: {state:?}");
    assert!(state.starts_with("Z\n2"), "it isn't a zombie leader with a live thread, so this proves nothing: {state}");
    // non-dumpable and root's in /proc, or the Uid line: what's true decides the comment
    let undumpable = on_desktop(m, "c2k_a", &a, "cat ~/undumpable.pid");
    let owners = docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", &format!("stat -c '%u %n' /proc/{undumpable} /proc/{undumpable}/status /proc/{undumpable}/mem; grep ^Uid /proc/{undumpable}/status")]);
    println!("what /proc says of a non-dumpable process:\n{owners}");

    // beside it: a command, and the other Bot's own stray
    assert_eq!(p.run(m, &a, "setsid nohup python3 -c 'import time;time.sleep(600)' >/dev/null 2>&1 &", 5000).await.unwrap().exit_code, Some(0));
    on_desktop(m, "c2k_b", &b, "setsid nohup python3 -c 'import time;time.sleep(601)' >/dev/null 2>&1 &");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let gate = Gate::start().await.unwrap();
    let (mut ws, ..) = real_socket(&gate, "c2k_a", p.viewer(m, &a).await.unwrap().unwrap(), Mode::Drive).await;
    type_key(&mut ws, 97).await;
    let keys = || on_desktop(m, "c2k_a", &a, "cat ~/keys.log").matches("key ").count();
    assert_eq!(keys(), 1, "the keylogger must work, or this proves nothing");

    // a process that can't be read isn't one that's fine: this one's status read fails, as if the
    // kernel said no, with the rest of the scan as it is
    on_desktop(m, "c2k_a", &a, "setsid nohup python3 -c 'import os,time;open(os.path.expanduser(\"~/unread.pid\"),\"w\").write(str(os.getpid()));time.sleep(599)' >/dev/null 2>&1 &");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let unread = on_desktop(m, "c2k_a", &a, "cat ~/unread.pid");
    let wrap = "import builtins,sys\nsource=sys.argv.pop();pid=sys.argv.pop()\nreal=builtins.open\ndef refusing(path,*a,**k):\n    if path=='/proc/%s/status'%pid:\n        raise PermissionError(13,'no')\n    return real(path,*a,**k)\nbuiltins.open=refusing\nexec(source)\n";
    let scanned = docker(&["exec", "--user", "0", "--privileged", &name_of(m), "/usr/bin/python3", "-I", "-S", "-c", wrap, "c2k_a", &unread, buwiz_computer::docker::CLEAR]);
    println!("with one unreadable: {scanned}");
    assert!(scanned.contains("unreadable"), "a process that can't be read was left: {scanned}");
    assert_eq!(on_desktop(m, "c2k_a", &a, &format!("test -d /proc/{unread} && echo alive; true")), "", "it's still there");

    let ended: Vec<String> = serde_json::from_str(&scanned).unwrap();
    assert!(!ended.iter().any(|n| n == "box-active-tab"), "the browser's own helper went: {ended:?}");
    assert!(!ended.iter().any(|n| n == "box-chromium" || n == "box-chrome-pipe" || n == "chromium"), "the open browser's chain went: {ended:?}");
    assert!(ended.iter().any(|n| n == "Xvfb"), "the allowed program with a library was spared: {ended:?}");
    let after = docker(&["exec", "--user", "0", &name_of(m), "ps", "-eo", "pid,uid,stat,comm,args", "--forest"]);
    println!("after:\n{after}");
    let mine: String = after.lines().filter(|l| l.split_whitespace().nth(1) == Some(&desktop_a.to_string())).collect::<Vec<_>>().join("\n");
    for word in ["KL_COOKIE", "innocent", "time.sleep(600)", "sleep 0.05", "while true", "Xvfb :77"] {
        assert!(!mine.contains(word), "{word} survived:\n{mine}");
    }
    let gone = format!("for f in name undumpable zombie; do test -d /proc/$(cat ~/$f.pid) && echo $f survived; done; true");
    assert_eq!(on_desktop(m, "c2k_a", &a, &gone), "", "hidden from the scan");
    for word in ["box-keeper", "box-exec", "box-host", "--port=6901", "--port=6902", "xfce4-session", "xfwm4", "xfce4-panel", "/usr/lib/chromium/chromium", "Xvfb :1", "Xvfb :2"] {
        assert!(after.contains(word), "{word} went:\n{after}");
    }
    assert!(after.contains("time.sleep(601)"), "the other Bot's work went:\n{after}");
    assert!(after.contains("time.sleep(600)"), "a command went, and it couldn't see anything:\n{after}");

    // the person types: nothing hears it, then or later; the desktop is still theirs
    type_key(&mut ws, 98).await;
    ws.write_all(&command("m,222,111")).await.unwrap();
    assert!(pointer(p, m, &a).await.contains("X=222"), "the person's input stopped reaching the desktop");
    assert_eq!(keys(), 1, "a key was logged after the clearing");
    assert!(p.screenshot(m, &a).await.is_ok());
    assert_eq!(p.clear_strays(m, &a).await.unwrap(), Vec::<String>::new(), "a second pass finds nothing: the desktop isn't a stray");
    assert_ne!(docker(&["exec", "--user", "0", &name_of(m), "pgrep", "-c", "-u", &desktop_a.to_string(), "chromium"]), "0", "the browser went");
    // the same browser, not one started again: every part of it lived through both clearings
    for pid in &browser {
        assert_eq!(docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", &format!("test -d /proc/{pid} && echo alive; true")]), "alive", "{pid} of {browser:?} went:\n{after}");
    }
}

/// The main screen goes, and a command takes its name at once. A call that names no display must not
/// offer that name a cookie. The box stops itself about a second later: this is that second.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_call_for_the_main_screen_gives_no_cookie_to_who_took_its_name() {
    with_computer("c2d", |p, m, _| async move { main_name_taken(&p, &m).await }).await;
}

async fn main_name_taken(p: &DockerProvider, m: &buwiz_computer::Machine) {
    p.wake(m).await.unwrap();
    let a = p.screen_up(m, "c2d_a").await.unwrap();
    let curl = format!("curl -s -m 2 -o /dev/null -w '%{{http_code}}\\n' -X POST -H \"Authorization: Bearer $T\" 'http://127.0.0.1:1337/v1/cua/screenshot?format=png'");
    let ask = |script: &str| docker(&["exec", "--user", "0", "-e", &format!("T={}", m.token.expose()), &name_of(m), "sh", "-c", script]);
    assert_eq!(ask(&curl), "200", "the default display works while it's real");
    let squat = "import socket,time\nend=time.time()+20\ns=socket.socket(socket.AF_UNIX)\nwhile time.time()<end:\n    try:\n        s.bind('\\0/tmp/.X11-unix/X1');break\n    except OSError:\n        time.sleep(0.001)\ns.listen(5);s.settimeout(4)\nf=open('/workspace/agents/c2d_a/squat.log','w',buffering=1);f.write('holding\\n')\ntry:\n    while True:\n        c,_=s.accept();c.settimeout(1)\n        f.write('HEARD %d\\n'%len(c.recv(256)))\nexcept socket.timeout:\n    f.write('done\\n')\n";
    p.write_file(m, "/workspace/agents/c2d_a/squat.py", squat.as_bytes()).await.unwrap();
    assert_eq!(p.run(m, &a, "setsid nohup python3 squat.py >/dev/null 2>&1 &", 5000).await.unwrap().exit_code, Some(0));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let tried = ask(&format!("pkill -KILL -f '^Xvfb :1 '; for i in 1 2 3 4 5 6; do {curl}; sleep 0.1; done"));
    println!("{tried}");
    assert!(tried.contains("503") && !tried.lines().any(|l| l == "200"), "{tried}");
    // the box stops itself meanwhile: what the squatter heard is on the volume
    for _ in 0..40 {
        if p.power(m).await.unwrap() != Power::Awake {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let log = docker(&["run", "--rm", "--mount", &format!("type=volume,source={}-workspace,target=/w", name_of(m)), "alpine:latest", "cat", "/w/agents/c2d_a/squat.log"]);
    println!("the squatter: {log:?}");
    assert!(log.contains("holding"), "the command never got the name, so this proves nothing: {log}");
    // the check connects and sends nothing, and that's all anybody may see here
    assert!(log.lines().skip(1).all(|l| l == "HEARD 0"), "a service sent a squatter something: {log}");
    assert!(log.contains("HEARD 0"), "the calls never reached the check, so this proves nothing: {log}");
}

/// With the box's own browser on, as Compose has it: there's no DevTools port for a command to use,
/// a fake one changes nothing, and the browser's downloads land in the shared folder.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn the_main_browser_has_no_door_a_command_can_use() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2m-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (token, password) = ("c2m-token-of-enough-length-0123456789", "c2m-password-of-enough-length-0123456");
    std::fs::write(dir.join("token"), token).unwrap();
    std::fs::write(dir.join("viewer"), password).unwrap();
    let name = format!("c2m-{}", std::process::id());
    let caps: Vec<String> = ["CHOWN", "DAC_OVERRIDE", "FOWNER", "FSETID", "KILL", "NET_BIND_SERVICE", "SETGID", "SETUID"].iter().flat_map(|c| ["--cap-add".to_owned(), (*c).to_owned()]).collect();
    let mut run = vec!["run", "-d", "--name", &name, "--cap-drop", "ALL", "--sysctl", "net.ipv4.ip_unprivileged_port_start=6906", "--security-opt", "no-new-privileges", "--shm-size", "1g", "-e", "BOX_TOKEN_FILE=/run/buwiz-computer/token", "-e", "BOX_VIEWER_PASSWORD_FILE=/run/buwiz-computer/viewer", "-e", "BOX_CHROME=1"];
    run.extend(caps.iter().map(String::as_str));
    let mount = format!("type=bind,source={},target=/run/buwiz-computer,readonly", dir.display());
    run.extend(["--mount", &mount, &image]);
    docker(&run);
    let outcome = tokio::spawn({
        let (name, token) = (name.clone(), token.to_owned());
        async move { main_browser(&name, &token).await }
    })
    .await;
    println!("{}", docker(&["logs", "--tail", "5", &name]));
    docker(&["rm", "-f", "-v", &name]);
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn main_browser(name: &str, token: &str) {
    let mut up = String::new();
    for _ in 0..80 {
        up = docker(&["exec", "--user", "0", name, "sh", "-c", "pgrep -u 1000 -c chromium; true"]);
        if up != "0" && !up.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(up != "0" && !up.is_empty(), "the main browser never started");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let args = docker(&["exec", "--user", "0", name, "sh", "-c", "tr '\\0' ' ' < /proc/$(cat /run/box/x/1/run/chrome.pid)/cmdline"]);
    println!("{args}");
    assert!(args.contains("--user-data-dir") && !args.contains("remote-debugging"), "{args}");
    let listening = docker(&["exec", "--user", "0", name, "sh", "-c", "awk 'NR>1 && $4==\"0A\" {print $2}' /proc/net/tcp | cut -d: -f2 | sort -u"]);
    let ports: Vec<u32> = listening.lines().map(|h| u32::from_str_radix(h, 16).unwrap()).collect();
    println!("listening: {ports:?}");
    assert!(ports.iter().all(|p| [1337, 1340, 6901].contains(p)), "something else listens: {ports:?}");
    let door = "import socket,json\ntry:\n    s=socket.create_connection(('127.0.0.1',9222),2);print('REACHED')\nexcept OSError as e:\n    print('refused',e)\n";
    let tried = docker(&["exec", "--user", "1500", name, "python3", "-c", door]);
    assert!(tried.starts_with("refused"), "{tried}");
    // a command listening there is nobody's business: the box asks it nothing
    let fake = "import socket,time\ns=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.bind(('127.0.0.1',9222));s.listen(5);s.settimeout(5)\nopen('/tmp/fake.log','w').close()\ntry:\n    c,_=s.accept();open('/tmp/fake.log','w').write('asked')\nexcept socket.timeout:\n    pass\ntime.sleep(1)\n";
    docker(&["exec", "-d", "--user", "1500", name, "python3", "-c", fake]);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let chrome = docker(&["exec", "--user", "0", "-e", &format!("T={token}"), name, "sh", "-c", "curl -s -m 5 -H \"Authorization: Bearer $T\" http://127.0.0.1:1340/v1/chrome"]);
    println!("{chrome}");
    assert!(chrome.contains("\"ready\":true"), "{chrome}");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    assert_eq!(docker(&["exec", "--user", "0", name, "cat", "/tmp/fake.log"]), "", "the box asked a command's port");
    // item: its downloads go to the shared folder, and it can write there
    let prefs = docker(&["exec", "--user", "0", name, "cat", "/home/box/desktops/main/chrome-profile/Default/Preferences"]);
    assert!(prefs.contains("/workspace/shared"), "{prefs}");
    let wrote = docker(&["exec", "--user", "0", name, "sh", "-c", "setpriv --reuid=1000 --regid=1000 --groups=1400,1500 --inh-caps=-all touch /workspace/shared/ok && stat -c '%U %G %a' /workspace/shared/ok /workspace/shared"]);
    assert!(wrote.contains("box agent") && wrote.contains("agent agent 2775"), "{wrote}");
}

/// A computer made on an image from before users, with what that image left on its volumes: files
/// owned by the one user, a planted autostart, a link. Waking it makes it again, keeps every file,
/// gives each Bot its own and runs nothing that was planted.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_computer_from_before_users_wakes_with_its_files_and_nothing_planted_runs() {
    with_computer("c2o", |p, m, _| async move { old_computer(&p, &m).await }).await;
}

async fn old_computer(p: &DockerProvider, m: &buwiz_computer::Machine) {
    let name = name_of(m);
    let old = "alpine:latest";
    docker(&["rm", "-f", &name, &format!("{name}-wall")]);
    let seed = r#"
        mkdir -p /workspace/agents/c2o_a/notes /workspace/shared /home/box/.config/autostart /home/box/chrome-profile/bots/c2o_a/Default /home/box/.box-screens
        echo mine > /workspace/agents/c2o_a/notes/keep.txt
        echo loose > /workspace/loose.txt
        ln -s /etc/passwd /workspace/agents/c2o_a/link
        ln -s /etc /workspace/agents/c2o_a/dir-link
        ln -s /usr/local/bin /workspace/agents/c2o_evil
        echo stray > /workspace/agents/c2o_file
        echo cookies > /home/box/chrome-profile/bots/c2o_a/Default/Cookies
        echo 2 > /home/box/.box-screens/c2o_a
        printf '[Desktop Entry]\nType=Application\nName=old\nExec=touch /tmp/planted-ran\n' > /home/box/.config/autostart/old.desktop
        echo 'touch /tmp/planted-ran' > /home/box/.bashrc
        chown -hR 1000:1000 /workspace /home/box
    "#;
    docker(&["run", "--rm", "--mount", &format!("type=volume,source={name}-workspace,target=/workspace"), "--mount", &format!("type=volume,source={name}-home,target=/home/box"), old, "sh", "-c", seed]);
    // what Docker holds of an old computer: neither container has the label
    docker(&["create", "--name", &format!("{name}-wall"), "--label", &format!("app.buwiz.computer.id=cmp_c2otest"), old, "sleep", "infinity"]);
    docker(&["create", "--name", &name, "--network", &format!("container:{name}-wall"), "--mount", &format!("type=volume,source={name}-workspace,target=/workspace"), "--mount", &format!("type=volume,source={name}-home,target=/home/box"), old, "sleep", "infinity"]);

    p.wake(m).await.unwrap();
    assert_eq!(docker(&["inspect", &name, "--format", "{{index .Config.Labels \"app.buwiz.box.users\"}}"]), "1", "it wasn't made again");
    let a = p.screen_up(m, "c2o_a").await.unwrap();
    let (agent, _) = users_of(m, "c2o_a");
    assert_eq!(p.read_file(m, "/workspace/agents/c2o_a/notes/keep.txt").await.unwrap(), b"mine\n");
    let own = p.run(m, &a, "stat -c %u notes/keep.txt; echo more >> notes/keep.txt; cat notes/keep.txt", 5000).await.unwrap();
    assert_eq!(own.exit_code, Some(0), "{}", own.stderr);
    assert!(own.stdout.starts_with(&agent.to_string()) && own.stdout.contains("more"), "{}", own.stdout);
    let as_root = |script: &str| docker(&["exec", "--user", "0", &name, "sh", "-c", script]);
    assert_eq!(as_root("stat -c '%U %u' /etc/passwd /etc"), "root 0\nroot 0", "a link was followed");
    assert_eq!(as_root("stat -c %u /workspace/agents/c2o_a/link"), agent.to_string());
    assert_eq!(as_root("cat /workspace/loose.txt"), "loose");
    assert_eq!(as_root("cat /home/box/old-home-*/chrome-profile/bots/c2o_a/Default/Cookies"), "cookies", "the old home went");
    assert_eq!(as_root("stat -c '%a %U %G' /home/box/old-home-*"), "700 root root", "old sign-ins are nobody's in the box");
    // a link under `agents` must not hand a Bot what it leads to: that was root in the box
    let evil = p.screen_up(m, "c2o_evil").await.unwrap();
    assert_eq!(as_root("stat -c '%u %a' /usr/local/bin"), "0 755", "a Bot owns /usr/local/bin");
    assert_eq!(as_root("stat -c %F /workspace/agents/c2o_evil"), "directory");
    assert_eq!(as_root("readlink /workspace/old-agents-*/c2o_evil"), "/usr/local/bin", "the link should be kept, aside");
    assert_eq!(as_root("cat /workspace/old-agents-*/c2o_file"), "stray");
    assert_eq!(as_root("stat -c '%a %U' /workspace/old-agents-*"), "700 root");
    assert_ne!(p.run(m, &evil, "touch /usr/local/bin/box-screen", 5000).await.unwrap().exit_code, Some(0));
    // nor may a link in a desktop's own home, which that desktop could make at any time
    as_root("pkill -KILL -f 'user-data-dir=/home/box/desktops/c2o_a' ; true");
    let (_, desktop) = users_of(m, "c2o_a");
    p.sleep(m, Sleep::Deep).await.unwrap();
    p.wake(m).await.unwrap();
    as_root(&format!("mv /home/box/desktops/c2o_a/chrome-profile /home/box/desktops/c2o_a/kept; ln -s /usr/local/sbin /home/box/desktops/c2o_a/chrome-profile; chown -h {desktop}:{desktop} /home/box/desktops/c2o_a/chrome-profile"));
    let _ = p.screen_up(m, "c2o_a").await;
    assert_eq!(as_root("stat -c '%u' /usr/local/sbin"), "0", "a desktop's link took root's chown");
    as_root("unlink /home/box/desktops/c2o_a/chrome-profile; mv /home/box/desktops/c2o_a/kept /home/box/desktops/c2o_a/chrome-profile");
    assert_eq!(as_root("stat -c %u /home/box"), "0");
    assert_eq!(as_root("test -e /home/box/desktops/c2o_a/.config/autostart/old.desktop && echo there"), "", "the plant came along");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert_eq!(as_root("ls /tmp/planted-ran 2>/dev/null"), "", "something planted ran");
    let old_home = as_root("ls -d /home/box/old-home-*");
    assert_ne!(p.run(m, &a, &format!("ls {old_home}"), 5000).await.unwrap().exit_code, Some(0), "a Bot reached the old home");

    // and it stays so: a second start doesn't move anything again
    p.sleep(m, Sleep::Deep).await.unwrap();
    p.wake(m).await.unwrap();
    assert_eq!(as_root("ls -d /home/box/old-home-* | wc -l"), "1");
    p.screen_up(m, "c2o_a").await.unwrap();
    assert!(p.read_file(m, "/workspace/agents/c2o_a/notes/keep.txt").await.unwrap().ends_with(b"more\n"));
}

/// A computer made with the proxy and the tunnel: it starts, walled, on the rules staged for it; this
/// Mac's client attaches through the published loopback port, whose replies the wall lets through
/// to the gateway alone; a command can't send from the tunnel's port, nor from a neighbour of it;
/// and the rules hold at the proxy. Nothing goes to the internet: every name asked for is refused
/// by the rules or doesn't resolve.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_computer_made_with_the_tunnel_starts_walled_and_its_port_is_the_proxys_alone() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image, "c2e");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let m = p.create(&Spec { id: "cmp_c2etest".into(), dir: dir.clone(), network: buwiz_computer::Network::Tunnel }).await.unwrap();
    let outcome = tokio::spawn({
        let (m, dir) = (m.clone(), dir.clone());
        async move { tunnel(&m, &dir).await }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2e-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn tunnel(m: &Machine, dir: &std::path::Path) {
    use buwiz_computer::box_egress::{client, rules, Allowlist, ClientState, RulesCell, TunnelClientConfig};
    let p = DockerProvider::new(std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned()), "c2e");
    let egress = m.egress.clone().expect("this image has the proxy: build it from the computer repo");
    assert!(egress.tunnel);
    buwiz_computer::docker::stage_rules(dir, br#"{"v":1,"version":5,"exit":"direct","mode":"listed","allow":["allowed.test"],"block":["blocked.test"]}"#).unwrap();
    p.wake(m).await.unwrap();
    let (_, m) = p.look(m).await.unwrap();
    let st = p.egress(&m).await.unwrap();
    assert_eq!(st["proxy_up"], true, "{st}");
    assert_eq!(st["rules"]["version"], 5, "it started with the staged rules: {st}");
    let screen = p.screen_up(&m, "c2e_a").await.unwrap();

    // a command can't send from the tunnel's port (the proxy holds it), nor from the next one up
    let gateway = docker(&["inspect", &format!("{}-wall", name_of(&m)), "--format", "{{range .NetworkSettings.Networks}}{{.Gateway}}{{end}}"]);
    assert!(!gateway.is_empty());
    for from in [8790, 8789] {
        let out = p.run(&m, &screen, &reach(&gateway, 47000, from), 10_000).await.unwrap();
        println!("from {from}: {} {}", out.stdout.trim(), out.stderr.trim());
        assert!(!out.stdout.contains("REACHED"), "from {from}: {} {}", out.stdout, out.stderr);
    }

    // this Mac's side attaches through the published port: the wall passes the tunnel's replies
    let url = p.tunnel_url(&m).await.unwrap().expect("8790 is published on loopback");
    assert!(url.starts_with("ws://127.0.0.1:"), "{url}");
    let state = ClientState::default();
    let rules = rules::Rules::parse(br#"{"v":1,"version":6,"exit":"tunnel","mode":"listed","allow":["localhost"],"block":["blocked.test"]}"#).unwrap();
    let cfg = TunnelClientConfig { url, bearer: egress.bearer.unwrap().expose().to_owned(), allowlist: Allowlist::any(), destination: Default::default(), rules: RulesCell::new(Some(rules)), reconnect: true, state: state.clone() };
    let client = tokio::spawn(client::run(cfg));
    let mut attached = false;
    for _ in 0..100 {
        if p.egress(&m).await.unwrap()["client_attached"] == true {
            attached = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(attached, "{:?}", state.snapshot());
    let put = p.set_egress_rules(&m, &serde_json::json!({"v": 1, "version": 6, "exit": "tunnel", "mode": "listed", "allow": ["localhost"], "block": ["blocked.test"]})).await.unwrap();
    assert_eq!(put["applied"], true);
    // the rules at the box's proxy, as a command reaches it: blocked and not listed refused there,
    // before any lookup; an allowed name that is this Mac's loopback refused by the client's address
    // policy. Not a byte, nor a lookup, goes to the internet
    let via = |host: &str| format!("curl -s -o /dev/null -w '%{{http_connect}}' --noproxy '' -p -x http://127.0.0.1:8791 https://{host}/ || true");
    for (host, want) in [("blocked.test", "403"), ("elsewhere.test", "403"), ("localhost", "502")] {
        let out = p.run(&m, &screen, &via(host), 20_000).await.unwrap();
        assert_eq!(out.stdout.trim(), want, "{host}: {} {}", out.stdout, out.stderr);
    }
    client.abort();
    let _ = p.screen_down(&m, "c2e_a").await;
}

/// A remake keeps what the named volumes keep and loses what runs; a reset loses the files too.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn a_remake_keeps_the_files_and_a_reset_wipes_them() {
    let image = std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir: PathBuf = std::env::temp_dir().join(format!("c2r-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = DockerProvider::new(image.clone(), "c2r");
    p.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let m = p.create(&Spec { id: "cmp_c2remake".into(), dir: dir.clone(), network: buwiz_computer::Network::Tunnel }).await.unwrap();
    let outcome = tokio::spawn({
        let m = m.clone();
        async move {
            let p = DockerProvider::new(std::env::var(IMAGE_ENV).unwrap_or_else(|_| "buwiz-computer:local".to_owned()), "c2r");
            remake_and_reset(&p, &m, &image).await
        }
    })
    .await;
    p.destroy(&m).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2r-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    outcome.unwrap();
}

async fn remake_and_reset(p: &DockerProvider, m: &buwiz_computer::Machine, image: &str) {
    p.wake(m).await.unwrap();
    let s = p.screen_up(m, "c2r_a").await.unwrap();
    // a file in each kept volume, and a process that must not come back
    p.write_file(m, "keep-workspace.txt", b"kept").await.unwrap();
    docker(&["exec", "--user", "0", &name_of(m), "sh", "-c", "echo home > /home/box/keep-home.txt"]);
    p.run(m, &s, "setsid sleep 297 >/dev/null 2>&1 </dev/null & echo go", 20_000).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_ne!(p.run(m, &s, "pgrep -cf 'sleep 29[7]'", 10_000).await.unwrap().stdout.trim(), "0", "it was running");

    let name = name_of(m);
    let container_id = docker(&["inspect", "-f", "{{.Id}}", &name]);
    let image_id = docker(&["image", "inspect", "-f", "{{.Id}}", image]);
    let bearer = |m: &buwiz_computer::Machine| m.egress.as_ref().and_then(|e| e.bearer.as_ref()).map(|b| b.expose().to_owned());

    let m2 = p.remake(m, buwiz_computer::Network::Tunnel).await.unwrap();
    assert_ne!(docker(&["inspect", "-f", "{{.Id}}", &name]), container_id, "the box was made again");
    assert_eq!(docker(&["inspect", "-f", "{{.Image}}", &name]), image_id, "on the provider's current image");
    assert_ne!(bearer(&m2), bearer(m), "a new tunnel bearer");

    p.wake(&m2).await.unwrap();
    let s2 = p.screen_up(&m2, "c2r_a").await.unwrap();
    assert_eq!(p.read_file(&m2, "keep-workspace.txt").await.unwrap(), b"kept", "the workspace volume kept it");
    assert_eq!(docker(&["exec", "--user", "0", &name, "cat", "/home/box/keep-home.txt"]), "home", "the home volume kept it");
    assert_eq!(p.run(&m2, &s2, "pgrep -cf 'sleep 29[7]'", 10_000).await.unwrap().stdout.trim(), "0", "what ran on it ended");

    p.reset(&m2).await.unwrap();
    assert_eq!(p.power(&m2).await.unwrap(), Power::Absent);
    p.wake(&m2).await.unwrap();
    assert!(p.read_file(&m2, "keep-workspace.txt").await.is_err(), "the reset left the file");
    assert!(docker(&["exec", "--user", "0", &name, "cat", "/home/box/keep-home.txt"]).is_empty(), "the reset left the home file");
}
