//! The local Docker provider: two containers per computer, driven through the `docker` CLI.
//!
//! - **The wall comes first.** On Docker Desktop a container reaches the Mac's loopback through
//!   the host gateway, broker included. A box's home persists, so a model can leave something in
//!   its autostart to run at the next start. So the box never owns its network: a small holder
//!   container does (`<name>-wall`, a `sleep`), and the box joins it. The holder starts, the wall
//!   goes up in its network as root from outside ([`WALL`]), and only then does the box start.
//!   No process of the box ever runs without the wall, and the box keeps no capability to take
//!   it down. If it can't go up, the box doesn't start.
//! - **Ports** are published by the holder, on loopback only, from a range the Bot's shell is
//!   kept off ([`crate::PORT_FIRST`]). They are read for each request.
//! - **Secrets** never go on an argument list, where any process's `ps` reads them: the container
//!   gets file paths, in a folder it mounts read-only. They are put there before every start
//!   (the box exits without them, so a plain `docker restart` fails safe) and taken away once it
//!   answers, or the start is abandoned ([`Staged`]). The image would unlink them as it reads them
//!   and says it couldn't: a mount the box can write is a place to plant things in a Mac folder,
//!   so this side does the removing.
//! - **Sleep** is `docker pause`: it comes back at once with everything open. Deep sleep is
//!   `docker stop`: the memory is free, and the files stay on the volumes. The holder stays up
//!   through both, a `sleep` and little else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::guest::BoxGuest;
use crate::{
    random_hex, CommandOutput, Input, Machine, Power, Problem, Provider, ProviderKind, Reach, Result, Screen, Screenshot, Secret, Seen, Settled, Sleep, Spec, Viewer, WaitFor, Woke, DOCKER_PORT_LAST, PORT_FIRST,
};

/// The image's tag on this Mac (the computer repo's compose file builds it).
pub const DEFAULT_IMAGE: &str = "buwiz-computer:local";
pub const DEFAULT_PREFIX: &str = "buwiz-computer";
pub const IMAGE_ENV: &str = "BUWIZ_COMPUTER_IMAGE";
pub const PREFIX_ENV: &str = "BUWIZ_COMPUTER_PREFIX";

const EXEC_PORT: u16 = 1337;
const HOST_PORT: u16 = 1340;
/// The screens' viewers, `6900 + slot`: the box's own, then the Bots'.
const VIEWER_PORTS: [u16; 5] = [6901, 6902, 6903, 6904, 6905];
const LABEL: &str = "app.buwiz.computer";
/// Where the container sees the folder its secrets are put in for a start.
const SECRET_MOUNT: &str = "/run/buwiz-computer";
const TOKEN_FILE: &str = "token";
const VIEWER_FILE: &str = "viewer";
/// How long `docker` itself gets; a hung daemon counts as down.
const DOCKER_PATIENCE: Duration = Duration::from_secs(60);
/// How long a start gets to answer.
const WAKE_PATIENCE: Duration = Duration::from_secs(120);
const STOP_SECONDS: &str = "5";
const IFCONFIG: &str = "/sbin/ifconfig";
const PORT_TRIES: usize = 4;

/// Where Docker Desktop puts its CLI: an app started from Finder has a bare `PATH`.
const DOCKER_DIRS: [&str; 4] = ["/usr/local/bin", "/opt/homebrew/bin", "/Applications/Docker.app/Contents/Resources/bin", "~/.docker/bin"];

/// The wall, over netlink, with nothing but python3 in the image. Its arguments are this Mac's
/// own addresses. Safe to repeat.
///
/// - Blackhole routes for every range that isn't the open internet: the private ranges (which
///   hold the host gateway, `192.168.65.x`, the way to the Mac's loopback), link-local, CGNAT, the
///   benchmark range some VPNs use, the reserved block.
/// - Blackhole routes for each of this Mac's addresses, wherever they are.
/// - Any route the box has with a next hop of its own, beyond the default.
/// - The box's own subnets, bridge gateway included, by rule: a route would stop the replies on
///   published ports too, which come from that gateway. The rules first let through what leaves
///   from those ports, then drop the rest. A default route keeps working: its next hop is
///   resolved on the link and never looked up as a destination.
/// - IPv6 is off (`create_args`); a box where it is on doesn't pass.
const WALL: &str = r#"
import socket, struct, sys
s = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)
def ask(kind, flags, body):
    s.send(struct.pack('=IHHII', 16 + len(body), kind, 0x1 | 0x4 | flags, 1, 0) + body)
    return struct.unpack('=i', s.recv(4096)[16:20])[0]
def attr(kind, data):
    return struct.pack('=HH', 4 + len(data), kind) + data + b'\0' * (-len(data) % 4)
def blackhole(dst, plen):
    err = ask(24, 0x400 | 0x100, struct.pack('=BBBBBBBBI', 2, plen, 0, 0, 254, 4, 0, 6, 0) + attr(1, socket.inet_aton(dst)))
    if err:
        raise OSError(-err, dst)
def rule(priority, action, dst=None, plen=0, ports=None):
    body = struct.pack('=BBBBBBBBI', 2, plen, 0, 0, 254 if action == 1 else 0, 0, 0, action, 0) + attr(6, struct.pack('=I', priority))
    if dst:
        body += attr(1, socket.inet_aton(dst))
    if ports:
        body += attr(22, b'\x06') + attr(23, struct.pack('=HH', *ports))
    err = ask(32, 0x400 | 0x200, body)
    if err and err != -17:
        raise OSError(-err, 'rule')
for dst, plen in [('0.0.0.0', 8), ('10.0.0.0', 8), ('100.64.0.0', 10), ('169.254.0.0', 16), ('172.16.0.0', 12), ('192.0.0.0', 24), ('192.168.0.0', 16), ('198.18.0.0', 15), ('240.0.0.0', 4)]:
    blackhole(dst, plen)
