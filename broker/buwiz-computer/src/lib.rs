//! A Bot's computer: the trait the hub talks to, and the providers behind it.
//!
//! The hub holds a `dyn Provider` and a saved [`Machine`] per computer, and never speaks Docker.
//! A computer can be a container here, a VM, a cloud sandbox, or something a person points at:
//! [`Reach`] says which, and [`ProviderKind`] names the provider that knows how to reach it.
//! One computer serves one or more Bots, and each gets a [`Screen`] of its own: its own display,
//! input and folder. Everything that acts takes the screen it acts on; the hub picks it, from
//! the Bot, and the model never names one.
//! Docker, AWS EC2 ([`ec2::Ec2Provider`]), E2B ([`e2b::E2bProvider`]), boat.dev
//! ([`boat::BoatProvider`]) and a remote computer a person points at ([`remote::RemoteProvider`])
//! are built. Lume, Docker Sandboxes and Fly have their slot ([`planned::Planned`]), and each says
//! so in a sentence until it is built. [`ProviderKind::from_env`] (`BUWIZ_COMPUTER_PROVIDER`) picks
//! the provider new computers are made with.
//!
//! Credentials are read in two places today, both from the environment as a development source,
//! and each is the one place for its providers: [`ec2::from_env`] (the `BUWIZ_EC2_*` settings, and
//! keys through [`ec2::creds::Keys`] from the environment or the `buwiz-broker` profile) for EC2,
//! and [`cloud::settings`] (an API key and an image, as a [`cloud::Settings`]) for E2B and
//! boat.dev. A per-person source replaces those two functions and nothing else.
//!
//! What a provider can't do stays a method: it answers [`Problem::Unsupported`], so the hub's
//! routes and tools are the same whatever the computer is. An SSH host with no box has `run` and
//! files and no screen.
//!
//! Nothing here panics: a gone box, a daemon that is down and a quiet guest are all typed
//! [`Problem`]s, each with the sentence a person reads.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod boat;
pub mod cloud;
pub mod docker;
pub mod e2b;
pub mod ec2;
pub mod gate;
pub mod guest;
pub mod job;
#[cfg(feature = "mock")]
pub mod loopback;
#[cfg(feature = "mock")]
pub mod mock;
pub mod planned;
pub mod remote;

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use docker::DockerProvider;
pub use boat::BoatProvider;
pub use e2b::E2bProvider;
pub use ec2::Ec2Provider;
pub use gate::{Gate, Mode};
pub use guest::BoxGuest;
pub use planned::Planned;
pub use remote::RemoteProvider;
/// The box's own client, rules and address policy for the browser's network (the computer repo's
/// `box-egress`): the hub runs a computer's tunnel client with it.
pub use box_egress;

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

fn hidden_keys() -> &'static RwLock<std::collections::HashMap<String, Vec<Secret>>> {
    static HIDDEN: std::sync::OnceLock<RwLock<std::collections::HashMap<String, Vec<Secret>>>> = std::sync::OnceLock::new();
    HIDDEN.get_or_init(RwLock::default)
}

/// Makes [`Machine::scrub`] hide `secrets` too: the keys `owner` holds in memory (the broker names
/// itself and the provider). Each call replaces what that owner had, so an old key stops being one
/// to look for; an empty list lets it go.
pub fn hide(owner: &str, secrets: Vec<Secret>) {
    hidden_keys().write().unwrap_or_else(|e| e.into_inner()).insert(owner.to_owned(), secrets);
}

fn hidden() -> Vec<Secret> {
    hidden_keys().read().unwrap_or_else(|e| e.into_inner()).values().flatten().cloned().collect()
}

/// Settings a person can change while the provider runs. The provider reads them at each use, so
/// a new key takes effect at once and no connection is dropped for it.
#[derive(Debug, Default)]
pub struct Shared<T>(Arc<RwLock<T>>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Clone> Shared<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(RwLock::new(value)))
    }

    pub fn get(&self) -> T {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, value: T) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = value;
    }
}

