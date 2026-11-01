//! The Bot's computer: a trait, and the local Docker box behind it.
//!
//! Transcribed from opengrok-server `crates/opengrok-box/src/lib.rs`. The hub (later slices)
//! talks to a `dyn Computer` and never to Docker; a hosted provider (box.ascii.dev) slots in
//! as a second implementation of the same trait without the hub noticing.
//!
//! Nothing here panics: a box that is gone, a daemon that is down, a guest that is quiet are
//! all typed errors the pane can show.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod docker;
mod guest;
pub mod unavailable;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use docker::DockerComputer;
pub use unavailable::{NoComputer, Unavailability, local_docker};

/// Whether the box's outbound traffic goes through the broker's egress tunnel: `enabled` is
/// the intent the box was created with, `ready` is the guest's tunnel reporting up.
// Shape: opengrok-server crates/opengrok-box/src/lib.rs `EgressTunnel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressTunnel {
    pub enabled: bool,
    pub ready: bool,
}

/// How long a guest info probe may take before the pane paints "no answer yet". A guest that
/// is booting (desktop images take tens of seconds) must not hold a details fetch hostage.
// opengrok-server crates/opengrok-box/src/lib.rs `GUEST_INFO_PATIENCE`.
pub const GUEST_INFO_PATIENCE: Duration = Duration::from_secs(1);

/// After a guest took longer than [`GUEST_INFO_PATIENCE`], leave it alone this long before
/// probing again; every pane refresh would otherwise pay the full patience.
// opengrok-server crates/opengrok-box/src/lib.rs `SILENT_GUEST_FOR`.
pub const SILENT_GUEST_FOR: Duration = Duration::from_secs(10);

static SILENT_GUESTS: LazyLock<Mutex<HashMap<String, (Instant, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The egress tunnel to show for a box: the probe's answer, or `None` while the guest is too
/// slow to say. A slow guest is remembered for [`SILENT_GUEST_FOR`] so the pane does not pay
/// the patience on every refresh; the memory is per box *generation* — a stop, start or
/// rebuild is a new guest and gets probed afresh.
// opengrok-server crates/opengrok-box/src/lib.rs `shown_egress`.
pub async fn shown_egress(computer: &dyn Computer, box_id: &str) -> Option<EgressTunnel> {
    let generation = computer.generation(box_id).await;
    let silenced = match SILENT_GUESTS.lock() {
        Ok(map) => map
            .get(box_id)
            .is_some_and(|(since, g)| *g == generation && since.elapsed() < SILENT_GUEST_FOR),
        Err(_) => false,
    };
    if silenced {
        return None;
    }
    let (shown, slow) = computer.egress_probe(box_id).await;
    if let Ok(mut map) = SILENT_GUESTS.lock() {
        if slow {
            map.insert(box_id.to_string(), (Instant::now(), generation));
        } else {
            map.remove(box_id);
        }
    }
    shown
}

/// The provider's state word for a box, grouped the way the pane and the tool loop care about
/// it. The word itself travels untouched in `BoxState::Other`, so a provider state we have not
/// met is shown, not swallowed.
// Grouping: opengrok-server crates/opengrok-box/src/lib.rs `is_asleep` / `is_starting`, with
// docker.rs's "absent" for a container that is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxState {
    Running,
    /// Stopped, paused, exited, created, archived: `resume` brings it back.
    Asleep(String),
    /// Provisioning, restarting, resuming...: wait, do not act.
    Starting(String),
    /// The container is gone (or never existed): `create` is the only move.
    Absent,
    Other(String),
}

impl BoxState {
    pub fn parse(state: &str) -> Self {
        let word = state.trim();
        if word == "running" {
            Self::Running
        } else if word == "absent" {
            Self::Absent
        } else if is_asleep(word) {
            Self::Asleep(word.to_string())
        } else if is_starting(word) {
            Self::Starting(word.to_string())
        } else {
            Self::Other(word.to_string())
        }
    }

    /// The provider's word back, for the wire.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Absent => "absent",
            Self::Asleep(w) | Self::Starting(w) | Self::Other(w) => w,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// A box that `resume` brings back.
// opengrok-server crates/opengrok-box/src/lib.rs `is_asleep`.
pub fn is_asleep(state: &str) -> bool {
    matches!(
        state,
        "stopped" | "paused" | "exited" | "created" | "archived"
    )
}

/// A box on its way to running.
// opengrok-server crates/opengrok-box/src/lib.rs `is_starting`.
pub fn is_starting(state: &str) -> bool {
    matches!(
        state,
        "provisioning"
            | "provisioned"
            | "init"
            | "cloning"
            | "restarting"
            | "resuming"
            | "archiving"
    )
}

/// A finished command.
// opengrok-server crates/opengrok-box/src/lib.rs `CommandOutput`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
}

