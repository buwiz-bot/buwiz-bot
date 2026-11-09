//! A computer on AWS EC2: one instance per computer, launched from the `buwiz-computer` launch
//! template that `deploy/aws` in the computer repo makes (OpenTofu), running the same box image
//! as on a Mac. Sleep is hibernation: the instance's memory goes to its encrypted root volume and
//! comes back on the next start, desktop and browser as they were.
//!
//! - **Lifecycle** is the EC2 Query API ([`api`]), signed here ([`sigv4`]) with the broker's own
//!   IAM user ([`creds`]). An instance carries `buwiz:managed=true`, `buwiz:computer-id=<id>` and
//!   `Name=buwiz-computer-<id>` from its launch, on it and its volume; a lost record is found, and
//!   a lost computer swept, by both tags and nothing else.
//! - **Reach** is the remote provider's SSH connection ([`crate::remote::ssh`]): one `ssh` master
//!   per computer logs in with a key made for this computer alone and forwards the box's ports,
//!   on the instance's loopback, to ports here in the range the Bot's shell is kept off. The
//!   security group lets in SSH and nothing else. The private key is in the computer's folder, a
//!   0600 file beside the record that holds its token, and only that instance takes it; its
//!   public half goes in with the launch's user data, which is no secret.
//! - **Trust.** The instance proves itself by the host key it printed to its serial console on its
//!   first boot, read through the AWS API ([`console`]), not from anything at its address. That key
//!   is pinned before the first login, and `ssh` refuses any other. Nothing is sent before.
//! - **Secrets** are the record's, `Secret::fresh` at its making. They go to the instance on the
//!   stdin of the start ([`instance::start`]), into a root-only tmpfs folder the box reads them
//!   from, and are removed once it answers. Never in user data, whose copy every process on the
//!   instance can read, and never on a command line.
//! - **Sleep** is a hibernation (light) or a stop with the box stopped first (deep). An instance
//!   that can't hibernate is stopped, and the log says so: it starts afresh, so its screens do.
//!   A wake starts it, waits for SSH, and finds the box still running after a hibernation
//!   ([`Woke::Resumed`]), or starts it ([`Woke::Started`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::guest::BoxGuest;
use crate::remote::{clean, free_ports, guest_at, guest_ports, ssh};
use crate::{random_hex, CommandOutput, Far, Input, Machine, Power, Problem, Provider, ProviderKind, Reach, Result, Screen, Screenshot, Secret, Settled, Sleep, Spec, Viewer, WaitFor, Woke};

pub mod api;
pub mod console;
pub mod creds;
pub mod instance;
pub mod sigv4;
pub mod xml;

use api::{Api, Called, Instance};

pub const REGION_ENV: &str = "BUWIZ_EC2_REGION";
pub const TEMPLATE_ENV: &str = "BUWIZ_EC2_LAUNCH_TEMPLATE";
pub const USER_ENV: &str = "BUWIZ_EC2_USER";
pub const IMAGE_ENV: &str = "BUWIZ_EC2_IMAGE";
pub const PROFILE_ENV: &str = "BUWIZ_EC2_PROFILE";
pub const DEFAULT_REGION: &str = "ap-southeast-1";
pub const DEFAULT_TEMPLATE: &str = "buwiz-computer";
/// Ubuntu's login on its AMIs; Amazon Linux's is `ec2-user`.
pub const DEFAULT_USER: &str = "ubuntu";
pub const DEFAULT_IMAGE: &str = "ghcr.io/buwiz-bot/computer:latest";
pub const KEYGEN: &str = "/usr/bin/ssh-keygen";

const FOLDER: &str = "ec2";
const KEY: &str = "key";
const PIN: &str = "pin";
/// How long a box has to answer through a fresh connection before it counts as not running.
const ANSWER_CHECK: Duration = Duration::from_secs(15);
/// What `guard` gives a box on a link that's up.
const GUARD_CHECK: Duration = Duration::from_secs(5);
const SMALL_OUTPUT: usize = 64 * 1024;