for address in sys.argv[1:]:
    blackhole(address, 32)
rule(100, 1, ports=(1337, 1337))
rule(101, 1, ports=(1340, 1340))
rule(102, 1, ports=(6901, 6905))
priority = 200
for line in open('/proc/net/route').read().splitlines()[1:]:
    iface, dst, gateway, flags, _, _, _, mask = line.split()[:8]
    ip = lambda h: socket.inet_ntoa(struct.pack('<I', int(h, 16)))
    plen = bin(int(mask, 16)).count('1')
    if iface == '*' or dst == '00000000':
        continue
    if gateway != '00000000':
        blackhole(ip(dst), plen)
    else:
        rule(priority, 6, dst=ip(dst), plen=plen)
        priority += 1
try:
    if open('/proc/sys/net/ipv6/conf/all/disable_ipv6').read().strip() != '1':
        raise OSError('IPv6 is on')
except FileNotFoundError:
    pass
"#;

/// The local Docker computers.
pub struct DockerProvider {
    image: String,
    prefix: String,
    /// the program, when it isn't found the usual way (tests)
    program: Option<PathBuf>,
    /// where to read this Mac's addresses (tests)
    ifconfig: PathBuf,
    /// by computer: the start of its network the wall went up on
    walled: Mutex<HashMap<String, String>>,
}

impl DockerProvider {
    pub fn new(image: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self { image: image.into(), prefix: prefix.into(), program: None, ifconfig: PathBuf::from(IFCONFIG), walled: Mutex::new(HashMap::new()) }
    }