/// Names the provider new computers are made with ([`ProviderKind::from_env`]).
pub const PROVIDER_ENV: &str = "BUWIZ_COMPUTER_PROVIDER";

/// Who runs the computer. The saved word survives a build that doesn't know the provider yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// a machine a person points at: a box over SSH or TLS, or a plain SSH host
    Remote,
    /// an E2B sandbox, from a template built on the computer image ([`e2b`])
    E2b,
    /// a boat.dev sandbox, reached over SSH only ([`boat`])
    Boat,
    /// a Fly Machine: its slot only, until it's built
    Fly,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 8] = [Self::Docker, Self::Lume, Self::Sbx, Self::Ec2, Self::Remote, Self::E2b, Self::Boat, Self::Fly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Lume => "lume",
            Self::Sbx => "sbx",
            Self::Ec2 => "ec2",
            Self::Remote => "remote",
            Self::E2b => "e2b",
            Self::Boat => "boat",
            Self::Fly => "fly",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == word)
    }

    /// Where a new computer is made: `BUWIZ_COMPUTER_PROVIDER` (`docker`, `ec2`, `e2b`, `boat`, …),
    /// Docker when it says nothing or a word this build doesn't know, so a typo never makes a
    /// computer somewhere nobody chose.
    pub fn from_env() -> Self {
        let word = std::env::var(PROVIDER_ENV).unwrap_or_default();
        let word = word.trim();
        Self::parse(word).unwrap_or_else(|| {
            if !word.is_empty() {
                eprintln!("buwiz-computer: {PROVIDER_ENV}={word:?} isn't a provider; making computers with Docker");
            }
            Self::Docker
        })
    }

    /// What a person calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Docker => "Docker on this Mac",
            Self::Lume => "Lume",
            Self::Sbx => "Docker Sandboxes",
            Self::Ec2 => "AWS EC2",
            Self::Remote => "A remote computer",
            Self::E2b => "E2B",
            Self::Boat => "boat.dev",
            Self::Fly => "Fly.io",
        }
    }
}

/// What a computer is, as far as reaching it goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Reach {
    Container { name: String },
    /// a box behind TLS, at `https://<name>`: see [`remote`]
    Url { url: String },
    /// a sandbox a platform's API names by `id`: see [`cloud`] and [`e2b`]
    Sandbox { id: String },
    Ssh { host: String, user: Option<String>, port: Option<u16> },
    /// a machine a cloud runs for the broker, by the cloud's id for it: an EC2 instance
    Instance { id: String, region: String },
}

/// What a person handed over for a computer that isn't the Bot's to run. No secret here is ever
/// sent to the far end but the box's own token and viewer password.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// a `buwiz-computer` box answers there. Without one: commands and files over SSH, no screen
    pub boxed: bool,
    /// who the far end must prove to be: an SSH host key line (`ssh-ed25519 AAAA…`), or the SHA-256
    /// of a TLS certificate in hex. A person saw it and said yes. `None` until they have, and
    /// nothing is sent before then
    pub pin: Option<String>,
    /// the SSH private key. Never in the saved record: the broker keeps it in its keystore and
    /// puts it here when it reads the record
    #[serde(skip)]
    pub key: Option<Secret>,
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
    /// set for a remote computer
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,
    /// what it was made with for its browser's network: `None` for a computer made without the
    /// proxy, which can't take browsing rules or the tunnel until it's made again
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress: Option<Egress>,
    /// what [`Provider::look`] found for this request; never saved
    #[serde(skip)]
    pub seen: Option<Seen>,
}

/// How a new computer's browser reaches the web. Fixed when it's made: Docker can't add a published
/// port or a setting to a container afterwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Network {
    /// as before: the browser goes straight out, under the network wall alone
    #[default]
    Plain,
    /// every screen's browser goes through the box's proxy, under the computer's rules, and the box
    /// dials each site itself
    Proxy,
    /// the proxy, and the tunnel's WebSocket on a loopback port of this Mac, so a person can turn
    /// the tunnel on without making it again
    Tunnel,
}