const NO_ADDRESS: &str = "This computer's instance has no public address, so it can't be reached. Its subnet must give one (deploy/aws does).";
const DROPPED: &str = "The connection to this computer dropped. Click it to connect again.";
const NO_KEY: &str = "This computer's SSH key is missing from its folder, so it can't be reached. Delete the Bot's computer and make a new one.";
const TERMINATED: &str = "This computer's instance was terminated outside Buwiz Bot, and its files went with it. Delete the Bot's computer to make a new one.";

/// The one place the broker's EC2 settings and keys come from today: the environment and the
/// `buwiz-broker` profile. A per-person source (a settings page, the Keychain) replaces this
/// function, and nothing else.
pub fn from_env() -> (Config, Arc<dyn creds::Keys>) {
    let config = Config::from_env();
    let keys = Arc::new(creds::EnvAndProfile { profile: config.profile.clone() });
    (config, keys)
}

/// What a person sets, with its defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub region: String,
    /// a name, or an `lt-…` id
    pub template: String,
    /// the login on the instance
    pub user: String,
    pub image: String,
    /// the broker's AWS profile
    pub profile: String,
}

impl Default for Config {
    fn default() -> Self {
        Self { region: DEFAULT_REGION.to_owned(), template: DEFAULT_TEMPLATE.to_owned(), user: DEFAULT_USER.to_owned(), image: DEFAULT_IMAGE.to_owned(), profile: creds::DEFAULT_PROFILE.to_owned() }
    }
}

impl Config {
    /// From `BUWIZ_EC2_REGION`, `BUWIZ_EC2_LAUNCH_TEMPLATE`, `BUWIZ_EC2_USER`, `BUWIZ_EC2_IMAGE` and
    /// `BUWIZ_EC2_PROFILE`, each with its default.
    pub fn from_env() -> Self {
        let set = |k: &str, default: String| std::env::var(k).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()).unwrap_or(default);
        let d = Self::default();
        Self { region: set(REGION_ENV, d.region), template: set(TEMPLATE_ENV, d.template), user: set(USER_ENV, d.user), image: set(IMAGE_ENV, d.image), profile: set(PROFILE_ENV, d.profile) }
    }
}

/// How long each wait goes on, and how often it looks.
#[derive(Clone, Copy, Debug)]
pub struct Patience {
    /// an instance getting to `running` or `stopped`
    pub state: Duration,
    /// its first boot printing its host key
    pub console: Duration,
    /// SSH answering after a start
    pub login: Duration,
    /// the first boot's cloud-init, and a start that pulls the image
    pub setup: Duration,
    /// the box answering once started
    pub answer: Duration,
    /// between two looks at AWS
    pub poll: Duration,
}

impl Default for Patience {
    fn default() -> Self {
        Self { state: Duration::from_secs(600), console: Duration::from_secs(900), login: Duration::from_secs(180), setup: Duration::from_secs(1200), answer: Duration::from_secs(180), poll: Duration::from_secs(5) }
    }
}

/// One open connection: the master, and the box at the ports it forwards.
struct Open {
    master: ssh::Master,
    guest: BoxGuest,
    /// guest port → port here
    ports: HashMap<u16, u16>,
}

/// Every EC2 computer's connections, by the folder the broker keeps for it. The hub keeps one.
pub struct Ec2Provider {
    config: Config,
    keys: Arc<dyn creds::Keys>,
    /// `http://host:port` instead of AWS (tests)
    endpoint: Option<String>,
    apis: Mutex<HashMap<String, Arc<Api>>>,
    ssh: PathBuf,
    keygen: PathBuf,
    patience: Patience,
    links: Mutex<HashMap<PathBuf, Arc<Open>>>,
    /// one wake, sleep, connect or removal at a time, per computer
    turns: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl Ec2Provider {
    /// With these settings, signing with `keys`: neither is read from anywhere here.
    pub fn new(config: Config, keys: Arc<dyn creds::Keys>) -> Self {
        Self { config, keys, endpoint: None, apis: Mutex::default(), ssh: PathBuf::from(ssh::SSH), keygen: PathBuf::from(KEYGEN), patience: Patience::default(), links: Mutex::default(), turns: Mutex::default() }
    }

    /// Settings and keys from [`from_env`].
    pub fn from_env() -> Self {
        let (config, keys) = from_env();
        Self::new(config, keys)
    }

    /// Talks to `endpoint`, plain HTTP, instead of AWS (tests).
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        self.endpoint = Some(endpoint.to_owned());
        self
    }

