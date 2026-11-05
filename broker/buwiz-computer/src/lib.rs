//! A Bot's computer: the trait the hub talks to, and the providers behind it.
//!
//! The hub holds a `dyn Provider` and a saved [`Machine`] per computer, and never speaks Docker.
//! A computer can be a container here, a VM, a cloud sandbox, or something a person points at:
//! [`Reach`] says which, and [`ProviderKind`] names the provider that knows how to reach it.
//! One computer serves one or more Bots, and each gets a [`Screen`] of its own: its own display,
//! input and folder. Everything that acts takes the screen it acts on; the hub picks it, from
//! the Bot, and the model never names one.
//! Only Docker is built. The others have their slot ([`planned::Planned`]), and each says so in
//! a sentence until it is built.
//!
//! What a provider can't do stays a method: it answers [`Problem::Unsupported`], so the hub's
//! routes and tools are the same whatever the computer is. A plain URL has a viewer and nothing
//! else; an SSH host has `run` and files and no screen.
//!
//! Nothing here panics: a gone box, a daemon that is down and a quiet guest are all typed
//! [`Problem`]s, each with the sentence a person reads.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod docker;
pub mod gate;
pub mod guest;
#[cfg(feature = "mock")]
pub mod mock;
pub mod planned;

use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use docker::DockerProvider;
pub use gate::Gate;
pub use guest::BoxGuest;
pub use planned::Planned;

/// The loopback ports a computer's control and viewer ports are published on, and the ones the
/// broker's viewer gate binds: all of them, on every broker of this Mac, so the Bot's sandboxed
/// shell can be kept off them by a fixed rule, whatever a wake picked (`policy::sandbox`).
pub const PORT_FIRST: u16 = 47000;
pub const PORT_LAST: u16 = 47219;
/// The part of that range Docker picks from; the gate binds in the rest.
pub const DOCKER_PORT_LAST: u16 = 47199;

/// Text on a screen stays readable to a model at this, at a quarter of the PNG's weight.
const JPEG_QUALITY: u8 = 75;

/// A value that must not reach a log, the model, the tree or the tile. It has no `Display`, and
/// its `Debug` says nothing; [`Secret::expose`] is the one way out, for the wire to the computer.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    /// `bytes` random bytes from the OS, as hex.
    pub fn fresh(bytes: usize) -> Result<Self> {
        Ok(Self(random_hex(bytes)?))
    }

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// `bytes` random bytes as lowercase hex, from the OS CSPRNG.
pub fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| Problem::Failed(format!("no randomness: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Who runs the computer. The saved word survives a build that doesn't know the provider yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// a container on this Mac
    Docker,
    /// a macOS VM on this Mac
    Lume,
    /// Docker Sandboxes, in the cloud
    Sbx,
    /// an AWS instance
    Ec2,
    /// any URL, SSH host, VNC or RDP a person gives it
    Remote,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 5] = [Self::Docker, Self::Lume, Self::Sbx, Self::Ec2, Self::Remote];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Lume => "lume",
            Self::Sbx => "sbx",
            Self::Ec2 => "ec2",
            Self::Remote => "remote",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == word)
    }

    /// What a person calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Lume => "Lume",
            Self::Sbx => "Docker Sandboxes",
            Self::Ec2 => "AWS EC2",
            Self::Remote => "A remote computer",
        }
    }
}

/// What a computer is, as far as reaching it goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Reach {
    Container { name: String },
    Url { url: String },
    Ssh { host: String, user: Option<String>, port: Option<u16> },
}

/// A computer as the broker saves it. The secrets stay in the broker's data dir, in a file
/// of their own: they aren't in app.db, and no route or tool answers with them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub provider: ProviderKind,
    pub reach: Reach,
    /// what it was made from, when that means anything
    pub image: Option<String>,
    /// the control API's bearer
    pub token: Secret,
    pub viewer_password: Secret,
    /// the folder the broker keeps for it
    pub dir: PathBuf,
    /// what [`Provider::look`] found for this request; never saved
    #[serde(skip)]
    pub seen: Option<Seen>,
}

/// What a provider saw of a computer: one read, good for one request and no longer. Ports can
/// change with a start, and an old port can be another computer's by then.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub status: String,
    /// when the network it sits in came up: the wall stands for that long, however often the
    /// computer itself starts
    pub started: String,
    /// whether that network is up
    pub net_running: bool,
    /// its network's holder doesn't exist: the computer's own containers must be made again
    pub net_missing: bool,
    /// when the computer itself last started
    pub box_started: String,
    /// guest port → `127.0.0.1:port`
    pub ports: std::collections::HashMap<u16, String>,
}

