//! The local Docker provider: one container per computer, driven through the `docker` CLI.
//!
//! - **Ports** are published on loopback only, from a range the Bot's shell is kept off
//!   ([`crate::PORT_FIRST`]). A `docker start` can pick new ones, so they are read back each wake.
//! - **Secrets** never go on an argument list, where any process's `ps` reads them: the container
//!   gets file paths. The files are put in a folder only it mounts just before it starts, and
//!   taken away once it answers, because the model's commands run as the same user as the
//!   daemons. A file that's gone can't be read.
//! - **Walled off from this Mac.** On Docker Desktop a container reaches the Mac's loopback
//!   through the host gateway, broker included. So each start puts blackhole routes in the
//!   container's network, as root, from outside it: the private ranges. The box keeps no
//!   capability to take them out. If that fails the computer doesn't wake.
//! - **Sleep** is `docker pause`: it comes back at once with everything open. Deep sleep is
//!   `docker stop`: the memory is free, and the files stay on the volumes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::guest::BoxGuest;
use crate::{
    random_hex, CommandOutput, Input, Machine, Power, Problem, Provider, ProviderKind, Reach, Result, Screen, Screenshot, Secret, Settled, Sleep, Spec, Viewer, WaitFor, DOCKER_PORT_LAST, PORT_FIRST,
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

/// Where Docker Desktop puts its CLI: an app started from Finder has a bare `PATH`.
const DOCKER_DIRS: [&str; 4] = ["/usr/local/bin", "/opt/homebrew/bin", "/Applications/Docker.app/Contents/Resources/bin", "~/.docker/bin"];

/// Adds blackhole routes over netlink, with nothing but python3 in the image: the private
/// ranges, which hold the host gateway (`192.168.65.x`, the way to the Mac's loopback). The
/// container's own subnet stays, being more specific: its bridge gateway is where published
/// ports come in from, and a reply has to get back there.
const WALL: &str = r#"
import socket, struct
s = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)
def blackhole(dst, plen):
    body = struct.pack('=BBBBBBBBI', 2, plen, 0, 0, 254, 4, 0, 6, 0) + struct.pack('=HH', 8, 1) + socket.inet_aton(dst)
    s.send(struct.pack('=IHHII', 16 + len(body), 24, 0x1 | 0x4 | 0x400 | 0x100, 1, 0) + body)
    err = struct.unpack('=i', s.recv(4096)[16:20])[0]
    if err:
        raise OSError(-err, dst)
for dst, plen in [('10.0.0.0', 8), ('172.16.0.0', 12), ('192.168.0.0', 16), ('100.64.0.0', 10), ('169.254.0.0', 16)]:
    blackhole(dst, plen)
"#;

/// The local Docker computers.
pub struct DockerProvider {
    image: String,
    prefix: String,
    /// the program, when it isn't found the usual way (tests)
    program: Option<PathBuf>,
    guests: Mutex<HashMap<String, Arc<BoxGuest>>>,
}

impl DockerProvider {
    pub fn new(image: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self { image: image.into(), prefix: prefix.into(), program: None, guests: Mutex::new(HashMap::new()) }
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
        Inspected::parse(&self.docker(&["inspect", "--type", "container", name], None).await?)
    }

    fn forget(&self, name: &str) {
        self.guests.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
    }

    /// The running computer's services: a sleeping one has none.
    async fn guest(&self, machine: &Machine) -> Result<Arc<BoxGuest>> {
        let name = container(machine)?;
        if let Some(g) = self.guests.lock().unwrap_or_else(|e| e.into_inner()).get(name) {
            return Ok(g.clone());
        }
        let found = self.inspect(name).await?;
        if found.status != "running" {
            return Err(Problem::Asleep);
        }
        let guest = Arc::new(connect(&found, machine)?);
        self.guests.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_owned(), guest.clone());
        Ok(guest)
    }

    /// `docker start`, walled off and answering. The secrets are there only for the start.
    async fn start(&self, machine: &Machine, name: &str) -> Result<()> {
        let dir = secret_dir(&machine.dir);
        stage_secrets(&dir, machine)?;
        let up = self.start_staged(machine, name).await;
        clear_secrets(&dir);
        up
    }

    async fn start_staged(&self, machine: &Machine, name: &str) -> Result<()> {
        self.docker(&["start", name], None).await?;
        if let Err(e) = self.wall(name).await {
            let _ = self.docker(&["stop", "-t", "0", name], None).await;
            return Err(e);
        }
        let inspected = self.inspect(name).await?;
        let plain = connect(&inspected, machine)?;
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

    /// The routes that keep the container off this Mac. Safe to repeat.
    async fn wall(&self, name: &str) -> Result<()> {
        self.docker(&["exec", "--user", "0", "--privileged", name, "python3", "-c", WALL], None).await.map(|_| ()).map_err(|e| match e {
            Problem::Failed(why) => Problem::Unwalled(why),
            other => other,
        })
    }
}