    /// From `BUWIZ_COMPUTER_IMAGE` and `BUWIZ_COMPUTER_PREFIX`.
    pub fn from_env() -> Self {
        let set = |k: &str, default: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_owned());
        Self::new(set(IMAGE_ENV, DEFAULT_IMAGE), set(PREFIX_ENV, DEFAULT_PREFIX))
    }

    /// Runs `program` as `docker`.
    pub fn with_program(mut self, program: PathBuf) -> Self {
        self.program = Some(program);
        self
    }

    /// Reads this Mac's addresses with `program`.
    pub fn with_ifconfig(mut self, program: PathBuf) -> Self {
        self.ifconfig = program;
        self
    }

    fn network(&self) -> String {
        format!("{}-net", self.prefix)
    }

    fn program(&self) -> PathBuf {
        self.program.clone().unwrap_or_else(find_docker)
    }

    async fn docker(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
        let raw = run_docker(&self.program(), args, stdin, DOCKER_PATIENCE).await?;
        match raw.status {
            Some(0) => Ok(String::from_utf8_lossy(&raw.stdout).into_owned()),
            status => Err(classify(status, &String::from_utf8_lossy(&raw.stderr), &self.image)),
        }
    }

    async fn ensure_network(&self) -> Result<()> {
        let net = self.network();
        if self.docker(&["network", "inspect", "--format", "{{.Id}}", &net], None).await.is_ok() {
            return Ok(());
        }
        self.docker(&["network", "create", "--opt", "com.docker.network.bridge.enable_icc=false", "--label", &format!("{LABEL}=1"), &net], None).await.map(|_| ())
    }

    async fn inspect(&self, name: &str) -> Result<Inspected> {
        Inspected::parse(&self.docker(&["inspect", "--type", "container", name, &holder(name)], None).await?)
    }

    /// What this request's [`Provider::look`] saw, else a read now.
    async fn seen(&self, machine: &Machine) -> Result<Inspected> {
        match &machine.seen {
            Some(seen) => Ok(seen.clone()),
            None => self.inspect(container(machine)?).await,
        }
    }

    /// The running computer's services, at the ports read for this request: nothing is kept
    /// between requests, because a start can pick new ports, and an old one can be another
    /// computer's by then.
    async fn guest(&self, machine: &Machine) -> Result<BoxGuest> {
        let found = self.seen(machine).await?;
        if found.status != "running" {
            return Err(Problem::Asleep);
        }
        connect(&found, machine)
    }

    /// `docker start`, answering. The secrets are there only for the start, and go however it
    /// ends, a dropped future included.
    async fn start(&self, machine: &Machine, name: &str) -> Result<()> {
        let staged = Staged::new(secret_dir(&machine.dir), machine)?;
        let up = self.start_staged(machine, name).await;
        drop(staged);
        up
    }

    async fn start_staged(&self, machine: &Machine, name: &str) -> Result<()> {
        self.docker(&["start", name], None).await?;
        let plain = connect(&self.inspect(name).await?, machine)?;
        let deadline = Instant::now() + WAKE_PATIENCE;
        while !plain.ready().await {
            if Instant::now() > deadline {
                return Err(Problem::Failed(format!("it didn't answer in {}s", WAKE_PATIENCE.as_secs())));
            }
            let status = self.inspect(name).await?.status;
            if status != "running" {
                return Err(Problem::Failed(format!("it stopped while starting ({status})")));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Its network up and walled, before anything of the box runs in it. Whatever is running
    /// there stops if that fails.
    async fn net_up(&self, name: &str, found: &Inspected) -> Result<()> {
        let walled = self.walled.lock().unwrap_or_else(|e| e.into_inner()).get(name) == Some(&found.started);
        if found.net_running && walled {
            return Ok(());
        }
        let up = async {
            if !found.net_running {
                self.start_holder(name).await?;
            }
            self.wall(name).await
        };
        match up.await {
            Ok(started) => {
                self.walled.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_owned(), started);
                Ok(())
            }
            Err(e) => {
                self.walled.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
                self.halt(name, found).await;
                Err(e)
            }
        }
    }

    /// Docker picks each port from the range as it starts, and can lose a race for one just freed.
    async fn start_holder(&self, name: &str) -> Result<()> {
        let net = holder(name);
        for _ in 1..PORT_TRIES {
            match self.docker(&["start", &net], None).await {
                Err(Problem::Failed(why)) if why.contains("ports are not available") => tokio::time::sleep(Duration::from_millis(300)).await,
                done => return done.map(|_| ()),
            }
        }
        self.docker(&["start", &net], None).await.map(|_| ())
    }

    /// The box down, whatever state it's in.
    async fn halt(&self, name: &str, found: &Inspected) {
        if found.status == "paused" {
            let _ = self.docker(&["unpause", name], None).await;
        }
        if matches!(found.status.as_str(), "paused" | "running") {
            let _ = self.docker(&["stop", "-t", "0", name], None).await;
        }
    }

    /// Puts the wall up in `name`'s network: the start of that network it's up on.
    async fn wall(&self, name: &str) -> Result<String> {
        let macs = mac_addresses(&self.ifconfig).await?;
        let net = holder(name);
        let mut args = vec!["exec", "--user", "0", "--privileged", &net, "python3", "-c", WALL];
        args.extend(macs.iter().map(String::as_str));
        match self.docker(&args, None).await {
            Ok(_) => Ok(self.inspect(name).await?.started),
            Err(Problem::Failed(why)) => Err(Problem::Unwalled(why)),
            Err(e) => Err(e),
        }
    }
}

/// The container that owns `name`'s network.
fn holder(name: &str) -> String {
    format!("{name}-wall")
}

/// This Mac's own IPv4 addresses, loopback aside, as `ifconfig` lists them: a box must not reach
/// what listens on them, whatever kind of address they are. Not being able to tell is a failure:
/// the wall would go up with holes. A Mac with no network has none, and that's a fine answer.
async fn mac_addresses(ifconfig: &Path) -> Result<Vec<String>> {
    let failed = || Problem::Unwalled("this Mac's own addresses can't be read".to_owned());
    let out = Command::new(ifconfig).stdin(Stdio::null()).stderr(Stdio::null()).output().await.map_err(|_| failed())?;
    if !out.status.success() {
        return Err(failed());
    }
    Ok(parse_inets(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_inets(ifconfig: &str) -> Vec<String> {
    let mut found: Vec<String> = ifconfig
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("inet ")?.split_whitespace().next())
        .filter(|a| !a.starts_with("127.") && a.parse::<std::net::Ipv4Addr>().is_ok())
        .map(str::to_owned)
        .collect();
    found.dedup();
    found
}

fn connect(found: &Inspected, machine: &Machine) -> Result<BoxGuest> {
    let port = |p: u16| found.published(p).ok_or_else(|| Problem::Failed(format!("port {p} isn't published on loopback")));
    Ok(BoxGuest::connect(&format!("http://{}", port(EXEC_PORT)?), &format!("http://{}", port(HOST_PORT)?), machine.token.clone()))
}

/// The `docker create` arguments of the container that owns a computer's network: `sleep`, the
/// published ports, and IPv6 off.
fn holder_args(image: &str, name: &str, network: &str, id: &str) -> Vec<String> {
    let net = holder(name);
    let mut args: Vec<String> = ["create", "--name", &net, "--network", network, "--label"].map(str::to_owned).to_vec();
    args.push(format!("{LABEL}=1"));
    args.push("--label".to_owned());
    args.push(format!("{LABEL}.id={id}"));
    for a in ["--memory", "64m", "--pids-limit", "64", "--security-opt", "no-new-privileges", "--sysctl", "net.ipv6.conf.all.disable_ipv6=1"] {
        args.push(a.to_owned());
    }
    for port in [EXEC_PORT, HOST_PORT].into_iter().chain(VIEWER_PORTS) {
        args.push("-p".to_owned());
        args.push(format!("127.0.0.1:{PORT_FIRST}-{DOCKER_PORT_LAST}:{port}"));
    }
    for a in ["--entrypoint", "sleep", image, "infinity"] {
        args.push(a.to_owned());
    }
    args
}

/// The `docker create` arguments of a new computer: no secret among them, and no network of its
/// own.
fn create_args(image: &str, name: &str, id: &str, secrets: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["create", "--name", name, "--network"].map(str::to_owned).to_vec();
    args.push(format!("container:{}", holder(name)));
    args.push("--label".to_owned());
    args.push(format!("{LABEL}=1"));
    args.push("--label".to_owned());
    args.push(format!("{LABEL}.id={id}"));
    for (source, target) in [(format!("{name}-workspace"), "/workspace"), (format!("{name}-home"), "/home/box")] {
        args.push("--mount".to_owned());
        args.push(format!("type=volume,source={source},target={target}"));
    }
    args.push("--mount".to_owned());
    args.push(format!("type=bind,source={},target={SECRET_MOUNT},readonly", secrets.display()));
    // Chromium needs more than Docker's 64 MiB; no setuid program may lift the box's user
    for a in ["--shm-size", "1g", "--memory", "4g", "--pids-limit", "2048", "--security-opt", "no-new-privileges"] {
        args.push(a.to_owned());
    }
    for env in [
        format!("BOX_TOKEN_FILE={SECRET_MOUNT}/{TOKEN_FILE}"),
        format!("BOX_VIEWER_PASSWORD_FILE={SECRET_MOUNT}/{VIEWER_FILE}"),
        "BOX_DESKTOP=1".to_owned(),
        "BOX_DESKTOP_REQUIRED=1".to_owned(),
        "BOX_CHROME=0".to_owned(),
    ] {
        args.push("-e".to_owned());
        args.push(env);
    }
    args.push(image.to_owned());
    args
}

fn container(machine: &Machine) -> Result<&str> {
    match &machine.reach {
        Reach::Container { name } => Ok(name),
        _ => Err(Problem::Failed("this computer isn't a container".to_owned())),
    }
}

fn secret_dir(dir: &Path) -> PathBuf {
    dir.join("start")
}

fn io_problem(e: std::io::Error) -> Problem {
    Problem::Failed(format!("its folder: {e}"))
}

/// A folder only this user reads, whatever the umask: `create_dir` modes are filtered by it, and
/// the folder may already be there.
fn private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(io_problem)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io_problem)
}

/// The secrets, on disk for as long as this lives: dropped, however a start ends, they're gone.
struct Staged(PathBuf);

impl Staged {
    fn new(dir: PathBuf, machine: &Machine) -> Result<Self> {
        let staged = Self(dir);
        stage_secrets(&staged.0, machine)?;
        Ok(staged)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        clear_secrets(&self.0);
    }
}

fn stage_secrets(dir: &Path, machine: &Machine) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    private_dir(dir)?;
    for (file, secret) in [(TOKEN_FILE, &machine.token), (VIEWER_FILE, &machine.viewer_password)] {
        let path = dir.join(file);
        // whatever is there goes, link or not, and this is a new file: a link is never followed
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_problem(e)),
            _ => {}
        }
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(io_problem)?;
        f.write_all(secret.expose().as_bytes()).map_err(io_problem)?;
    }
    Ok(())
}