impl Machine {
    /// `text` without this computer's secrets: the last check on anything bound for the model.
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for s in [&self.token, &self.viewer_password] {
            if !s.expose().is_empty() {
                out = out.replace(s.expose(), "[hidden]");
            }
        }
        out
    }
}

/// Where a computer is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Power {
    /// none was made, or it is gone
    Absent,
    Asleep,
    /// on the way up: wait, don't act
    Waking,
    Awake,
    /// the provider's word for a state nothing here can use
    Broken(String),
}

impl Power {
    pub fn word(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Asleep => "asleep",
            Self::Waking => "waking",
            Self::Awake => "awake",
            Self::Broken(_) => "error",
        }
    }
}

/// How a wake went: what was open is still there, or it started afresh and its screens are gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woke {
    Resumed,
    Started,
}

/// How deep a sleep goes. Light keeps everything open and comes back at once, and costs its
/// memory; deep frees the memory and keeps the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sleep {
    Light,
    Deep,
}

/// What a new computer is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// the computer's id in the broker
    pub id: String,
    /// the folder the broker keeps for it
    pub dir: PathBuf,
}

/// One Bot's place on a computer: its display, its input, its viewer, its folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    /// `:N`
    pub display: String,
    /// the viewer's port inside the computer
    pub viewer_port: u16,
    /// what this Bot makes lands here, relative to the workspace (`agents/<bot>`). Artifacts are
    /// collected from it, per agent.
    pub folder: String,
    /// its browser's profile, inside the computer
    pub profile: Option<String>,
}

/// What a viewer needs: where the page is on this Mac, and the word it wants. The gate keeps it;
/// the app only ever gets a ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Viewer {
    /// `127.0.0.1:port`
    pub addr: String,
    pub password: Secret,
}

/// A finished command.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub timed_out: bool,
}

/// A picture of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Screenshot {
    /// The picture for a model: JPEG, a fraction of the PNG's weight, at the same size, so a
    /// point in the picture is a point on the screen. The PNG itself when it can't be read.
    pub fn for_model(&self) -> (Vec<u8>, &'static str) {
        jpeg(&self.png).map_or_else(|| (self.png.clone(), "image/png"), |j| (j, "image/jpeg"))
    }
}

fn jpeg(png_bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    let (w, h) = (u16::try_from(info.width).ok()?, u16::try_from(info.height).ok()?);
    let kind = match info.color_type {
        png::ColorType::Rgb => jpeg_encoder::ColorType::Rgb,
        png::ColorType::Rgba => jpeg_encoder::ColorType::Rgba,
        png::ColorType::Grayscale => jpeg_encoder::ColorType::Luma,
        _ => return None,
    };
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY).encode(pixels.get(..info.buffer_size())?, w, h, kind).ok()?;
    Some(out)
}

/// What a person or the model does to the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Click { x: i32, y: i32, button: u8, double: bool },
    Move { x: i32, y: i32 },
    Drag { from: (i32, i32), to: (i32, i32) },
    Type { text: String },
    Key { key: String },
    Scroll { x: i32, y: i32, dx: i32, dy: i32 },
}

/// When [`Provider::settle`] is done waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitFor {
    /// the screen has been still this long
    pub idle_ms: u64,
    /// give up after this long
    pub timeout_ms: u64,
    /// count only what changed after this mark ([`Provider::mark`])
    pub since: Option<u64>,
}

impl Default for WaitFor {
    fn default() -> Self {
        Self { idle_ms: 300, timeout_ms: 8000, since: None }
    }
}

/// How a wait ended. `fallback` is a fixed pause: no viewer could say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Settled {
    pub changed: bool,
    pub timed_out: bool,
    pub waited_ms: u64,
    pub seq: Option<u64>,
    pub fallback: bool,
}

/// Why the computer couldn't do what was asked. [`Problem::sentence`] is what a person reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// the provider's program isn't on this Mac
    ProviderMissing(String),
    /// it's there and not answering
    ProviderDown(String),
    ImageMissing(String),
    /// a provider with a slot and no code yet
    NotBuilt(ProviderKind),
    /// the computer is gone
    Gone,
    /// this computer can't do that
    Unsupported(String),
    /// the computer's own services said no, or said nothing
    Guest { status: u16, message: String },
    Asleep,
    /// it's walled off from the Mac, or it doesn't start
    Unwalled(String),
    Failed(String),
}