/// A computer made with the browser's proxy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Egress {
    /// made with the tunnel's WebSocket too
    pub tunnel: bool,
    /// what the broker's tunnel client presents: 32 random bytes, made with the computer, staged by
    /// file before each start as the token is, never in an argument or a setting
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer: Option<Secret>,
}

/// The box's port for the tunnel's WebSocket, published on loopback with the tunnel only.
pub const EGRESS_PORT: u16 = 8790;

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
    /// both its containers come from an image that runs commands as users of their own, read from
    /// Docker's records of them and not from anything inside
    pub users_apart: bool,
    /// guest port → `127.0.0.1:port`
    pub ports: std::collections::HashMap<u16, String>,
    /// made with the browser's proxy, by what Docker says the container was made with: `Some(true)`
    /// with the tunnel too
    pub egress: Option<bool>,
}

impl Machine {
    /// `text` without this computer's secrets, or any provider's keys ([`hide`]): the last check on
    /// anything bound for the model.
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_owned();
        let key = self.link.as_ref().and_then(|l| l.key.as_ref());
        let bearer = self.egress.as_ref().and_then(|e| e.bearer.as_ref());
        for s in [&self.token, &self.viewer_password].into_iter().chain(key).chain(bearer) {
            if !s.expose().is_empty() {
                out = out.replace(s.expose(), "[hidden]");
            }
        }
        for s in hidden() {
            if !s.expose().is_empty() {
                out = out.replace(s.expose(), "[hidden]");
            }
        }
        out
    }

    /// Whether it has a screen at all: a plain SSH host hasn't.
    pub fn has_screen(&self) -> bool {
        self.link.as_ref().is_none_or(|l| l.boxed)
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
    /// its browser's network, for a provider that can make it so; the others make it [`Network::Plain`]
    pub network: Network,
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

impl Screen {
    /// The Bot whose screen this is.
    pub fn bot(&self) -> Option<&str> {
        self.folder.strip_prefix("agents/").filter(|b| !b.is_empty())
    }
}

/// What a viewer needs: where the page is on this Mac, and the word it wants. The gate keeps it;
/// the app only ever gets a ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Viewer {
    /// `127.0.0.1:port`
    pub addr: String,
    pub password: Secret,
    /// it isn't ours: what comes back from it is read too (`gate.rs`)
    pub far: Option<Far>,
}

/// A viewer at the far end of a connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Far {
    /// the name to ask for instead of `addr`, when a proxy in front goes by it
    pub name: Option<String>,
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

/// One of the box's `/v1/browser/*` routes the broker uses: what reads a page's controls, acts on
/// one by its ref, and keeps a screen's signed-in session. No model tool reaches them: only the
/// broker's own login fill and a person's session commands do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserOp {
    Snap,
    Act,
    State,
}

impl BrowserOp {
    pub fn path(self) -> &'static str {
        match self {
            Self::Snap => "/v1/browser/snap",
            Self::Act => "/v1/browser/act",
            Self::State => "/v1/browser/state",
        }
    }
}

/// The box's answer to a browser call, a refusal included: its status and body as they came, so
/// its `error.code` (`stale_ref`, `confirm_required`, …) is the caller's to read. Never logged: a
/// body could name what a page shows.
#[derive(Clone, PartialEq)]
pub struct BrowserReply {
    pub status: u16,
    pub body: serde_json::Value,
}

impl fmt::Debug for BrowserReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BrowserReply({} {})", self.status, self.code().unwrap_or("ok"))
    }
}

impl BrowserReply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// A refusal's `error.code`.
    pub fn code(&self) -> Option<&str> {
        self.body.pointer("/error/code").and_then(serde_json::Value::as_str)
    }
}

/// The page in front of a screen's browser, as the box's extension last reported it
/// (`GET /v1/chrome/active-tab`). A desktop's own program writes the report, so it says which page
/// is in front, and can't prove nothing lied about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveTab {
    /// `None` when nothing has reported
    pub url: Option<String>,
    /// `password`, `text`, `other` or `none`
    pub focus: Option<String>,
    pub seq: Option<u64>,
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