fn clear_secrets(dir: &Path) {
    for file in [TOKEN_FILE, VIEWER_FILE] {
        let _ = std::fs::remove_file(dir.join(file));
    }
}

impl DockerProvider {
    /// The container and its volumes. Already gone is fine.
    async fn remove(&self, name: &str) -> Result<()> {
        self.walled.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        for container in [name.to_owned(), holder(name)] {
            match self.docker(&["rm", "-f", "-v", &container], None).await {
                Ok(_) | Err(Problem::Gone) => {}
                Err(e) => return Err(e),
            }
        }
        for volume in [format!("{name}-workspace"), format!("{name}-home")] {
            match self.docker(&["volume", "rm", &volume], None).await {
                Ok(_) | Err(Problem::Gone) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Provider for DockerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Docker
    }

    async fn check(&self) -> Result<()> {
        self.docker(&["version", "--format", "{{.Server.Version}}"], None).await?;
        self.docker(&["image", "inspect", "--format", "{{.Id}}", &self.image], None).await.map(|_| ()).map_err(|e| match e {
            Problem::Gone => Problem::ImageMissing(self.image.clone()),
            other => other,
        })
    }

    async fn create(&self, spec: &Spec) -> Result<Machine> {
        self.check().await?;
        self.ensure_network().await?;
        let name = format!("{}-{}", self.prefix, random_hex(4)?);
        let start = secret_dir(&spec.dir);
        private_dir(&start)?;
        let net = holder_args(&self.image, &name, &self.network(), &spec.id);
        self.docker(&net.iter().map(String::as_str).collect::<Vec<_>>(), None).await?;
        let args = create_args(&self.image, &name, &spec.id, &start);
        if let Err(e) = self.docker(&args.iter().map(String::as_str).collect::<Vec<_>>(), None).await {
            let _ = self.remove(&name).await;
            return Err(e);
        }
        Ok(Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name },
            image: Some(self.image.clone()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            seen: None,
        })
    }

    async fn power(&self, machine: &Machine) -> Result<Power> {
        match self.inspect(container(machine)?).await {
            Ok(found) => Ok(power_of(&found.status)),
            Err(Problem::Gone) => Ok(Power::Absent),
            Err(e) => Err(e),
        }
    }

    async fn look(&self, machine: &Machine) -> Result<(Power, Machine)> {
        match self.inspect(container(machine)?).await {
            Ok(found) => Ok((power_of(&found.status), Machine { seen: Some(found), ..machine.clone() })),
            Err(Problem::Gone) => Ok((Power::Absent, machine.clone())),
            Err(e) => Err(e),
        }
    }

    async fn wake(&self, machine: &Machine) -> Result<Woke> {
        let name = container(machine)?;
        let found = self.inspect(name).await?;
        let up = matches!(found.status.as_str(), "paused" | "running");
        // the network went away under a running box: only a fresh start joins the new one
        if up && !found.net_running {
            self.halt(name, &found).await;
        }
        // first the wall, then anything of the box
        self.net_up(name, &found).await?;
        match found.status.as_str() {
            "paused" if found.net_running => {
                self.docker(&["unpause", name], None).await?;
                Ok(Woke::Resumed)
            }
            "running" if found.net_running => Ok(Woke::Started),
            _ => self.start(machine, name).await.map(|()| Woke::Started),
        }
    }

    async fn sleep(&self, machine: &Machine, depth: Sleep) -> Result<()> {
        let name = container(machine)?;
        let status = self.inspect(name).await?.status;
        match (depth, status.as_str()) {
            (Sleep::Light, "running") => self.docker(&["pause", name], None).await.map(|_| ()),
            (Sleep::Light, _) => Ok(()),
            (Sleep::Deep, "paused") => {
                self.docker(&["unpause", name], None).await?;
                self.docker(&["stop", "-t", STOP_SECONDS, name], None).await.map(|_| ())
            }
            (Sleep::Deep, "running") => self.docker(&["stop", "-t", STOP_SECONDS, name], None).await.map(|_| ()),
            (Sleep::Deep, _) => Ok(()),
        }
    }

    async fn destroy(&self, machine: &Machine) -> Result<()> {
        self.remove(container(machine)?).await
    }

    async fn sweep(&self, id: &str) -> Result<()> {
        let filter = format!("label={LABEL}.id={id}");
        let names = self.docker(&["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"], None).await?;
        let mut boxes: Vec<&str> = names.lines().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.strip_suffix("-wall").unwrap_or(n)).collect();
        boxes.sort_unstable();
        boxes.dedup();
        for name in boxes {
            self.remove(name).await?;
        }
        Ok(())
    }