/// A command started in the background; `watch` reads it later.
// opengrok-server crates/opengrok-box/src/lib.rs `StartedCommand`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedCommand {
    pub process_id: String,
    pub running: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// A picture of the box's screen.
// opengrok-server crates/opengrok-box/src/lib.rs `Screenshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Which of the box's screens: the one everyone shares, or a Bot's own (`:N`). The local
/// Docker provider serves the shared one; own screens are a later slice.
// opengrok-server crates/opengrok-box/src/lib.rs `Screen`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Shared,
    Own(String),
}

/// What the provider knows about the box's image: the tag it runs, whether a newer one is
/// pulled, whether the running box is behind it.
// opengrok-server crates/opengrok-box/src/lib.rs `ImageStatus`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageStatus {
    pub image: String,
    pub local: bool,
    pub running_image_id: Option<String>,
    pub local_image_id: Option<String>,
    pub behind: bool,
}

/// What `create` needs to know.
// opengrok-server crates/opengrok-box/src/lib.rs `CreateBox`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateBox {
    /// A name for the box; the provider may ignore it.
    pub name: Option<String>,
    /// Seconds the box may live; `None` is until destroyed.
    pub ttl_seconds: Option<u64>,
    /// Route the box's outbound traffic through the broker's egress tunnel.
    pub egress_tunnel: bool,
    /// The bearer the guest presents to the tunnel; the provider generates one when absent.
    pub egress_tunnel_bearer: Option<String>,
}

/// Why the computer could not do what was asked.
// opengrok-server crates/opengrok-box/src/lib.rs `BoxError`, plus `Unavailable` for the
// provider itself being absent (Docker not installed / not running).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoxError {
    /// The provider could not be reached at all (connection, DNS, the CLI missing).
    #[error("the box provider is unreachable: {0}")]
    Unreachable(String),
    /// The provider answered and said no.
    #[error("the box provider refused ({status}): {body}")]
    Refused { status: u16, body: String },
    /// The box is gone or never existed.
    #[error("no such box")]
    NoSuchBox,
    /// A secret the box needs could not be made or read.
    #[error("box secret: {0}")]
    Secret(String),
    /// The call was cut short (a timeout, a cancelled task).
    #[error("interrupted: {0}")]
    Interrupted(String),
    /// There is no computer here: the provider is not installed or not running. Every call on
    /// a [`NoComputer`] ends here; the message is for the pane.
    #[error("no computer: {0}")]
    Unavailable(String),
}

impl BoxError {
    /// A stable word for the wire (`error.code`), the same ones opengrok-server emits.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unreachable(_) | Self::Unavailable(_) => "provider_unreachable",
            Self::Refused { .. } => "provider_error",
            Self::NoSuchBox => "no_such_box",
            Self::Secret(_) => "box_secret",
            Self::Interrupted(_) => "interrupted",
        }
    }

    pub fn is_no_such_box(&self) -> bool {
        matches!(self, Self::NoSuchBox)
    }
}

pub type BoxResult<T> = Result<T, BoxError>;

/// `501`: this provider does not do that.
// opengrok-server crates/opengrok-box/src/lib.rs `not_supported`.
pub fn not_supported(what: &str) -> BoxError {
    BoxError::Refused {
        status: 501,
        body: format!("this computer cannot {what}"),
    }
}

/// `409`: the box has no screen (a headless image).
// opengrok-server crates/opengrok-box/src/lib.rs `no_screen`.
pub fn no_screen() -> BoxError {
    BoxError::Refused {
        status: 409,
        body: "this box has no screen".to_string(),
    }
}

/// A Bot's computer. One implementation per provider; the hub holds a `dyn Computer`.
///
/// Methods a provider cannot honour return [`not_supported`] rather than being absent from
/// the trait, so the hub's routes and tools are the same whatever the provider.
// opengrok-server crates/opengrok-box/src/lib.rs `Computer`.
#[async_trait]
pub trait Computer: Send + Sync {
    /// A new box; the id names it in every other call.
    async fn create(&self, spec: CreateBox) -> BoxResult<String>;

    /// Run a shell command to completion, `timeout_seconds` clamped to the provider's range.
    async fn run(
        &self,
        box_id: &str,
        command: &str,
        timeout_seconds: Option<u64>,
    ) -> BoxResult<CommandOutput>;

    /// Start a command in the background; `watch` it later.
    async fn start(&self, box_id: &str, command: &str) -> BoxResult<StartedCommand>;

    /// Where a started command is.
    async fn watch(&self, box_id: &str, process_id: &str) -> BoxResult<StartedCommand>;

    async fn read_file(&self, box_id: &str, path: &str) -> BoxResult<String>;

