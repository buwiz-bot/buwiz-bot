//! A computer a person points at: a running `buwiz-computer` box somewhere else, or a plain SSH
//! host. Nothing here makes one, and nothing here removes one: this provider opens a connection
//! and closes it.
//!
//! - **`ssh://user@host`, with a box.** One `ssh` master ([`ssh`]) forwards the box's ports, which
//!   are on that machine's loopback (the image's `docker-compose.yml`), to ports here.
//! - **`https://<name>`, with a box.** A proxy on the far end speaks TLS for each service:
//!   `exec.<name>`, `host.<name>` and `desktop-690N.<name>`. A relay ([`tls`]) per service ends on
//!   a port here.
//! - **`ssh://user@host`, no box.** Commands and files over the master, in
//!   `~/buwiz-computer/agents/<bot>`. There is no screen, and every screen call says so.
//!
//! Either way a box ends up at plain HTTP on loopback, so [`BoxGuest`] and the gate don't change,
//! and those ports are in the range the Bot's shell is kept off.
//!
//! **Nothing the far end says is trusted.** It proves who it is first: an SSH host key or a
//! certificate a person saw and pinned. No secret goes before that, whatever the public roots say
//! of a certificate. It can answer anything after that, so every read has a limit and a time, a redirect is an error, sentences
//! that reach a person are ours and not its own, and its viewer goes through the gate's far rules.
//!
//! **Nothing here touches the machine's data.** Sleep is a disconnect. So is retiring a computer:
//! only the local record goes. A Bot leaving doesn't even send `forget`, because the screens on a
//! box a person runs are theirs to clear. There is no outside check to be had, so
//! [`Provider::users_apart`] stays a no, and with that the hub's refusals for a shared computer and
//! for handing a screen over stay on.
//!
//! The hub keeps one of these, because it holds the connections.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;

use crate::docker::{EXEC_PORT, HOST_PORT, VIEWER_PORTS};
use crate::guest::BoxGuest;
use crate::{CommandOutput, Far, Input, Link, Machine, Power, Problem, Provider, ProviderKind, Reach, Result, Screen, Screenshot, Secret, Settled, Sleep, Spec, Viewer, WaitFor, Woke, DOCKER_PORT_LAST, PORT_FIRST};

pub mod ssh;
pub mod tls;

/// Where a plain SSH host keeps what the Bots make, under that login's home.
const SHELL_ROOT: &str = "buwiz-computer";
/// How long a box or a plain host gets to prove it's there before anyone uses it.
pub(crate) const CHECK_PATIENCE: Duration = Duration::from_secs(5);
/// How long a box gets to answer once the tunnel is up.
pub(crate) const FIRST_ANSWER: Duration = Duration::from_secs(15);
/// The biggest key file a person can mean.
const MAX_KEY: u64 = 64 * 1024;
/// The most of a command's output or a file that comes back over plain SSH.
const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_FILE: usize = 8 * 1024 * 1024;
/// A file-tool call over plain SSH.
const FILE_PATIENCE: Duration = Duration::from_secs(30);
const SERVICES: usize = 2 + VIEWER_PORTS.len();
/// How often a box is asked if it's there, and how many misses in a row mean it isn't.
pub(crate) const BEAT: Duration = Duration::from_secs(15);
const MISSES: u32 = 3;

const NOT_THIS_WAY: &str = "A remote computer isn't made here: add it in the Computer tab.";
const NO_BOX: &str = "Nothing that answers there is a Buwiz computer. Check that the box is running on that machine.";
const BAD_TOKEN: &str = "The box refused this token.";
const NEEDS_TOKENS: &str = "A box needs its token and its viewer password.";
const DROPPED: &str = "The connection to this computer dropped. Click it to connect again.";
const TOO_BIG: &str = "is too big to read whole here, so none of it was read. Use computer_run to look at part of it.";
const NOT_POSIX: &str = "That machine didn't run a plain shell command, so it can't be used over SSH.";

/// What a person typed in the form, not yet checked.
#[derive(Clone, Debug, Default)]
pub struct Asked {
    pub url: String,
    /// the box's bearer; with none, a plain SSH host
    pub token: Option<Secret>,
    pub viewer_password: Option<Secret>,
    /// where the SSH private key is, on this Mac
    pub key_file: Option<PathBuf>,
    pub pin: Option<String>,
}