    /// Stands in for `ssh` and `ssh-keygen` (tests).
    pub fn with_programs(mut self, ssh: PathBuf, keygen: PathBuf) -> Self {
        self.ssh = ssh;
        self.keygen = keygen;
        self
    }

    /// Waits this long (tests).
    pub fn with_patience(mut self, patience: Patience) -> Self {
        self.patience = patience;
        self
    }

    fn api(&self, region: &str) -> Result<Arc<Api>> {
        let mut apis = self.apis.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(api) = apis.get(region) {
            return Ok(api.clone());
        }
        let mut api = Api::new(region, self.keys.clone())?;
        if let Some(e) = &self.endpoint {
            api = api.with_endpoint(e);
        }
        let api = Arc::new(api);
        apis.insert(region.to_owned(), api.clone());
        Ok(api)
    }

    fn turn(&self, machine: &Machine) -> Arc<tokio::sync::Mutex<()>> {
        self.turns.lock().unwrap_or_else(|e| e.into_inner()).entry(machine.dir.clone()).or_default().clone()
    }

    fn link_of(&self, machine: &Machine) -> Option<Arc<Open>> {
        self.links.lock().unwrap_or_else(|e| e.into_inner()).get(&machine.dir).cloned()
    }

    fn up(&self, machine: &Machine) -> Result<Arc<Open>> {
        self.link_of(machine).filter(|o| o.master.alive()).ok_or(Problem::Asleep)
    }

    async fn drop_link(&self, machine: &Machine) {
        let found = self.links.lock().unwrap_or_else(|e| e.into_inner()).remove(&machine.dir);
        if let Some(open) = found {
            open.master.close().await;
        }
    }

    /// Hangs up `open`, if it's still this computer's.
    async fn drop_this(&self, machine: &Machine, open: &Arc<Open>) {
        {
            let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
            if links.get(&machine.dir).is_some_and(|held| Arc::ptr_eq(held, open)) {
                links.remove(&machine.dir);
            }
        }
        open.master.close().await;
    }

    /// The instance, waited on until it's `running` with an address: a stopping one is let stop and
    /// started, a stopped one started.
    async fn until_running(&self, api: &Api, id: &str) -> Result<Instance> {
        let began = Instant::now();
        let mut started = false;
        loop {
            // just launched, EC2 may not know it yet everywhere
            let found = match api.describe(id).await? {
                Some(found) => found,
                None if began.elapsed() < Duration::from_secs(60).min(self.patience.state) => {
                    tokio::time::sleep(self.patience.poll).await;
                    continue;
                }
                None => return Err(Problem::Remote(TERMINATED.to_owned())),
            };
            match found.state.as_str() {
                "running" => return found.public_ip.is_some().then_some(found).ok_or_else(|| Problem::Remote(NO_ADDRESS.to_owned())),
                "stopped" if !started => {
                    api.start(id).await?;
                    started = true;
                }
                "shutting-down" | "terminated" => return Err(Problem::Remote(TERMINATED.to_owned())),
                _ => {}
            }
            if began.elapsed() > self.patience.state {
                return Err(Problem::Remote(format!("The instance didn't get running in {} minutes: it's {}.", self.patience.state.as_secs() / 60, found.state)));
            }
            tokio::time::sleep(self.patience.poll).await;
        }
    }