/// Something of a computer that's gone, still on the machine its provider runs on: a Docker
/// volume whose box is no more, say. Only ever listed: what's in it is a person's to keep or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leftover {
    /// the provider's own name for it, as a person would look it up
    pub name: String,
    /// the computer it was made for, when it says
    pub id: Option<String>,
    /// the box it belonged to: the name its record would have
    pub owner: String,
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
    /// another run of this Bot has the screen, until it ends
    Held,
    /// a person has been on the screen for longer than the agent waits
    PersonOn,
    /// it's walled off from the Mac, or it doesn't start
    Unwalled(String),
    /// a connection to a remote computer: already the sentence a person reads
    Remote(String),
    /// the keys or settings a person gave for a provider didn't work, or can't be changed now:
    /// `code` says how (`bad_key`, `expired`, `no_key`, `sso_profile`, `region`, `not_allowed`,
    /// `no_template`, `in_use`, …), and `message` is already the sentence a person reads
    Refused { code: &'static str, message: String },
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
            Self::Held => "held",
            Self::PersonOn => "person_on",
            Self::Unwalled(_) => "unwalled",
            Self::Remote(_) => "remote",
            Self::Refused { code, .. } => code,
            Self::Failed(_) => "failed",
        }
    }

    /// One plain sentence, for the pane and the model.
    pub fn sentence(&self) -> String {
        match self {
            Self::ProviderMissing(what) => format!("{what} isn't installed on this Mac. Install it, then click the computer again."),
            Self::ProviderDown(what) => format!("{what} isn't running. Open it, wait until it says it's running, then click the computer again."),
            Self::ImageMissing(image) => format!("This Mac doesn't have the computer's image ({image}). Build or pull it, then click the computer again."),
            Self::NotBuilt(kind) => format!("{} computers aren't built yet. Lume, Docker Sandboxes and Fly.io are still to come; every other kind of computer works today.", kind.label()),
            Self::Gone => "This computer is gone. Click it to make a new one.".to_owned(),
            Self::Unsupported(what) => format!("This computer can't {what}."),
            Self::Guest { message, .. } => format!("The computer said: {message}"),
            Self::Asleep => "The computer is asleep. Only a person can wake it: ask them to click it in the Computer tab.".to_owned(),
            Self::Held => "Another run of this Bot is using this screen, and it keeps it until it ends. Nothing was done here: try again once it has finished.".to_owned(),
            Self::PersonOn => "A person is still using this screen, so nothing was done. Try again later, or ask them when they'll be finished.".to_owned(),
            Self::Unwalled(why) => format!("The computer didn't start, because it couldn't be walled off from this Mac: {why}"),
            Self::Remote(why) => why.clone(),
            Self::Refused { message, .. } => message.clone(),
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
    /// A person's Check of the keys and settings this provider has: [`Provider::check`], and a call
    /// to the platform that proves the key works and a computer could be made, with nothing made.
    async fn verify(&self) -> Result<()> {
        self.check().await
    }

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

    /// What the last [`Provider::sleep`] of it did that a person should know, as a sentence: a
    /// light sleep that couldn't keep what was open, say. The sleep still worked, so it isn't a
    /// [`Problem`]. Taken: a second ask has nothing until another sleep leaves one.
    fn take_sleep_note(&self, _machine: &Machine) -> Option<String> {
        None
    }

    /// Removes it and everything it kept. Only for a computer a person agreed to delete, or one
    /// just made that holds nothing yet.
    async fn destroy(&self, machine: &Machine) -> Result<()>;

    /// [`Provider::destroy`] of computer `id` when its saved record is lost. The same rule. `last`
    /// is where it was, when that's still known.
    async fn sweep(&self, _id: &str, _last: Option<&Reach>) -> Result<()> {
        Ok(())
    }

    /// What's left of computers whose box is gone, files and all, whether or not a record names
    /// them: the caller knows which records it has. Nothing is removed. `None` when this provider
    /// can't say, which is never "none left".
    async fn leftovers(&self) -> Result<Option<Vec<Leftover>>> {
        Ok(None)
    }

    /// A record for computer `spec.id` from what's left of it, with new secrets, when its saved
    /// one is lost: its files are still there. `last` is where it was, when that's still known, so
    /// its files can be found with nothing else left. `None` when nothing of it is found.
    async fn recover(&self, _spec: &Spec, _last: Option<&Reach>) -> Result<Option<Machine>> {
        Ok(None)
    }

    /// What the last [`Provider::recover`] of computer `id` left alone that a person should know,
    /// as a sentence: other things of it still there, say. Taken: a second ask has nothing.
    fn take_recover_note(&self, _id: &str) -> Option<String> {
        None
    }

    /// Why a computer of this kind found with its record lost can't be kept, as the sentence a
    /// person reads: [`Provider::recover`] never gives one back, whatever is found. `None` when it
    /// can, which is what a person is offered Keep for.
    fn cant_keep_lost(&self) -> Option<String> {
        None
    }

    /// Whether anything of computer `id`, its saved record lost, is there with these settings: what
    /// [`Provider::sweep`] would find, by its id and `last`. Nothing is changed. `None` when this
    /// provider can't tell.
    async fn finds(&self, _id: &str, _last: Option<&Reach>) -> Result<Option<bool>> {
        Ok(None)
    }

    /// Before anyone uses an awake computer: whatever must hold for it to be safe to use, checked
    /// for this start of it. A computer that fails this is stopped.
    async fn guard(&self, _machine: &Machine) -> Result<()> {
        Ok(())
    }

    /// Before a person is given `screen`: ends every program of that screen's desktop user that
    /// isn't the desktop or browser, and names what it ended. Ended, because a stopped program that
    /// holds an X connection reads the keys it missed once it's resumed. Commands are another user
    /// with no way to that screen, so they carry on, and so does every other Bot.
    async fn clear_strays(&self, _machine: &Machine, _screen: &Screen) -> Result<Vec<String>> {
        unsupported("hand a screen to a person")
    }

    /// Whether these Bots are kept apart on this computer, by a check made from outside it. Nothing
    /// the computer says of itself counts, and not being able to tell is a no.
    async fn users_apart(&self, _machine: &Machine, _bots: &[String]) -> Result<bool> {
        Ok(false)
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

    /// `POST /v1/browser/<op>` on `screen`, with `body` as given: the box's answer, refusals
    /// included. `body` may hold what's typed, so nothing of it is logged here or below.
    async fn browser(&self, _machine: &Machine, _screen: &Screen, _op: BrowserOp, _body: &serde_json::Value) -> Result<BrowserReply> {
        unsupported("use a page by its controls")
    }

    /// The page in front of `screen`'s browser.
    async fn active_tab(&self, _machine: &Machine, _screen: &Screen) -> Result<ActiveTab> {
        unsupported("say which page is in front")
    }

    /// `GET /v1/info`'s `capabilities`: what the box's image says it can do.
    async fn capabilities(&self, _machine: &Machine) -> Result<serde_json::Value> {
        unsupported("say what it can do")
    }

    /// `GET /v1/egress`: the browser's proxy and tunnel, as the box says. Names no site.
    async fn egress(&self, _machine: &Machine) -> Result<serde_json::Value> {
        unsupported("take browsing rules")
    }

    /// `PUT /v1/egress/rules`: puts a rules document in force while the box runs. `Ok` only once
    /// the box says that set is in force.
    async fn set_egress_rules(&self, _machine: &Machine, _rules: &serde_json::Value) -> Result<serde_json::Value> {
        unsupported("take browsing rules")
    }

    /// Where this Mac reaches the tunnel's WebSocket (`ws://127.0.0.1:port`), for an awake computer
    /// made with the tunnel: read for this request, like every port.
    async fn tunnel_url(&self, _machine: &Machine) -> Result<Option<String>> {
        Ok(None)
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

/// The provider for `kind`: Docker's, EC2's, a remote one, E2B's, boat.dev's, or the slot of one
/// that isn't built, each with its settings from the environment (a developer's). A remote or
/// cloud provider holds its connections, so the hub keeps one of each kind, with a person's
/// settings, and doesn't ask here for each call.
pub fn provider_for(kind: ProviderKind) -> std::sync::Arc<dyn Provider> {
    match kind {
        ProviderKind::Docker => std::sync::Arc::new(DockerProvider::from_env()),
        ProviderKind::Ec2 => std::sync::Arc::new(Ec2Provider::from_env()),
        ProviderKind::Remote => std::sync::Arc::new(RemoteProvider::new()),
        ProviderKind::E2b => std::sync::Arc::new(E2bProvider::new(cloud::settings(kind))),
        ProviderKind::Boat => std::sync::Arc::new(BoatProvider::new(cloud::settings(kind))),
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
            link: None,
            egress: None,
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
    fn scrub_hides_a_providers_keys_and_forgets_the_old_ones() {
        hide("test-owner", vec![Secret::new("prov-key-1")]);
        assert_eq!(machine().scrub("a prov-key-1 b"), "a [hidden] b");
        hide("test-owner", vec![Secret::new("prov-key-2")]);
        assert_eq!(machine().scrub("prov-key-1 prov-key-2"), "prov-key-1 [hidden]");
        hide("test-owner", Vec::new());
    }

    #[test]
    fn scrub_hides_both_secrets() {
        assert_eq!(machine().scrub("a tok-123 b pw-456 c"), "a [hidden] b [hidden] c");
    }

    #[test]
    fn scrub_hides_a_remote_computers_key_and_debug_doesnt_show_it() {
        let m = Machine { link: Some(Link { boxed: false, pin: None, key: Some(Secret::new("key-789")) }), ..machine() };
        assert_eq!(m.scrub("a key-789 c"), "a [hidden] c");
        assert!(!format!("{m:?}").contains("key-789"));
    }

    #[test]
    fn a_saved_record_doesnt_hold_the_ssh_key() {
        let m = Machine { link: Some(Link { boxed: false, pin: Some("p".into()), key: Some(Secret::new("key-789")) }), ..machine() };
        let saved = serde_json::to_string(&m).unwrap();
        assert!(!saved.contains("key-789"), "{saved}");
        assert_eq!(serde_json::from_str::<Machine>(&saved).unwrap().link.unwrap().key, None);
    }

    #[test]
    fn only_a_plain_ssh_host_has_no_screen() {
        assert!(machine().has_screen());
        let link = |boxed| Some(Link { boxed, pin: None, key: None });
        assert!(Machine { link: link(true), ..machine() }.has_screen());
        assert!(!Machine { link: link(false), ..machine() }.has_screen());
    }

    #[test]
    fn a_record_from_before_links_still_reads() {
        let old = r#"{"provider":"docker","reach":{"kind":"container","name":"c"},"image":null,"token":"t","viewer_password":"v","dir":"/x"}"#;
        let m = serde_json::from_str::<Machine>(old).unwrap();
        assert_eq!(m.link, None);
        assert_eq!(m.egress, None, "a computer from before the proxy was made without it");
    }

    #[test]
    fn the_tunnels_bearer_is_scrubbed_kept_out_of_debug_and_saved_with_the_record() {
        let m = Machine { egress: Some(Egress { tunnel: true, bearer: Some(Secret::new("bearer-0a1b2c")) }), ..machine() };
        assert_eq!(m.scrub("x bearer-0a1b2c y"), "x [hidden] y");
        assert!(!format!("{m:?}").contains("bearer-0a1b2c"));
        let saved = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Machine>(&saved).unwrap().egress, m.egress);
    }

    #[test]
    fn a_computer_can_be_a_container_a_url_or_an_ssh_host() {
        for reach in [
            Reach::Container { name: "c".into() },
            Reach::Url { url: "https://x.example/".into() },
            Reach::Ssh { host: "h".into(), user: Some("u".into()), port: Some(2222) },
            Reach::Instance { id: "i-0123456789abcdef0".into(), region: "ap-southeast-1".into() },
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
        assert!(Problem::NotBuilt(ProviderKind::Lume).sentence().contains("Lume"));
    }
}
