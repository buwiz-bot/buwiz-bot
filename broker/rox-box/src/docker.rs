//! The local Docker provider: one container per box, driven through the `docker` CLI (the
//! same way opengrok-server does it, so the `grok-box` image and its env serve both
//! products). Ports are published on 127.0.0.1 only; `/workspace` and the desktop home live
//! on named volumes so `recreate` keeps the Bot's files and `destroy` removes them.
//!
//! Transcribed from opengrok-server `crates/opengrok-box/src/docker.rs`.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::guest::{Guest, png_dimensions};
use crate::unavailable::Unavailability;
use crate::{
    BoxError, BoxResult, CommandOutput, Computer, CreateBox, EgressTunnel, GUEST_INFO_PATIENCE,
    ImageStatus, Screen, Screenshot, StartedCommand, egress_from_info, no_screen, not_supported,
};

/// A box without a desktop: enough for `run`, files and ports.
// opengrok-server crates/opengrok-box/src/docker.rs `DEFAULT_IMAGE`.
pub const DEFAULT_IMAGE: &str = "debian:stable-slim";
/// The guest ports a box may `expose_port`; published on loopback at create time, because
/// Docker cannot publish a port after the container exists.
// opengrok-server crates/opengrok-box/src/docker.rs `PUBLISHED_PORTS`.
pub const PUBLISHED_PORTS: &[u16] = &[3000, 5173, 8000, 8080];
/// Desktop images also publish noVNC and the two guest services (box-exec, box-host).
// opengrok-server crates/opengrok-box/src/docker.rs `DESKTOP_PORTS`.
pub const DESKTOP_PORTS: &[u16] = &[NOVNC_PORT, GUEST_EXEC_PORT, GUEST_HOST_PORT];
pub const NOVNC_PORT: u16 = 6080;
pub const GUEST_EXEC_PORT: u16 = 1337;
pub const GUEST_HOST_PORT: u16 = 1340;
/// The egress tunnel's guest port. Its helpers on 8791/8792 are guest-internal and are never
/// published.
// opengrok-server crates/opengrok-box/src/docker.rs `EGRESS_TUNNEL_PORT`.
pub const EGRESS_TUNNEL_PORT: u16 = 8790;

/// Env the broker reads (the container's own env is the image's contract, see `create_args`).
pub const IMAGE_ENV: &str = "ROX_BOX_IMAGE";
pub const DESKTOP_ENV: &str = "ROX_BOX_DESKTOP";
pub const EGRESS_ENV: &str = "ROX_BOX_EGRESS_TUNNEL";

const LABEL_OWNER: &str = "bot.buwiz.rox-box";
const LABEL_VOLUME_WS: &str = "bot.buwiz.rox-box.volume.workspace";
const LABEL_VOLUME_HOME: &str = "bot.buwiz.rox-box.volume.home";
const WORKSPACE_MOUNT: &str = "/workspace";
const HOME_MOUNT: &str = "/home/box";
/// Where `start` keeps a background command's output inside the box.
const SPOOL_DIR: &str = "/tmp/rox-box";
const WATCH_SEPARATOR: &str = "\n--rox-box-separator--\n";
const MIN_TIMEOUT: u64 = 1;
const MAX_TIMEOUT: u64 = 600;
const DEFAULT_TIMEOUT: u64 = 60;
/// A command's captured output is cut here; the flag says so.
const MAX_OUTPUT: usize = 64 * 1024;
/// `docker` itself (not the command inside) gets this long; a hung daemon is "unreachable".
const DOCKER_PATIENCE: Duration = Duration::from_secs(120);
/// `docker pull` may take minutes.
const PULL_PATIENCE: Duration = Duration::from_secs(1800);

/// The local Docker computer.
pub struct DockerComputer {
    image: String,
    desktop: bool,
    egress_tunnel: bool,
    inspected: Mutex<HashMap<String, Inspected>>,
    generations: Mutex<HashMap<String, u64>>,
}

impl DockerComputer {
    /// A provider for `image`; desktop services are assumed when the image is a `grok-box`.
    pub fn new(image: impl Into<String>) -> Self {
        let image = image.into();
        let desktop = image_is_desktop(&image);
        Self {
            image,
            desktop,
            egress_tunnel: false,
            inspected: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
        }
    }

    /// From `ROX_BOX_IMAGE`, `ROX_BOX_DESKTOP=1`, `ROX_BOX_EGRESS_TUNNEL=1`.
    pub fn from_env() -> Self {
        let image = std::env::var(IMAGE_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_IMAGE.to_string());
        let mut this = Self::new(image);
        if env_flag(DESKTOP_ENV) {
            this.desktop = true;
        }
        this.egress_tunnel = env_flag(EGRESS_ENV);
        this
    }

    pub fn with_desktop(mut self, desktop: bool) -> Self {
        self.desktop = desktop;
        self
    }

    pub fn with_egress_tunnel(mut self, enabled: bool) -> Self {
        self.egress_tunnel = enabled;
        self
    }

    pub fn image(&self) -> &str {
        &self.image
    }

    pub fn is_desktop(&self) -> bool {
        self.desktop
    }

    /// Is Docker there and answering? The reason when not.
    pub async fn availability(&self) -> Result<(), Unavailability> {
        match docker(
            &["version", "--format", "{{.Server.Version}}"],
            None,
            DOCKER_PATIENCE,
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(BoxError::Unavailable(message)) => Err(unavailability_from_message(&message)),
            Err(other) => Err(Unavailability::NotRunning(other.to_string())),
        }
    }