    /// The pinned host key: the one saved, or the first boot's from the console, saved now.
    async fn pin(&self, api: &Api, id: &str, dir: &Path) -> Result<String> {
        let path = dir.join(FOLDER).join(PIN);
        if let Ok(saved) = std::fs::read_to_string(&path) {
            let saved = saved.trim().to_owned();
            ssh::check_pin(&saved)?;
            return Ok(saved);
        }
        let began = Instant::now();
        loop {
            match console::host_key(&api.console(id).await?) {
                console::Read::Key(pin) => {
                    ssh::check_pin(&pin)?;
                    private_dir(&dir.join(FOLDER))?;
                    std::fs::write(&path, format!("{pin}\n")).map_err(io_problem)?;
                    return Ok(pin);
                }
                console::Read::Doubtful(why) => {
                    return Err(Problem::Remote(format!("The instance's console doesn't say for sure which SSH key is its own ({why}), so nothing was sent to it. Delete the Bot's computer and make a new one.")));
                }
                console::Read::NotYet if began.elapsed() > self.patience.console => {
                    return Err(Problem::Remote(format!("The instance didn't print its SSH key to its console in {} minutes, so it can't be trusted yet. Click it to try again.", self.patience.console.as_secs() / 60)));
                }
                console::Read::NotYet => tokio::time::sleep(self.patience.poll).await,
            }
        }
    }

    /// Logs in, pinned, and forwards the box's ports. A machine still booting is waited for; a key
    /// that isn't the pinned one ends it at once.
    async fn connect(&self, machine: &Machine, ip: &str, pin: &str) -> Result<Arc<Open>> {
        let key = std::fs::read_to_string(machine.dir.join(FOLDER).join(KEY)).map(Secret::new).map_err(|_| Problem::Remote(NO_KEY.to_owned()))?;
        let at = ssh::At { host: ip.to_owned(), user: Some(self.config.user.clone()), port: None };
        let began = Instant::now();
        let mut lost_ports = 0;
        loop {
            let ports = free_ports(guest_ports().count())?;
            let forwards: Vec<(u16, u16)> = ports.iter().copied().zip(guest_ports()).collect();
            match ssh::Master::open(&self.ssh, &machine.dir, &at, pin, Some(&key), &forwards).await {
                Ok(master) => {
                    let ports: HashMap<u16, u16> = forwards.into_iter().map(|(here, there)| (there, here)).collect();
                    let guest = guest_at(&ports, machine.token.clone())?;
                    return Ok(Arc::new(Open { master, guest, ports }));
                }
                Err(ssh::Failed::PortTaken) if lost_ports < 4 => lost_ports += 1,
                Err(ssh::Failed::PortTaken) => return Err(Problem::Remote("This Mac has no free ports left for a connection.".to_owned())),
                Err(ssh::Failed::Said(p)) if p.sentence() == ssh::UNREACHED && began.elapsed() < self.patience.login => tokio::time::sleep(self.patience.poll).await,
                Err(ssh::Failed::Said(p)) => return Err(p),
            }
        }
    }

    /// `script` as root on the instance, `args` after it.
    async fn root(open: &Open, script: &str, args: &[&str], stdin: Option<&[u8]>, patience: Duration) -> Result<ssh::Ran> {
        let mut all = vec![script];
        all.extend_from_slice(args);
        open.master.run(instance::AS_ROOT, &all, stdin, patience, SMALL_OUTPUT).await
    }