    async fn guard(&self, machine: &Machine) -> Result<()> {
        let name = container(machine)?;
        let found = self.seen(machine).await?;
        if !found.net_running {
            self.halt(name, &found).await;
            return Err(Problem::Unwalled("its network went away".to_owned()));
        }
        self.net_up(name, &found).await
    }

    async fn screens(&self, machine: &Machine) -> Result<Vec<String>> {
        self.guest(machine).await?.screens().await
    }

    async fn resolve(&self, machine: &Machine, path: &str) -> Result<String> {
        self.guest(machine).await?.resolve(path).await
    }

    async fn screen_up(&self, machine: &Machine, bot: &str) -> Result<Screen> {
        self.guest(machine).await?.screen_up(bot).await
    }

    async fn screen_down(&self, machine: &Machine, bot: &str) -> Result<()> {
        self.guest(machine).await?.screen_down(bot).await
    }

    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        let found = self.seen(machine).await?;
        Ok(found.published(screen.viewer_port).map(|addr| Viewer { addr, password: machine.viewer_password.clone() }))
    }

    async fn screenshot(&self, machine: &Machine, screen: &Screen) -> Result<Screenshot> {
        self.guest(machine).await?.screenshot(screen).await
    }

    async fn input(&self, machine: &Machine, screen: &Screen, input: &Input) -> Result<()> {
        self.guest(machine).await?.input(screen, input).await
    }

    async fn run(&self, machine: &Machine, screen: &Screen, command: &str, timeout_ms: u64) -> Result<CommandOutput> {
        self.guest(machine).await?.run(screen, command, timeout_ms.clamp(1000, 600_000)).await
    }

    async fn open_url(&self, machine: &Machine, screen: &Screen, url: &str) -> Result<()> {
        self.guest(machine).await?.open_url(screen, url).await
    }

    async fn read_file(&self, machine: &Machine, path: &str) -> Result<Vec<u8>> {
        self.guest(machine).await?.read_file(path).await
    }

    async fn write_file(&self, machine: &Machine, path: &str, content: &[u8]) -> Result<()> {
        self.guest(machine).await?.write_file(path, content).await
    }

    async fn mark(&self, machine: &Machine, screen: &Screen) -> Option<u64> {
        self.guest(machine).await.ok()?.mark(screen).await
    }

    async fn settle(&self, machine: &Machine, screen: &Screen, wait: WaitFor) -> Result<Settled> {
        self.guest(machine).await?.settle(screen, wait).await
    }

    async fn record_start(&self, machine: &Machine, screen: &Screen) -> Result<u64> {
        self.guest(machine).await?.record_start(screen).await
    }

    async fn record_stop(&self, machine: &Machine, screen: &Screen, id: u64) -> Result<String> {
        self.guest(machine).await?.record_stop(screen, id).await
    }
}

/// `docker`'s state word as a [`Power`].
pub fn power_of(status: &str) -> Power {
    match status {
        "running" => Power::Awake,
        "paused" | "exited" | "created" | "stopped" => Power::Asleep,
        "restarting" => Power::Waking,
        other => Power::Broken(format!("Docker says it is {other}.")),
    }
}

/// What `docker inspect` said.
pub type Inspected = Seen;

impl Inspected {
    pub fn parse(json: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(json).map_err(|e| Problem::Failed(format!("docker inspect: {e}")))?;
        let entry = v.get(0).ok_or(Problem::Gone)?;
        let net = v.get(1).ok_or(Problem::Gone)?;
        let status = entry.pointer("/State/Status").and_then(Value::as_str).unwrap_or_default().to_owned();
        let started = net.pointer("/State/StartedAt").and_then(Value::as_str).unwrap_or_default().to_owned();
        let net_running = net.pointer("/State/Status").and_then(Value::as_str) == Some("running");
        let mut ports = HashMap::new();
        for (spec, bindings) in net.pointer("/NetworkSettings/Ports").and_then(Value::as_object).into_iter().flatten() {
            let Some(guest) = spec.strip_suffix("/tcp").and_then(|p| p.parse::<u16>().ok()) else { continue };
            // loopback, or nothing: a binding anywhere else isn't ours to use
            let found = bindings.as_array().into_iter().flatten().find(|b| b.get("HostIp").and_then(Value::as_str) == Some("127.0.0.1"));
            if let Some(port) = found.and_then(|b| b.get("HostPort")).and_then(Value::as_str).filter(|p| !p.is_empty()) {
                ports.insert(guest, format!("127.0.0.1:{port}"));
            }
        }
        Ok(Self { status, started, net_running, ports })
    }

    pub fn published(&self, guest_port: u16) -> Option<String> {
        self.ports.get(&guest_port).cloned()
    }
}

fn home_dir(dir: &str) -> PathBuf {
    match (dir.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(dir),
    }
}

/// `PATH` with Docker's folders on it, worked out once: an app from Finder has a bare one, and
/// its credential helpers sit next to it.
fn docker_path() -> &'static std::ffi::OsStr {
    static P: OnceLock<std::ffi::OsString> = OnceLock::new();
    P.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).chain(DOCKER_DIRS.map(home_dir)).collect();
        std::env::join_paths(dirs).unwrap_or(path)
    })
}

/// `docker`: on [`docker_path`].
fn find_docker() -> PathBuf {
    std::env::split_paths(docker_path()).map(|d| d.join("docker")).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("docker"))
}