    async fn write_file(&self, box_id: &str, path: &str, content: &str) -> BoxResult<()> {
        self.write_file_bytes(box_id, path, content.as_bytes())
            .await
    }

    async fn write_file_bytes(&self, box_id: &str, path: &str, content: &[u8]) -> BoxResult<()>;

    /// A URL on this machine that reaches `port` inside the box.
    async fn expose_port(&self, box_id: &str, port: u16) -> BoxResult<String>;

    async fn stop(&self, box_id: &str) -> BoxResult<()>;

    async fn resume(&self, box_id: &str) -> BoxResult<()>;

    /// `resume` only when asleep; a no-op otherwise.
    async fn wake(&self, box_id: &str) -> BoxResult<()> {
        if is_asleep(&self.state(box_id).await?) {
            self.resume(box_id).await
        } else {
            Ok(())
        }
    }

    /// Remove the box and everything it kept.
    async fn destroy(&self, box_id: &str) -> BoxResult<()>;

    /// Remove the box, keep its data for the next `create` (`recreate` uses this).
    async fn discard(&self, box_id: &str) -> BoxResult<()>;

    /// The provider's state word ([`BoxState::parse`] groups it).
    async fn state(&self, box_id: &str) -> BoxResult<String>;

    /// Whether this box has a screen at all (a desktop image).
    async fn has_screen(&self, box_id: &str) -> BoxResult<bool>;

    /// The page a browser opens to watch the screen (noVNC on this machine's loopback).
    async fn screen_url(&self, box_id: &str, screen: &Screen) -> BoxResult<String>;

    async fn screenshot(&self, box_id: &str, screen: &Screen) -> BoxResult<Screenshot>;

    /// The image behind the box and whether it is behind the latest pulled.
    async fn image_status(&self, box_id: Option<&str>) -> BoxResult<ImageStatus>;

    /// Pull the latest image; the running box keeps the old one until `recreate`.
    async fn pull_latest(&self) -> BoxResult<ImageStatus>;

    /// Discard the box and create it again on the current image, keeping its data.
    async fn recreate(&self, box_id: &str, spec: CreateBox) -> BoxResult<String>;

    /// The egress tunnel the box was created with (`None` when the provider does not know).
    async fn egress_intent(&self, box_id: &str) -> BoxResult<Option<bool>>;

    /// Ask the guest whether its tunnel is up: `(answer, slow)`. `slow` is true only when the
    /// guest took longer than [`GUEST_INFO_PATIENCE`]; [`shown_egress`] then leaves it alone
    /// for a while. A guest that is plainly down answers `(None, false)`.
    async fn egress_probe(&self, box_id: &str) -> (Option<EgressTunnel>, bool);

    /// Bumps whenever the guest behind `box_id` is a new process (stop, start, rebuild,
    /// removal), so remembered observations about the old one can be dropped.
    async fn generation(&self, box_id: &str) -> u64;
}

