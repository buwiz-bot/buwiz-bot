//! Where new computers are made, their **homes**: the providers a person can use, the settings and
//! keys they gave each, the Mac's default and each Bot's own choice.
//!
//! - The keys are a person's own; none are ours. Secret fields go to the keystore (service
//!   `com.buwiz.bot.provider-cred`, account `<kind>/default`), the rest to `computer_providers` in
//!   app.db. No route answers with a key: a set one shows as set, with its last 4 characters.
//! - A save is checked first ([`Provider::verify`]) on a provider of its own, with the new
//!   settings, and only kept when it works. A key that can't see the computers the old one made
//!   isn't kept either: nothing could stop them, and they would go on costing.
//! - There is one provider of each kind for the process, because it holds the connections. It reads
//!   its settings through a [`Shared`] handle, so a save takes effect at once and drops nothing.
//! - A new computer is made where the Bot's own choice says, else the Mac's default, else
//!   `BUWIZ_COMPUTER_PROVIDER` (a developer's), else Docker. A Bot keeps the computer it has: a
//!   choice is for its next one.
//! - With nothing saved for a kind, its settings come from the environment, as before.
//! - Only a person changes any of this: these routes answer the app's host token alone. The model
//!   may only suggest a setup (`computer_setup`), which posts a card.
//! - A key isn't removed while a computer made with it is still there: nothing could sleep or
//!   delete that computer then, and it would keep costing.
//! - A computer on its way out (deleted, or a make that failed part way) goes once its platform
//!   deletes it, with the keys in use. When those keys can't reach it any more (rotated, revoked,
//!   expired), a save or a removal says where to look, and a person who looked can forget it: no
//!   key change is ever held up by a row nothing can clear.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buwiz_computer::ec2::{self, creds, sigv4::Credentials};
use buwiz_computer::{cloud, provider_for, BoatProvider, DockerProvider, E2bProvider, Ec2Provider, Power, Problem, Provider, ProviderKind, Secret, Shared};
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::{bot_name, db_problem, keys, Computers, HubCtx};
use crate::hub::policy;

/// The homes a person is offered, in the order they're listed.
pub const OFFERED: [ProviderKind; 5] = [ProviderKind::Docker, ProviderKind::Ec2, ProviderKind::E2b, ProviderKind::Boat, ProviderKind::Remote];

/// The limited policy for the broker's AWS user, for the Copy button. `broker-policy.json` here is a
/// copy of `deploy/aws/broker-policy.json` in buwiz-bot/computer, which is the one to change: keep
/// this copy in step with it. A JSON file holds no comment, so this is where that's said.
const AWS_POLICY: &str = include_str!("broker-policy.json");
/// The stack's template, in the owner's public bucket. It's the same in every region: the console's
/// region is the one it's made in.
const STACK_TEMPLATE: &str = "https://buwiz-computer-templates.s3.ap-southeast-1.amazonaws.com/buwiz-computer/buwiz-computer.yaml";

/// The AWS console's quick-create page for the stack, in `region`.
fn launch_stack(region: &str) -> String {
    let template: String = url::form_urlencoded::byte_serialize(STACK_TEMPLATE.as_bytes()).collect();
    format!("https://console.aws.amazon.com/cloudformation/home?region={region}#/stacks/quickcreate?templateURL={template}&stackName=buwiz-computer")
}
const E2B_BUILD: &str = "https://github.com/buwiz-bot/computer/tree/main/deploy/e2b";

/// How long a keystore that didn't answer is left before it's asked again.
const UNREAD_WAIT: Duration = Duration::from_secs(30);

/// The longest value a field takes.
const MAX_VALUE: usize = 4096;

/// One thing a person sets for a home. Secret ones go to the keystore and are never shown again.
struct Field {
    key: &'static str,
    label: &'static str,
    placeholder: &'static str,
    secret: bool,
    required: bool,
    hint: &'static str,
}

const EC2_FIELDS: &[Field] = &[
    Field { key: "access_key_id", label: "Access key ID", placeholder: "AKIA…", secret: true, required: false, hint: "The access key of the AWS user made for Buwiz (buwiz-computer-broker). To use an AWS profile on this Mac instead, name it below; with a key saved, press Use the profile instead, since a blank box keeps the saved key." },
    Field { key: "secret_access_key", label: "Secret access key", placeholder: "", secret: true, required: false, hint: "Shown once by AWS when the access key is made." },
    Field { key: "profile", label: "AWS profile", placeholder: "buwiz-broker", secret: false, required: false, hint: "Used when no access key is pasted: a profile in ~/.aws with an access key. A profile that signs in through the browser (SSO, aws login) doesn't work yet." },
    Field { key: "region", label: "Region", placeholder: ec2::DEFAULT_REGION, secret: false, required: true, hint: "Where the computers run, like ap-southeast-1. The launch template must be in this region." },
    Field { key: "template", label: "Launch template", placeholder: ec2::DEFAULT_TEMPLATE, secret: false, required: false, hint: "Its name or lt-… id, made by the computer repo's deploy/aws." },
];
const E2B_FIELDS: &[Field] = &[
    Field { key: "api_key", label: "API key", placeholder: "e2b_…", secret: true, required: true, hint: "From the E2B dashboard, Team › API keys." },
    Field { key: "template", label: "Template", placeholder: "buwiz-computer", secret: false, required: true, hint: "The template built from the computer image in your own E2B team (deploy/e2b/build.sh prints it)." },
];
const BOAT_FIELDS: &[Field] = &[
    Field { key: "api_key", label: "API key", placeholder: "", secret: true, required: true, hint: "From your boat.dev account." },
    Field { key: "image", label: "Image", placeholder: "ghcr.io/buwiz-bot/computer:latest", secret: false, required: false, hint: "What the computers are made from. Leave blank for the computer image." },
];

fn fields(kind: ProviderKind) -> &'static [Field] {
    match kind {
        ProviderKind::Ec2 => EC2_FIELDS,
        ProviderKind::E2b => E2B_FIELDS,
        ProviderKind::Boat => BOAT_FIELDS,
        _ => &[],
    }
}