impl Problem {
    /// A stable word for the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProviderMissing(_) => "provider_missing",
            Self::ProviderDown(_) => "provider_down",
            Self::ImageMissing(_) => "image_missing",
            Self::NotBuilt(_) => "not_built",
            Self::Gone => "gone",
            Self::Unsupported(_) => "unsupported",
            Self::Guest { .. } => "guest",
            Self::Asleep => "asleep",
            Self::Unwalled(_) => "unwalled",
            Self::Failed(_) => "failed",
        }
    }

    /// One plain sentence, for the pane and the model.
    pub fn sentence(&self) -> String {
        match self {
            Self::ProviderMissing(what) => format!("{what} isn't installed on this Mac. Install it, then click the computer again."),
            Self::ProviderDown(what) => format!("{what} isn't running. Open it, wait until it says it's running, then click the computer again."),
            Self::ImageMissing(image) => format!("This Mac doesn't have the computer's image ({image}). Build or pull it, then click the computer again."),
            Self::NotBuilt(kind) => format!("{} computers aren't built yet. Only Docker on this Mac works today.", kind.label()),
            Self::Gone => "This computer is gone. Click it to make a new one.".to_owned(),
            Self::Unsupported(what) => format!("This computer can't {what}."),
            Self::Guest { message, .. } => format!("The computer said: {message}"),
            Self::Asleep => "The computer is asleep. Only a person can wake it: ask them to click it in the Computer tab.".to_owned(),
            Self::Unwalled(why) => format!("The computer didn't start, because it couldn't be walled off from this Mac: {why}"),
            Self::Failed(why) => format!("The computer failed: {why}"),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.sentence())
    }
}

impl std::error::Error for Problem {}

pub type Result<T> = std::result::Result<T, Problem>;

fn unsupported<T>(what: &str) -> Result<T> {
    Err(Problem::Unsupported(what.to_owned()))
}

/// A kind of computer. Lifecycle first, then what acts inside; what a provider can't do it
/// leaves to the defaults here.
#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    /// Whether a computer could run at all, and the reason when not.
    async fn check(&self) -> Result<()>;

    /// Makes a computer, asleep: [`Provider::wake`] starts it.
    async fn create(&self, spec: &Spec) -> Result<Machine>;

    /// Where it is, from the provider itself: a saved word goes stale.
    async fn power(&self, machine: &Machine) -> Result<Power>;

    /// [`Provider::power`], and the machine to use for the rest of this request: one read of
    /// the computer serves all its calls.
    async fn look(&self, machine: &Machine) -> Result<(Power, Machine)> {
        Ok((self.power(machine).await?, machine.clone()))
    }

    /// Brings it up. Returns once it answers: a person or the model can use it.
    async fn wake(&self, machine: &Machine) -> Result<Woke>;

    async fn sleep(&self, machine: &Machine, depth: Sleep) -> Result<()>;

    /// Removes it and everything it kept. Only for a computer a person agreed to delete, or one
    /// just made that holds nothing yet.
    async fn destroy(&self, machine: &Machine) -> Result<()>;

    /// [`Provider::destroy`] of computer `id` when its saved record is lost. The same rule. `last`
    /// is where it was, when that's still known.
    async fn sweep(&self, _id: &str, _last: Option<&Reach>) -> Result<()> {
        Ok(())
    }

    /// A record for computer `spec.id` from what's left of it, with new secrets, when its saved
    /// one is lost: its files are still there. `last` is where it was, when that's still known, so
    /// its files can be found with nothing else left. `None` when nothing of it is found.
    async fn recover(&self, _spec: &Spec, _last: Option<&Reach>) -> Result<Option<Machine>> {
        Ok(None)
    }

    /// Before anyone uses an awake computer: whatever must hold for it to be safe to use, checked
    /// for this start of it. A computer that fails this is stopped.
    async fn guard(&self, _machine: &Machine) -> Result<()> {
        Ok(())
    }

    /// Starts `bot`'s screen, or finds it running. Awake only; a bot keeps its screen's number
    /// and folder across sleeps.
    async fn screen_up(&self, _machine: &Machine, _bot: &str) -> Result<Screen> {
        unsupported("give a Bot a screen")
    }

    /// Stops `bot`'s screen and gives up its slot. What it made stays.
    async fn screen_down(&self, _machine: &Machine, _bot: &str) -> Result<()> {
        unsupported("give a Bot a screen")
    }

    /// The Bots with a screen on it, running or not.
    async fn screens(&self, _machine: &Machine) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// `screen`'s live viewer, if it has one.
    async fn viewer(&self, _machine: &Machine, _screen: &Screen) -> Result<Option<Viewer>> {
        Ok(None)
    }

    async fn screenshot(&self, _machine: &Machine, _screen: &Screen) -> Result<Screenshot> {
        unsupported("show its screen")
    }

    async fn input(&self, _machine: &Machine, _screen: &Screen, _input: &Input) -> Result<()> {
        unsupported("take input")
    }

    /// Runs a command for `screen`, in its folder, `timeout_ms` clamped to the provider's range.
    async fn run(&self, _machine: &Machine, _screen: &Screen, _command: &str, _timeout_ms: u64) -> Result<CommandOutput> {
        unsupported("run commands")
    }

    async fn open_url(&self, _machine: &Machine, _screen: &Screen, _url: &str) -> Result<()> {
        unsupported("open a page")
    }

    /// Where `path`, relative to the workspace, really is inside the computer, links followed.
    async fn resolve(&self, _machine: &Machine, _path: &str) -> Result<String> {
        unsupported("read files")
    }

    async fn read_file(&self, _machine: &Machine, _path: &str) -> Result<Vec<u8>> {
        unsupported("read files")
    }

    async fn write_file(&self, _machine: &Machine, _path: &str, _content: &[u8]) -> Result<()> {
        unsupported("write files")
    }

    /// A mark to pass [`WaitFor::since`], when the provider numbers screen changes.
    async fn mark(&self, _machine: &Machine, _screen: &Screen) -> Option<u64> {
        None
    }

    /// Waits for the screen to settle.
    async fn settle(&self, _machine: &Machine, _screen: &Screen, _wait: WaitFor) -> Result<Settled> {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok(Settled { waited_ms: 500, fallback: true, ..Settled::default() })
    }

    /// Starts recording the screen; the answer names the recording.
    async fn record_start(&self, _machine: &Machine, _screen: &Screen) -> Result<u64> {
        unsupported("record its screen")
    }

    /// Ends a recording; the answer is the path inside the computer.
    async fn record_stop(&self, _machine: &Machine, _screen: &Screen, _id: u64) -> Result<String> {
        unsupported("record its screen")
    }
}