/// Parse a guest `/v1/info` body into the tunnel it reports: `egress_tunnel.enabled` and
/// `.ready`, either absent meaning false. A body without the key is a guest that has no
/// tunnel at all (an older image): `None`.
// opengrok-server crates/opengrok-box/src/docker.rs `egress_from_info`.
pub fn egress_from_info(info: &serde_json::Value) -> Option<EgressTunnel> {
    let tunnel = info.get("egress_tunnel")?;
    let flag = |key: &str| tunnel.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
    Some(EgressTunnel {
        enabled: flag("enabled"),
        ready: flag("ready"),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    #[test]
    fn state_words_group_as_the_pane_expects() {
        assert_eq!(BoxState::parse("running"), BoxState::Running);
        assert_eq!(BoxState::parse("absent"), BoxState::Absent);
        assert_eq!(BoxState::parse("exited"), BoxState::Asleep("exited".into()));
        assert_eq!(BoxState::parse("paused"), BoxState::Asleep("paused".into()));
        assert_eq!(
            BoxState::parse("restarting"),
            BoxState::Starting("restarting".into())
        );
        assert_eq!(BoxState::parse("dead"), BoxState::Other("dead".into()));
        assert_eq!(BoxState::parse(" running\n"), BoxState::Running);
        assert_eq!(BoxState::parse("dead").as_str(), "dead");
        assert!(BoxState::Running.is_running());
        assert!(is_asleep("stopped") && is_asleep("created") && is_asleep("archived"));
        assert!(is_starting("provisioning") && is_starting("resuming"));
        assert!(!is_asleep("running") && !is_starting("running"));
    }

    #[test]
    fn egress_info_parses_and_defaults() {
        let both = serde_json::json!({"egress_tunnel": {"enabled": true, "ready": true}});
        assert_eq!(
            egress_from_info(&both),
            Some(EgressTunnel {
                enabled: true,
                ready: true
            })
        );
        let half = serde_json::json!({"egress_tunnel": {"enabled": true}});
        assert_eq!(
            egress_from_info(&half),
            Some(EgressTunnel {
                enabled: true,
                ready: false
            })
        );
        let garbage = serde_json::json!({"egress_tunnel": {"enabled": "yes"}});
        assert_eq!(
            egress_from_info(&garbage),
            Some(EgressTunnel {
                enabled: false,
                ready: false
            })
        );
        assert_eq!(egress_from_info(&serde_json::json!({"version": "1"})), None);
    }

    #[test]
    fn error_codes_are_the_wire_words() {
        assert_eq!(
            BoxError::Unavailable("x".into()).code(),
            "provider_unreachable"
        );
        assert_eq!(
            BoxError::Unreachable("x".into()).code(),
            "provider_unreachable"
        );
        assert_eq!(not_supported("fly").code(), "provider_error");
        assert_eq!(BoxError::NoSuchBox.code(), "no_such_box");
        assert_eq!(
            no_screen(),
            BoxError::Refused {
                status: 409,
                body: "this box has no screen".into()
            }
        );
        assert_eq!(
            not_supported("fly").to_string(),
            "the box provider refused (501): this computer cannot fly"
        );
    }

    /// A computer whose probe is slow, counting how often it is asked.
    struct SlowGuest {
        probes: AtomicUsize,
        generation: AtomicU64,
    }

    #[async_trait]
    impl Computer for SlowGuest {
        async fn create(&self, _: CreateBox) -> BoxResult<String> {
            Err(not_supported("create"))
        }
        async fn run(&self, _: &str, _: &str, _: Option<u64>) -> BoxResult<CommandOutput> {
            Err(not_supported("run"))
        }
        async fn start(&self, _: &str, _: &str) -> BoxResult<StartedCommand> {
            Err(not_supported("start"))
        }
        async fn watch(&self, _: &str, _: &str) -> BoxResult<StartedCommand> {
            Err(not_supported("watch"))
        }
        async fn read_file(&self, _: &str, _: &str) -> BoxResult<String> {
            Err(not_supported("read"))
        }
        async fn write_file_bytes(&self, _: &str, _: &str, _: &[u8]) -> BoxResult<()> {
            Err(not_supported("write"))
        }
        async fn expose_port(&self, _: &str, _: u16) -> BoxResult<String> {
            Err(not_supported("expose"))
        }
        async fn stop(&self, _: &str) -> BoxResult<()> {
            Ok(())
        }
        async fn resume(&self, _: &str) -> BoxResult<()> {
            Ok(())
        }
        async fn destroy(&self, _: &str) -> BoxResult<()> {
            Ok(())
        }
        async fn discard(&self, _: &str) -> BoxResult<()> {
            Ok(())
        }
        async fn state(&self, _: &str) -> BoxResult<String> {
            Ok("running".into())
        }
        async fn has_screen(&self, _: &str) -> BoxResult<bool> {
            Ok(false)
        }
        async fn screen_url(&self, _: &str, _: &Screen) -> BoxResult<String> {
            Err(no_screen())
        }
        async fn screenshot(&self, _: &str, _: &Screen) -> BoxResult<Screenshot> {
            Err(no_screen())
        }
        async fn image_status(&self, _: Option<&str>) -> BoxResult<ImageStatus> {
            Ok(ImageStatus::default())
        }
        async fn pull_latest(&self) -> BoxResult<ImageStatus> {
            Ok(ImageStatus::default())
        }
        async fn recreate(&self, _: &str, _: CreateBox) -> BoxResult<String> {
            Err(not_supported("recreate"))
        }
        async fn egress_intent(&self, _: &str) -> BoxResult<Option<bool>> {
            Ok(Some(true))
        }
        async fn egress_probe(&self, _: &str) -> (Option<EgressTunnel>, bool) {
            self.probes.fetch_add(1, Ordering::SeqCst);
            (None, true)
        }
        async fn generation(&self, _: &str) -> u64 {
            self.generation.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn a_slow_guest_is_left_alone_until_its_generation_changes() {
        let guest = SlowGuest {
            probes: AtomicUsize::new(0),
            generation: AtomicU64::new(1),
        };
        let id = format!("silent-guest-{}", std::process::id());
        assert_eq!(shown_egress(&guest, &id).await, None);
        assert_eq!(shown_egress(&guest, &id).await, None);
        assert_eq!(
            guest.probes.load(Ordering::SeqCst),
            1,
            "the second look within SILENT_GUEST_FOR does not probe"
        );
        guest.generation.store(2, Ordering::SeqCst);
        assert_eq!(shown_egress(&guest, &id).await, None);
        assert_eq!(
            guest.probes.load(Ordering::SeqCst),
            2,
            "a new guest is probed afresh"
        );
    }
}