fn notes(kind: ProviderKind) -> Vec<String> {
    match kind {
        ProviderKind::Docker => vec!["Runs on this Mac, walled off from it. Needs Docker Desktop running.".to_owned()],
        ProviderKind::Ec2 => vec![
            "Your own AWS account pays for the computers. Sleep hibernates them, so a sleeping one costs only its disk.".to_owned(),
            "To set up AWS, launch the stack (Launch the AWS stack) in the region above. It asks for AllowedSshCidr: this Mac's public address with /32 after it, like 203.0.113.7/32 (search the web for \"what is my IP\" to find it). Its output BrokerAccessKeyConsole opens the page where you make the access key to paste here.".to_owned(),
            "Copy the limited policy names your account as <ACCOUNT_ID>: put your 12-digit account number there, or use the stack's BrokerPolicyJson output, which has it filled in.".to_owned(),
            "A stack made from an older template has no buwiz:managed tag on its launch template, subnet and security group, and AWS refuses the launch: update the stack with this template.".to_owned(),
        ],
        ProviderKind::E2b => vec![
            "Your own E2B team pays for the computers.".to_owned(),
            format!("Build the template in your team first: E2B_API_KEY=<key> bash deploy/e2b/build.sh ({E2B_BUILD}). It prints the template name to use here."),
        ],
        ProviderKind::Boat => vec!["Your own boat.dev account pays for the computers.".to_owned(), "EU regions only today.".to_owned(), "Waking is a cold boot: what was open on the screen starts again; the files stay.".to_owned()],
        ProviderKind::Remote => vec!["Connect a machine of your own from a Bot's Computer tab. It can't be the default: each one is its own address and its own yes.".to_owned()],
        _ => Vec::new(),
    }
}

fn offered(kind: ProviderKind) -> bool {
    OFFERED.contains(&kind)
}

fn can_default(kind: ProviderKind) -> bool {
    offered(kind) && kind != ProviderKind::Remote
}

/// What a person saved for a home: its settings, and its keys.
#[derive(Clone, Default, PartialEq)]
struct Saved {
    config: BTreeMap<String, String>,
    secrets: BTreeMap<String, Secret>,
}

impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved").field("config", &self.config).field("secrets", &self.secrets.keys().collect::<Vec<_>>()).finish()
    }
}

impl Saved {
    fn get(&self, key: &str) -> Option<String> {
        self.config.get(key).cloned().filter(|v| !v.is_empty())
    }

    fn secret(&self, key: &str) -> Option<Secret> {
        self.secrets.get(key).cloned().filter(|v| !v.expose().is_empty())
    }
}

/// Where the AWS keys come from: the ones a person pasted, a profile they named, or (nothing saved)
/// the environment and the broker's profile, as a developer has it.
#[derive(Clone)]
pub enum AwsKeys {
    Saved(Credentials),
    Profile(String),
    Env(String),
}

/// Says which source, never the keys: an access key ID is kept as secret as its secret.
impl std::fmt::Debug for AwsKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Saved(_) => f.write_str("AwsKeys::Saved(..)"),
            Self::Profile(p) => write!(f, "AwsKeys::Profile({p:?})"),
            Self::Env(p) => write!(f, "AwsKeys::Env({p:?})"),
        }
    }
}

/// [`creds::Keys`] over a person's choice, read at each call.
#[derive(Debug)]
struct StoreKeys(Shared<AwsKeys>);

impl creds::Keys for StoreKeys {
    fn read(&self) -> buwiz_computer::Result<Credentials> {
        match self.0.get() {
            AwsKeys::Saved(c) => Ok(c),
            // a profile a person named: the files, never the environment's keys
            AwsKeys::Profile(profile) => {
                let files = |k: &str| matches!(k, "AWS_SHARED_CREDENTIALS_FILE" | "AWS_CONFIG_FILE").then(|| std::env::var(k).ok()).flatten();
                creds::resolve(&files, std::env::var_os("HOME").map(PathBuf::from).as_deref(), &profile)
            }
            AwsKeys::Env(profile) => creds::EnvAndProfile { profile }.read(),
        }
    }
}

/// What a provider is made with.
pub enum Made {
    Cloud(ProviderKind, Shared<cloud::Settings>),
    Ec2(Shared<ec2::Config>, Arc<dyn creds::Keys>),
}

/// Makes a provider from its settings: the real ones, or a test's.
pub type Factory = Arc<dyn Fn(Made) -> Arc<dyn Provider> + Send + Sync>;

fn real(made: Made) -> Arc<dyn Provider> {
    match made {
        Made::Cloud(ProviderKind::E2b, s) => Arc::new(E2bProvider::shared(s)),
        Made::Cloud(ProviderKind::Boat, s) => Arc::new(BoatProvider::shared(s)),
        Made::Cloud(kind, _) => provider_for(kind),
        Made::Ec2(config, keys) => Arc::new(Ec2Provider::shared(config, keys)),
    }
}

/// What a kind's live provider reads.
#[derive(Clone)]
enum Handles {
    Cloud(Shared<cloud::Settings>),
    Ec2(Shared<ec2::Config>, Shared<AwsKeys>),
}

/// The settings for `kind` from what a person saved, or from the environment with nothing saved.
fn cloud_settings(kind: ProviderKind, saved: Option<&Saved>) -> cloud::Settings {
    let Some(s) = saved else { return cloud::settings(kind) };
    let image = if kind == ProviderKind::E2b { s.get("template") } else { s.get("image") };
    cloud::Settings::new(s.secret("api_key"), image)
}