fn connect(found: &Inspected, machine: &Machine) -> Result<BoxGuest> {
    let port = |p: u16| found.published(p).ok_or_else(|| Problem::Failed(format!("port {p} isn't published on loopback")));
    Ok(BoxGuest::connect(&format!("http://{}", port(EXEC_PORT)?), &format!("http://{}", port(HOST_PORT)?), machine.token.clone()))
}

/// The `docker create` arguments of a new computer: no secret among them.
fn create_args(image: &str, name: &str, network: &str, id: &str, secrets: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["create", "--name", name, "--network", network, "--label"].map(str::to_owned).to_vec();
    args.push(format!("{LABEL}=1"));
    args.push("--label".to_owned());
    args.push(format!("{LABEL}.id={id}"));
    for (source, target) in [(format!("{name}-workspace"), "/workspace"), (format!("{name}-home"), "/home/box")] {
        args.push("--mount".to_owned());
        args.push(format!("type=volume,source={source},target={target}"));
    }
    args.push("--mount".to_owned());
    args.push(format!("type=bind,source={},target={SECRET_MOUNT}", secrets.display()));
    // Chromium needs more than Docker's 64 MiB; no setuid program may lift the box's user
    for a in ["--shm-size", "1g", "--memory", "4g", "--pids-limit", "2048", "--security-opt", "no-new-privileges"] {
        args.push(a.to_owned());
    }
    for port in [EXEC_PORT, HOST_PORT].into_iter().chain(VIEWER_PORTS) {
        args.push("-p".to_owned());
        args.push(format!("127.0.0.1:{PORT_FIRST}-{DOCKER_PORT_LAST}:{port}"));
    }
    for env in [
        format!("BOX_TOKEN_FILE={SECRET_MOUNT}/{TOKEN_FILE}"),
        format!("BOX_VIEWER_PASSWORD_FILE={SECRET_MOUNT}/{VIEWER_FILE}"),
        // the old image's name for the same password
        format!("BOX_VNC_PASSWORD_FILE={SECRET_MOUNT}/{VIEWER_FILE}"),
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

fn stage_secrets(dir: &Path, machine: &Machine) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(io_problem)?;
    for (file, secret) in [(TOKEN_FILE, &machine.token), (VIEWER_FILE, &machine.viewer_password)] {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(dir.join(file)).map_err(io_problem)?;
        f.write_all(secret.expose().as_bytes()).map_err(io_problem)?;
    }
    Ok(())
}

fn clear_secrets(dir: &Path) {
    for file in [TOKEN_FILE, VIEWER_FILE] {
        let _ = std::fs::remove_file(dir.join(file));
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
        std::fs::create_dir_all(&start).map_err(io_problem)?;
        let args = create_args(&self.image, &name, &self.network(), &spec.id, &start);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.docker(&refs, None).await?;
        Ok(Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name },
            image: Some(self.image.clone()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
        })
    }

    async fn power(&self, machine: &Machine) -> Result<Power> {
        match self.inspect(container(machine)?).await {
            Ok(found) => Ok(power_of(&found.status)),
            Err(Problem::Gone) => Ok(Power::Absent),
            Err(e) => Err(e),
        }
    }

    async fn wake(&self, machine: &Machine) -> Result<()> {
        let name = container(machine)?;
        self.forget(name);
        match self.inspect(name).await?.status.as_str() {
            "paused" => self.docker(&["unpause", name], None).await.map(|_| ()),
            "running" => self.wall(name).await,
            _ => self.start(machine, name).await,
        }
    }

    async fn sleep(&self, machine: &Machine, depth: Sleep) -> Result<()> {
        let name = container(machine)?;
        self.forget(name);
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
        let name = container(machine)?;
        self.forget(name);
        match self.docker(&["rm", "-f", "-v", name], None).await {
            Ok(_) | Err(Problem::Gone) => {}
            Err(e) => return Err(e),
        }
        for volume in [format!("{name}-workspace"), format!("{name}-home")] {
            match self.docker(&["volume", "rm", &volume], None).await {
                Ok(_) | Err(Problem::Gone) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    async fn screen_up(&self, machine: &Machine, bot: &str) -> Result<Screen> {
        self.guest(machine).await?.screen_up(bot).await
    }

    async fn screen_down(&self, machine: &Machine, bot: &str) -> Result<()> {
        self.guest(machine).await?.screen_down(bot).await
    }

    async fn commands_stay_on_their_screen(&self, machine: &Machine) -> bool {
        let Ok(guest) = self.guest(machine).await else { return false };
        guest.info().await.ok().and_then(|i| i.pointer("/capabilities/exec/per_screen_user").and_then(Value::as_bool)).unwrap_or(false)
    }

    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        let found = self.inspect(container(machine)?).await?;
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inspected {
    pub status: String,
    /// guest port → `127.0.0.1:port`
    pub ports: HashMap<u16, String>,
}

impl Inspected {
    pub fn parse(json: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(json).map_err(|e| Problem::Failed(format!("docker inspect: {e}")))?;
        let entry = v.get(0).ok_or(Problem::Gone)?;
        let status = entry.pointer("/State/Status").and_then(Value::as_str).unwrap_or_default().to_owned();
        let mut ports = HashMap::new();
        for (spec, bindings) in entry.pointer("/NetworkSettings/Ports").and_then(Value::as_object).into_iter().flatten() {
            let Some(guest) = spec.strip_suffix("/tcp").and_then(|p| p.parse::<u16>().ok()) else { continue };
            // loopback, or nothing: a binding anywhere else isn't ours to use
            let found = bindings.as_array().into_iter().flatten().find(|b| b.get("HostIp").and_then(Value::as_str) == Some("127.0.0.1"));
            if let Some(port) = found.and_then(|b| b.get("HostPort")).and_then(Value::as_str).filter(|p| !p.is_empty()) {
                ports.insert(guest, format!("127.0.0.1:{port}"));
            }
        }
        Ok(Self { status, ports })
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

/// `docker`: on `PATH`, else where Docker Desktop keeps it.
fn find_docker() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).chain(DOCKER_DIRS.map(home_dir)).map(|d| d.join("docker")).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("docker"))
}

struct Raw {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_docker(program: &Path, args: &[&str], stdin: Option<&[u8]>, patience: Duration) -> Result<Raw> {
    let mut command = Command::new(program);
    // its credential helpers sit next to it
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).chain(DOCKER_DIRS.map(home_dir)).collect();
    if let Ok(joined) = std::env::join_paths(dirs) {
        command.env("PATH", joined);
    }
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
    } else if said(&["No such container", "No such object", "No such volume", "not found"]) {
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
        }
    }

    #[test]
    fn create_publishes_on_loopback_in_the_range_with_no_secret_on_the_command_line() {
        let args = create_args("img:1", "c2-x", "c2-net", "cmp_1", Path::new("/d/start")).join(" ");
        for port in [1337, 1340, 6901, 6902, 6905] {
            assert!(args.contains(&format!("-p 127.0.0.1:47000-47199:{port}")), "{args}");
        }
        assert!(!args.contains("0.0.0.0"), "{args}");
        assert!(args.contains("BOX_TOKEN_FILE=/run/buwiz-computer/token"), "{args}");
        assert!(args.contains("BOX_VIEWER_PASSWORD_FILE=/run/buwiz-computer/viewer"), "{args}");
        assert!(!args.contains("BOX_TOKEN=") && !args.contains("BOX_VIEWER_PASSWORD="), "{args}");
        assert!(args.contains("no-new-privileges") && args.contains("--pids-limit 2048"), "{args}");
    }

    #[test]
    fn only_a_loopback_binding_counts() {
        let json = r#"[{"State":{"Status":"running"},"NetworkSettings":{"Ports":{
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

    #[tokio::test]
    async fn the_secrets_are_gone_after_a_start_that_failed() {
        let dir = tmp("start");
        let program = fake_docker(&dir, "case \"$1\" in inspect) echo '[{\"State\":{\"Status\":\"exited\"}}]';; start) ls \"$(dirname \"$0\")/start\" >> \"$(dirname \"$0\")/seen\"; echo 'boom' >&2; exit 1;; esac");
        let p = DockerProvider::new("img:1", "c2").with_program(program);
        let m = machine(&dir);
        assert_eq!(p.wake(&m).await.unwrap_err(), Problem::Failed("boom".into()));
        assert_eq!(std::fs::read_to_string(dir.join("seen")).unwrap(), "token\nviewer\n", "they were there for the start");
        assert_eq!(std::fs::read_dir(dir.join("start")).unwrap().count(), 0, "and aren't now");
    }

    #[tokio::test]
    async fn a_computer_that_cant_be_walled_off_is_stopped() {
        let dir = tmp("wall");
        let program = fake_docker(&dir, "case \"$1\" in inspect) echo '[{\"State\":{\"Status\":\"exited\"}}]';; exec) echo 'no python3' >&2; exit 1;; esac");
        let p = DockerProvider::new("img:1", "c2").with_program(program);
        let e = p.wake(&machine(&dir)).await.unwrap_err();
        assert_eq!(e.code(), "unwalled");
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        assert!(calls.contains("stop -t 0 c2-x"), "{calls}");
    }

    #[tokio::test]
    async fn a_paused_computer_wakes_without_a_start() {
        let dir = tmp("pause");
        let program = fake_docker(&dir, "case \"$1\" in inspect) echo '[{\"State\":{\"Status\":\"paused\"}}]';; esac");
        let p = DockerProvider::new("img:1", "c2").with_program(program);
        p.wake(&machine(&dir)).await.unwrap();
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        assert!(calls.contains("unpause c2-x") && !calls.contains("start c2-x"), "{calls}");
    }

    #[test]
    fn a_daemon_that_is_down_isnt_a_gone_computer() {
        assert_eq!(classify(Some(1), "Cannot connect to the Docker daemon", "i"), Problem::ProviderDown("Docker".into()));
        assert_eq!(classify(Some(1), "Error: No such container: x", "i"), Problem::Gone);
        assert_eq!(classify(Some(1), "Unable to find image 'i' locally", "i"), Problem::ImageMissing("i".into()));
    }
}