    /// `docker run` arguments for a new box: the image's env contract (`BOX_*`), loopback
    /// port publication, the data volumes, and the labels that let us find them again.
    fn create_args(
        &self,
        spec: &CreateBox,
        secrets: &Secrets,
        volumes: &BoxVolumes,
    ) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "run".into(),
            "-d".into(),
            "--label".into(),
            format!("{LABEL_OWNER}=1"),
        ];
        if let Some(name) = spec
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            args.push("--label".into());
            args.push(format!("{LABEL_OWNER}.name={name}"));
        }
        args.push("--label".into());
        args.push(format!("{LABEL_VOLUME_WS}={}", volumes.workspace));
        args.push("-v".into());
        args.push(format!("{}:{WORKSPACE_MOUNT}", volumes.workspace));
        args.push("-w".into());
        args.push(WORKSPACE_MOUNT.into());
        if self.desktop {
            args.push("--label".into());
            args.push(format!("{LABEL_VOLUME_HOME}={}", volumes.home));
            args.push("-v".into());
            args.push(format!("{}:{HOME_MOUNT}", volumes.home));
            // Chrome inside the box needs a bigger /dev/shm than Docker's 64 MiB default.
            args.push("--shm-size".into());
            args.push("1g".into());
        }
        let mut ports: Vec<u16> = PUBLISHED_PORTS.to_vec();
        if self.desktop {
            ports.extend_from_slice(DESKTOP_PORTS);
        }
        if spec.egress_tunnel {
            ports.push(EGRESS_TUNNEL_PORT);
        }
        for port in ports {
            args.push("-p".into());
            // `127.0.0.1::PORT`: loopback only, host port chosen by Docker.
            args.push(format!("127.0.0.1::{port}"));
        }
        // The image's env contract: the same names opengrok-server sets, so one image serves both.
        args.push("-e".into());
        args.push(format!("BOX_TOKEN={}", secrets.token));
        if self.desktop {
            for kv in [
                "BOX_DESKTOP=1",
                "BOX_DESKTOP_REQUIRED=1",
                "BOX_CHROME=0",
                "BOX_ALLOW_INSECURE_DEV=1",
            ] {
                args.push("-e".into());
                args.push(kv.into());
            }
            args.push("-e".into());
            args.push(format!("BOX_VNC_PASSWORD={}", secrets.vnc_password));
        }
        if spec.egress_tunnel {
            args.push("-e".into());
            args.push("BOX_EGRESS_TUNNEL=1".into());
            let bearer = spec
                .egress_tunnel_bearer
                .clone()
                .unwrap_or_else(|| secrets.egress_bearer.clone());
            args.push("-e".into());
            args.push(format!("BOX_EGRESS_TUNNEL_BEARER={bearer}"));
        }
        args.push(self.image.clone());
        if !self.desktop {
            // A plain image has no long-lived entrypoint: keep it up until stopped (or the TTL).
            args.push("sh".into());
            args.push("-c".into());
            args.push(match spec.ttl_seconds {
                Some(ttl) => format!("sleep {ttl}"),
                None => "sleep infinity".into(),
            });
        }
        args
    }

    async fn inspect(&self, box_id: &str) -> BoxResult<Inspected> {
        let output = docker(
            &["inspect", "--type", "container", box_id],
            None,
            DOCKER_PATIENCE,
        )
        .await?;
        let inspected = Inspected::parse(&output.stdout)?;
        if let Ok(mut cache) = self.inspected.lock() {
            cache.insert(box_id.to_string(), inspected.clone());
        }
        Ok(inspected)
    }

    /// The last inspection if we have one, else a fresh one. Used on the hot paths (egress
    /// probe, screenshot) where the ports and secrets cannot change while the container lives.
    async fn inspected(&self, box_id: &str) -> BoxResult<Inspected> {
        let cached = self
            .inspected
            .lock()
            .ok()
            .and_then(|cache| cache.get(box_id).cloned());
        match cached {
            Some(i) => Ok(i),
            None => self.inspect(box_id).await,
        }
    }

    /// A stop, start, or removal: the next guest is a new one.
    fn forget(&self, box_id: &str) {
        if let Ok(mut cache) = self.inspected.lock() {
            cache.remove(box_id);
        }
        if let Ok(mut generations) = self.generations.lock() {
            let g = generations.entry(box_id.to_string()).or_insert(0);
            *g += 1;
        }
    }

    fn guest(&self, inspected: &Inspected) -> BoxResult<Guest> {
        let exec = inspected.published(GUEST_EXEC_PORT).ok_or_else(no_screen)?;
        let host = inspected.published(GUEST_HOST_PORT).ok_or_else(no_screen)?;
        let token = inspected
            .env
            .get("BOX_TOKEN")
            .ok_or_else(|| BoxError::Secret("the box has no BOX_TOKEN".into()))?;
        Ok(Guest::connect(
            &format!("http://{exec}"),
            &format!("http://{host}"),
            token,
        ))
    }

    async fn local_image_id(&self) -> BoxResult<Option<String>> {
        match docker(
            &["image", "inspect", "--format", "{{.Id}}", &self.image],
            None,
            DOCKER_PATIENCE,
        )
        .await
        {
            Ok(out) => Ok(Some(out.stdout.trim().to_string()).filter(|s| !s.is_empty())),
            Err(BoxError::NoSuchBox) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn create_with_volumes(
        &self,
        spec: &CreateBox,
        volumes: &BoxVolumes,
    ) -> BoxResult<String> {
        let secrets = Secrets::fresh()?;
        let args = self.create_args(spec, &secrets, volumes);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = docker(&refs, None, DOCKER_PATIENCE).await?;
        let id = output.stdout.trim().to_string();
        if id.is_empty() {
            return Err(BoxError::Refused {
                status: 500,
                body: "docker run printed no container id".into(),
            });
        }
        // The short id is what `docker ps` shows and what every other call accepts.
        let short: String = id.chars().take(12).collect();
        self.forget(&short);
        Ok(short)
    }
}

#[async_trait]
impl Computer for DockerComputer {
    async fn create(&self, spec: CreateBox) -> BoxResult<String> {
        let volumes = BoxVolumes::fresh()?;
        self.create_with_volumes(&spec, &volumes).await
    }

    async fn run(
        &self,
        box_id: &str,
        command: &str,
        timeout_seconds: Option<u64>,
    ) -> BoxResult<CommandOutput> {
        let seconds = clamp_timeout(timeout_seconds);
        let args = ["exec", box_id, "sh", "-c", command];
        match docker_raw(&args, None, Duration::from_secs(seconds)).await {
            Ok(raw) => {
                // A non-zero exit is the command's, unless the daemon itself refused (the box is
                // gone, the daemon is down): that is ours to report, not the command's output.
                if raw.status != Some(0)
                    && let Some(refusal) = daemon_refusal(&String::from_utf8_lossy(&raw.stderr))
                {
                    return Err(refusal);
                }
                Ok(raw.into_command_output())
            }
            Err(RawError::TimedOut) => Ok(CommandOutput {
                exit_code: 124,
                timed_out: true,
                ..CommandOutput::default()
            }),
            Err(RawError::Box(e)) => Err(e),
        }
    }

    async fn start(&self, box_id: &str, command: &str) -> BoxResult<StartedCommand> {
        let process_id = Secrets::hex(8)?;
        let script = format!("{SPOOL_DIR}/{process_id}.sh");
        self.write_file_bytes(box_id, &script, command.as_bytes())
            .await?;
        let launcher = format!(
            "nohup sh -c 'sh {script} > {SPOOL_DIR}/{process_id}.out 2> {SPOOL_DIR}/{process_id}.err; echo $? > {SPOOL_DIR}/{process_id}.code' > /dev/null 2>&1 &"
        );
        docker(
            &["exec", "-d", box_id, "sh", "-c", &launcher],
            None,
            DOCKER_PATIENCE,
        )
        .await?;
        Ok(StartedCommand {
            process_id,
            running: true,
            ..StartedCommand::default()
        })
    }

    async fn watch(&self, box_id: &str, process_id: &str) -> BoxResult<StartedCommand> {
        if process_id.is_empty() || !process_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(BoxError::Refused {
                status: 400,
                body: "not a process id".into(),
            });
        }
        let script = format!(
            "cat {SPOOL_DIR}/{process_id}.out 2>/dev/null; printf '%s' '{WATCH_SEPARATOR}'; cat {SPOOL_DIR}/{process_id}.err 2>/dev/null; printf '%s' '{WATCH_SEPARATOR}'; cat {SPOOL_DIR}/{process_id}.code 2>/dev/null"
        );
        let output = docker(
            &["exec", box_id, "sh", "-c", &script],
            None,
            DOCKER_PATIENCE,
        )
        .await?;
        Ok(parse_watch(process_id, &output.stdout))
    }

    async fn read_file(&self, box_id: &str, path: &str) -> BoxResult<String> {
        let output = docker(&["exec", box_id, "cat", "--", path], None, DOCKER_PATIENCE).await?;
        Ok(output.stdout)
    }

    /// The bytes go in on stdin: the path is a shell argument, never interpolated.
    async fn write_file_bytes(&self, box_id: &str, path: &str, content: &[u8]) -> BoxResult<()> {
        let script = r#"mkdir -p "$(dirname -- "$1")" && cat > "$1""#;
        docker(
            &["exec", "-i", box_id, "sh", "-c", script, "sh", path],
            Some(content),
            DOCKER_PATIENCE,
        )
        .await?;
        Ok(())
    }

    async fn expose_port(&self, box_id: &str, port: u16) -> BoxResult<String> {
        if !PUBLISHED_PORTS.contains(&port) {
            return Err(BoxError::Refused {
                status: 409,
                body: format!(
                    "port {port} is not published; a local box publishes {}",
                    join_ports(PUBLISHED_PORTS)
                ),
            });
        }
        let inspected = self.inspect(box_id).await?;
        let binding = inspected.published(port).ok_or_else(|| BoxError::Refused {
            status: 409,
            body: format!("port {port} has no loopback binding"),
        })?;
        Ok(format!("http://{binding}"))
    }

    async fn stop(&self, box_id: &str) -> BoxResult<()> {
        docker(&["stop", box_id], None, DOCKER_PATIENCE).await?;
        self.forget(box_id);
        Ok(())
    }

    async fn resume(&self, box_id: &str) -> BoxResult<()> {
        docker(&["start", box_id], None, DOCKER_PATIENCE).await?;
        self.forget(box_id);
        Ok(())
    }

    async fn destroy(&self, box_id: &str) -> BoxResult<()> {
        let volumes = match self.inspect(box_id).await {
            Ok(i) => i.volumes(),
            Err(BoxError::NoSuchBox) => BoxVolumes::default(),
            Err(e) => return Err(e),
        };
        match docker(&["rm", "-f", "-v", box_id], None, DOCKER_PATIENCE).await {
            Ok(_) | Err(BoxError::NoSuchBox) => {}
            Err(e) => return Err(e),
        }
        self.forget(box_id);
        for name in volumes.names() {
            // Already gone is fine; anything else (in use by another container) is reported.
            match docker(&["volume", "rm", &name], None, DOCKER_PATIENCE).await {
                Ok(_) | Err(BoxError::NoSuchBox) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    async fn discard(&self, box_id: &str) -> BoxResult<()> {
        docker(&["rm", "-f", box_id], None, DOCKER_PATIENCE).await?;
        self.forget(box_id);
        Ok(())
    }

    async fn state(&self, box_id: &str) -> BoxResult<String> {
        match self.inspect(box_id).await {
            Ok(i) => Ok(i.state),
            Err(BoxError::NoSuchBox) => Ok("absent".into()),
            Err(e) => Err(e),
        }
    }

    async fn has_screen(&self, box_id: &str) -> BoxResult<bool> {
        let inspected = self.inspected(box_id).await?;
        Ok(inspected.has_screen())
    }

    async fn screen_url(&self, box_id: &str, screen: &Screen) -> BoxResult<String> {
        if let Screen::Own(_) = screen {
            return Err(not_supported("open a Bot's own screen on a local box"));
        }
        let inspected = self.inspected(box_id).await?;
        let binding = inspected.published(NOVNC_PORT).ok_or_else(no_screen)?;
        let password = inspected
            .env
            .get("BOX_VNC_PASSWORD")
            .map(String::as_str)
            .unwrap_or("");
        Ok(vnc_page_url(&binding, password))
    }

    async fn screenshot(&self, box_id: &str, screen: &Screen) -> BoxResult<Screenshot> {
        if let Screen::Own(_) = screen {
            return Err(not_supported(
                "photograph a Bot's own screen on a local box",
            ));
        }
        let inspected = self.inspected(box_id).await?;
        if !inspected.has_screen() {
            return Err(no_screen());
        }
        let png = self.guest(&inspected)?.screenshot_png().await?;
        let (width, height) = png_dimensions(&png).ok_or_else(|| BoxError::Refused {
            status: 502,
            body: "the guest's screenshot is not a PNG".into(),
        })?;
        Ok(Screenshot { png, width, height })
    }

    async fn image_status(&self, box_id: Option<&str>) -> BoxResult<ImageStatus> {
        let local_image_id = self.local_image_id().await?;
        let running_image_id = match box_id {
            Some(id) => match self.inspect(id).await {
                Ok(i) => i.image_id,
                Err(BoxError::NoSuchBox) => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        Ok(image_status(&self.image, running_image_id, local_image_id))
    }

    async fn pull_latest(&self) -> BoxResult<ImageStatus> {
        docker(&["pull", "--quiet", &self.image], None, PULL_PATIENCE).await?;
        self.image_status(None).await
    }

    async fn recreate(&self, box_id: &str, spec: CreateBox) -> BoxResult<String> {
        let volumes = self.inspect(box_id).await?.volumes();
        self.discard(box_id).await?;
        let volumes = if volumes.workspace.is_empty() {
            BoxVolumes::fresh()?
        } else {
            volumes
        };
        self.create_with_volumes(&spec, &volumes).await
    }

    async fn egress_intent(&self, box_id: &str) -> BoxResult<Option<bool>> {
        let inspected = self.inspect(box_id).await?;
        Ok(Some(
            inspected
                .env
                .get("BOX_EGRESS_TUNNEL")
                .is_some_and(|v| v == "1"),
        ))
    }

    async fn egress_probe(&self, box_id: &str) -> (Option<EgressTunnel>, bool) {
        let Ok(inspected) = self.inspected(box_id).await else {
            return (None, false);
        };
        if !inspected.has_screen() || inspected.state != "running" {
            return (None, false);
        }
        let Ok(guest) = self.guest(&inspected) else {
            return (None, false);
        };
        match tokio::time::timeout(GUEST_INFO_PATIENCE, guest.info()).await {
            Ok(Ok(info)) => (egress_from_info(&info), false),
            Ok(Err(_)) => (None, false),
            Err(_) => (None, true),
        }
    }

    async fn generation(&self, box_id: &str) -> u64 {
        self.generations
            .lock()
            .ok()
            .and_then(|g| g.get(box_id).copied())
            .unwrap_or(0)
    }
}

/// What `docker inspect` told us about a container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspected {
    pub state: String,
    pub env: HashMap<String, String>,
    /// Guest port → `127.0.0.1:host_port`.
    pub ports: HashMap<u16, String>,
    pub labels: HashMap<String, String>,
    pub image_id: Option<String>,
}

#[derive(Deserialize)]
struct InspectEntry {
    #[serde(rename = "State", default)]
    state: InspectState,
    #[serde(rename = "Config", default)]
    config: InspectConfig,
    #[serde(rename = "NetworkSettings", default)]
    network: InspectNetwork,
    #[serde(rename = "Image", default)]
    image: String,
}

#[derive(Deserialize, Default)]
struct InspectState {
    #[serde(rename = "Status", default)]
    status: String,
}

#[derive(Deserialize, Default)]
struct InspectConfig {
    #[serde(rename = "Env", default)]
    env: Vec<String>,
    #[serde(rename = "Labels", default)]
    labels: HashMap<String, String>,
}

#[derive(Deserialize, Default)]
struct InspectNetwork {
    #[serde(rename = "Ports", default)]
    ports: HashMap<String, Option<Vec<PortBinding>>>,
}

#[derive(Deserialize)]
struct PortBinding {
    #[serde(rename = "HostIp", default)]
    host_ip: String,
    #[serde(rename = "HostPort", default)]
    host_port: String,
}

impl Inspected {
    /// From `docker inspect`'s JSON (an array; the first entry is the container).
    pub fn parse(json: &str) -> BoxResult<Self> {
        let entries: Vec<InspectEntry> =
            serde_json::from_str(json).map_err(|e| BoxError::Refused {
                status: 502,
                body: format!("docker inspect is not the JSON we know: {e}"),
            })?;
        let entry = entries.into_iter().next().ok_or(BoxError::NoSuchBox)?;
        let env = entry
            .config
            .env
            .iter()
            .filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();
        let mut ports = HashMap::new();
        for (spec, bindings) in entry.network.ports {
            let Some(guest) = spec
                .strip_suffix("/tcp")
                .and_then(|p| p.parse::<u16>().ok())
            else {
                continue;
            };
            let Some(bindings) = bindings else { continue };
            // Prefer the IPv4 loopback binding; `::1` would need brackets and a v6 browser.
            let binding = bindings
                .iter()
                .find(|b| b.host_ip == "127.0.0.1")
                .or_else(|| bindings.iter().find(|b| !b.host_ip.contains(':')));
            if let Some(b) = binding.filter(|b| !b.host_port.is_empty()) {
                let host = if b.host_ip.is_empty() || b.host_ip == "0.0.0.0" {
                    "127.0.0.1"
                } else {
                    b.host_ip.as_str()
                };
                ports.insert(guest, format!("{host}:{}", b.host_port));
            }
        }
        Ok(Self {
            state: entry.state.status,
            env,
            ports,
            labels: entry.config.labels,
            image_id: Some(entry.image).filter(|s| !s.is_empty()),
        })
    }

    /// `127.0.0.1:port` on this machine for a guest port, if it was published.
    pub fn published(&self, guest_port: u16) -> Option<String> {
        self.ports.get(&guest_port).cloned()
    }

    pub fn has_screen(&self) -> bool {
        self.env.get("BOX_DESKTOP").is_some_and(|v| v == "1")
            || self.ports.contains_key(&NOVNC_PORT)
    }

    pub fn volumes(&self) -> BoxVolumes {
        BoxVolumes {
            workspace: self
                .labels
                .get(LABEL_VOLUME_WS)
                .cloned()
                .unwrap_or_default(),
            home: self
                .labels
                .get(LABEL_VOLUME_HOME)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// The named volumes a box keeps its data on; `recreate` hands them to the next container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoxVolumes {
    pub workspace: String,
    pub home: String,
}

impl BoxVolumes {
    fn fresh() -> BoxResult<Self> {
        let suffix = Secrets::hex(6)?;
        Ok(Self {
            workspace: format!("rox-box-{suffix}-ws"),
            home: format!("rox-box-{suffix}-home"),
        })
    }

    fn names(&self) -> Vec<String> {
        [&self.workspace, &self.home]
            .into_iter()
            .filter(|n| !n.is_empty())
            .cloned()
            .collect()
    }
}

/// The secrets a container is born with.
struct Secrets {
    token: String,
    vnc_password: String,
    egress_bearer: String,
}

impl Secrets {
    fn fresh() -> BoxResult<Self> {
        Ok(Self {
            // grok-box wants a token of 16+ chars; "rox-" + 32 hex is well over.
            token: format!("rox-{}", Self::hex(16)?),
            // VNC passwords are 8 significant characters.
            vnc_password: Self::hex(4)?,
            egress_bearer: format!("rox-{}", Self::hex(16)?),
        })
    }

    /// `bytes` random bytes as lowercase hex, from the OS CSPRNG.
    fn hex(bytes: usize) -> BoxResult<String> {
        let mut buf = vec![0u8; bytes];
        getrandom::getrandom(&mut buf)
            .map_err(|e| BoxError::Secret(format!("no randomness: {e}")))?;
        Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
    }
}

/// The noVNC page for a published binding, autoconnecting with the box's password.
// opengrok-server crates/opengrok-box/src/docker.rs `vnc_page_url`.
pub fn vnc_page_url(binding: &str, password: &str) -> String {
    let mut url = format!("http://{binding}/vnc.html?autoconnect=true&resize=scale&reconnect=true");
    if !password.is_empty() {
        url.push_str("&password=");
        url.push_str(password);
    }
    url
}

/// A `grok-box` image is a desktop; anything else is headless unless `ROX_BOX_DESKTOP=1`.
// opengrok-server crates/opengrok-box/src/docker.rs `image_is_desktop`.
pub fn image_is_desktop(image: &str) -> bool {
    let name = image.rsplit('/').next().unwrap_or(image);
    let name = name.split(['@', ':']).next().unwrap_or(name);
    name == "grok-box" || name.starts_with("grok-box-")
}

/// Running image vs pulled image: `behind` only when both are known and differ.
// opengrok-server crates/opengrok-box/src/docker.rs `image_status`.
pub fn image_status(
    image: &str,
    running_image_id: Option<String>,
    local_image_id: Option<String>,
) -> ImageStatus {
    let behind = matches!((&running_image_id, &local_image_id), (Some(r), Some(l)) if r != l);
    ImageStatus {
        image: image.to_string(),
        local: local_image_id.is_some(),
        running_image_id,
        local_image_id,
        behind,
    }
}

/// `timeout_seconds` into the provider's range (1..=600, 60 when unsaid).
// opengrok-server crates/opengrok-box/src/docker.rs `clamp_timeout`.
pub fn clamp_timeout(timeout_seconds: Option<u64>) -> u64 {
    timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT)
        .clamp(MIN_TIMEOUT, MAX_TIMEOUT)
}

/// The three spool files back into a `StartedCommand`: no exit code yet means still running.
fn parse_watch(process_id: &str, combined: &str) -> StartedCommand {
    let mut parts = combined.splitn(3, WATCH_SEPARATOR);
    let stdout = parts.next().unwrap_or("").to_string();
    let stderr = parts.next().unwrap_or("").to_string();
    let exit_code = parts.next().and_then(|c| c.trim().parse::<i32>().ok());
    StartedCommand {
        process_id: process_id.to_string(),
        running: exit_code.is_none(),
        stdout,
        stderr,
        exit_code,
    }
}

fn join_ports(ports: &[u16]) -> String {
    ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
}

/// Docker's own words for "not here", mapped to the two reasons the pane distinguishes.
fn unavailability_from_message(message: &str) -> Unavailability {
    if message.contains("not installed") {
        Unavailability::NotInstalled
    } else {
        Unavailability::NotRunning(
            message
                .trim_start_matches("Docker is not running: ")
                .to_string(),
        )
    }
}

/// What a finished `docker` process said.
struct Raw {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Raw {
    fn into_command_output(self) -> CommandOutput {
        let (stdout, stdout_truncated) = cut(&self.stdout);
        let (stderr, stderr_truncated) = cut(&self.stderr);
        CommandOutput {
            exit_code: self.status.unwrap_or(-1),
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            timed_out: false,
        }
    }
}

enum RawError {
    TimedOut,
    Box(BoxError),
}

fn cut(bytes: &[u8]) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_OUTPUT {
        return (text.into_owned(), false);
    }
    let mut end = MAX_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// A finished `docker` call's text, or the typed reason it failed.
struct Finished {
    stdout: String,
}

/// Run `docker` with `args`, `stdin` piped in when given. A non-zero exit becomes the typed
/// error Docker's stderr implies: the container is gone, the daemon is down, or a refusal.
async fn docker(args: &[&str], stdin: Option<&[u8]>, patience: Duration) -> BoxResult<Finished> {
    let raw = match docker_raw(args, stdin, patience).await {
        Ok(raw) => raw,
        Err(RawError::TimedOut) => {
            return Err(BoxError::Unreachable(format!(
                "docker {} did not finish in {}s",
                args.first().unwrap_or(&""),
                patience.as_secs()
            )));
        }
        Err(RawError::Box(e)) => return Err(e),
    };
    let stderr = String::from_utf8_lossy(&raw.stderr).trim().to_string();
    match raw.status {
        Some(0) => Ok(Finished {
            stdout: String::from_utf8_lossy(&raw.stdout).into_owned(),
        }),
        status => Err(classify_failure(status, &stderr)),
    }
}

/// Docker's stderr into the error the caller can act on.
// opengrok-server crates/opengrok-box/src/docker.rs `docker` (the error mapping).
fn classify_failure(status: Option<i32>, stderr: &str) -> BoxError {
    if stderr.contains("No such container")
        || stderr.contains("No such object")
        || stderr.contains("No such image")
        || stderr.contains("No such volume")
    {
        BoxError::NoSuchBox
    } else if stderr.contains("Cannot connect to the Docker daemon")
        || stderr.contains("Is the docker daemon running")
        || stderr.contains("docker daemon is not running")
    {
        BoxError::Unavailable(format!("Docker is not running: {}", first_line(stderr)))
    } else {
        BoxError::Refused {
            status: 500,
            body: if stderr.is_empty() {
                format!(
                    "docker exited with {}",
                    status.map_or("a signal".to_string(), |s| s.to_string())
                )
            } else {
                stderr.to_string()
            },
        }
    }
}

/// Only the daemon's own refusals (not a failing command's stderr): gone, or down.
fn daemon_refusal(stderr: &str) -> Option<BoxError> {
    match classify_failure(Some(1), stderr) {
        e @ (BoxError::NoSuchBox | BoxError::Unavailable(_)) => Some(e),
        _ => None,
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

/// Spawn `docker`; a missing binary is "not installed", a hang past `patience` is a timeout.
async fn docker_raw(
    args: &[&str],
    stdin: Option<&[u8]>,
    patience: Duration,
) -> Result<Raw, RawError> {
    let mut command = Command::new("docker");
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| {
        RawError::Box(if e.kind() == std::io::ErrorKind::NotFound {
            BoxError::Unavailable("Docker is not installed: no `docker` on PATH".into())
        } else {
            BoxError::Unreachable(format!("could not run docker: {e}"))
        })
    })?;
    if let Some(bytes) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        let bytes = bytes.to_vec();
        tokio::spawn(async move {
            let _ = pipe.write_all(&bytes).await;
            let _ = pipe.shutdown().await;
        });
    }
    match tokio::time::timeout(patience, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(Raw {
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        }),
        Ok(Err(e)) => Err(RawError::Box(BoxError::Unreachable(format!(
            "docker did not finish: {e}"
        )))),
        Err(_) => Err(RawError::TimedOut),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const INSPECT: &str = r#"[{
      "Image": "sha256:abc",
      "State": {"Status": "running"},
      "Config": {
        "Env": ["BOX_TOKEN=rox-secret", "BOX_DESKTOP=1", "BOX_VNC_PASSWORD=pw12", "BOX_EGRESS_TUNNEL=1", "PATH=/usr/bin"],
        "Labels": {"bot.buwiz.rox-box.volume.workspace": "rox-box-aa-ws", "bot.buwiz.rox-box.volume.home": "rox-box-aa-home"}
      },
      "NetworkSettings": {"Ports": {
        "6080/tcp": [{"HostIp": "127.0.0.1", "HostPort": "49153"}],
        "1337/tcp": [{"HostIp": "::1", "HostPort": "49160"}, {"HostIp": "127.0.0.1", "HostPort": "49154"}],
        "3000/tcp": [{"HostIp": "0.0.0.0", "HostPort": "49155"}],
        "8791/tcp": null,
        "5173/udp": [{"HostIp": "127.0.0.1", "HostPort": "1"}]
      }}
    }]"#;

    #[test]
    fn inspect_parses_state_env_ports_and_volumes() {
        let i = Inspected::parse(INSPECT).unwrap();
        assert_eq!(i.state, "running");
        assert_eq!(
            i.env.get("BOX_TOKEN").map(String::as_str),
            Some("rox-secret")
        );
        assert_eq!(i.published(6080).as_deref(), Some("127.0.0.1:49153"));
        assert_eq!(
            i.published(1337).as_deref(),
            Some("127.0.0.1:49154"),
            "the v4 loopback binding wins"
        );
        assert_eq!(
            i.published(3000).as_deref(),
            Some("127.0.0.1:49155"),
            "0.0.0.0 is reached on loopback"
        );
        assert_eq!(i.published(8791), None, "unpublished ports are absent");
        assert_eq!(i.published(5173), None, "udp is not a thing we publish");
        assert!(i.has_screen());
        assert_eq!(
            i.volumes(),
            BoxVolumes {
                workspace: "rox-box-aa-ws".into(),
                home: "rox-box-aa-home".into()
            }
        );
        assert_eq!(i.image_id.as_deref(), Some("sha256:abc"));
        assert_eq!(Inspected::parse("[]"), Err(BoxError::NoSuchBox));
        assert!(matches!(
            Inspected::parse("nope"),
            Err(BoxError::Refused { status: 502, .. })
        ));
    }

    #[test]
    fn headless_inspect_has_no_screen() {
        let i = Inspected::parse(r#"[{"State":{"Status":"exited"},"Config":{"Env":["BOX_TOKEN=x"]},"NetworkSettings":{"Ports":{}}}]"#).unwrap();
        assert!(!i.has_screen());
        assert_eq!(i.state, "exited");
        assert_eq!(i.volumes(), BoxVolumes::default());
    }

    #[test]
    fn vnc_url_is_loopback_with_the_password() {
        assert_eq!(
            vnc_page_url("127.0.0.1:49153", "pw12"),
            "http://127.0.0.1:49153/vnc.html?autoconnect=true&resize=scale&reconnect=true&password=pw12"
        );
        assert_eq!(
            vnc_page_url("127.0.0.1:1", ""),
            "http://127.0.0.1:1/vnc.html?autoconnect=true&resize=scale&reconnect=true"
        );
    }

    #[test]
    fn desktop_images_are_grok_box() {
        assert!(image_is_desktop("grok-box"));
        assert!(image_is_desktop("ghcr.io/hexuria/grok-box:latest"));
        assert!(image_is_desktop("grok-box-dev@sha256:abc"));
        assert!(!image_is_desktop("debian:stable-slim"));
        assert!(!image_is_desktop("ghcr.io/x/grok-boxer"));
    }

    #[test]
    fn create_args_follow_the_image_contract() {
        let secrets = Secrets {
            token: "rox-tok".into(),
            vnc_password: "pw".into(),
            egress_bearer: "rox-bearer".into(),
        };
        let volumes = BoxVolumes {
            workspace: "rox-box-aa-ws".into(),
            home: "rox-box-aa-home".into(),
        };
        let spec = CreateBox {
            name: Some("bot one".into()),
            ttl_seconds: None,
            egress_tunnel: true,
            egress_tunnel_bearer: None,
        };

        let desktop = DockerComputer::new("ghcr.io/hexuria/grok-box:latest");
        let args = desktop.create_args(&spec, &secrets, &volumes);
        let joined = args.join(" ");
        assert!(args.starts_with(&["run".to_string(), "-d".to_string()]));
        for port in [3000, 5173, 8000, 8080, 6080, 1337, 1340, 8790] {
            assert!(
                args.contains(&format!("127.0.0.1::{port}")),
                "port {port} published on loopback: {joined}"
            );
        }
        assert!(
            !joined.contains("8791") && !joined.contains("8792"),
            "tunnel helpers stay guest-internal"
        );
        assert!(
            !joined.contains("0.0.0.0"),
            "nothing on all interfaces: {joined}"
        );
        for env in [
            "BOX_TOKEN=rox-tok",
            "BOX_DESKTOP=1",
            "BOX_VNC_PASSWORD=pw",
            "BOX_EGRESS_TUNNEL=1",
            "BOX_EGRESS_TUNNEL_BEARER=rox-bearer",
        ] {
            assert!(args.contains(&env.to_string()), "{env} in {joined}");
        }
        assert!(args.contains(&"rox-box-aa-ws:/workspace".to_string()));
        assert!(args.contains(&"rox-box-aa-home:/home/box".to_string()));
        assert_eq!(
            args.last().map(String::as_str),
            Some("ghcr.io/hexuria/grok-box:latest"),
            "a desktop image keeps its entrypoint"
        );

        let headless = DockerComputer::new(DEFAULT_IMAGE);
        let plain = CreateBox {
            ttl_seconds: Some(90),
            ..CreateBox::default()
        };
        let args = headless.create_args(&plain, &secrets, &volumes);
        let joined = args.join(" ");
        assert!(
            joined.ends_with("debian:stable-slim sh -c sleep 90"),
            "{joined}"
        );
        assert!(
            !joined.contains("6080") && !joined.contains("8790"),
            "no desktop or tunnel ports: {joined}"
        );
        assert!(
            !joined.contains("BOX_DESKTOP") && !joined.contains("BOX_EGRESS_TUNNEL"),
            "{joined}"
        );
        assert!(
            !joined.contains("/home/box"),
            "a headless box has no desktop home"
        );
        let forever = headless.create_args(&CreateBox::default(), &secrets, &volumes);
        assert!(forever.join(" ").ends_with("sh -c sleep infinity"));

        let own_bearer = CreateBox {
            egress_tunnel: true,
            egress_tunnel_bearer: Some("mine".into()),
            ..CreateBox::default()
        };
        assert!(
            headless
                .create_args(&own_bearer, &secrets, &volumes)
                .contains(&"BOX_EGRESS_TUNNEL_BEARER=mine".to_string())
        );
    }

    #[test]
    fn timeouts_clamp_to_the_providers_range() {
        assert_eq!(clamp_timeout(None), 60);
        assert_eq!(clamp_timeout(Some(0)), 1);
        assert_eq!(clamp_timeout(Some(30)), 30);
        assert_eq!(clamp_timeout(Some(10_000)), 600);
    }

    #[test]
    fn watch_output_splits_into_the_three_files() {
        let running = format!("partial{WATCH_SEPARATOR}{WATCH_SEPARATOR}");
        let w = parse_watch("ab12", &running);
        assert_eq!(
            (w.running, w.exit_code, w.stdout.as_str(), w.stderr.as_str()),
            (true, None, "partial", "")
        );
        let done = format!("out\n{WATCH_SEPARATOR}warn\n{WATCH_SEPARATOR}3\n");
        let w = parse_watch("ab12", &done);
        assert_eq!(
            (w.running, w.exit_code, w.stdout.as_str(), w.stderr.as_str()),
            (false, Some(3), "out\n", "warn\n")
        );
        assert_eq!(w.process_id, "ab12");
    }

    #[test]
    fn docker_failures_become_typed_errors() {
        assert_eq!(
            classify_failure(
                Some(1),
                "Error response from daemon: No such container: abc"
            ),
            BoxError::NoSuchBox
        );
        assert_eq!(
            classify_failure(Some(1), "Error: No such object: abc"),
            BoxError::NoSuchBox
        );
        assert_eq!(
            classify_failure(Some(1), "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?"),
            BoxError::Unavailable("Docker is not running: Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?".into())
        );
        assert_eq!(
            classify_failure(Some(125), "docker: invalid reference format"),
            BoxError::Refused {
                status: 500,
                body: "docker: invalid reference format".into()
            }
        );
        assert_eq!(
            classify_failure(None, ""),
            BoxError::Refused {
                status: 500,
                body: "docker exited with a signal".into()
            }
        );
        assert_eq!(
            unavailability_from_message("Docker is not installed: no `docker` on PATH"),
            Unavailability::NotInstalled
        );
        assert_eq!(
            unavailability_from_message("Docker is not running: boom"),
            Unavailability::NotRunning("boom".into())
        );
    }

    #[test]
    fn only_the_daemons_words_are_refusals() {
        assert_eq!(
            daemon_refusal("Error response from daemon: No such container: abc"),
            Some(BoxError::NoSuchBox)
        );
        assert!(matches!(
            daemon_refusal("Cannot connect to the Docker daemon at unix:///x"),
            Some(BoxError::Unavailable(_))
        ));
        assert_eq!(
            daemon_refusal("sh: 1: nope: not found"),
            None,
            "a command's own stderr is output"
        );
        assert_eq!(daemon_refusal(""), None);
    }

    #[test]
    fn image_status_is_behind_only_when_both_known_and_different() {
        let s = image_status("img", Some("a".into()), Some("b".into()));
        assert!(s.behind && s.local);
        assert!(!image_status("img", Some("a".into()), Some("a".into())).behind);
        let unpulled = image_status("img", Some("a".into()), None);
        assert!(!unpulled.behind && !unpulled.local);
        assert!(!image_status("img", None, Some("b".into())).behind);
    }

    #[test]
    fn long_output_is_cut_on_a_char_boundary() {
        let mut big = "é".repeat(MAX_OUTPUT / 2);
        big.push_str("tail");
        let (text, truncated) = cut(big.as_bytes());
        assert!(truncated);
        assert!(text.len() <= MAX_OUTPUT);
        assert!(text.is_char_boundary(text.len()));
        assert_eq!(cut(b"short"), ("short".to_string(), false));
    }

    #[test]
    fn secrets_are_hex_of_the_asked_size() {
        let s = Secrets::fresh().unwrap();
        assert!(s.token.starts_with("rox-") && s.token.len() == 4 + 32);
        assert_eq!(s.vnc_password.len(), 8);
        assert!(s.vnc_password.chars().all(|c| c.is_ascii_hexdigit()));
        let v = BoxVolumes::fresh().unwrap();
        assert!(v.workspace.starts_with("rox-box-") && v.workspace.ends_with("-ws"));
        assert_eq!(v.names().len(), 2);
    }
}