fn ec2_settings(saved: Option<&Saved>) -> (ec2::Config, AwsKeys) {
    let env = ec2::Config::from_env();
    let Some(s) = saved else {
        let profile = env.profile.clone();
        return (env, AwsKeys::Env(profile));
    };
    // what a person left blank is as the environment says, or its default
    let config = ec2::Config {
        region: s.get("region").unwrap_or(env.region),
        template: s.get("template").unwrap_or(env.template),
        profile: s.get("profile").unwrap_or(env.profile),
        user: env.user,
        image: env.image,
    };
    // pasted keys, else a profile the person named, else as a developer has it: the environment's
    // keys, then the profile's
    let keys = match (s.secret("access_key_id"), s.secret("secret_access_key"), s.get("profile")) {
        (Some(id), Some(secret), _) => AwsKeys::Saved(Credentials { access_key: id.expose().to_owned(), secret, token: None }),
        (_, _, Some(profile)) => AwsKeys::Profile(profile),
        _ => AwsKeys::Env(config.profile.clone()),
    };
    (config, keys)
}

/// The homes: one provider per kind, their settings, and who picked what.
pub struct Homes {
    providers: Mutex<HashMap<ProviderKind, Arc<dyn Provider>>>,
    handles: Mutex<HashMap<ProviderKind, Handles>>,
    factory: Factory,
    /// when each kind's keystore last didn't answer
    unread: Mutex<HashMap<ProviderKind, Instant>>,
    /// by kind: how many saves were applied, so a first read can tell one landed meanwhile
    generations: Mutex<HashMap<ProviderKind, u64>>,
    /// by kind: a save or a removal holds it to write, and the making of a computer to read. A key
    /// can't be removed while a computer is being made with it, nor two saves cross, and one home's
    /// save never waits on another's launch
    locks: HashMap<ProviderKind, tokio::sync::RwLock<()>>,
    /// for a kind `locks` lacks, which `ProviderKind::ALL` rules out
    fallback_lock: tokio::sync::RwLock<()>,
    /// what this broker's keys are hidden under ([`Homes::hide`])
    id: String,
}

impl Drop for Homes {
    /// Its keys aren't anyone's to hide any more.
    fn drop(&mut self) {
        for kind in ProviderKind::ALL {
            buwiz_computer::hide(&format!("{}/{}", self.id, kind.as_str()), Vec::new());
        }
    }
}

impl Homes {
    /// `first` serves its own kind: Docker's provider, or a test's.
    pub fn new(first: Arc<dyn Provider>) -> Self {
        // no randomness from the system: its address in memory is this broker's own too
        let id = buwiz_computer::random_hex(8).unwrap_or_else(|_| format!("{:p}", Arc::as_ptr(&first)));
        let providers = HashMap::from([(first.kind(), first)]);
        Self { providers: Mutex::new(providers), handles: Mutex::default(), factory: Arc::new(real), unread: Mutex::default(), generations: Mutex::default(), locks: ProviderKind::ALL.into_iter().map(|k| (k, tokio::sync::RwLock::new(()))).collect(), fallback_lock: tokio::sync::RwLock::new(()), id }
    }

    /// Makes [`buwiz_computer::Machine::scrub`] hide `kind`'s keys, and only its current ones. Under
    /// this broker's own name: another's (a test's) never replaces them.
    fn hide(&self, kind: ProviderKind, saved: Option<&Saved>) {
        let secrets = saved.map(|s| s.secrets.values().filter(|v| !v.expose().is_empty()).cloned().collect()).unwrap_or_default();
        buwiz_computer::hide(&format!("{}/{}", self.id, kind.as_str()), secrets);
    }

    /// `kind`'s lock: [`Homes::locks`].
    pub(super) fn lock(&self, kind: ProviderKind) -> &tokio::sync::RwLock<()> {
        // every kind has one; a missing one would be a new kind not in `ALL`
        self.locks.get(&kind).unwrap_or(&self.fallback_lock)
    }

    /// Makes providers with `factory` (tests).
    #[cfg(test)]
    pub fn with_factory(mut self, factory: Factory) -> Self {
        self.factory = factory;
        self
    }

    /// The provider of `kind`, made the first time it's asked for, with what a person saved.
    pub fn provider(&self, ctx: &HubCtx, kind: ProviderKind) -> Arc<dyn Provider> {
        if let Some(p) = self.providers.lock().unwrap_or_else(|e| e.into_inner()).get(&kind) {
            return p.clone();
        }
        let made: Arc<dyn Provider> = match kind {
            ProviderKind::Docker => Arc::new(DockerProvider::from_env()),
            ProviderKind::Ec2 | ProviderKind::E2b | ProviderKind::Boat => match self.handles_of(ctx, kind) {
                Ok(Handles::Cloud(s)) => (self.factory)(Made::Cloud(kind, s)),
                Ok(Handles::Ec2(c, k)) => (self.factory)(Made::Ec2(c, Arc::new(StoreKeys(k)))),
                // the keystore didn't answer: a provider that refuses everything, for this call alone.
                // Never one with nothing set: AWS's would sign with this Mac's own keys
                Err(_) => return Arc::new(Unread(kind)),
            },
            other => provider_for(other),
        };
        self.providers.lock().unwrap_or_else(|e| e.into_inner()).entry(kind).or_insert(made).clone()
    }