    /// The box started with this record's secrets, answering. They're gone from the instance
    /// however this ends.
    async fn start_box(&self, machine: &Machine, open: &Open) -> Result<()> {
        let id = computer_id(machine);
        let image = machine.image.clone().unwrap_or_else(|| self.config.image.clone());
        let secrets = format!("{}\n{}\n", machine.token.expose(), machine.viewer_password.expose());
        let started = async {
            let ran = Self::root(open, &instance::start(&image, &id), &[], Some(secrets.as_bytes()), self.patience.setup).await?;
            if String::from_utf8_lossy(&ran.stdout).trim() != "started" {
                return Err(Problem::Remote(instance::start_failure(ran.code, &image)));
            }
            let began = Instant::now();
            while !open.guest.ready().await {
                if began.elapsed() > self.patience.answer {
                    return Err(Problem::Remote(format!("The box on the instance didn't answer in {}s.", self.patience.answer.as_secs())));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
        .await;
        let _ = Self::root(open, &instance::clear(), &[], None, Duration::from_secs(30)).await;
        started
    }

    /// Up and answering on a link that's kept: resumed if the box was running, else started.
    async fn bring_up(&self, machine: &Machine, open: &Arc<Open>) -> Result<Woke> {
        let ran = Self::root(open, &instance::prepare(), &[], None, self.patience.setup).await?;
        let state = String::from_utf8_lossy(&ran.stdout).trim().to_owned();
        if state == "nodocker" {
            return Err(Problem::Remote("Docker isn't running on the instance: its first boot didn't finish setting it up. Delete the Bot's computer and make a new one.".to_owned()));
        }
        // after a hibernation it's as it was; one that won't take this record's token is started again
        if state == "running" {
            if open.guest.ready_within(ANSWER_CHECK).await {
                return Ok(Woke::Resumed);
            }
            Self::root(open, &instance::stop(), &[], None, Duration::from_secs(60)).await?;
        }
        self.start_box(machine, open).await.map(|()| Woke::Started)
    }

    /// Hibernates it, or stops it when it can't, and says so.
    async fn hibernate(&self, api: &Api, found: &Instance) -> Result<()> {
        if found.hibernation {
            match api.stop(&found.id, true).await? {
                Called::Done(_) => return Ok(()),
                Called::Refused(r) if r.not_about_hibernation() => return Err(r.problem()),
                Called::Refused(r) => eprintln!("buwiz-computer: {} couldn't hibernate ({}), so it was stopped: what was open on it won't be there at the next wake", found.id, r.code),
            }
        } else {
            eprintln!("buwiz-computer: {} wasn't launched able to hibernate, so it was stopped: what was open on it won't be there at the next wake", found.id);
        }
        match api.stop(&found.id, false).await? {
            Called::Done(_) => Ok(()),
            Called::Refused(r) => Err(r.problem()),
        }
    }

    /// Instances of computer `id`, by both tags: the config's region and `last`'s.
    async fn found(&self, id: &str, last: Option<&Reach>) -> Result<Vec<(Arc<Api>, Instance)>> {
        let mut regions = vec![self.config.region.clone()];
        if let Some(Reach::Instance { region, .. }) = last {
            if !regions.contains(region) {
                regions.push(region.clone());
            }
        }
        let mut out = Vec::new();
        for region in regions {
            let api = self.api(&region)?;
            for found in api.tagged(id).await? {
                out.push((api.clone(), found));
            }
            // the last one known, if it's still ours and the filter missed it
            if let Some(Reach::Instance { id: last_id, region: r }) = last {
                if *r == region && !out.iter().any(|(_, i)| i.id == *last_id) {
                    if let Some(found) = api.describe(last_id).await?.filter(|i| i.is_computer(id) && !matches!(i.state.as_str(), "shutting-down" | "terminated")) {
                        out.push((api.clone(), found));
                    }
                }
            }
        }
        Ok(out)
    }

    /// The link up for this request: kept, or made again after a broker restart while the instance
    /// ran. A box that doesn't answer isn't connected to: a person wakes it.
    async fn reconnect(&self, machine: &Machine) -> Result<Arc<Open>> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        if let Some(open) = self.link_of(machine).filter(|o| o.master.alive()) {
            return Ok(open);
        }
        self.drop_link(machine).await;
        let (id, region) = instance_of(machine)?;
        let api = self.api(region)?;
        let found = api.describe(id).await?.ok_or(Problem::Asleep)?;
        let ip = found.public_ip.filter(|_| found.state == "running").ok_or(Problem::Asleep)?;
        let pin = std::fs::read_to_string(machine.dir.join(FOLDER).join(PIN)).map_err(|_| Problem::Asleep)?;
        let open = self.connect(machine, &ip, pin.trim()).await?;
        if !open.guest.ready_within(ANSWER_CHECK).await {
            open.master.close().await;
            return Err(Problem::Asleep);
        }
        self.links.lock().unwrap_or_else(|e| e.into_inner()).insert(machine.dir.clone(), open.clone());
        Ok(open)
    }
}

/// EC2's state word as a [`Power`].
pub fn power_of(state: &str) -> Power {
    match state {
        "pending" => Power::Waking,
        "running" => Power::Awake,
        "stopping" | "stopped" => Power::Asleep,
        "shutting-down" | "terminated" => Power::Absent,
        other => Power::Broken(format!("EC2 says it is {other}.")),
    }
}

fn instance_of(machine: &Machine) -> Result<(&str, &str)> {
    match &machine.reach {
        Reach::Instance { id, region } => Ok((id, region)),
        _ => Err(Problem::Failed("this computer isn't an EC2 instance".to_owned())),
    }
}

/// The computer's id: its folder's name, as the broker made it.
fn computer_id(machine: &Machine) -> String {
    machine.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn io_problem(e: std::io::Error) -> Problem {
    Problem::Failed(format!("its folder: {e}"))
}

fn private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(io_problem)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io_problem)
}

/// A key pair for computer `id` in `folder`, made by `ssh-keygen`: the public line.
async fn keygen(program: &Path, folder: &Path, id: &str) -> Result<String> {
    private_dir(folder)?;
    let key = folder.join(KEY);
    let _ = std::fs::remove_file(&key);
    let _ = std::fs::remove_file(folder.join(format!("{KEY}.pub")));
    let made = tokio::process::Command::new(program)
        .args(["-q", "-t", "ed25519", "-N", "", "-C", &format!("buwiz-computer-{id}"), "-f"])
        .arg(&key)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|_| Problem::ProviderMissing("ssh-keygen".to_owned()))?;
    if !made.success() {
        return Err(Problem::Failed("ssh-keygen didn't make a key".to_owned()));
    }
    let public = std::fs::read_to_string(folder.join(format!("{KEY}.pub"))).map_err(io_problem)?;
    let public = public.trim().to_owned();
    ssh::check_pin(&public.split_whitespace().take(2).collect::<Vec<_>>().join(" "))?;
    Ok(public)
}