impl Asked {
    /// The record this asks for, kept in `dir`. Nothing is reached yet.
    pub fn machine(&self, dir: &Path) -> Result<Machine> {
        let reach = target(&self.url)?;
        let filled = |s: &Option<Secret>| s.clone().filter(|s| !s.expose().trim().is_empty());
        let (token, password) = (filled(&self.token), filled(&self.viewer_password));
        let boxed = token.is_some() || password.is_some();
        if boxed && (token.is_none() || password.is_none()) {
            return Err(Problem::Remote(NEEDS_TOKENS.to_owned()));
        }
        let pin = self.pin.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned);
        let key = match reach {
            Reach::Ssh { .. } => {
                let path = self.key_file.as_deref().ok_or_else(|| Problem::Remote("SSH needs a private key file.".to_owned()))?;
                if let Some(pin) = &pin {
                    ssh::check_pin(pin)?;
                }
                Some(read_key(path)?)
            }
            _ if !boxed => return Err(Problem::Remote(NEEDS_TOKENS.to_owned())),
            _ => {
                if let Some(pin) = &pin {
                    tls::check_pin(pin)?;
                }
                None
            }
        };
        Ok(Machine {
            provider: ProviderKind::Remote,
            reach,
            image: None,
            token: token.unwrap_or_else(|| Secret::new("")),
            viewer_password: password.unwrap_or_else(|| Secret::new("")),
            dir: dir.to_owned(),
            link: Some(Link { boxed, pin, key }),
            seen: None,
        })
    }
}

/// Where `url` says the computer is. Only what's built gets through, and a sentence says why not.
pub fn target(url: &str) -> Result<Reach> {
    let url = url.trim();
    let refused = |why: &str| Err(Problem::Remote(why.to_owned()));
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    match scheme.to_ascii_lowercase().as_str() {
        "https" => {
            let name = rest.trim_end_matches('/');
            let (host, port) = split_port(name)?;
            if !dns_name(host) {
                return refused("An https computer goes by a name, like box.example.com, with nothing after it.");
            }
            Ok(Reach::Url { url: port.map_or_else(|| format!("https://{host}"), |p| format!("https://{host}:{p}")) })
        }
        "ssh" | "" => {
            if scheme.is_empty() && !rest.contains('@') {
                return refused("Give the computer as ssh://user@host or https://box.example.com.");
            }
            let (user, at) = rest.trim_end_matches('/').rsplit_once('@').map_or((None, rest.trim_end_matches('/')), |(u, h)| (Some(u), h));
            let (host, port) = split_port(at)?;
            if !host_ok(host) || user.is_some_and(|u| !user_ok(u)) {
                return refused("That SSH address has something in it that isn't a user or a host.");
            }
            Ok(Reach::Ssh { host: host.to_owned(), user: user.map(str::to_owned), port })
        }
        "http" => refused("Plain http would send the computer's token in the open. Use https, or ssh."),
        "vnc" | "rdp" => refused("VNC and RDP computers aren't built yet. A box over SSH or https, or a plain SSH host, works today."),
        _ => refused("A remote computer is an ssh:// or https:// address."),
    }
}

fn split_port(at: &str) -> Result<(&str, Option<u16>)> {
    let Some((host, port)) = at.rsplit_once(':') else { return Ok((at, None)) };
    let port = port.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| Problem::Remote("That address's port isn't a port.".to_owned()))?;
    Ok((host, Some(port)))
}

fn dns_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && name.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) && name.parse::<std::net::IpAddr>().is_err()
}