    /// The live handles of `kind`, made from what's saved the first time. The keystore is read with
    /// no lock held, since it can take seconds; a save applied meanwhile moves `kind`'s generation
    /// on, and the read is made again, so it's never lost. A keystore that didn't answer is asked
    /// again only after [`UNREAD_WAIT`].
    fn handles_of(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<Handles, Problem> {
        for _ in 0..3 {
            if let Some(h) = self.handles.lock().unwrap_or_else(|e| e.into_inner()).get(&kind) {
                return Ok(h.clone());
            }
            if self.unread.lock().unwrap_or_else(|e| e.into_inner()).get(&kind).is_some_and(|at| at.elapsed() < UNREAD_WAIT) {
                return Err(Problem::Failed("the keystore didn't answer a moment ago".to_owned()));
            }
            let began = self.generation(kind);
            let saved = match load(ctx, kind) {
                Ok(saved) => saved,
                Err(p) => {
                    eprintln!("buwiz-broker: {}'s keys couldn't be read: {}", kind.as_str(), p.code());
                    self.unread.lock().unwrap_or_else(|e| e.into_inner()).insert(kind, Instant::now());
                    return Err(p);
                }
            };
            self.unread.lock().unwrap_or_else(|e| e.into_inner()).remove(&kind);
            let mut all = self.handles.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = all.get(&kind) {
                return Ok(h.clone());
            }
            // a save landed while this read: read again
            if self.generation(kind) != began {
                continue;
            }
            let h = if kind == ProviderKind::Ec2 {
                let (config, keys) = ec2_settings(saved.as_ref());
                Handles::Ec2(Shared::new(config), Shared::new(keys))
            } else {
                Handles::Cloud(Shared::new(cloud_settings(kind, saved.as_ref())))
            };
            self.hide(kind, saved.as_ref());
            all.insert(kind, h.clone());
            return Ok(h);
        }
        Err(Problem::Failed("the keys kept changing while they were read: try again".to_owned()))
    }

    fn generation(&self, kind: ProviderKind) -> u64 {
        self.generations.lock().unwrap_or_else(|e| e.into_inner()).get(&kind).copied().unwrap_or(0)
    }

    /// Puts `saved` (or the environment's, for `None`) in front of `kind`'s live provider.
    fn apply(&self, kind: ProviderKind, saved: Option<&Saved>) {
        let all = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        // under the handles' lock: a first read that began before this sees it moved on
        *self.generations.lock().unwrap_or_else(|e| e.into_inner()).entry(kind).or_insert(0) += 1;
        match all.get(&kind).cloned() {
            Some(Handles::Cloud(s)) => s.set(cloud_settings(kind, saved)),
            Some(Handles::Ec2(c, k)) => {
                let (config, keys) = ec2_settings(saved);
                c.set(config);
                k.set(keys);
            }
            // no provider was made yet: it reads the store when it is
            None => {}
        }
        self.hide(kind, saved);
    }

    /// A provider of its own with `saved`, for a check that mustn't touch the live one.
    fn candidate(&self, kind: ProviderKind, saved: &Saved) -> Arc<dyn Provider> {
        if kind == ProviderKind::Ec2 {
            let (config, keys) = ec2_settings(Some(saved));
            (self.factory)(Made::Ec2(Shared::new(config), Arc::new(StoreKeys(Shared::new(keys)))))
        } else {
            (self.factory)(Made::Cloud(kind, Shared::new(cloud_settings(kind, Some(saved)))))
        }
    }

    /// Whether `kind` has what a computer needs, as far as can be told without asking the platform.
    fn configured(&self, ctx: &HubCtx, kind: ProviderKind) -> bool {
        configured_with(kind, &self.read(ctx, kind))
    }

    /// What's saved for `kind`, for a view: nothing for a home that keeps nothing, and a keystore
    /// that didn't answer a moment ago isn't asked again until [`UNREAD_WAIT`] is up.
    fn read(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<Option<Saved>, Problem> {
        if fields(kind).is_empty() {
            return Ok(None);
        }
        if self.unread.lock().unwrap_or_else(|e| e.into_inner()).get(&kind).is_some_and(|at| at.elapsed() < UNREAD_WAIT) {
            return Err(Problem::Failed("the keystore didn't answer a moment ago".to_owned()));
        }
        let read = load(ctx, kind);
        let mut unread = self.unread.lock().unwrap_or_else(|e| e.into_inner());
        if read.is_err() {
            unread.insert(kind, Instant::now());
        } else {
            unread.remove(&kind);
        }
        read
    }
}

/// [`Homes::configured`] with what was read for `kind`. A keystore that didn't answer is a no.
fn configured_with(kind: ProviderKind, read: &Result<Option<Saved>, Problem>) -> bool {
    let Ok(saved) = read else { return false };
    match kind {
        ProviderKind::Docker | ProviderKind::Remote => true,
        // a profile, named or the developer's, counts once its files give keys: read here, no call
        ProviderKind::Ec2 => match ec2_settings(saved.as_ref()).1 {
            AwsKeys::Saved(_) => true,
            other => creds::Keys::read(&StoreKeys(Shared::new(other))).is_ok(),
        },
        ProviderKind::E2b | ProviderKind::Boat => {
            let s = cloud_settings(kind, saved.as_ref());
            s.key.is_some() && (kind != ProviderKind::E2b || s.image.is_some())
        }
        _ => false,
    }
}


// ---- the store ----

/// What a person saved for `kind`, or `None` when they saved nothing (the environment's, then).
fn load(ctx: &HubCtx, kind: ProviderKind) -> Result<Option<Saved>, Problem> {
    let row: Option<String> = ctx.db.with(|c| c.query_row("SELECT config FROM computer_providers WHERE kind=?", [kind.as_str()], |r| r.get(0)).optional()).map_err(db_problem)?;
    let secrets = keys(ctx)?.get_provider_cred(kind.as_str()).map_err(Problem::Failed)?;
    if row.is_none() && secrets.is_none() {
        return Ok(None);
    }
    let config: BTreeMap<String, String> = row.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let secrets: BTreeMap<String, String> = secrets.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    Ok(Some(Saved { config, secrets: secrets.into_iter().map(|(k, v)| (k, Secret::new(v))).collect() }))
}

/// Puts `secrets` in the keystore for `kind`, or empties it for none.
fn keep_secrets(ctx: &HubCtx, kind: ProviderKind, secrets: &BTreeMap<String, Secret>) -> Result<(), Problem> {
    let ks = keys(ctx)?;
    if secrets.is_empty() {
        return ks.remove_provider_cred(kind.as_str()).map_err(Problem::Failed);
    }
    let plain: BTreeMap<&str, &str> = secrets.iter().map(|(k, v)| (k.as_str(), v.expose())).collect();
    let text = serde_json::to_string(&plain).map_err(|e| Problem::Failed(e.to_string()))?;
    ks.put_provider_cred(kind.as_str(), &text).map_err(|e| Problem::Failed(format!("The keys couldn't be kept in this Mac's Keychain: {e}")))
}

/// Keeps `saved` for `kind`, keys first: a row never says there are keys the keystore lost. A row
/// that doesn't write puts `before`'s keys back, so what's kept and what's used stay one.
fn store(ctx: &HubCtx, kind: ProviderKind, saved: &Saved, before: &Saved) -> Result<(), Problem> {
    keep_secrets(ctx, kind, &saved.secrets)?;
    let config = serde_json::to_string(&saved.config).map_err(|e| Problem::Failed(e.to_string()))?;
    let at = policy::now(ctx);
    let row = ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO computer_providers(kind, config, updated_at) VALUES(?,?,?)", rusqlite::params![kind.as_str(), config, at]));
    if let Err(e) = row {
        let _ = keep_secrets(ctx, kind, &before.secrets);
        return Err(db_problem(e));
    }
    Ok(())
}

fn forget(ctx: &HubCtx, kind: ProviderKind) -> Result<(), Problem> {
    keys(ctx)?.remove_provider_cred(kind.as_str()).map_err(Problem::Failed)?;
    ctx.db.with(|c| c.execute("DELETE FROM computer_providers WHERE kind=?", [kind.as_str()])).map(|_| ()).map_err(db_problem)
}

/// Where the Mac's new computers go, and who said: `saved` (a person), `env` or `builtin`.
pub fn default_home(ctx: &HubCtx) -> (ProviderKind, &'static str) {
    let saved: Option<String> = ctx.db.with(|c| c.query_row("SELECT kind FROM computer_home WHERE id=1", [], |r| r.get(0)).optional()).ok().flatten();
    if let Some(kind) = saved.as_deref().and_then(ProviderKind::parse).filter(|k| can_default(*k)) {
        return (kind, "saved");
    }
    let env = std::env::var(buwiz_computer::PROVIDER_ENV).unwrap_or_default();
    if !env.trim().is_empty() {
        return (ProviderKind::from_env(), "env");
    }
    (ProviderKind::Docker, "builtin")
}

/// Where `bot`'s next computer goes, and who said: `bot` (its own choice) or the default's word.
pub fn home_of(ctx: &HubCtx, bot: &str) -> (ProviderKind, &'static str) {
    let own: Option<String> = ctx.db.with(|c| c.query_row("SELECT kind FROM bot_computer_home WHERE bot_id=?", [bot], |r| r.get(0)).optional()).ok().flatten();
    match own.as_deref().and_then(ProviderKind::parse).filter(|k| can_default(*k)) {
        Some(kind) => (kind, "bot"),
        None => (default_home(ctx).0, "default"),
    }
}

/// `bot` was deleted: its choice goes with it.
pub fn forget_bot(ctx: &HubCtx, bot: &str) {
    let _ = ctx.db.with(|c| c.execute("DELETE FROM bot_computer_home WHERE bot_id=?", [bot]));
}

pub fn home_json(ctx: &HubCtx, bot: &str) -> Value {
    let (kind, source) = home_of(ctx, bot);
    json!({"kind": kind.as_str(), "label": kind.label(), "source": source})
}

fn default_json(ctx: &HubCtx) -> Value {
    let (kind, source) = default_home(ctx);
    json!({"kind": kind.as_str(), "label": kind.label(), "source": source})
}

/// A computer made on a home that is still there.
struct On {
    id: String,
    /// the names of the Bots at it
    bots: Vec<String>,
    /// on its way out: deleted, or a make that failed part way. Its keys are what delete it
    leaving: bool,
}

/// The computers made on `kind` that are still there. One on its way out counts.
fn computers_on(ctx: &HubCtx, kind: ProviderKind) -> Result<Vec<On>, Problem> {
    let rows: Vec<(String, bool)> = ctx
        .db
        .with(|c| {
            let mut q = c.prepare("SELECT id, gone_at IS NOT NULL FROM computers WHERE provider=? ORDER BY created_at, id")?;
            let rows = q.query_map([kind.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .map_err(db_problem)?;
    rows.into_iter().map(|(id, leaving)| Ok(On { bots: Computers::seats(ctx, &id)?.iter().map(|s| bot_name(ctx, &s.bot)).collect(), id, leaving })).collect()
}

/// Where a person finds what's left of computer `id` on `kind`'s platform, to delete it.
fn look_for(kind: ProviderKind, id: &str) -> String {
    match kind {
        ProviderKind::Ec2 => format!("In the EC2 console, in each region you've used, terminate any instance tagged {}={id}", ec2::api::ID_TAG),
        ProviderKind::E2b => format!("In the E2B dashboard, kill any sandbox whose metadata has {}={id}", buwiz_computer::e2b::METADATA_KEY),
        ProviderKind::Boat => format!("In boat.dev, delete any sandbox labelled {}={id}", buwiz_computer::boat::LABEL),
        _ => format!("Delete anything made for {id} on that platform"),
    }
}

/// What the keys in use made of each computer on its way out that's still there: `None` for one
/// being woken, which the sweep is left to, else why its delete didn't go.
type Settled = HashMap<String, Option<Problem>>;

/// Why computer `id`, on its way out, still holds up a change of `kind`'s keys. Only the keys'
/// own refusal says they can't reach it: anything else is a try again, never a Forget, since
/// the keys may still delete it.
fn still_leaving(kind: ProviderKind, id: &str, settled: &Settled) -> Problem {
    match settled.get(id) {
        Some(Some(Problem::Refused { code, .. })) if *code != "keychain" => stuck(kind, id),
        Some(Some(why)) => refusal("in_use", format!("Computer {id} on {} couldn't be deleted just now: {} Try again in a minute.", kind.label(), why.sentence())),
        _ => refusal("in_use", format!("Computer {id} on {} is being deleted right now. Try again in a minute.", kind.label())),
    }
}

/// Computer `id` is on its way out, and the keys in use can't delete it.
fn stuck(kind: ProviderKind, id: &str) -> Problem {
    refusal(
        "stuck",
        format!(
            "Computer {id} on {} is being deleted, and the keys in use can't reach it any more, so it may still be there and costing. {}. Then press Forget {id} in Settings › Computers, and try again.",
            kind.label(),
            look_for(kind, id)
        ),
    )
}

/// The last 4 characters of a secret long enough that they say little.
fn last4(s: &Secret) -> Option<String> {
    let chars: Vec<char> = s.expose().chars().collect();
    (chars.len() >= 12).then(|| chars[chars.len() - 4..].iter().collect())
}

impl Computers {
    fn provider_json(&self, ctx: &HubCtx, kind: ProviderKind) -> Value {
        // one read of the keystore for the whole answer, and none for a home with nothing to keep
        let read = self.homes.read(ctx, kind);
        let saved = read.as_ref().ok().cloned().flatten();
        let (env_config, _) = ec2_settings(None);
        let shown: Vec<Value> = fields(kind)
            .iter()
            .map(|f| {
                let mut o = json!({"key": f.key, "label": f.label, "placeholder": f.placeholder, "secret": f.secret, "required": f.required, "hint": f.hint});
                if f.secret {
                    let s = saved.as_ref().and_then(|s| s.secret(f.key));
                    o["set"] = json!(s.is_some());
                    if let Some(l) = s.as_ref().and_then(last4) {
                        o["last4"] = json!(l);
                    }
                } else {
                    let value = saved.as_ref().and_then(|s| s.get(f.key)).or_else(|| match (kind, f.key) {
                        (ProviderKind::Ec2, "region") => Some(env_config.region.clone()),
                        _ => None,
                    });
                    o["value"] = json!(value.unwrap_or_default());
                }
                o
            })
            .collect();
        let computers: Vec<Value> = computers_on(ctx, kind)
            .unwrap_or_default()
            .into_iter()
            .map(|o| {
                let mut c = json!({"id": o.id, "bots": o.bots});
                // only a home with keys of its own: a key that stopped working is what leaves one stuck
                if o.leaving && !fields(kind).is_empty() {
                    c["leaving"] = json!(true);
                    c["look"] = json!(look_for(kind, &o.id));
                }
                c
            })
            .collect();
        let (default, _) = default_home(ctx);
        let built = built(kind);
        let mut o = json!({
            "kind": kind.as_str(), "label": kind.label(), "built": built, "configured": built && configured_with(kind, &read),
            "canDefault": can_default(kind), "isDefault": default == kind, "fields": shown, "notes": notes(kind), "computers": computers,
            "source": if saved.is_some() { "saved" } else if fields(kind).is_empty() { "none" } else { "env" },
        });
        if kind == ProviderKind::Ec2 {
            let region = saved.as_ref().and_then(|s| s.get("region")).unwrap_or(env_config.region);
            o["policy"] = json!(AWS_POLICY.replace("<REGION>", &region));
            if ec2::api::region_ok(&region) {
                o["launch"] = json!(launch_stack(&region));
            }
        }
        o
    }

    /// Every home, and the Mac's default.
    pub fn providers_json(&self, ctx: &HubCtx) -> Value {
        let providers: Vec<Value> = OFFERED.iter().map(|k| self.provider_json(ctx, *k)).collect();
        json!({"default": default_json(ctx), "providers": providers})
    }

    /// Saves what a person gave `kind`, once it works. `values` holds the fields they filled in (a
    /// blank secret keeps the one there), and `clear` the ones to empty.
    pub async fn save_provider(&self, ctx: &HubCtx, kind: ProviderKind, values: &Map<String, Value>, clear: &[String]) -> Result<Value, Problem> {
        if fields(kind).is_empty() {
            return Err(refusal("nothing_to_set", format!("{} has nothing to set here.", kind.label())));
        }
        // what a person typed, and whether the keys work, first: a save refused for either changes
        // nothing anywhere. With no lock held: a platform that hangs mustn't hold up the Bots
        let checked = next_of(kind, &load(ctx, kind)?.unwrap_or_default(), values, clear)?;
        let candidate = self.homes.candidate(kind, &checked);
        candidate.verify().await?;
        let settled = self.settle_leaving(ctx, kind).await?;
        let _saving = self.homes.lock(kind).write().await;
        let before = load(ctx, kind)?.unwrap_or_default();
        let next = next_of(kind, &before, values, clear)?;
        // the new settings on a provider of their own: the live one goes on as it was until they work.
        // Made and checked again only when a save landed meanwhile and made them other than the ones checked
        let candidate = if next == checked {
            candidate
        } else {
            let other = self.homes.candidate(kind, &next);
            other.verify().await?;
            other
        };
        self.sees_its_computers(ctx, kind, &*candidate, &settled).await?;
        store(ctx, kind, &next, &before)?;
        self.homes.apply(kind, Some(&next));
        Ok(self.provider_json(ctx, kind))
    }

    /// A key that can't see the computers already made on `kind` isn't kept: with it, nothing could
    /// sleep or delete them, and they'd go on costing.
    /// Every one of them is asked about. One that the keys in use can't see either is already gone,
    /// and doesn't count against the new ones.
    async fn sees_its_computers(&self, ctx: &HubCtx, kind: ProviderKind, candidate: &dyn Provider, settled: &Settled) -> Result<(), Problem> {
        let unseen = |p: &Result<Power, Problem>| matches!(p, Ok(Power::Absent) | Err(Problem::Gone) | Err(Problem::Refused { .. }));
        // only a platform's own word that it's gone counts as gone: keys the live provider lacks
        // (none saved, or a keystore that didn't answer) say nothing about the computer
        let gone = |p: &Result<Power, Problem>| matches!(p, Ok(Power::Absent) | Err(Problem::Gone));
        let live = self.homes.provider(ctx, kind);
        for o in computers_on(ctx, kind)? {
            let id = o.id;
            let Ok(machine) = self.machine(ctx, &id) else {
                // on its way out with no record (a make that failed part way): the keys in use
                // couldn't delete it just now, and nothing can say the new ones see it
                if o.leaving {
                    return Err(still_leaving(kind, &id, settled));
                }
                // a record that can't be read now may be back: nothing can say these keys see it
                return Err(refusal("in_use", format!("A computer on {} is being made or removed right now. Try again in a minute.", kind.label())));
            };
            let seen = candidate.power(&machine).await;
            if unseen(&seen) {
                if gone(&live.power(&machine).await) {
                    continue;
                }
                // deleted, and neither key can reach it: a person may know it's gone
                if o.leaving {
                    return Err(still_leaving(kind, &id, settled));
                }
                return Err(refusal(
                    "other_account",
                    format!("These keys can't see a computer already made on {} (they may be another account's). Nothing was changed: delete those Bots' computers first, or use keys of the same account.", kind.label()),
                ));
            }
            seen?;
        }
        Ok(())
    }

    /// Has the keys in use delete the computers on `kind` on their way out that they still can:
    /// what [`Computers::finish_retire`] does each minute, now. With no lock of `kind`'s held, so
    /// a platform that hangs holds up no Bot.
    async fn settle_leaving(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<Settled, Problem> {
        let mut settled = Settled::new();
        for o in computers_on(ctx, kind)?.into_iter().filter(|o| o.leaving) {
            // a wake in flight could put back what this removes, as for the sweep
            if self.now().waking.contains(&o.id) {
                settled.insert(o.id, None);
            } else if let Err(why) = self.finish_retire(ctx, &o.id).await {
                settled.insert(o.id, Some(why));
            }
        }
        Ok(settled)
    }

    /// A person looked, and computer `id` on `kind`, on its way out, has nothing left on the
    /// platform: the keys in use try once more, and then it's forgotten. Only one on its way out:
    /// a computer in use is deleted with its Bot first.
    pub async fn forget_leaving(&self, ctx: &HubCtx, kind: ProviderKind, id: &str) -> Result<Value, Problem> {
        if fields(kind).is_empty() {
            return Err(refusal("nothing_to_set", format!("{} has nothing to forget here.", kind.label())));
        }
        let leaving = |ctx: &HubCtx| computers_on(ctx, kind).map(|on| on.into_iter().any(|o| o.leaving && o.id == id));
        if !leaving(ctx)? {
            return Err(refusal("not_leaving", format!("{id} isn't a computer on {} that's being deleted. Delete its Bot first.", kind.label())));
        }
        // being woken, it may be coming up right now: forgotten, nothing would ever stop it
        if self.now().waking.contains(id) {
            return Err(refusal("in_use", format!("Computer {id} on {} is being woken right now. Try again in a minute.", kind.label())));
        }
        // the keys in use once more, with no lock held. Only their own refusal lets a person's word
        // stand for the platform's: anything else, and they may still reach it
        match self.finish_retire(ctx, id).await {
            Ok(()) => return Ok(self.provider_json(ctx, kind)),
            Err(Problem::Refused { code, .. }) if code != "keychain" => {}
            Err(why) => return Err(refusal("in_use", format!("{} couldn't be asked about {id} just now: {} Nothing was forgotten. Try again in a minute.", kind.label(), why.sentence()))),
        }
        let _saving = self.homes.lock(kind).write().await;
        if leaving(ctx)? && !self.now().waking.contains(id) {
            self.drop_record(ctx, id)?;
        }
        Ok(self.provider_json(ctx, kind))
    }

    /// A person's Check of what `kind` has now. `Ok` is the sentence to show.
    pub async fn check_provider(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<String, Problem> {
        match kind {
            ProviderKind::Remote => Ok("A remote computer is tested when you connect it, from a Bot's Computer tab.".to_owned()),
            ProviderKind::Docker => {
                self.homes.provider(ctx, kind).check().await?;
                Ok("Docker is running, and this Mac can make computers.".to_owned())
            }
            ProviderKind::Ec2 | ProviderKind::E2b | ProviderKind::Boat => {
                self.homes.provider(ctx, kind).verify().await?;
                Ok(format!("{} works with these keys: it can make computers.", kind.label()))
            }
            other => Err(Problem::NotBuilt(other)),
        }
    }

    /// Forgets `kind`'s keys and settings, unless a computer made with them is still there.
    pub async fn remove_provider(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<Value, Problem> {
        if fields(kind).is_empty() {
            return Err(refusal("nothing_to_set", format!("{} has no keys to remove.", kind.label())));
        }
        // before the lock: a platform that hangs mustn't hold up the Bots' clicks
        let settled = self.settle_leaving(ctx, kind).await?;
        let _saving = self.homes.lock(kind).write().await;
        let left = computers_on(ctx, kind)?;
        // in use: each goes with its Bot first
        let in_use: Vec<&On> = left.iter().filter(|o| !o.leaving).collect();
        if !in_use.is_empty() {
            let names: Vec<String> = in_use.iter().flat_map(|o| o.bots.clone()).collect();
            let whose = if names.is_empty() { format!("{} of them", in_use.len()) } else { names.join(", ") };
            return Err(refusal(
                "in_use",
                format!("{} still has computers made with these keys ({whose}). Without the keys nothing could sleep or delete them, and they'd go on costing. Delete those Bots' computers first (delete the Bot, from its menu), then remove the keys.", kind.label()),
            ));
        }
        // on its way out, and still there after the keys in use tried: why, and Forget once stuck
        if let Some(o) = left.iter().find(|o| o.leaving) {
            return Err(still_leaving(kind, &o.id, &settled));
        }
        forget(ctx, kind)?;
        // with its keys gone it can't be where new computers go: the Mac's default and any Bot's
        // own choice of it go back to the default
        let _ = ctx.db.with(|c| c.execute("DELETE FROM computer_home WHERE kind=?", [kind.as_str()]));
        let _ = ctx.db.with(|c| c.execute("DELETE FROM bot_computer_home WHERE kind=?", [kind.as_str()]));
        self.homes.apply(kind, None);
        Ok(self.provider_json(ctx, kind))
    }

    /// Makes `kind` the Mac's default home.
    pub fn set_default(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<Value, Problem> {
        self.choosable(ctx, kind)?;
        let at = policy::now(ctx);
        ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO computer_home(id, kind, at) VALUES(1,?,?)", rusqlite::params![kind.as_str(), at])).map_err(db_problem)?;
        Ok(default_json(ctx))
    }

    /// Where `bot`'s next computer goes: `kind`, or the default with `None`.
    pub fn set_bot_home(&self, ctx: &HubCtx, bot: &str, kind: Option<ProviderKind>) -> Result<(), Problem> {
        let Some(kind) = kind else {
            forget_bot(ctx, bot);
            return Ok(());
        };
        self.choosable(ctx, kind)?;
        let at = policy::now(ctx);
        ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO bot_computer_home(bot_id, kind, at) VALUES(?,?,?)", rusqlite::params![bot, kind.as_str(), at])).map(|_| ()).map_err(db_problem)
    }

    fn choosable(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<(), Problem> {
        if kind == ProviderKind::Remote {
            return Err(refusal("cant_default", "A remote computer can't be where new computers go: connect one from a Bot's Computer tab.".to_owned()));
        }
        if !offered(kind) || !built(kind) {
            return Err(refusal("not_offered", format!("{} isn't offered yet.", kind.label())));
        }
        if !self.homes.configured(ctx, kind) {
            return Err(refusal("not_configured", format!("Add {}'s keys first, then pick it.", kind.label())));
        }
        Ok(())
    }
}

/// What `kind` keeps once `values` (the fields a person filled in; a blank secret keeps the one
/// there) and `clear` (the ones to empty) are put over `before`, or why they can't be.
fn next_of(kind: ProviderKind, before: &Saved, values: &Map<String, Value>, clear: &[String]) -> Result<Saved, Problem> {
    let mut next = before.clone();
    for f in fields(kind) {
        let typed = values.get(f.key).and_then(Value::as_str).map(str::trim);
        if let Some(v) = typed {
            if v.len() > MAX_VALUE || v.chars().any(char::is_control) {
                return Err(refusal("invalid", format!("{} isn't something {} takes.", f.label, kind.label())));
            }
        }
        match (f.secret, typed) {
            (true, Some(v)) if !v.is_empty() => {
                next.secrets.insert(f.key.to_owned(), Secret::new(v));
            }
            (false, Some(v)) if !v.is_empty() => {
                next.config.insert(f.key.to_owned(), v.to_owned());
            }
            (false, Some(_)) => {
                next.config.remove(f.key);
            }
            _ => {}
        }
        if clear.iter().any(|c| c == f.key) {
            next.secrets.remove(f.key);
            next.config.remove(f.key);
        }
    }
    for f in fields(kind).iter().filter(|f| f.required) {
        let has = if f.secret { next.secret(f.key).is_some() } else { next.get(f.key).is_some() };
        if !has {
            return Err(refusal("missing", format!("{} needs its {} first.", kind.label(), f.label.to_lowercase())));
        }
    }
    if kind == ProviderKind::Ec2 && next.secret("access_key_id").is_some() != next.secret("secret_access_key").is_some() {
        return Err(refusal("missing", "An AWS access key needs both its ID and its secret. Paste both, or leave both blank to use a profile.".to_owned()));
    }
    Ok(next)
}

/// Whether this build makes computers of `kind`: not a slot with no code yet.
fn built(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::Docker | ProviderKind::Ec2 | ProviderKind::E2b | ProviderKind::Boat | ProviderKind::Remote)
}

fn refusal(code: &'static str, message: String) -> Problem {
    Problem::Refused { code, message }
}

/// A provider whose keys couldn't be read: it refuses everything, in words that say so.
struct Unread(ProviderKind);

impl Unread {
    fn no<T>(&self) -> buwiz_computer::Result<T> {
        Err(Problem::Refused { code: "keychain", message: format!("This Mac's Keychain didn't answer, so {}'s keys couldn't be read. Unlock the Mac's Keychain, then try again.", self.0.label()) })
    }
}

#[async_trait::async_trait]
impl Provider for Unread {
    fn kind(&self) -> ProviderKind {
        self.0
    }
    async fn check(&self) -> buwiz_computer::Result<()> {
        self.no()
    }
    async fn create(&self, _spec: &buwiz_computer::Spec) -> buwiz_computer::Result<buwiz_computer::Machine> {
        self.no()
    }
    async fn power(&self, _machine: &buwiz_computer::Machine) -> buwiz_computer::Result<Power> {
        self.no()
    }
    async fn wake(&self, _machine: &buwiz_computer::Machine) -> buwiz_computer::Result<buwiz_computer::Woke> {
        self.no()
    }
    async fn sleep(&self, _machine: &buwiz_computer::Machine, _depth: buwiz_computer::Sleep) -> buwiz_computer::Result<()> {
        self.no()
    }
    async fn destroy(&self, _machine: &buwiz_computer::Machine) -> buwiz_computer::Result<()> {
        self.no()
    }
    async fn sweep(&self, _id: &str, _last: Option<&buwiz_computer::Reach>) -> buwiz_computer::Result<()> {
        self.no()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn saved(config: &[(&str, &str)], secrets: &[(&str, &str)]) -> Saved {
        Saved {
            config: config.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
            secrets: secrets.iter().map(|(k, v)| ((*k).to_owned(), Secret::new(*v))).collect(),
        }
    }

    #[test]
    fn aws_keys_come_from_pasted_keys_then_a_named_profile_then_as_a_developer_has_them() {
        let pasted = saved(&[("region", "eu-west-1")], &[("access_key_id", "id-1"), ("secret_access_key", "secret-1")]);
        assert!(matches!(ec2_settings(Some(&pasted)).1, AwsKeys::Saved(_)));
        let named = saved(&[("region", "eu-west-1"), ("profile", "work")], &[]);
        assert!(matches!(ec2_settings(Some(&named)).1, AwsKeys::Profile(p) if p == "work"));
        // a region alone names no profile: the environment's keys still sign
        let region_only = saved(&[("region", "eu-west-1")], &[]);
        assert!(matches!(ec2_settings(Some(&region_only)).1, AwsKeys::Env(_)));
        assert_eq!(ec2_settings(Some(&region_only)).0.region, "eu-west-1");
    }
}