#[async_trait]
impl Provider for Ec2Provider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ec2
    }

    /// The broker's keys work, and the template is there.
    async fn check(&self) -> Result<()> {
        if !self.ssh.is_file() {
            return Err(Problem::ProviderMissing("ssh".to_owned()));
        }
        self.api(&self.config.region)?.template_exists(&self.config.template).await
    }

    /// Launches its instance, which starts at once: the wake that follows finds it on its way up.
    async fn create(&self, spec: &Spec) -> Result<Machine> {
        self.check().await?;
        let api = self.api(&self.config.region)?;
        let folder = spec.dir.join(FOLDER);
        let public = keygen(&self.keygen, &folder, &spec.id).await?;
        let own = api.template_user_data(&self.config.template).await?;
        let data = instance::user_data(own.as_deref(), &public, &format!("buwiz-{}", random_hex(12)?));
        let id = match api.run(&self.config.template, &spec.id, &data, &format!("{}-{}", spec.id, random_hex(6)?)).await {
            Ok(id) => id,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&folder);
                return Err(e);
            }
        };
        Ok(Machine {
            provider: ProviderKind::Ec2,
            reach: Reach::Instance { id, region: self.config.region.clone() },
            image: Some(self.config.image.clone()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        })
    }

    async fn power(&self, machine: &Machine) -> Result<Power> {
        let (id, region) = instance_of(machine)?;
        Ok(self.api(region)?.describe(id).await?.map_or(Power::Absent, |i| power_of(&i.state)))
    }

    async fn wake(&self, machine: &Machine) -> Result<Woke> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        let (id, region) = instance_of(machine)?;
        let api = self.api(region)?;
        let found = self.until_running(&api, id).await?;
        let ip = found.public_ip.ok_or_else(|| Problem::Remote(NO_ADDRESS.to_owned()))?;
        let pin = self.pin(&api, id, &machine.dir).await?;
        let open = match self.link_of(machine).filter(|o| o.master.alive()) {
            Some(open) => open,
            None => {
                self.drop_link(machine).await;
                self.connect(machine, &ip, &pin).await?
            }
        };
        match self.bring_up(machine, &open).await {
            Ok(woke) => {
                self.links.lock().unwrap_or_else(|e| e.into_inner()).insert(machine.dir.clone(), open);
                Ok(woke)
            }
            Err(e) => {
                self.drop_this(machine, &open).await;
                Err(e)
            }
        }
    }

    async fn sleep(&self, machine: &Machine, depth: Sleep) -> Result<()> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        let (id, region) = instance_of(machine)?;
        let api = self.api(region)?;
        if depth == Sleep::Deep {
            if let Some(open) = self.link_of(machine).filter(|o| o.master.alive()) {
                let _ = Self::root(&open, &instance::stop(), &[], None, Duration::from_secs(60)).await;
            }
        }
        self.drop_link(machine).await;
        let mut found = api.describe(id).await?.ok_or(Problem::Gone)?;
        if found.state == "pending" {
            found = self.until_running(&api, id).await?;
        }
        if found.state != "running" {
            return Ok(());
        }
        match depth {
            Sleep::Light => self.hibernate(&api, &found).await,
            Sleep::Deep => match api.stop(id, false).await? {
                Called::Done(_) => Ok(()),
                Called::Refused(r) => Err(r.problem()),
            },
        }
    }

    /// Terminates its instance: its root volume, the computer's files, goes with it.
    async fn destroy(&self, machine: &Machine) -> Result<()> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        self.drop_link(machine).await;
        let (id, region) = instance_of(machine)?;
        self.api(region)?.terminate(&[id.to_owned()]).await?;
        drop(_turn);
        self.turns.lock().unwrap_or_else(|e| e.into_inner()).remove(&machine.dir);
        Ok(())
    }

    /// Terminates every instance tagged as computer `id`'s.
    async fn sweep(&self, id: &str, last: Option<&Reach>) -> Result<()> {
        let mut by_region: HashMap<String, (Arc<Api>, Vec<String>)> = HashMap::new();
        for (api, found) in self.found(id, last).await? {
            by_region.entry(api.region.clone()).or_insert_with(|| (api.clone(), Vec::new())).1.push(found.id);
        }
        for (_, (api, ids)) in by_region {
            api.terminate(&ids).await?;
        }
        Ok(())
    }

    /// Its instance, found by its tags, with new secrets: the box is started again with them at the
    /// next wake. Only while its key is still in its folder: no other way in is left.
    async fn recover(&self, spec: &Spec, last: Option<&Reach>) -> Result<Option<Machine>> {
        if !spec.dir.join(FOLDER).join(KEY).is_file() {
            return Ok(None);
        }
        let Some((api, found)) = self.found(&spec.id, last).await?.into_iter().next() else { return Ok(None) };
        Ok(Some(Machine {
            provider: ProviderKind::Ec2,
            reach: Reach::Instance { id: found.id, region: api.region.clone() },
            image: Some(self.config.image.clone()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        }))
    }

    /// Connected, and the box answering this record's token. A link that fails this is closed.
    async fn guard(&self, machine: &Machine) -> Result<()> {
        let open = self.reconnect(machine).await?;
        if !open.guest.ready_within(GUARD_CHECK).await {
            self.drop_this(machine, &open).await;
            return Err(Problem::Remote(DROPPED.to_owned()));
        }
        Ok(())
    }

    async fn clear_strays(&self, machine: &Machine, screen: &Screen) -> Result<Vec<String>> {
        let open = self.up(machine)?;
        let bot = screen.bot().ok_or_else(|| Problem::Failed("this screen has no user of its own".to_owned()))?;
        let ran = Self::root(&open, &instance::inside(), &[crate::docker::CLEAR, bot], None, Duration::from_secs(60)).await?;
        serde_json::from_slice(String::from_utf8_lossy(&ran.stdout).trim().as_bytes()).map_err(|_| Problem::Failed("the programs left on it couldn't be told apart".to_owned()))
    }

    /// Read as root from outside the box, as on a Mac (`docker.rs`'s check).
    async fn users_apart(&self, machine: &Machine, bots: &[String]) -> Result<bool> {
        let open = self.up(machine)?;
        let mut args = vec![crate::docker::APART];
        args.extend(bots.iter().map(String::as_str));
        let ran = Self::root(&open, &instance::inside(), &args, None, Duration::from_secs(60)).await?;
        let said = String::from_utf8_lossy(&ran.stdout).trim().to_owned();
        if said != "apart" {
            eprintln!("buwiz-computer: its Bots aren't apart: {said}");
        }
        Ok(ran.code == Some(0) && said == "apart")
    }

    async fn screen_up(&self, machine: &Machine, bot: &str) -> Result<Screen> {
        clean(machine, self.up(machine)?.guest.screen_up(bot).await)
    }

    async fn screen_down(&self, machine: &Machine, bot: &str) -> Result<()> {
        clean(machine, self.up(machine)?.guest.screen_down(bot).await)
    }

    async fn screens(&self, machine: &Machine) -> Result<Vec<String>> {
        clean(machine, self.up(machine)?.guest.screens().await)
    }

    /// Through the gate's rules for a far viewer: the page comes over a network.
    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        let open = self.up(machine)?;
        Ok(open.ports.get(&screen.viewer_port).map(|here| Viewer { addr: format!("127.0.0.1:{here}"), password: machine.viewer_password.clone(), far: Some(Far::default()) }))
    }

    async fn screenshot(&self, machine: &Machine, screen: &Screen) -> Result<Screenshot> {
        clean(machine, self.up(machine)?.guest.screenshot(screen).await)
    }

    async fn input(&self, machine: &Machine, screen: &Screen, input: &Input) -> Result<()> {
        clean(machine, self.up(machine)?.guest.input(screen, input).await)
    }

    async fn run(&self, machine: &Machine, screen: &Screen, command: &str, timeout_ms: u64) -> Result<CommandOutput> {
        clean(machine, self.up(machine)?.guest.run(screen, command, timeout_ms.clamp(1000, 600_000)).await)
    }

    async fn open_url(&self, machine: &Machine, screen: &Screen, url: &str) -> Result<()> {
        clean(machine, self.up(machine)?.guest.open_url(screen, url).await)
    }

    async fn resolve(&self, machine: &Machine, path: &str) -> Result<String> {
        clean(machine, self.up(machine)?.guest.resolve(path).await)
    }

    async fn read_file(&self, machine: &Machine, path: &str) -> Result<Vec<u8>> {
        clean(machine, self.up(machine)?.guest.read_file(path).await)
    }

    async fn write_file(&self, machine: &Machine, path: &str, content: &[u8]) -> Result<()> {
        clean(machine, self.up(machine)?.guest.write_file(path, content).await)
    }

    async fn mark(&self, machine: &Machine, screen: &Screen) -> Option<u64> {
        self.up(machine).ok()?.guest.mark(screen).await
    }

    async fn settle(&self, machine: &Machine, screen: &Screen, wait: WaitFor) -> Result<Settled> {
        clean(machine, self.up(machine)?.guest.settle(screen, wait).await)
    }

    async fn record_start(&self, machine: &Machine, screen: &Screen) -> Result<u64> {
        clean(machine, self.up(machine)?.guest.record_start(screen).await)
    }

    async fn record_stop(&self, machine: &Machine, screen: &Screen, id: u64) -> Result<String> {
        clean(machine, self.up(machine)?.guest.record_stop(screen, id).await)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