/// A host for `ssh`'s command line: a name or an address, and never a flag.
fn host_ok(host: &str) -> bool {
    !host.is_empty() && host.len() <= 253 && !host.starts_with('-') && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

fn user_ok(user: &str) -> bool {
    !user.is_empty() && user.len() <= 64 && !user.starts_with('-') && user.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The key at `path`, read now, with `~/` meaning this login's home. It must be a key the `ssh`
/// behind this can use with nobody to type a passphrase.
fn read_key(path: &Path) -> Result<Secret> {
    let bad = |why: &str| Problem::Remote(why.to_owned());
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let path = path.strip_prefix("~").map_or_else(|_| path.to_owned(), |rest| home.join(rest));
    let meta = std::fs::metadata(&path).map_err(|_| bad("That key file can't be read."))?;
    if !meta.is_file() || meta.len() > MAX_KEY {
        return Err(bad("That isn't a private key file."));
    }
    let text = std::fs::read_to_string(&path).map_err(|_| bad("That isn't a private key file."))?;
    if !text.contains("PRIVATE KEY-----") {
        return Err(bad("That isn't a private key file."));
    }
    if key_has_passphrase(&text) {
        return Err(bad("This key has a passphrase, and nobody is there to type it. Use a key made for this computer, with no passphrase."));
    }
    Ok(Secret::new(text))
}

/// Whether a private key's text says it's locked: the old PEM word, or OpenSSH's cipher field.
fn key_has_passphrase(text: &str) -> bool {
    if text.contains("ENCRYPTED") {
        return true;
    }
    let body: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(body.trim()) else { return false };
    let Some(rest) = raw.strip_prefix(b"openssh-key-v1\0") else { return false };
    rest.get(..4).and_then(|n| <[u8; 4]>::try_from(n).ok()).map(u32::from_be_bytes).is_some_and(|n| rest.get(4..4 + n as usize) != Some(b"none"))
}

/// `count` ports of the computers' range that are free now. What's free can be gone by the time
/// `ssh` binds it, which its caller retries.
pub(crate) fn free_ports(count: usize) -> Result<Vec<u16>> {
    let mut start = [0u8; 2];
    getrandom::getrandom(&mut start).map_err(|e| Problem::Failed(format!("no randomness: {e}")))?;
    let span = DOCKER_PORT_LAST - PORT_FIRST + 1;
    let from = u16::from_be_bytes(start) % span;
    let ports: Vec<u16> = (0..span).map(|i| DOCKER_PORT_LAST - (from + i) % span).filter(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok()).take(count).collect();
    if ports.len() < count {
        return Err(Problem::Remote("This Mac has no free ports left for a connection.".to_owned()));
    }
    Ok(ports)
}

/// How a test went, for a person to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probed {
    /// nothing is trusted yet: the far end's key, for a person to check and say yes to. `vouched`:
    /// the public roots take its certificate too
    Trust { fingerprint: String, pin: String, vouched: bool },
    /// it works. `system` is what a plain host said it runs, when that's a word
    Ready { boxed: bool, system: Option<String> },
}

/// One open connection.
pub(crate) struct Open {
    pub(crate) way: Way,
    pub(crate) guest: Option<BoxGuest>,
    /// guest port → port here
    pub(crate) ports: HashMap<u16, u16>,
    /// what a viewer is asked for by, at this guest port
    pub(crate) names: HashMap<u16, String>,
    /// a box stopped answering ([`beat`])
    pub(crate) silent: AtomicBool,
}

pub(crate) enum Way {
    Ssh(Box<ssh::Master>),
    Tls(tls::Relays),
}

impl Open {
    pub(crate) fn alive(&self) -> bool {
        !self.silent.load(Ordering::Relaxed)
            && match &self.way {
                Way::Ssh(master) => master.alive(),
                Way::Tls(_) => true,
            }
    }

    pub(crate) async fn close(&self) {
        match &self.way {
            Way::Ssh(master) => master.close().await,
            Way::Tls(relays) => relays.stop(),
        }
    }

    pub(crate) fn master(&self) -> Result<&ssh::Master> {
        match &self.way {
            Way::Ssh(master) => Ok(master),
            Way::Tls(_) => Err(Problem::Failed("this computer has no SSH connection".to_owned())),
        }
    }
}

/// Asks a box if it's there, every `every`, for as long as its connection lasts: [`Provider::power`]
/// is read too often to ask, and a relay stays up with nothing behind it. A few misses in a row and
/// it's not alive, so the tab shows it and a click connects again.
pub(crate) fn beat(open: Weak<Open>, every: Duration) {
    tokio::spawn(async move {
        let mut misses = 0;
        loop {
            tokio::time::sleep(every).await;
            let Some(open) = open.upgrade() else { return };
            let Some(guest) = &open.guest else { return };
            misses = if guest.ready_within(CHECK_PATIENCE.min(every.max(Duration::from_millis(200)))).await { 0 } else { misses + 1 };
            if misses >= MISSES {
                open.silent.store(true, Ordering::Relaxed);
                return;
            }
        }
    });
}

/// Every remote computer's connections, by the folder the broker keeps for it.
pub struct RemoteProvider {
    ssh: PathBuf,
    keyscan: PathBuf,
    /// where every TLS name goes instead of what it resolves to (tests)
    dial: Option<std::net::SocketAddr>,
    /// a root the public ones are joined by (tests)
    root: Option<Vec<u8>>,
    every: Duration,
    links: Mutex<HashMap<PathBuf, Arc<Open>>>,
    /// one connect or disconnect at a time, per computer
    turns: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl Default for RemoteProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteProvider {
    pub fn new() -> Self {
        Self { ssh: PathBuf::from(ssh::SSH), keyscan: PathBuf::from(ssh::KEYSCAN), dial: None, root: None, every: BEAT, links: Mutex::default(), turns: Mutex::default() }
    }

    /// Sends every TLS name to `addr`, as a test's own resolver would.
    pub fn with_dial(mut self, addr: std::net::SocketAddr) -> Self {
        self.dial = Some(addr);
        self
    }

    /// Takes `root` as one of the public roots, for a test's own certificate authority.
    pub fn with_root(mut self, root: Vec<u8>) -> Self {
        self.root = Some(root);
        self
    }

    /// Asks a box if it's there this often, for tests.
    pub fn with_beat(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    /// Stands in for `ssh` and `ssh-keyscan`, for tests.
    pub fn with_programs(mut self, ssh: PathBuf, keyscan: PathBuf) -> Self {
        self.ssh = ssh;
        self.keyscan = keyscan;
        self
    }

    fn turn(&self, machine: &Machine) -> Arc<tokio::sync::Mutex<()>> {
        self.turns.lock().unwrap_or_else(|e| e.into_inner()).entry(machine.dir.clone()).or_default().clone()
    }

    fn link_of(&self, machine: &Machine) -> Option<Arc<Open>> {
        self.links.lock().unwrap_or_else(|e| e.into_inner()).get(&machine.dir).cloned()
    }

    fn up(&self, machine: &Machine) -> Result<Arc<Open>> {
        self.link_of(machine).filter(|o| o.alive()).ok_or(Problem::Asleep)
    }

    /// The box, or that this computer has none: `what` is what it can't do.
    fn boxed(&self, machine: &Machine, what: &str) -> Result<Arc<Open>> {
        let open = self.up(machine)?;
        if open.guest.is_none() {
            return Err(Problem::Unsupported(what.to_owned()));
        }
        Ok(open)
    }

    async fn drop_link(&self, machine: &Machine) {
        let found = self.links.lock().unwrap_or_else(|e| e.into_inner()).remove(&machine.dir);
        if let Some(open) = found {
            open.close().await;
        }
    }

    /// Hangs up `open`, if it's still this computer's: a check that took its time can come back
    /// after a person reconnected, and that link isn't what failed.
    async fn drop_this(&self, machine: &Machine, open: &Arc<Open>) {
        {
            let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
            if links.get(&machine.dir).is_some_and(|held| Arc::ptr_eq(held, open)) {
                links.remove(&machine.dir);
            }
        }
        open.close().await;
    }

    /// What a person's Test does: who the far end says it is and, once they've trusted that, whether
    /// the way in works. No box token or viewer password goes: those wait for their yes, and
    /// [`RemoteProvider::probe`]. Over TLS that leaves the handshake, and over SSH the login.
    pub async fn trial(&self, machine: &Machine) -> Result<Probed> {
        let link = link_of(machine)?;
        match &machine.reach {
            Reach::Url { url } => match tls::look(&tls::Far::parse(url)?, link.pin.as_deref(), self.dial, self.root.as_deref()).await? {
                tls::Seen::Unknown { fingerprint, vouched } => Ok(Probed::Trust { fingerprint: fingerprint.clone(), pin: fingerprint, vouched }),
                tls::Seen::Trusted => Ok(Probed::Ready { boxed: true, system: None }),
            },
            Reach::Ssh { host, port, .. } if link.pin.is_none() => {
                let (fingerprint, pin) = ssh::scan(&self.keyscan, host, *port).await?;
                Ok(Probed::Trust { fingerprint, pin, vouched: false })
            }
            _ => {
                let mut bare = machine.clone();
                bare.token = Secret::new("");
                bare.viewer_password = Secret::new("");
                if let Some(l) = bare.link.as_mut() {
                    l.boxed = false;
                }
                let open = self.connect(&bare).await?;
                let system = self.system(&open).await;
                open.close().await;
                // a login made for tunnels may run no command, and a box needs none
                if !link.boxed && system.is_none() {
                    return Err(Problem::Remote(NOT_POSIX.to_owned()));
                }
                Ok(Probed::Ready { boxed: link.boxed, system })
            }
        }
    }

    /// Asks the far end who it is and whether it answers, with this record, its secrets included:
    /// for after a person's yes. Leaves nothing open.
    pub async fn probe(&self, machine: &Machine) -> Result<Probed> {
        let link = link_of(machine)?;
        match &machine.reach {
            Reach::Ssh { host, port, .. } if link.pin.is_none() => {
                let (fingerprint, pin) = ssh::scan(&self.keyscan, host, *port).await?;
                Ok(Probed::Trust { fingerprint, pin, vouched: false })
            }
            Reach::Url { url } => {
                let far = tls::Far::parse(url)?;
                match tls::look(&far, link.pin.as_deref(), self.dial, self.root.as_deref()).await? {
                    tls::Seen::Unknown { fingerprint, vouched } => Ok(Probed::Trust { fingerprint: fingerprint.clone(), pin: fingerprint, vouched }),
                    tls::Seen::Trusted => {
                        let open = self.connect(machine).await?;
                        open.close().await;
                        Ok(Probed::Ready { boxed: true, system: None })
                    }
                }
            }
            _ => {
                let open = self.connect(machine).await?;
                let system = if link.boxed { None } else { self.system(&open).await };
                open.close().await;
                if !link.boxed && system.is_none() {
                    return Err(Problem::Remote(NOT_POSIX.to_owned()));
                }
                Ok(Probed::Ready { boxed: link.boxed, system })
            }
        }
    }

    /// What a plain host says it runs: a word, or `None` when it doesn't run a shell command.
    async fn system(&self, open: &Open) -> Option<String> {
        let ran = open.master().ok()?.run("uname -s", &[], None, CHECK_PATIENCE, 64).await.ok()?;
        let word = String::from_utf8_lossy(&ran.stdout).trim().to_owned();
        (ran.code == Some(0) && !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')).then_some(word)
    }

    async fn connect(&self, machine: &Machine) -> Result<Arc<Open>> {
        let link = link_of(machine)?;
        let open = match &machine.reach {
            Reach::Ssh { host, user, port } => {
                let pin = link.pin.as_deref().ok_or_else(|| Problem::Remote("This computer has no trusted SSH key: remove it and add it again.".to_owned()))?;
                let at = ssh::At { host: host.clone(), user: user.clone(), port: *port };
                let mut tries = 0;
                let (master, ports) = loop {
                    let ports = if link.boxed { free_ports(SERVICES)? } else { Vec::new() };
                    let forwards: Vec<(u16, u16)> = ports.iter().copied().zip(guest_ports()).collect();
                    match ssh::Master::open(&self.ssh, &machine.dir, &at, pin, link.key.as_ref(), &forwards).await {
                        Ok(master) => break (master, forwards),
                        Err(ssh::Failed::PortTaken) if tries < 4 => tries += 1,
                        Err(ssh::Failed::PortTaken) => return Err(Problem::Remote("This Mac has no free ports left for a connection.".to_owned())),
                        Err(ssh::Failed::Said(p)) => return Err(p),
                    }
                };
                let ports: HashMap<u16, u16> = ports.into_iter().map(|(here, there)| (there, here)).collect();
                let guest = link.boxed.then(|| guest_at(&ports, machine.token.clone())).transpose()?;
                Open { way: Way::Ssh(Box::new(master)), guest, ports, names: HashMap::new(), silent: AtomicBool::new(false) }
            }
            Reach::Url { url } => {
                let far = tls::Far::parse(url)?;
                let pin = link.pin.as_deref().ok_or_else(|| Problem::Remote(tls::UNTRUSTED.to_owned()))?;
                tls::look(&far, Some(pin), self.dial, None).await?;
                let names: HashMap<u16, String> = guest_ports().map(|p| (p, far.service(p))).collect();
                let (relays, ports) = tls::Relays::open(&far, &names, pin, self.dial).await?;
                let guest = guest_at(&ports, machine.token.clone())?.named(&names[&EXEC_PORT], &names[&HOST_PORT]);
                Open { way: Way::Tls(relays), guest: Some(guest), ports, names, silent: AtomicBool::new(false) }
            }
            Reach::Container { .. } | Reach::Sandbox { .. } => return Err(Problem::Failed("that computer is not reached over a remote connection".to_owned())),
        };
        let open = Arc::new(open);
        if let Err(p) = answers(&open).await {
            open.close().await;
            return Err(p);
        }
        beat(Arc::downgrade(&open), self.every);
        Ok(open)
    }
}

/// Every guest port a box has, in one order.
pub(crate) fn guest_ports() -> impl Iterator<Item = u16> {
    [EXEC_PORT, HOST_PORT].into_iter().chain(VIEWER_PORTS)
}

pub(crate) fn guest_at(ports: &HashMap<u16, u16>, token: Secret) -> Result<BoxGuest> {
    let at = |p: u16| ports.get(&p).map(|here| format!("http://127.0.0.1:{here}")).ok_or_else(|| Problem::Failed("a port isn't forwarded".to_owned()));
    Ok(BoxGuest::connect(&at(EXEC_PORT)?, &at(HOST_PORT)?, token))
}

/// A box answers, takes the token and says it's a box. What it says is good for this much: it
/// widens nothing.
pub(crate) async fn answers(open: &Open) -> Result<()> {
    let Some(guest) = &open.guest else { return Ok(()) };
    let info = tokio::time::timeout(FIRST_ANSWER, guest.info()).await.unwrap_or(Err(Problem::Guest { status: 504, message: String::new() }));
    let info = match info {
        Ok(info) => info,
        Err(Problem::Guest { status: 401 | 403, .. }) => return Err(Problem::Remote(BAD_TOKEN.to_owned())),
        Err(_) => return Err(Problem::Remote(NO_BOX.to_owned())),
    };
    let viewer = info.pointer("/capabilities/viewer/kind").and_then(Value::as_str);
    if viewer != Some("voxl") || !guest.ready_within(FIRST_ANSWER).await || guest.listed().await.is_err() {
        return Err(Problem::Remote(NO_BOX.to_owned()));
    }
    Ok(())
}

fn link_of(machine: &Machine) -> Result<&Link> {
    machine.link.as_ref().ok_or_else(|| Problem::Failed("this computer's record has no connection".to_owned()))
}

/// Anything from a box, with this computer's secrets out of what it said.
pub(crate) fn clean<T>(machine: &Machine, got: Result<T>) -> Result<T> {
    got.map_err(|p| match p {
        Problem::Guest { status, message } => Problem::Guest { status, message: machine.scrub(&message) },
        other => other,
    })
}

/// `/workspace/<rel>` as a plain host's path relative to its root, or `path` as it came.
fn relative(path: &str) -> &str {
    path.strip_prefix("/workspace/").unwrap_or(path)
}

/// Prints the real root, then where `$1` under it really is, walking up past what doesn't exist
/// yet. A link at the end that isn't a folder is refused: `cd -P` can't say where it goes.
const RESOLVE: &str = r#"root=$HOME/$2; mkdir -p -- "$root" && root=$(cd -P -- "$root" && pwd) || exit 9
p=$root/$1; tail=
while [ ! -e "$p" ] && [ ! -L "$p" ]; do tail=/${p##*/}$tail; p=${p%/*}; done
if [ -d "$p" ]; then real=$(cd -P -- "$p" && pwd)$tail
elif [ -L "$p" ]; then exit 8
else real=$(cd -P -- "${p%/*}" && pwd)/${p##*/}$tail; fi
printf '%s\n%s\n' "$root" "$real""#;

/// Writes stdin to `$HOME/$1`, all `$2` bytes of it or not at all: a link that drops part way
/// leaves the old file as it was. A folder there is refused: `mv` would put the file inside it. Over
/// an old file the new one is private until it has the old one's mode, and stays private if that
/// can't be read, since a wider mode could show what the old one hid.
const WRITE: &str = r#"f=$HOME/$1; [ ! -d "$f" ] || exit 6
mkdir -p -- "${f%/*}" || exit 1
t=$f.buwiz-$$
if [ -f "$f" ]; then
  (umask 077; set -C; : > "$t") || exit 1
  m=$(stat -c %a -- "$f" 2>/dev/null || stat -f %Lp -- "$f" 2>/dev/null) && chmod "$m" "$t"
else (set -C; : > "$t") || exit 1; fi
cat > "$t"; got=$(wc -c < "$t")
if [ "$got" -eq "$2" ] && [ ! -d "$f" ] && mv -f -- "$t" "$f"; then exit 0; fi
rm -f -- "$t"; exit 7"#;

/// What a read over plain SSH comes to: all of the file, or why not. A cut read as whole is what
/// gets written back over it.
fn read_answer(ran: ssh::Ran, path: &str) -> Result<Vec<u8>> {
    if ran.timed_out {
        return Err(Problem::Guest { status: 504, message: format!("{path} took too long to read, so none of it was read. The connection may be slow.") });
    }
    if ran.code != Some(0) {
        return Err(Problem::Guest { status: 404, message: format!("{path} can't be read") });
    }
    if ran.truncated {
        return Err(Problem::Guest { status: 413, message: format!("{path} {TOO_BIG}") });
    }
    Ok(ran.stdout)
}

#[async_trait]
impl Provider for RemoteProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Remote
    }

    async fn check(&self) -> Result<()> {
        Ok(())
    }

    async fn create(&self, _: &Spec) -> Result<Machine> {
        Err(Problem::Remote(NOT_THIS_WAY.to_owned()))
    }

    /// From what's held here, with no call to the far end: this is read every few seconds.
    async fn power(&self, machine: &Machine) -> Result<Power> {
        Ok(if self.link_of(machine).is_some_and(|o| o.alive()) { Power::Awake } else { Power::Asleep })
    }

    /// Connects. `Started`, always: a box may have restarted while nobody was connected, so every
    /// Bot's screen is asked for again, and what's running is found.
    async fn wake(&self, machine: &Machine) -> Result<Woke> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        if self.link_of(machine).is_some_and(|o| o.alive()) {
            return Ok(Woke::Started);
        }
        self.drop_link(machine).await;
        let open = self.connect(machine).await?;
        self.links.lock().unwrap_or_else(|e| e.into_inner()).insert(machine.dir.clone(), open);
        Ok(Woke::Started)
    }

    /// Disconnects. Whatever the depth, nothing at the far end is told to stop.
    async fn sleep(&self, machine: &Machine, _: Sleep) -> Result<()> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        self.drop_link(machine).await;
        Ok(())
    }

    /// Disconnects. The record is the broker's to remove, and nothing else goes.
    async fn destroy(&self, machine: &Machine) -> Result<()> {
        self.sleep(machine, Sleep::Deep).await?;
        self.turns.lock().unwrap_or_else(|e| e.into_inner()).remove(&machine.dir);
        Ok(())
    }

    /// That it's still there, for this request. A connection that fails this is closed.
    async fn guard(&self, machine: &Machine) -> Result<()> {
        let open = self.up(machine)?;
        let fine = match &open.guest {
            Some(guest) => guest.ready_within(CHECK_PATIENCE).await,
            None => open.master()?.run(":", &[], None, CHECK_PATIENCE, 64).await.is_ok_and(|r| r.code == Some(0)),
        };
        if !fine {
            self.drop_this(machine, &open).await;
            return Err(Problem::Remote(DROPPED.to_owned()));
        }
        Ok(())
    }

    async fn screen_up(&self, machine: &Machine, bot: &str) -> Result<Screen> {
        let open = self.up(machine)?;
        if let Some(guest) = &open.guest {
            return clean(machine, guest.screen_up(bot).await);
        }
        let folder = format!("agents/{bot}");
        let ran = open.master()?.run(r#"mkdir -p -- "$HOME/$1""#, &[&format!("{SHELL_ROOT}/{folder}")], None, FILE_PATIENCE, 1024).await?;
        if ran.code != Some(0) {
            return Err(Problem::Remote("That machine wouldn't make this Bot's folder in its home.".to_owned()));
        }
        Ok(Screen { display: String::new(), viewer_port: 0, folder, profile: None })
    }

    /// Nothing at all: a box a person runs keeps its screens until they clear them.
    async fn screen_down(&self, _: &Machine, _: &str) -> Result<()> {
        Ok(())
    }

    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        let open = self.up(machine)?;
        if open.guest.is_none() {
            return Ok(None);
        }
        Ok(open.ports.get(&screen.viewer_port).map(|here| Viewer { addr: format!("127.0.0.1:{here}"), password: machine.viewer_password.clone(), far: Some(Far { name: open.names.get(&screen.viewer_port).cloned() }) }))
    }

    async fn screenshot(&self, machine: &Machine, screen: &Screen) -> Result<Screenshot> {
        let open = self.boxed(machine, "show its screen")?;
        let Some(guest) = &open.guest else { return Err(Problem::Asleep) };
        clean(machine, guest.screenshot(screen).await)
    }

    async fn input(&self, machine: &Machine, screen: &Screen, input: &Input) -> Result<()> {
        let open = self.boxed(machine, "take input")?;
        let Some(guest) = &open.guest else { return Err(Problem::Asleep) };
        clean(machine, guest.input(screen, input).await)
    }

    async fn run(&self, machine: &Machine, screen: &Screen, command: &str, timeout_ms: u64) -> Result<CommandOutput> {
        let open = self.up(machine)?;
        if let Some(guest) = &open.guest {
            return clean(machine, guest.run(screen, command, timeout_ms).await);
        }
        let folder = format!("{SHELL_ROOT}/{}", screen.folder);
        let script = r#"mkdir -p -- "$HOME/$1" && cd -- "$HOME/$1" && exec /bin/sh -c "$2""#;
        let ran = open.master()?.run(script, &[&folder, command], None, Duration::from_millis(timeout_ms), MAX_OUTPUT).await?;
        Ok(CommandOutput { exit_code: ran.code, stdout: String::from_utf8_lossy(&ran.stdout).into_owned(), stderr: String::from_utf8_lossy(&ran.stderr).into_owned(), truncated: ran.truncated, timed_out: ran.timed_out })
    }

    async fn open_url(&self, machine: &Machine, screen: &Screen, url: &str) -> Result<()> {
        let open = self.boxed(machine, "open a page")?;
        let Some(guest) = &open.guest else { return Err(Problem::Asleep) };
        clean(machine, guest.open_url(screen, url).await)
    }

    /// `/workspace/<path>` for a plain host's path inside its root, so a Bot's folder reads the
    /// same here as on a box. Anything else comes back as it is, and the tools refuse it.
    async fn resolve(&self, machine: &Machine, path: &str) -> Result<String> {
        let open = self.up(machine)?;
        if let Some(guest) = &open.guest {
            return clean(machine, guest.resolve(path).await);
        }
        let ran = open.master()?.run(RESOLVE, &[path, SHELL_ROOT], None, FILE_PATIENCE, 16 * 1024).await?;
        let text = String::from_utf8_lossy(&ran.stdout);
        let mut lines = text.lines();
        match (ran.code, lines.next(), lines.next()) {
            (Some(0), Some(root), Some(real)) => Ok(real.strip_prefix(root).and_then(|r| r.strip_prefix('/')).map_or_else(|| real.to_owned(), |r| format!("/workspace/{r}"))),
            (Some(8), ..) => Ok("/not/inside/a/link".to_owned()),
            _ => Err(Problem::Guest { status: 502, message: "that path can't be followed there".to_owned() }),
        }
    }

    async fn read_file(&self, machine: &Machine, path: &str) -> Result<Vec<u8>> {
        let open = self.up(machine)?;
        if let Some(guest) = &open.guest {
            return clean(machine, guest.read_file(path).await);
        }
        let file = format!("{SHELL_ROOT}/{}", relative(path));
        let ran = open.master()?.run(r#"[ -f "$HOME/$1" ] && cat -- "$HOME/$1""#, &[&file], None, FILE_PATIENCE, MAX_FILE).await?;
        read_answer(ran, relative(path))
    }

    async fn write_file(&self, machine: &Machine, path: &str, content: &[u8]) -> Result<()> {
        let open = self.up(machine)?;
        if let Some(guest) = &open.guest {
            return clean(machine, guest.write_file(path, content).await);
        }
        let file = format!("{SHELL_ROOT}/{}", relative(path));
        let ran = open.master()?.run(WRITE, &[&file, &content.len().to_string()], Some(content), FILE_PATIENCE, 1024).await?;
        match ran.code {
            Some(0) => {}
            Some(6) => return Err(Problem::Guest { status: 409, message: format!("{} is a folder, so nothing was written", relative(path)) }),
            _ => return Err(Problem::Guest { status: 500, message: format!("{} can't be written", relative(path)) }),
        }
        Ok(())
    }

    async fn mark(&self, machine: &Machine, screen: &Screen) -> Option<u64> {
        let open = self.boxed(machine, "show its screen").ok()?;
        open.guest.as_ref()?.mark(screen).await
    }

    async fn settle(&self, machine: &Machine, screen: &Screen, wait: WaitFor) -> Result<Settled> {
        let open = self.boxed(machine, "show its screen")?;
        let Some(guest) = &open.guest else { return Err(Problem::Asleep) };
        clean(machine, guest.settle(screen, wait).await)
    }

    async fn record_start(&self, machine: &Machine, screen: &Screen) -> Result<u64> {
        let open = self.boxed(machine, "record its screen")?;
        let Some(guest) = &open.guest else { return Err(Problem::Asleep) };
        clean(machine, guest.record_start(screen).await)
    }

    async fn record_stop(&self, machine: &Machine, screen: &Screen, id: u64) -> Result<String> {
        let open = self.boxed(machine, "record its screen")?;
        let Some(guest) = &open.guest else { return Err(Problem::Asleep) };
        clean(machine, guest.record_stop(screen, id).await)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