struct Raw {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_docker(program: &Path, args: &[&str], stdin: Option<&[u8]>, patience: Duration) -> Result<Raw> {
    let mut command = Command::new(program);
    command.env("PATH", docker_path());
    command.args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Problem::ProviderMissing("Docker".to_owned())
        } else {
            Problem::Failed(format!("docker wouldn't start: {e}"))
        }
    })?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes).await.map_err(|e| Problem::Failed(format!("docker's input: {e}")))?;
    }
    match tokio::time::timeout(patience, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(Raw { status: out.status.code(), stdout: out.stdout, stderr: out.stderr }),
        Ok(Err(e)) => Err(Problem::Failed(format!("docker: {e}"))),
        Err(_) => Err(Problem::ProviderDown("Docker".to_owned())),
    }
}

/// Docker's stderr as the problem a person can act on.
fn classify(status: Option<i32>, stderr: &str, image: &str) -> Problem {
    let said = |words: &[&str]| words.iter().any(|w| stderr.contains(w));
    if said(&["No such image", "Unable to find image", "pull access denied"]) {
        Problem::ImageMissing(image.to_owned())
    } else if said(&["No such container", "No such object", "No such volume"]) {
        Problem::Gone
    } else if said(&["Cannot connect to the Docker daemon", "Is the docker daemon running", "docker daemon is not running", "failed to connect to the docker API"]) {
        Problem::ProviderDown("Docker".to_owned())
    } else {
        let first = stderr.lines().next().unwrap_or_default().trim();
        Problem::Failed(if first.is_empty() { format!("docker exited with {status:?}") } else { first.to_owned() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn fake_docker(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("docker");
        std::fs::write(&path, format!("#!/bin/sh\necho \"$*\" >> \"{}/calls\"\n{body}\n", dir.display())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buwiz-computer-{name}-{}", random_hex(4).unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn machine(dir: &Path) -> Machine {
        Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name: "c2-x".into() },
            image: None,
            token: Secret::new("the-token"),
            viewer_password: Secret::new("the-password"),
            dir: dir.to_path_buf(),
            seen: None,
        }
    }

    #[test]
    fn the_holder_publishes_on_loopback_in_the_range_and_the_box_only_joins_it() {
        let net = holder_args("img:1", "c2-x", "c2-net", "cmp_1").join(" ");
        for port in [1337, 1340, 6901, 6902, 6905] {
            assert!(net.contains(&format!("-p 127.0.0.1:47000-47199:{port}")), "{net}");
        }
        assert!(!net.contains("0.0.0.0"), "{net}");
        assert!(net.contains("--name c2-x-wall") && net.contains("--entrypoint sleep img:1 infinity"), "{net}");
        assert!(net.contains("disable_ipv6=1") && !net.contains("BOX_"), "{net}");
        let args = create_args("img:1", "c2-x", "cmp_1", Path::new("/d/start")).join(" ");
        assert!(args.contains("--network container:c2-x-wall"), "{args}");
        assert!(!args.contains("-p "), "a box in another's network publishes nothing: {args}");
        assert!(args.contains("BOX_TOKEN_FILE=/run/buwiz-computer/token"), "{args}");
        assert!(args.contains("target=/run/buwiz-computer,readonly"), "the box can't plant anything there: {args}");
        assert!(args.contains("BOX_VIEWER_PASSWORD_FILE=/run/buwiz-computer/viewer"), "{args}");
        assert!(!args.contains("BOX_TOKEN=") && !args.contains("BOX_VIEWER_PASSWORD="), "{args}");
        assert!(args.contains("no-new-privileges") && args.contains("--pids-limit 2048"), "{args}");
    }

    #[test]
    fn only_a_loopback_binding_counts() {
        let json = r#"[{"State":{"Status":"running"}},{"State":{"Status":"running","StartedAt":"T1"},"NetworkSettings":{"Ports":{
            "1337/tcp":[{"HostIp":"127.0.0.1","HostPort":"47001"}],
            "1340/tcp":[{"HostIp":"0.0.0.0","HostPort":"47002"}],
            "6901/tcp":null}}}]"#;
        let found = Inspected::parse(json).unwrap();
        assert_eq!(found.status, "running");
        assert_eq!(found.published(1337).as_deref(), Some("127.0.0.1:47001"));
        assert_eq!(found.published(1340), None);
        assert_eq!(found.published(6901), None);
    }

    #[test]
    fn dockers_states_are_a_power() {
        assert_eq!(power_of("running"), Power::Awake);
        assert_eq!(power_of("paused"), Power::Asleep);
        assert_eq!(power_of("exited"), Power::Asleep);
        assert_eq!(power_of("created"), Power::Asleep);
        assert_eq!(power_of("dead").word(), "error");
    }

    #[tokio::test]
    async fn docker_not_running_is_a_sentence() {
        let dir = tmp("down");
        let program = fake_docker(&dir, "echo 'Cannot connect to the Docker daemon at unix:///x. Is the docker daemon running?' >&2; exit 1");
        let p = DockerProvider::new("img:1", "c2").with_program(program);
        let e = p.check().await.unwrap_err();
        assert_eq!(e, Problem::ProviderDown("Docker".into()));
        assert!(e.sentence().starts_with("Docker isn't running."));
    }

    #[tokio::test]
    async fn docker_not_installed_is_a_sentence() {
        let p = DockerProvider::new("img:1", "c2").with_program(PathBuf::from("/nonexistent/docker"));
        assert_eq!(p.check().await.unwrap_err(), Problem::ProviderMissing("Docker".into()));
    }

    #[tokio::test]
    async fn a_missing_image_is_a_sentence() {
        let dir = tmp("image");
        let program = fake_docker(&dir, "case \"$1\" in version) echo 29.8;; image) echo 'Error: No such object: img:1' >&2; exit 1;; esac");
        let p = DockerProvider::new("img:1", "c2").with_program(program);
        assert_eq!(p.check().await.unwrap_err(), Problem::ImageMissing("img:1".into()));
    }

    /// `docker inspect`'s answer: the box as `status`, its network up since `started`.
    fn inspected(status: &str, net: &str, started: &str) -> String {
        format!(r#"[{{"State":{{"Status":"{status}"}}}},{{"State":{{"Status":"{net}","StartedAt":"{started}"}},"NetworkSettings":{{"Ports":{{"1337/tcp":[{{"HostIp":"127.0.0.1","HostPort":"47001"}}],"1340/tcp":[{{"HostIp":"127.0.0.1","HostPort":"47002"}}],"6902/tcp":[{{"HostIp":"127.0.0.1","HostPort":"47003"}}]}}}}}}]"#)
    }

    /// A provider whose `docker` answers `inspect` from the file `state`, and `exec` and `start`
    /// as told. A good `ifconfig` beside it.
    fn faked(dir: &Path, state: &str, exec: &str, start: &str) -> DockerProvider {
        let body = format!("case \"$1\" in inspect) cat \"{}/state\";; exec) {exec};; start) {start};; esac", dir.display());
        std::fs::write(dir.join("state"), state).unwrap();
        DockerProvider::new("img:1", "c2").with_program(fake_docker(dir, &body)).with_ifconfig(PathBuf::from("/usr/bin/true"))
    }

    fn calls(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default()
    }

    fn at(dir: &Path, word: &str) -> usize {
        calls(dir).find(word).unwrap_or_else(|| panic!("no {word} in {}", calls(dir)))
    }

    const WALL_CALL: &str = "exec --user 0 --privileged c2-x-wall";

    #[tokio::test]
    async fn nothing_of_the_box_runs_before_the_wall_is_up() {
        let dir = tmp("order");
        let p = faked(&dir, &inspected("exited", "exited", "T1"), "true", "case \"$2\" in *-wall) ;; *) exit 1;; esac");
        let _ = p.wake(&machine(&dir)).await;
        assert!(at(&dir, "start c2-x-wall") < at(&dir, WALL_CALL), "{}", calls(&dir));
        assert!(at(&dir, WALL_CALL) < at(&dir, "start c2-x\n"), "the box started before its wall: {}", calls(&dir));
        // the usual deep wake: its network never went down, and this broker hasn't walled it
        let dir = tmp("order-up");
        let p = faked(&dir, &inspected("exited", "running", "T1"), "true", "exit 1");
        let _ = p.wake(&machine(&dir)).await;
        assert!(at(&dir, WALL_CALL) < at(&dir, "start c2-x\n"), "the box started before its wall: {}", calls(&dir));
    }

    #[tokio::test]
    async fn a_port_lost_to_a_race_is_tried_again() {
        let dir = tmp("port-race");
        let once = format!("case \"$2\" in *-wall) [ -e \"{0}/tried\" ] || {{ touch \"{0}/tried\"; echo 'Error response from daemon: ports are not available: exposing port TCP 127.0.0.1:47001' >&2; exit 1; }};; *) exit 1;; esac", dir.display());
        let p = faked(&dir, &inspected("exited", "exited", "T1"), "true", &once);
        let _ = p.wake(&machine(&dir)).await;
        assert_eq!(calls(&dir).matches("start c2-x-wall").count(), 2, "{}", calls(&dir));
        assert!(calls(&dir).contains(WALL_CALL), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn a_wall_that_fails_means_the_box_never_starts() {
        let dir = tmp("no-wall");
        let p = faked(&dir, &inspected("exited", "running", "T1"), "echo no >&2; exit 1", "true");
        assert_eq!(p.wake(&machine(&dir)).await.unwrap_err().code(), "unwalled");
        assert!(!calls(&dir).contains("start c2-x\n"), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn addresses_that_cant_be_read_stop_the_wall() {
        let dir = tmp("no-ifconfig");
        for bad in ["/usr/bin/false", "/nonexistent/ifconfig"] {
            let p = faked(&dir, &inspected("exited", "running", "T1"), "true", "true").with_ifconfig(PathBuf::from(bad));
            assert_eq!(p.wake(&machine(&dir)).await.unwrap_err().code(), "unwalled", "{bad}");
            assert!(!calls(&dir).contains(WALL_CALL) && !calls(&dir).contains("start c2-x\n"), "{}", calls(&dir));
        }
    }

    #[tokio::test]
    async fn the_secrets_are_gone_after_a_start_that_failed() {
        let dir = tmp("start");
        let start = "ls \"$(dirname \"$0\")/start\" >> \"$(dirname \"$0\")/seen\"; echo boom >&2; exit 1";
        let p = faked(&dir, &inspected("exited", "running", "T1"), "true", start);
        assert_eq!(p.wake(&machine(&dir)).await.unwrap_err(), Problem::Failed("boom".into()));
        assert_eq!(std::fs::read_to_string(dir.join("seen")).unwrap(), "token\nviewer\n", "they were there for the start");
        assert_eq!(std::fs::read_dir(dir.join("start")).unwrap().count(), 0, "and aren't now");
    }

    #[tokio::test]
    async fn the_secrets_are_gone_after_a_start_that_was_abandoned() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("cancel");
        let p = faked(&dir, &inspected("exited", "running", "T1"), "true", "sleep 30");
        let m = machine(&dir);
        let mut wake = Box::pin(p.wake(&m));
        // until the start is under way, with the secrets on disk, then let it go
        for _ in 0..500 {
            if tokio::time::timeout(Duration::from_millis(10), &mut wake).await.is_ok() {
                panic!("it can't finish: start sleeps");
            }
            if dir.join("start/viewer").exists() && calls(&dir).contains("start c2-x\n") {
                break;
            }
        }
        assert!(dir.join("start/viewer").exists());
        drop(wake);
        assert_eq!(std::fs::read_dir(dir.join("start")).unwrap().count(), 0, "a dropped start left the secrets on disk");
        assert_eq!(std::fs::metadata(dir.join("start")).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn the_secrets_folder_is_private_whatever_the_umask_made_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("private").join("start");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[tokio::test]
    async fn a_box_this_broker_didnt_start_is_walled_on_first_use_and_once() {
        let dir = tmp("first-use");
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true");
        let m = machine(&dir);
        p.guard(&m).await.unwrap();
        p.guard(&m).await.unwrap();
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 1, "{}", calls(&dir));
        // its network came up again: a new one, so a new wall
        std::fs::write(dir.join("state"), inspected("running", "running", "T2")).unwrap();
        p.guard(&m).await.unwrap();
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 2);
    }

    #[tokio::test]
    async fn a_guard_that_cant_wall_stops_the_box() {
        let dir = tmp("guard-fail");
        let p = faked(&dir, &inspected("running", "running", "T1"), "echo 'OCI runtime exec failed: exec: \"python3\": executable file not found in $PATH' >&2; exit 127", "true");
        let e = p.guard(&machine(&dir)).await.unwrap_err();
        assert_eq!(e.code(), "unwalled", "a missing python3 isn't a gone computer");
        assert!(calls(&dir).contains("stop -t 0 c2-x"), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn a_box_whose_network_went_is_stopped() {
        let dir = tmp("net-gone");
        let p = faked(&dir, &inspected("running", "exited", "T1"), "true", "true");
        assert_eq!(p.guard(&machine(&dir)).await.unwrap_err().code(), "unwalled");
        assert!(calls(&dir).contains("stop -t 0 c2-x"), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn every_wake_that_cant_wall_stops_the_box() {
        for status in ["running", "paused"] {
            let dir = tmp(status);
            let p = faked(&dir, &inspected(status, "running", "T1"), "echo no >&2; exit 1", "true");
            assert_eq!(p.wake(&machine(&dir)).await.unwrap_err().code(), "unwalled", "{status}");
            assert!(calls(&dir).contains("stop -t 0 c2-x"), "{status}: {}", calls(&dir));
        }
    }

    #[tokio::test]
    async fn a_paused_computer_wakes_without_a_start() {
        let dir = tmp("pause");
        let p = faked(&dir, &inspected("paused", "running", "T1"), "true", "true");
        assert_eq!(p.wake(&machine(&dir)).await.unwrap(), Woke::Resumed);
        assert!(calls(&dir).contains("unpause c2-x") && !calls(&dir).contains("start c2-x"), "{}", calls(&dir));
        assert!(at(&dir, WALL_CALL) < at(&dir, "unpause c2-x"), "the wall comes first here too: {}", calls(&dir));
    }

    #[tokio::test]
    async fn a_failed_second_create_leaves_no_holder() {
        let dir = tmp("create");
        let body = "case \"$1 $2 $3\" in version*|image*|network*) ;; 'create --name c2-'*-wall) ;; create*) echo nope >&2; exit 1;; esac";
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, body));
        assert!(p.create(&Spec { id: "cmp_1".into(), dir: dir.clone() }).await.is_err());
        assert!(calls(&dir).contains("-wall") && calls(&dir).lines().any(|l| l.starts_with("rm -f -v") && l.ends_with("-wall")), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn a_planted_link_gets_no_token_written_through_it() {
        let dir = tmp("link");
        let start = dir.join("start");
        std::fs::create_dir_all(&start).unwrap();
        let victim = dir.join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(&victim, start.join("token")).unwrap();
        stage_secrets(&start, &machine(&dir)).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(!std::fs::symlink_metadata(start.join("token")).unwrap().file_type().is_symlink());
    }

    #[tokio::test]
    async fn one_look_serves_a_request_and_the_next_request_looks_again() {
        let dir = tmp("ports");
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true");
        let screen = Screen { display: ":2".into(), viewer_port: 6902, folder: "agents/a".into(), profile: None };
        p.guard(&machine(&dir)).await.unwrap();
        let inspects = || calls(&dir).matches("inspect").count();
        let before = inspects();
        let (_, m) = p.look(&machine(&dir)).await.unwrap();
        p.guard(&m).await.unwrap();
        p.guest(&m).await.unwrap();
        p.guest(&m).await.unwrap();
        assert!(p.viewer(&m, &screen).await.unwrap().is_some());
        assert_eq!(inspects(), before + 1, "a request reads the container once");
        p.look(&machine(&dir)).await.unwrap();
        assert_eq!(inspects(), before + 2, "and each request reads it afresh: a cached port could be another computer's");
    }

    #[test]
    fn dockers_path_is_worked_out_once() {
        assert!(std::ptr::eq(docker_path(), docker_path()));
        assert!(std::env::split_paths(docker_path()).any(|d| d == Path::new("/usr/local/bin")));
    }

    #[test]
    fn what_was_seen_is_never_saved() {
        let dir = PathBuf::from("/tmp/x");
        let m = Machine { seen: Some(Seen::default()), ..machine(&dir) };
        let back: Machine = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.seen, None);
    }

    #[test]
    fn this_macs_addresses_are_read_from_ifconfig() {
        let out = "lo0: flags\n\tinet 127.0.0.1 netmask 0xff000000\nen0: flags\n\tinet6 fe80::1%en0\n\tinet 203.0.113.9 netmask 0xffffff00 broadcast 203.0.113.255\n";
        assert_eq!(parse_inets(out), ["203.0.113.9"]);
    }

    #[test]
    fn a_daemon_that_is_down_isnt_a_gone_computer() {
        assert_eq!(classify(Some(1), "Cannot connect to the Docker daemon", "i"), Problem::ProviderDown("Docker".into()));
        assert_eq!(classify(Some(1), "Error: No such container: x", "i"), Problem::Gone);
        assert_ne!(classify(Some(127), "exec: \"python3\": executable file not found in $PATH", "i"), Problem::Gone);
        assert_eq!(classify(Some(1), "Unable to find image 'i' locally", "i"), Problem::ImageMissing("i".into()));
    }
}