/// The provider for `kind`: Docker's, or the slot of one that isn't built.
pub fn provider_for(kind: ProviderKind) -> std::sync::Arc<dyn Provider> {
    match kind {
        ProviderKind::Docker => std::sync::Arc::new(DockerProvider::from_env()),
        other => std::sync::Arc::new(Planned(other)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn machine() -> Machine {
        Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name: "c2-x".into() },
            image: None,
            token: Secret::new("tok-123"),
            viewer_password: Secret::new("pw-456"),
            dir: PathBuf::from("/tmp/x"),
            seen: None,
        }
    }

    #[test]
    fn a_secret_stays_out_of_debug_and_a_machines_debug() {
        let m = machine();
        let shown = format!("{m:?}");
        assert!(!shown.contains("tok-123") && !shown.contains("pw-456"), "{shown}");
    }

    #[test]
    fn scrub_hides_both_secrets() {
        assert_eq!(machine().scrub("a tok-123 b pw-456 c"), "a [hidden] b [hidden] c");
    }

    #[test]
    fn a_computer_can_be_a_container_a_url_or_an_ssh_host() {
        for reach in [
            Reach::Container { name: "c".into() },
            Reach::Url { url: "https://x.example/".into() },
            Reach::Ssh { host: "h".into(), user: Some("u".into()), port: Some(2222) },
        ] {
            let saved = serde_json::to_string(&reach).unwrap();
            assert_eq!(serde_json::from_str::<Reach>(&saved).unwrap(), reach);
        }
    }

    #[test]
    fn a_model_gets_a_jpeg_at_the_screens_size() {
        let mut png_bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut png_bytes, 64, 40);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![200u8; 64 * 40 * 4]).unwrap();
        writer.finish().unwrap();
        let shot = Screenshot { png: png_bytes, width: 64, height: 40 };
        let (bytes, media) = shot.for_model();
        assert_eq!(media, "image/jpeg");
        assert_eq!(&bytes[..2], [0xFF, 0xD8]);
        // SOF0 holds the height and width: the same as the screen's, so clicks need no scaling
        let at = bytes.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        assert_eq!(&bytes[at + 5..at + 9], [0, 40, 0, 64]);
        let broken = Screenshot { png: b"not a png".to_vec(), width: 1, height: 1 };
        assert_eq!(broken.for_model().1, "image/png");
    }

    #[test]
    fn every_provider_has_a_slot() {
        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
            assert_eq!(provider_for(kind).kind(), kind);
        }
    }

    #[test]
    fn the_sentences_say_what_a_person_can_do() {
        assert!(Problem::ProviderDown("Docker".into()).sentence().starts_with("Docker isn't running."));
        assert!(Problem::ImageMissing("img:1".into()).sentence().contains("doesn't have the computer's image (img:1)"));
        assert!(Problem::NotBuilt(ProviderKind::Ec2).sentence().contains("AWS EC2"));
    }
}
