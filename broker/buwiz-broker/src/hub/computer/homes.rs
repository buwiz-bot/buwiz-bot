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

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
const CLOUDFORMATION: &str = "https://github.com/buwiz-bot/computer/blob/main/deploy/aws/cloudformation/buwiz-computer.yaml";
const E2B_BUILD: &str = "https://github.com/buwiz-bot/computer/tree/main/deploy/e2b";

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
    Field { key: "access_key_id", label: "Access key ID", placeholder: "AKIA…", secret: true, required: false, hint: "The access key of the AWS user made for Buwiz (buwiz-computer-broker). Leave both keys blank to use an AWS profile on this Mac instead." },
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

/// What a person calls a home.
pub fn label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Docker => "Docker on this Mac",
        ProviderKind::Ec2 => "AWS EC2",
        ProviderKind::E2b => "E2B",
        ProviderKind::Boat => "boat.dev",
        ProviderKind::Remote => "A remote computer",
        other => other.label(),
    }
}

fn notes(kind: ProviderKind) -> Vec<String> {
    match kind {
        ProviderKind::Docker => vec!["Runs on this Mac, walled off from it. Needs Docker Desktop running.".to_owned()],
        ProviderKind::Ec2 => vec![
            "Your own AWS account pays for the computers. Sleep hibernates them, so a sleeping one costs only its disk.".to_owned(),
            format!("To set up AWS: CloudFormation console › Create stack › Upload a template file, with {CLOUDFORMATION}. Its output BrokerAccessKeyConsole opens the page where you make the access key to paste here, and BrokerPolicyJson is the limited policy filled in for your account."),
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
#[derive(Clone, Debug)]
pub enum AwsKeys {
    Saved(Credentials),
    Profile(String),
    Env(String),
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
    let d = ec2::Config::default();
    let config = ec2::Config {
        region: s.get("region").unwrap_or(d.region),
        template: s.get("template").unwrap_or(d.template),
        profile: s.get("profile").unwrap_or(d.profile),
        user: env.user,
        image: env.image,
    };
    let keys = match (s.secret("access_key_id"), s.secret("secret_access_key")) {
        (Some(id), Some(secret)) => AwsKeys::Saved(Credentials { access_key: id.expose().to_owned(), secret, token: None }),
        _ => AwsKeys::Profile(config.profile.clone()),
    };
    (config, keys)
}

/// The homes: one provider per kind, their settings, and who picked what.
pub struct Homes {
    providers: Mutex<HashMap<ProviderKind, Arc<dyn Provider>>>,
    handles: Mutex<HashMap<ProviderKind, Handles>>,
    factory: Factory,
    /// one save or removal at a time: a check between a read and a write mustn't see another's
    saving: tokio::sync::Mutex<()>,
}

impl Homes {
    /// `first` serves its own kind: Docker's provider, or a test's.
    pub fn new(first: Arc<dyn Provider>) -> Self {
        let providers = HashMap::from([(first.kind(), first)]);
        Self { providers: Mutex::new(providers), handles: Mutex::default(), factory: Arc::new(real), saving: tokio::sync::Mutex::new(()) }
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
            ProviderKind::Ec2 | ProviderKind::E2b | ProviderKind::Boat => {
                let saved = load(ctx, kind).unwrap_or_else(|_| Some(Saved::default()));
                let handles = self.handles_for(kind, saved.as_ref());
                match handles {
                    Handles::Cloud(s) => (self.factory)(Made::Cloud(kind, s)),
                    Handles::Ec2(c, k) => (self.factory)(Made::Ec2(c, Arc::new(StoreKeys(k)))),
                }
            }
            other => provider_for(other),
        };
        self.providers.lock().unwrap_or_else(|e| e.into_inner()).entry(kind).or_insert(made).clone()
    }

    /// The live handles of `kind`, made from `saved` the first time.
    fn handles_for(&self, kind: ProviderKind, saved: Option<&Saved>) -> Handles {
        let mut all = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = all.get(&kind) {
            return h.clone();
        }
        let h = if kind == ProviderKind::Ec2 {
            let (config, keys) = ec2_settings(saved);
            Handles::Ec2(Shared::new(config), Shared::new(keys))
        } else {
            Handles::Cloud(Shared::new(cloud_settings(kind, saved)))
        };
        hide(kind, saved);
        all.insert(kind, h.clone());
        h
    }

    /// Puts `saved` (or the environment's, for `None`) in front of `kind`'s live provider.
    fn apply(&self, kind: ProviderKind, saved: Option<&Saved>) {
        let handles = self.handles.lock().unwrap_or_else(|e| e.into_inner()).get(&kind).cloned();
        match handles {
            Some(Handles::Cloud(s)) => s.set(cloud_settings(kind, saved)),
            Some(Handles::Ec2(c, k)) => {
                let (config, keys) = ec2_settings(saved);
                c.set(config);
                k.set(keys);
            }
            // no provider was made yet: it reads the store when it is
            None => {}
        }
        hide(kind, saved);
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
        match kind {
            ProviderKind::Docker | ProviderKind::Remote => true,
            ProviderKind::Ec2 | ProviderKind::E2b | ProviderKind::Boat => {
                let saved = load(ctx, kind).ok().flatten();
                match kind {
                    ProviderKind::Ec2 => match ec2_settings(saved.as_ref()).1 {
                        AwsKeys::Saved(_) | AwsKeys::Profile(_) => true,
                        env @ AwsKeys::Env(_) => creds::Keys::read(&StoreKeys(Shared::new(env))).is_ok(),
                    },
                    _ => {
                        let s = cloud_settings(kind, saved.as_ref());
                        s.key.is_some() && (kind != ProviderKind::E2b || s.image.is_some())
                    }
                }
            }
            _ => false,
        }
    }
}

/// Makes [`buwiz_computer::Machine::scrub`] hide `kind`'s keys, and only its current ones.
fn hide(kind: ProviderKind, saved: Option<&Saved>) {
    let secrets = saved.map(|s| s.secrets.values().filter(|v| !v.expose().is_empty()).cloned().collect()).unwrap_or_default();
    buwiz_computer::hide(kind.as_str(), secrets);
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

/// Keeps `saved` for `kind`: the keys first, so a row never says there are keys the keystore lost.
fn store(ctx: &HubCtx, kind: ProviderKind, saved: &Saved) -> Result<(), Problem> {
    let ks = keys(ctx)?;
    if saved.secrets.is_empty() {
        ks.remove_provider_cred(kind.as_str()).map_err(Problem::Failed)?;
    } else {
        let plain: BTreeMap<&str, &str> = saved.secrets.iter().map(|(k, v)| (k.as_str(), v.expose())).collect();
        let text = serde_json::to_string(&plain).map_err(|e| Problem::Failed(e.to_string()))?;
        ks.put_provider_cred(kind.as_str(), &text).map_err(|e| Problem::Failed(format!("The keys couldn't be kept in this Mac's Keychain: {e}")))?;
    }
    let config = serde_json::to_string(&saved.config).map_err(|e| Problem::Failed(e.to_string()))?;
    let at = policy::now(ctx);
    ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO computer_providers(kind, config, updated_at) VALUES(?,?,?)", rusqlite::params![kind.as_str(), config, at])).map(|_| ()).map_err(db_problem)
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
    json!({"kind": kind.as_str(), "label": label(kind), "source": source})
}

fn default_json(ctx: &HubCtx) -> Value {
    let (kind, source) = default_home(ctx);
    json!({"kind": kind.as_str(), "label": label(kind), "source": source})
}

/// The computers made on `kind` that are still there, with the names of the Bots at each. One on its
/// way out counts: its keys are what delete it.
fn computers_on(ctx: &HubCtx, kind: ProviderKind) -> Result<Vec<(String, Vec<String>)>, Problem> {
    let ids: Vec<String> = ctx
        .db
        .with(|c| {
            let mut q = c.prepare("SELECT id FROM computers WHERE provider=? ORDER BY created_at, id")?;
            let rows = q.query_map([kind.as_str()], |r| r.get(0))?;
            rows.collect()
        })
        .map_err(db_problem)?;
    ids.into_iter().map(|id| Ok((id.clone(), Computers::seats(ctx, &id)?.iter().map(|s| bot_name(ctx, &s.bot)).collect()))).collect()
}

/// The last 4 characters of a secret long enough that they say little.
fn last4(s: &Secret) -> Option<String> {
    let chars: Vec<char> = s.expose().chars().collect();
    (chars.len() >= 12).then(|| chars[chars.len() - 4..].iter().collect())
}

impl Computers {
    fn provider_json(&self, ctx: &HubCtx, kind: ProviderKind) -> Value {
        let saved = load(ctx, kind).ok().flatten();
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
        let computers: Vec<Value> = computers_on(ctx, kind).unwrap_or_default().into_iter().map(|(id, bots)| json!({"id": id, "bots": bots})).collect();
        let (default, _) = default_home(ctx);
        let built = built(kind);
        let mut o = json!({
            "kind": kind.as_str(), "label": label(kind), "built": built, "configured": built && self.homes.configured(ctx, kind),
            "canDefault": can_default(kind), "isDefault": default == kind, "fields": shown, "notes": notes(kind), "computers": computers,
            "source": if saved.is_some() { "saved" } else if fields(kind).is_empty() { "none" } else { "env" },
        });
        if kind == ProviderKind::Ec2 {
            let region = saved.as_ref().and_then(|s| s.get("region")).unwrap_or(env_config.region);
            o["policy"] = json!(AWS_POLICY.replace("<REGION>", &region));
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
            return Err(refusal("nothing_to_set", format!("{} has nothing to set here.", label(kind))));
        }
        let _saving = self.homes.saving.lock().await;
        let mut next = load(ctx, kind)?.unwrap_or_default();
        for f in fields(kind) {
            let typed = values.get(f.key).and_then(Value::as_str).map(str::trim);
            if let Some(v) = typed {
                if v.len() > MAX_VALUE || v.chars().any(char::is_control) {
                    return Err(refusal("invalid", format!("{} isn't something {} takes.", f.label, label(kind))));
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
                return Err(refusal("missing", format!("{} needs its {} first.", label(kind), f.label.to_lowercase())));
            }
        }
        if kind == ProviderKind::Ec2 && next.secret("access_key_id").is_some() != next.secret("secret_access_key").is_some() {
            return Err(refusal("missing", "An AWS access key needs both its ID and its secret. Paste both, or leave both blank to use a profile.".to_owned()));
        }
        // the new settings on a provider of their own: the live one goes on as it was until they work
        let candidate = self.homes.candidate(kind, &next);
        candidate.verify().await?;
        self.sees_its_computers(ctx, kind, &*candidate).await?;
        store(ctx, kind, &next)?;
        self.homes.apply(kind, Some(&next));
        Ok(self.provider_json(ctx, kind))
    }

    /// A key that can't see the computers already made on `kind` isn't kept: with it, nothing could
    /// sleep or delete them, and they'd go on costing.
    async fn sees_its_computers(&self, ctx: &HubCtx, kind: ProviderKind, candidate: &dyn Provider) -> Result<(), Problem> {
        for (id, _) in computers_on(ctx, kind)? {
            let Ok(machine) = self.machine(ctx, &id) else { continue };
            return match candidate.power(&machine).await {
                Ok(Power::Absent) | Err(Problem::Gone) | Err(Problem::Credentials { .. }) => Err(refusal(
                    "other_account",
                    format!("These keys can't see the computers already made on {} (they may be another account's). Nothing was changed: delete those Bots' computers first, or use keys of the same account.", label(kind)),
                )),
                Ok(_) => Ok(()),
                Err(p) => Err(p),
            };
        }
        Ok(())
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
                Ok(format!("{} works with these keys: it can make computers.", label(kind)))
            }
            other => Err(Problem::NotBuilt(other)),
        }
    }

    /// Forgets `kind`'s keys and settings, unless a computer made with them is still there.
    pub async fn remove_provider(&self, ctx: &HubCtx, kind: ProviderKind) -> Result<Value, Problem> {
        if fields(kind).is_empty() {
            return Err(refusal("nothing_to_set", format!("{} has no keys to remove.", label(kind))));
        }
        let _saving = self.homes.saving.lock().await;
        let left = computers_on(ctx, kind)?;
        if !left.is_empty() {
            let names: Vec<String> = left.iter().flat_map(|(_, bots)| bots.clone()).collect();
            let whose = if names.is_empty() { format!("{} of them", left.len()) } else { names.join(", ") };
            return Err(refusal(
                "in_use",
                format!("{} still has computers made with these keys ({whose}). Without the keys nothing could sleep or delete them, and they'd go on costing. Delete those Bots' computers first (delete the Bot, from its menu), then remove the keys.", label(kind)),
            ));
        }
        forget(ctx, kind)?;
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
            return Err(refusal("not_offered", format!("{} isn't offered yet.", label(kind))));
        }
        if !self.homes.configured(ctx, kind) {
            return Err(refusal("not_configured", format!("Add {}'s keys first, then pick it.", label(kind))));
        }
        Ok(())
    }
}

/// Whether this build makes computers of `kind`: not a slot with no code yet.
fn built(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::Docker | ProviderKind::Ec2 | ProviderKind::E2b | ProviderKind::Boat | ProviderKind::Remote)
}

fn refusal(code: &'static str, message: String) -> Problem {
    Problem::Credentials { code, message }
}
