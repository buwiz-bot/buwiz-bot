//! Connect Apps for v1 plugins (spec 13): the list as the broker gives
//! it, the install and update reviews, disable and enable, routines, and
//! the uninstall cascade. The UI (`plugins.rs`) draws what this returns
//! and calls these flows; they run on a thread of their own (the broker
//! spawns the plugin) and go through the [`Broker`] trait, so the tests
//! run them against a fake broker.
//!
//! - Install (13.1 step 3, LCY-01): `inspect` spawns the plugin for a
//!   moment and gives the review lines; the review shows every one of
//!   them. Cancel calls nothing.
//! - Update (13.3, LCY-02): an update whose permissions are wider shows
//!   what it adds and needs Approve; a lower version needs the explicit
//!   "Install older version" (12.1).
//! - Uninstall (13.6, LCY-06): opens close and sessions end, the broker
//!   uninstalls (the plugin's `buwiz.uninstall`, its key, its record), then
//!   Rox removes the plugin's attachments, pins and library entries and
//!   deletes its apps' storage.

use std::{path::Path, time::Duration};

use serde_json::{Value, json};

use super::{
    RoxError,
    host_api::{self, Broker, PluginAction},
    records::Records,
    session::{self, Ended},
    storage,
};

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn list(v: &Value, k: &str) -> Vec<Value> {
    v.get(k).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// One of a plugin's tools, with its switch (8.3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub disabled: bool,
}

/// A reserved lifecycle tool's name: `buwiz.<lowercase>` (protocol 1.0)
/// or `rox.<lowercase>` (an install from Rox).
fn reserved_tool(name: &str) -> bool {
    ["buwiz.", "rox."].iter().any(|p| name.strip_prefix(p).is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_lowercase() || c == '.')))
}

/// One of a plugin's routines (5.9) as the person manages it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub description: String,
    pub approved: bool,
    pub enabled: bool,
    pub available: bool,
    pub reason: String,
    /// The chat it reports to.
    pub thread: String,
    /// Its own thread (`home`), made once it is enabled ("" from an older
    /// broker, or before).
    pub home: String,
    /// Seconds between its checks.
    pub every_s: f64,
    /// What it reads, a line per plugin ("Google Workspace: gws_search_mail").
    pub reads: Vec<String>,
    /// Epoch ms of the last run (0 never), and its error ("" when it went fine).
    pub last_run: f64,
    pub last_error: String,
    /// New items the last run that went through ran (`None` unknown).
    pub last_items: Option<u64>,
    /// A run is in progress (Run now, or its schedule).
    pub running: bool,
    /// Its instances (5.9): each runs in one chat with its own params,
    /// trigger and thread. Empty from a broker before instances.
    pub instances: Vec<Instance>,
    /// Its `params` schema (`Null` when it takes none), and the plugin's
    /// own tools it may change data with.
    pub params_schema: Value,
    pub writes: Vec<String>,
}

/// One instance of a routine (5.9, "Routine instances").
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Instance {
    pub instance: String,
    /// Its label, else its params in one line ("" none).
    pub summary: String,
    pub params: Value,
    /// `schedule` or `webhook`.
    pub trigger: String,
    pub every_s: f64,
    pub enabled: bool,
    /// Waiting for the user's Enable on its consent card.
    pub pending: bool,
    pub thread: String,
    pub home: String,
    pub available: bool,
    pub reason: String,
    pub last_run: f64,
    pub last_error: String,
    pub last_items: Option<u64>,
    pub running: bool,
    /// Its webhook has a URL (shown once, when it is made).
    pub hook: bool,
}

impl Instance {
    fn of(v: &Value) -> Instance {
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        Instance {
            instance: s(v, "instance"),
            summary: s(v, "summary"),
            params: v.get("params").cloned().filter(Value::is_object).unwrap_or_else(|| json!({})),
            trigger: Some(s(v, "trigger")).filter(|t| !t.is_empty()).unwrap_or_else(|| "schedule".to_owned()),
            every_s: v.get("every_s").and_then(Value::as_f64).unwrap_or(0.0),
            enabled: b("enabled"),
            pending: b("pending"),
            thread: s(v, "thread"),
            home: s(v, "home"),
            available: b("available"),
            reason: s(v, "reason"),
            last_run: v.get("last_run").and_then(Value::as_f64).unwrap_or(0.0),
            last_error: s(v, "last_error"),
            last_items: v.get("last_items").and_then(Value::as_u64),
            running: b("running"),
            hook: b("hook"),
        }
    }

    /// The routine as this instance runs it, for its lines
    /// ([`Routine::last_check`], [`Routine::schedule`]).
    pub fn as_routine(&self, r: &Routine) -> Routine {
        Routine {
            enabled: self.enabled,
            available: self.available,
            reason: self.reason.clone(),
            thread: self.thread.clone(),
            home: self.home.clone(),
            every_s: if self.every_s > 0.0 { self.every_s } else { r.every_s },
            last_run: self.last_run,
            last_error: self.last_error.clone(),
            last_items: self.last_items,
            running: self.running,
            instances: Vec::new(),
            ..r.clone()
        }
    }
}

impl Routine {
    /// The quiet line of its thread, and whether it reads as a failure:
    /// "Checks every 5 min · last checked 3:42 PM · nothing new", why it
    /// can't run, or its last error. `checked` is a run's local time.
    pub fn quiet_line(&self, checked: impl Fn(f64) -> String) -> (String, bool) {
        let (last, failed) = self.last_check(checked);
        if !self.enabled || !self.available {
            return (last, failed);
        }
        (format!("{} · {last}", self.schedule()), failed)
    }

    /// "Checks every 5 min".
    pub fn schedule(&self) -> String {
        format!("Checks {}", crate::routines::interval(self.every_s))
    }

    /// Its instances, or for a broker before instances the routine itself
    /// as one (`instance` "", when it was ever enabled).
    pub fn instances_or_self(&self) -> Vec<Instance> {
        if !self.instances.is_empty() || self.thread.is_empty() {
            return self.instances.clone();
        }
        vec![Instance {
            instance: String::new(),
            summary: String::new(),
            params: json!({}),
            trigger: "schedule".to_owned(),
            every_s: self.every_s,
            enabled: self.enabled,
            pending: false,
            thread: self.thread.clone(),
            home: self.home.clone(),
            available: self.available,
            reason: self.reason.clone(),
            last_run: self.last_run,
            last_error: self.last_error.clone(),
            last_items: self.last_items,
            running: self.running,
            hook: false,
        }]
    }

    /// How its last check went, and whether that is a failure: "last
    /// checked 3:42 PM · nothing new", "last check failed: …", "waiting for
    /// its first check" — or "Turned off" / "Can't check: …".
    pub fn last_check(&self, checked: impl Fn(f64) -> String) -> (String, bool) {
        if !self.enabled {
            return ("Turned off".to_owned(), false);
        }
        if !self.available {
            return (format!("Can't check: {}", self.reason), true);
        }
        if !self.last_error.is_empty() {
            return (format!("last check failed: {}", self.last_error), true);
        }
        if self.last_run <= 0.0 {
            return ("waiting for its first check".to_owned(), false);
        }
        let found = match self.last_items {
            Some(0) => " · nothing new".to_owned(),
            Some(1) => " · 1 new item".to_owned(),
            Some(n) => format!(" · {n} new items"),
            None => String::new(),
        };
        (format!("last checked {}{found}", checked(self.last_run)), false)
    }
}

/// A v1 plugin as Connect Apps lists it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Listed {
    pub id: String,
    pub short: String,
    pub title: String,
    pub version: String,
    pub publisher: String,
    /// `starting`, `running`, `restarting`, `stopped`, `failed` or
    /// `update_available`.
    pub state: String,
    pub code: String,
    pub message: String,
    pub enabled: bool,
    pub generation: u64,
    pub tools: Vec<Tool>,
    pub routines: Vec<Routine>,
    /// Each app's title and placements ("BIR Forms · full screen").
    pub apps: Vec<String>,
    /// Its package's accounts (§20), a row per signed-in label; empty
    /// for a bare binary (no package).
    pub accounts: Vec<Account>,
    /// Its package's skills (§7.2); empty for a bare binary.
    pub skills: Vec<Skill>,
    /// The setup its accounts' providers need before a sign-in (the shared
    /// Google client), one per provider; empty when none.
    pub setup: Vec<Setup>,
    /// The host's account commands under its `@short:` (`login`, `logout`,
    /// `switch`, `setup`), for a package with accounts; empty otherwise.
    pub account_commands: Vec<String>,
}

/// What a provider's sign-in needs first (`setup` of the plugin list):
/// its fields and whether each is set, never a value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Setup {
    pub provider: String,
    /// Every field is set, each in its shape.
    pub ready: bool,
    pub fields: Vec<SetupField>,
    /// Why a stored value is no good ("The saved Google Client ID is not a
    /// Client ID…"), shown above its fields; "" when none.
    pub error: String,
}

/// One field of a [`Setup`] (or of a chat card's).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SetupField {
    pub key: String,
    pub label: String,
    pub placeholder: String,
    /// Typed under dots; an empty one keeps the stored value.
    pub secret: bool,
    pub hint: String,
    /// A value is stored.
    pub set: bool,
    /// The stored value of a field that is not secret (the Client ID), to
    /// change it; a secret's never comes.
    pub value: String,
    /// Its shape check, the broker's (`pattern` it must match, else
    /// `pattern_hint`; `not_pattern` it must not, else `not_pattern_hint`).
    pub pattern: String,
    pub pattern_hint: String,
    pub not_pattern: String,
    pub not_pattern_hint: String,
    /// The field a value of the other's shape was meant for, and what a
    /// swap is told.
    pub swap_with: String,
    pub swap_hint: String,
}

/// The line above a setup's fields: what it is for when it is added
/// (`change` false: the sign-in needs it), what changing it does when it is
/// already saved (the page's Change, an `app_setup` card).
pub fn setup_intro(provider: &str, change: bool) -> &'static str {
    match (provider == "google", change) {
        (true, false) => "Signing in needs your Google client first. It is shared by every Google plugin: saving it here updates all of them.",
        (true, true) => "Change the Google client every Google plugin uses. Leave the secret blank to keep the stored one. Changing it signs Google accounts out.",
        (false, false) => "Signing in needs these values first.",
        (false, true) => "Change these values. Leave a secret blank to keep the stored one. Changing them signs its accounts out.",
    }
}

/// What the typed setup values say before they are sent: the two typed
/// the wrong way round (`swapped`, its message), else each field whose
/// value fails its shape. Messages never hold a value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SetupCheck {
    pub swapped: Option<String>,
    pub errors: Vec<(String, String)>,
}

impl SetupCheck {
    /// Field `key`'s shape error (none while swapped: Swap them fixes both).
    pub fn field_error(&self, key: &str) -> Option<&str> {
        self.errors.iter().find(|(k, _)| k == key).map(|(_, e)| e.as_str()).filter(|_| self.swapped.is_none())
    }

    /// The first thing that keeps Save from sending.
    pub fn first(&self) -> Option<String> {
        self.swapped.clone().or_else(|| self.errors.first().map(|(_, e)| e.clone()))
    }
}

/// A field's value as it would be saved: typed (trimmed, not blank), else
/// its stored value when it shows one.
fn effective(f: &SetupField, typed: &std::collections::BTreeMap<String, String>) -> String {
    typed.get(&f.key).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()).unwrap_or_else(|| f.value.clone())
}

/// `fields`' shape checks over the values `typed` (by key), as the broker
/// checks them: only a typed value is checked for its own shape.
pub fn setup_check(fields: &[SetupField], typed: &std::collections::BTreeMap<String, String>) -> SetupCheck {
    // the broker's rules, run here as they are typed (`buwiz_protocol::setup`)
    let rules: Vec<Value> = fields.iter().map(SetupField::rules).collect();
    let as_saved: serde_json::Map<String, Value> = fields.iter().map(|f| (f.key.clone(), json!(effective(f, typed)))).collect();
    let mut out = SetupCheck { swapped: buwiz_protocol::setup::swapped(&rules, &as_saved), errors: Vec::new() };
    for (f, r) in fields.iter().zip(&rules) {
        let Some(v) = typed.get(&f.key).map(|v| v.trim()).filter(|v| !v.is_empty()) else { continue };
        if let Some(why) = buwiz_protocol::setup::shape_error(r, v) {
            out.errors.push((f.key.clone(), why));
        }
    }
    out
}

/// Whether Save may send now: not while a save of the same setup runs,
/// nor with a check failing; → why not (the caller's card hears it, so it
/// is never left busy).
pub fn save_gate(saving: bool, check: &SetupCheck) -> Result<(), String> {
    if saving {
        return Err("This setup is being saved already; try again in a moment".to_owned());
    }
    check.first().map_or(Ok(()), Err)
}

/// Swap them: each field with `swap_with` trades its value as it would be
/// saved (typed, else shown) with that field's, in `typed`.
pub fn swap_typed(fields: &[SetupField], typed: &mut std::collections::BTreeMap<String, String>) {
    for f in fields.iter().filter(|f| !f.swap_with.is_empty()) {
        let Some(o) = fields.iter().find(|x| x.key == f.swap_with) else { continue };
        let (mine, theirs) = (effective(f, typed), effective(o, typed));
        typed.insert(f.key.clone(), theirs);
        typed.insert(o.key.clone(), mine);
    }
}

impl Setup {
    pub fn of(v: &Value) -> Setup {
        Setup {
            provider: s(v, "provider"),
            ready: v.get("ready").and_then(Value::as_bool).unwrap_or(false),
            fields: list(v, "fields").iter().map(SetupField::of).filter(|f| !f.key.is_empty()).collect(),
            error: s(v, "error"),
        }
    }
}

impl SetupField {
    /// Its checks as the broker's field carries them.
    fn rules(&self) -> Value {
        json!({"key": self.key, "pattern": self.pattern, "pattern_hint": self.pattern_hint, "not_pattern": self.not_pattern,
            "not_pattern_hint": self.not_pattern_hint, "swap_with": self.swap_with, "swap_hint": self.swap_hint})
    }

    pub fn of(v: &Value) -> SetupField {
        SetupField {
            key: s(v, "key"),
            label: Some(s(v, "label")).filter(|l| !l.is_empty()).unwrap_or_else(|| s(v, "key")),
            placeholder: s(v, "placeholder"),
            secret: v.get("secret").and_then(Value::as_bool).unwrap_or(false),
            hint: s(v, "hint"),
            set: v.get("set").and_then(Value::as_bool).unwrap_or(false),
            value: if v.get("secret").and_then(Value::as_bool).unwrap_or(false) { String::new() } else { s(v, "value") },
            pattern: s(v, "pattern"),
            pattern_hint: s(v, "pattern_hint"),
            not_pattern: s(v, "not_pattern"),
            not_pattern_hint: s(v, "not_pattern_hint"),
            swap_with: s(v, "swap_with"),
            swap_hint: s(v, "swap_hint"),
        }
    }
}

/// One account row of a package (§20): only its state and who
/// it is; the token stays with the broker and the plugin.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Account {
    /// The manifest's account id (`google`).
    pub account: String,
    /// This sign-in's label (`default`, `work`).
    pub label: String,
    pub provider: String,
    /// The manifest's label ("Google account").
    pub title: String,
    /// Who is signed in ("" none).
    pub subject: String,
    /// `none`, `connected` or `expired`.
    pub state: String,
    pub required: bool,
    /// What Setup lacks before it can sign in ("Add your Google client in
    /// Setup"); "" when nothing.
    pub setup: String,
    /// Why its last sign-in failed ("" none).
    pub error: String,
    /// The setup fixes that failure (the provider refused the client):
    /// the row offers Change.
    pub fix_setup: bool,
    /// The active label of its account: the one the plugin gets.
    pub active: bool,
    /// A sign-in of it waits on its callback at the broker (gone after a
    /// broker restart, or once it finished).
    pub pending: bool,
    /// It's connected, but an update asks for scopes this sign-in doesn't
    /// have: signing in again grants them.
    pub needs_access: bool,
}

impl Account {
    fn of(v: &Value) -> Account {
        Account {
            account: s(v, "account"),
            label: Some(s(v, "label")).filter(|l| !l.is_empty()).unwrap_or_else(|| "default".to_owned()),
            provider: s(v, "provider"),
            title: s(v, "title"),
            subject: s(v, "subject"),
            state: Some(s(v, "state")).filter(|l| !l.is_empty()).unwrap_or_else(|| "none".to_owned()),
            required: v.get("required").and_then(Value::as_bool).unwrap_or(false),
            setup: s(v, "setup"),
            error: s(v, "error"),
            fix_setup: s(v, "fix") == "setup",
            active: v.get("active").and_then(Value::as_bool).unwrap_or(false),
            pending: v.get("pending").and_then(Value::as_bool).unwrap_or(false),
            needs_access: v.get("needs_access").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    pub fn connected(&self) -> bool {
        self.state == "connected"
    }

    /// The row's status word.
    pub fn status(&self) -> &'static str {
        match self.state.as_str() {
            "connected" => "Connected",
            "expired" => "Needs sign-in",
            _ => "Not signed in",
        }
    }

    /// "Google account", or "Google account (work)" for a label of its own:
    /// the name its controls are labelled with.
    pub fn name(&self) -> String {
        let title = if self.title.is_empty() { &self.account } else { &self.title };
        if self.label == "default" { title.clone() } else { format!("{title} ({})", self.label) }
    }

    /// The key a sign-in in progress is kept under.
    pub fn key(&self) -> String {
        account_key(&self.account, &self.label)
    }
}

/// `<account>/<label>`.
pub fn account_key(account: &str, label: &str) -> String {
    format!("{account}/{label}")
}

/// One of a package's skills, as its page lists it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

impl Listed {
    pub fn of(v: &Value) -> Listed {
        Listed {
            id: s(v, "id"),
            short: s(v, "short"),
            title: s(v, "title"),
            version: s(v, "version"),
            publisher: v.get("publisher").map(|p| s(p, "name")).unwrap_or_default(),
            state: s(v, "state"),
            code: s(v, "code"),
            message: s(v, "message"),
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            generation: v.get("generation").and_then(Value::as_u64).unwrap_or(0),
            // the reserved lifecycle tools (`buwiz.provision`, …; `rox.*` in a
            // record from Rox) are the broker's, never the
            // person's to switch: not listed, not counted
            tools: list(v, "tools")
                .iter()
                .filter(|t| !reserved_tool(&s(t, "name")))
                .map(|t| Tool {
                    name: s(t, "name"),
                    description: s(t, "description"),
                    disabled: t.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect(),
            routines: list(v, "routines")
                .iter()
                .map(|r| Routine {
                    id: s(r, "id"),
                    name: s(r, "name"),
                    description: s(r, "description"),
                    approved: r.get("approved").and_then(Value::as_bool).unwrap_or(false),
                    enabled: r.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                    available: r.get("available").and_then(Value::as_bool).unwrap_or(false),
                    reason: s(r, "reason"),
                    thread: s(r, "thread"),
                    home: s(r, "home"),
                    every_s: r.get("every_s").and_then(Value::as_f64).unwrap_or(0.0),
                    last_items: r.get("last_items").and_then(Value::as_u64),
                    last_run: r.get("last_run").and_then(Value::as_f64).unwrap_or(0.0),
                    last_error: s(r, "last_error"),
                    running: r.get("running").and_then(Value::as_bool).unwrap_or(false),
                    instances: list(r, "instances").iter().map(Instance::of).collect(),
                    params_schema: r.get("params").cloned().filter(Value::is_object).unwrap_or(Value::Null),
                    writes: list(r, "writes").iter().filter_map(|w| w.as_str().map(str::to_owned)).collect(),
                    reads: list(r, "reads_status")
                        .iter()
                        .map(|x| {
                            let tools: Vec<String> = list(x, "tools").iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
                            format!("{}: {}", s(x, "plugin"), tools.join(", "))
                        })
                        .collect(),
                })
                .collect(),
            apps: list(v, "apps")
                .iter()
                .map(|a| {
                    let places: Vec<String> = list(a, "placement")
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|p| if p == "inline" { "inline".to_owned() } else { "full screen".to_owned() })
                        .collect();
                    format!("{} · {}", s(a, "title"), places.join(", "))
                })
                .collect(),
            accounts: list(v, "accounts").iter().map(Account::of).filter(|a| !a.account.is_empty() && a.state != "removed").collect(),
            skills: list(v, "skills")
                .iter()
                .map(|k| Skill { name: s(k, "name"), description: s(k, "description") })
                .filter(|k| !k.name.is_empty())
                .collect(),
            setup: list(v, "setup").iter().map(Setup::of).filter(|x| !x.provider.is_empty() && !x.fields.is_empty()).collect(),
            account_commands: list(v, "account_commands").iter().filter_map(Value::as_str).filter(|c| super::tokens::ACCOUNT_COMMANDS.contains(c)).map(str::to_owned).collect(),
        }
    }

    /// The setup `provider` still lacks (its page shows the fields instead
    /// of the accounts), `None` when it is ready or needs none.
    pub fn setup_missing(&self, provider: &str) -> Option<&Setup> {
        self.setup.iter().find(|x| x.provider == provider && !x.ready)
    }

    /// Its setup to change from its page (the first; ready or not).
    pub fn first_setup(&self) -> Option<&Setup> {
        self.setup.first()
    }

    /// The first setup any of its accounts still lacks.
    pub fn first_missing_setup(&self) -> Option<&Setup> {
        self.setup.iter().find(|x| !x.ready)
    }

    /// Its accounts by manifest id, in the broker's order: (id, rows).
    pub fn account_groups(&self) -> Vec<(String, Vec<Account>)> {
        let mut out: Vec<(String, Vec<Account>)> = Vec::new();
        for a in &self.accounts {
            match out.iter_mut().find(|(id, _)| *id == a.account) {
                Some((_, rows)) => rows.push(a.clone()),
                None => out.push((a.account.clone(), vec![a.clone()])),
            }
        }
        out
    }

    /// The connected sign-ins of `account`, the active one marked.
    pub fn connected_of(&self, account: &str) -> Vec<&Account> {
        self.accounts.iter().filter(|a| a.account == account && a.connected()).collect()
    }

    /// The active sign-in of `account` (none signed in: None).
    pub fn active_of(&self, account: &str) -> Option<&Account> {
        let c = self.connected_of(account);
        c.iter().find(|a| a.active).or(c.first()).copied()
    }

    /// Whether any sign-in of `account` is connected.
    pub fn account_connected(&self, account: &str) -> bool {
        self.accounts.iter().any(|a| a.account == account && a.connected())
    }

    /// The label a chat card's Sign in uses for `account`: its first row's
    /// (an expired one signs in again), else `default`.
    pub fn signin_label(&self, account: &str) -> String {
        self.accounts.iter().find(|a| a.account == account).map(|a| a.label.clone()).unwrap_or_else(|| "default".to_owned())
    }

    /// The state as the list says it; running while a setup its accounts
    /// need is missing: "Needs setup".
    pub fn state_label(&self) -> String {
        match self.state.as_str() {
            "running" if self.needs_setup() => "Needs setup".to_owned(),
            "running" => "Running".to_owned(),
            "starting" => "Starting…".to_owned(),
            "restarting" => "Restarting…".to_owned(),
            "stopped" if !self.enabled => "Disabled".to_owned(),
            "stopped" => "Stopped".to_owned(),
            "failed" => "Failed".to_owned(),
            "update_available" => "Update available".to_owned(),
            other => other.to_owned(),
        }
    }

    /// Running, with a setup its accounts need still missing.
    pub fn needs_setup(&self) -> bool {
        self.state == "running" && self.first_missing_setup().is_some()
    }

    /// Whether the list offers Retry and Open logs (14.4, 14.5).
    pub fn failed(&self) -> bool {
        self.state == "failed"
    }
}

/// What a plugin's listing lets its cards, pins and pages show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// It may open.
    Open,
    /// It failed and may recover: its cards and pins stay, with Retry.
    Down,
    /// Uninstalled or disabled: nothing that points at it shows.
    Gone,
}

/// `plugin`'s [`Reach`] in the broker's list.
pub fn reach(listed: &[Listed], plugin: &str) -> Reach {
    match listed.iter().find(|p| p.id == plugin) {
        Some(p) if p.enabled && p.failed() => Reach::Down,
        Some(p) if p.enabled => Reach::Open,
        _ => Reach::Gone,
    }
}

/// Why `plugin` isn't running, as the broker lists it: its message, else
/// its code.
pub fn why_down(listed: &[Listed], plugin: &str) -> Option<String> {
    let p = listed.iter().find(|p| p.id == plugin)?;
    Some(if p.message.is_empty() { p.code.clone() } else { p.message.clone() }).filter(|why| !why.is_empty())
}

/// A message that is only `@short:login` (or `logout`, `switch`, `setup`)
/// for a listed plugin that offers it: (plugin id, command). Any other
/// message, or a short whose plugin declares no accounts: None (it goes to
/// the model as typed).
pub fn account_command_for(text: &str, listed: &[Listed]) -> Option<(String, String)> {
    let (short, cmd) = super::tokens::account_command(text)?;
    listed.iter().find(|p| p.short == short && p.account_commands.iter().any(|c| c == cmd)).map(|p| (p.id.clone(), cmd.to_owned()))
}

/// The sign-in an account command's answer asks the app to start at once
/// (`signin: {account, label}`: `@short:login` typed with the setup there,
/// the typed command being the press): (account, label).
pub fn signin_of(answer: &Value) -> Option<(String, String)> {
    let s = answer.get("signin")?;
    let (a, l) = (s.get("account")?.as_str()?, s.get("label").and_then(Value::as_str).unwrap_or("default"));
    (super::host_api::account_id_ok(a) && super::host_api::account_label_ok(l)).then(|| (a.to_owned(), l.to_owned()))
}

/// How long the app waits on a sign-in: the broker's pending flow's life
/// (`PENDING_FOR`, 10 minutes), then its card offers Sign in again.
pub const SIGNIN_WAIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// What the plugin list says of a sign-in being waited on.
#[derive(Debug, Clone, PartialEq)]
pub enum SigninWait {
    /// Still pending at the broker (or the list could not be read).
    Waiting,
    Connected,
    /// The callback failed: why (the row's error).
    Failed(String),
    /// No flow left (the broker restarted, or it ended without a word):
    /// Sign in again.
    Gone,
}

/// `account`'s `label` sign-in as `listed` (None: not read) says it is.
pub fn signin_wait(listed: Option<&Listed>, account: &str, label: &str) -> SigninWait {
    let Some(p) = listed else { return SigninWait::Waiting };
    // pending first: signing in again keeps an existing label connected
    // until the flow ends, and a failed re-sign-in leaves it connected with
    // the reason
    match p.accounts.iter().find(|a| a.account == account && a.label == label) {
        Some(a) if a.pending => SigninWait::Waiting,
        Some(a) if !a.error.is_empty() => SigninWait::Failed(a.error.clone()),
        Some(a) if a.connected() => SigninWait::Connected,
        _ => SigninWait::Gone,
    }
}

/// What an account command row says in the composer's popup.
pub fn account_command_detail(cmd: &str) -> &'static str {
    match cmd {
        "login" => "Sign in to its account",
        "logout" => "Sign out of its account",
        "switch" => "Switch to another signed-in account",
        _ => "Change its setup (the Google client)",
    }
}

/// The installed v1 plugins.
pub fn listed(b: &dyn Broker) -> Result<Vec<Listed>, RoxError> {
    Ok(host_api::plugins(b)?.iter().map(Listed::of).collect())
}

/// What an install or update shows before it goes ahead.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Review {
    pub command: String,
    pub args: Vec<String>,
    /// The environment the plugin is spawned with (`VAR=value` words).
    pub env: Option<Value>,
    pub id: String,
    pub title: String,
    pub publisher: String,
    pub version: String,
    pub description: String,
    /// Every 13.1 step 3 line, in the broker's order: the plugin, each
    /// app with its placement and inline size, each app's permissions,
    /// each routine's reads.
    pub lines: Vec<String>,
    /// 3.1 rule 5: commands this install makes ambiguous.
    pub warnings: Vec<String>,
    /// An update: the version installed now.
    pub from_version: Option<String>,
    pub downgrade: bool,
    /// An update's added permissions (13.2); empty when none.
    pub added: Vec<String>,
    pub manifest_sha256: String,
    /// A package's folder (a GitHub release's), sent back with Install.
    pub package: Option<String>,
}

impl Review {
    pub fn of(inspect: &Value, command: &str, args: &[String]) -> Review {
        let r = inspect.get("review").cloned().unwrap_or(Value::Null);
        let m = inspect.get("manifest").cloned().unwrap_or(Value::Null);
        let u = inspect.get("update").filter(|u| u.is_object());
        let strings = |v: &Value, k: &str| list(v, k).iter().filter_map(|x| x.as_str().map(str::to_owned)).collect::<Vec<_>>();
        Review {
            command: command.to_owned(),
            args: args.to_vec(),
            env: None,
            id: s(&m, "id"),
            title: s(&r, "title"),
            publisher: s(&r, "publisher"),
            version: s(&r, "version"),
            description: s(&r, "description"),
            lines: strings(&r, "lines"),
            warnings: strings(inspect, "warnings"),
            from_version: u.map(|u| s(u, "from_version")),
            downgrade: u.and_then(|u| u.get("downgrade")).and_then(Value::as_bool).unwrap_or(false),
            added: u.map(|u| strings(u, "added_permissions")).unwrap_or_default(),
            manifest_sha256: s(inspect, "manifest_sha256"),
            package: None,
        }
    }

    /// The main button: Install, Update, Approve and update (wider
    /// permissions, 13.2) or Install older version (12.1).
    pub fn action(&self) -> &'static str {
        match (&self.from_version, self.downgrade, self.added.is_empty()) {
            (None, _, _) => "Install",
            (Some(_), true, _) => "Install older version",
            (Some(_), false, false) => "Approve and update",
            (Some(_), false, true) => "Update",
        }
    }
}

/// A plugin's command line as typed: leading `VAR=value` words are its
/// environment, then the command and its arguments. Words split as a shell
/// splits them: `"…"` and `'…'` keep spaces (a path inside `Buwiz Bot.app`),
/// and `\` escapes the next character. An unclosed quote is no command line.
pub fn command_line(line: &str) -> Option<(Option<Value>, String, Vec<String>)> {
    let mut env = serde_json::Map::new();
    let mut words = shell_words(line)?.into_iter().peekable();
    while let Some(w) = words.peek() {
        let Some((k, v)) = w.split_once('=') else { break };
        let name_ok = !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !name_ok || k.starts_with(|c: char| c.is_ascii_digit()) {
            break;
        }
        env.insert(k.to_owned(), json!(v));
        words.next();
    }
    let command = words.next()?;
    Some(((!env.is_empty()).then_some(Value::Object(env)), command, words.collect()))
}

/// `line` split into words: whitespace separates them outside quotes;
/// `'…'` is literal, `"…"` takes `\"` and `\\`. Outside quotes `\` escapes
/// only a space, a quote or `\` and is kept before anything else, so a
/// Windows path (`C:\Program Files\…`, quoted) keeps its separators.
/// `None` for an unclosed quote or a trailing `\`.
fn shell_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\') => word.push(c),
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next()? {
                    c @ (' ' | '\t' | '"' | '\'' | '\\') => word.push(c),
                    c => {
                        word.push('\\');
                        word.push(c);
                    }
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

/// `POST /v1/host/plugins/inspect` for `command args` with `env`: the
/// review (nothing is installed).
pub fn inspect(b: &dyn Broker, command: &str, args: &[String], env: Option<Value>) -> Result<Review, RoxError> {
    let v = host_api::inspect(b, command, args, env.clone())?;
    Ok(Review { env, ..Review::of(&v, command, args) })
}

/// Whether Add… reads `line` as a release to fetch rather than a command
/// line: `https://github.com/…`, one `owner/repo` word, or words with no
/// path, `=` or quote in them (a mini app's name, `Google Workspace`).
pub fn release_source(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("https://github.com/") {
        return true;
    }
    let one_word = !t.contains(char::is_whitespace);
    !t.is_empty() && !t.contains(['=', '"', '\'', '\\']) && !t.starts_with(['.', '/', '~']) && (t.matches('/').count() == 0 || (one_word && t.matches('/').count() == 1))
}

/// Add…: a release source ([`release_source`]) fetched and reviewed by the
/// broker; one the broker can't read as a repository, a multi-word line
/// with no such release, or a command line, inspected as a command.
pub fn add(b: &dyn Broker, line: &str) -> Result<Review, RoxError> {
    if release_source(line) {
        match host_api::fetch_release(b, line) {
            Err(e) if e.code == "invalid_args" => {}
            // plain words with no such release ("fx mcp", "node server"): a
            // command line with arguments; neither: the release's answer
            Err(e) if e.code == "not_found" && line.trim().contains(char::is_whitespace) => {
                let Some((env, command, args)) = command_line(line) else { return Err(e) };
                return inspect(b, &command, &args, env).map_err(|_| e);
            }
            Ok(v) => {
                let args: Vec<String> = list(&v, "args").iter().filter_map(|a| a.as_str().map(str::to_owned)).collect();
                let env = v.get("env").filter(|e| e.as_object().is_some_and(|o| !o.is_empty())).cloned();
                let package = Some(s(&v, "package")).filter(|p| !p.is_empty());
                return Ok(Review { env, package, ..Review::of(&v, &s(&v, "command"), &args) });
            }
            Err(e) => return Err(e),
        }
    }
    let Some((env, command, args)) = command_line(line) else { return Err(RoxError::new("invalid_args", "Enter a mini app's name, a GitHub repository or the plugin's command")) };
    inspect(b, &command, &args, env)
}

/// How an install or update ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Installed {
    Done,
    /// The update adds these permissions: shown, and only Approve goes on.
    NeedsApproval(Vec<String>),
    /// An older version: only "Install older version" goes on.
    NeedsDowngrade,
}

/// Installs what `review` showed, after the person pressed its
/// [`Review::action`]: that press is the approval of its added
/// permissions or of its older version, and nothing else is approved.
pub fn install(b: &dyn Broker, review: &Review) -> Result<Installed, RoxError> {
    install_with(b, install_body(review))
}

/// The install route's body for what `review` showed.
pub fn install_body(review: &Review) -> Value {
    let mut body = json!({"command": review.command, "args": review.args, "manifest_sha256": review.manifest_sha256});
    if let Some(env) = &review.env {
        body["env"] = env.clone();
    }
    if review.from_version.is_some() && !review.added.is_empty() {
        body["approve_permissions"] = json!(true);
    }
    if review.downgrade {
        body["allow_downgrade"] = json!(true);
    }
    if let Some(p) = &review.package {
        body["package"] = json!(p);
    }
    body
}

/// A chat install card's body: what `review` showed, with the values typed
/// into its fields over its env (empty ones left out) or, with `env_from`
/// (an installed plugin, and the keys to take), those keys copied by the
/// broker from that plugin's settings instead.
pub fn chat_install_body(review: &Review, typed: &[(String, String)], env_from: Option<(&str, &[String])>) -> Value {
    let mut body = install_body(review);
    if let Some((from, keys)) = env_from {
        body["env_from"] = json!(from);
        body["env_from_keys"] = json!(keys);
        return body;
    }
    let typed: Vec<_> = typed.iter().filter(|(_, v)| !v.is_empty()).collect();
    if !typed.is_empty() {
        if !body.get("env").is_some_and(Value::is_object) {
            body["env"] = json!({});
        }
        for (k, v) in typed {
            body["env"][k.as_str()] = json!(v);
        }
    }
    body
}

/// `POST /v1/host/plugins/install` with `body` (see [`install`]).
pub fn install_with(b: &dyn Broker, body: Value) -> Result<Installed, RoxError> {
    match b.request("POST", "/v1/host/plugins/install", Some(body), Duration::from_secs(30)) {
        Ok(_) => Ok(Installed::Done),
        Err(e) => match refusal_reason(&e) {
            Some(r) => Ok(r),
            None => Err(e),
        },
    }
}

/// A refusal that asks the person for more: the broker sends its reason
/// in `details` (the [`RoxError`] keeps only the code and message, so the
/// message is read: `consent_declined` is the added permissions, an
/// `invalid_args` naming an older version the downgrade).
fn refusal_reason(e: &RoxError) -> Option<Installed> {
    match e.code.as_str() {
        "consent_declined" => Some(Installed::NeedsApproval(Vec::new())),
        "invalid_args" if e.message.contains("is older than") => Some(Installed::NeedsDowngrade),
        _ => None,
    }
}

/// Enable, disable (13.4, 13.5) or a failed plugin's Retry.
pub fn switch(b: &dyn Broker, plugin: &str, action: PluginAction) -> Result<(), RoxError> {
    host_api::plugin_action(b, plugin, action)?;
    if action == PluginAction::Disable {
        session::end_plugin(plugin, Ended::Closed);
    }
    Ok(())
}

/// What an uninstall removed on Rox's side.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Removed {
    pub sessions: usize,
    pub attachments: usize,
    pub pins: usize,
}

/// Uninstalls `plugin` (13.6): `close_opens` closes its opens on the
/// screen (step 2), its sessions end, the broker uninstalls (steps 1, 3,
/// 6), then its attachments and pins leave `records_dir`'s records and its
/// apps' storage under `plugins_dir` is deleted (steps 4, 5). A refusal by
/// the broker stops it before Rox removes anything.
pub fn uninstall(
    b: &dyn Broker,
    plugin: &str,
    records_dir: &Path,
    plugins_dir: &Path,
    close_opens: &dyn Fn(&str),
) -> Result<Removed, RoxError> {
    if !super::perms::plugin_id_ok(plugin) {
        return Err(RoxError::new("invalid_args", "not a plugin id"));
    }
    close_opens(plugin);
    let sessions = session::end_plugin(plugin, Ended::Closed);
    host_api::plugin_action(b, plugin, PluginAction::Uninstall)?;
    let mut records = Records::load(records_dir);
    let (attachments, pins) = records.remove_plugin(plugin);
    records.save(records_dir).map_err(|e| RoxError::new("internal", format!("the records could not be saved: {e}")))?;
    if let Err(e) = storage::delete_plugin(plugins_dir, plugin) {
        log::warn!("mini v1: {plugin}'s storage: {e}");
    }
    Ok(Removed { sessions, attachments, pins })
}

/// The plugin's log file (14.5): `<broker data dir>/v1/logs/<id>.log`, the
/// broker's data dir being where its host token is.
pub fn log_file(plugin: &str) -> Option<std::path::PathBuf> {
    if !super::perms::plugin_id_ok(plugin) {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let token = crate::client::host_token_path(buwiz_protocol::env::var("BUWIZ_HOST_TOKEN_FILE").as_deref(), &home);
    Some(token.parent()?.join("v1").join("logs").join(format!("{plugin}.log")))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::mini_v1::{links, load::PluginEntry, records::Pin};

    const T: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

    /// A broker that answers the lifecycle routes and remembers them.
    #[derive(Default)]
    struct Lifecycle {
        routes: Mutex<Vec<(String, Value)>>,
        installed: Mutex<Option<String>>,
        refuse: Option<RoxError>,
    }

    fn inspect_answer(update: Value) -> Value {
        json!({
            "ok": true,
            "manifest": {"id": "app.buwiz.fixture", "version": "1.1.0"},
            "manifest_sha256": "ab".repeat(32),
            "review": {
                "title": "Fixture", "publisher": "Goldcoders", "version": "1.1.0", "description": "Every feature.",
                "lines": [
                    "Fixture 1.1.0 by Goldcoders",
                    "Every feature.",
                    "Fixture Board (full screen, inline 480×320)",
                    "  Can call 8 Fixture tools",
                    "  Can open: Fixture Card",
                    "  Can attach: Item",
                    "  Can read files: .xlsx, .csv up to 25 MB",
                    "  Storage: 1 MB",
                    "  Copy to clipboard",
                    "Routine 'Mail' reads via Google Workspace: gws_search_mail, gws_read_mail",
                ],
            },
            "update": update,
            "warnings": ["/report is also a command of app.buwiz.acc; it will need /fx:report"],
        })
    }

    impl Broker for Lifecycle {
        fn request(&self, method: &str, route: &str, body: Option<Value>, _: Duration) -> Result<Value, RoxError> {
            self.routes.lock().unwrap().push((format!("{method} {route}"), body.clone().unwrap_or(Value::Null)));
            match route {
                "/v1/host/plugins/inspect" => match body.as_ref().and_then(|b| b.get("command")).and_then(Value::as_str) {
                    Some("nosuch") => Err(RoxError::new("plugin_unavailable", "spawn nosuch: No such file or directory")),
                    _ => Ok(inspect_answer(Value::Null)),
                },
                // as the broker answers: plain words are a name (buwiz-app/<slug>); one with no release, not_found
                "/v1/host/plugins/fetch" => match body.as_ref().and_then(|b| b.get("source")).and_then(Value::as_str) {
                    Some(n @ ("word" | "fx mcp" | "python3 server" | "nosuch arg")) => {
                        Err(RoxError::new("not_found", format!("buwiz-app/{} has no release yet", n.replace(' ', "-"))))
                    }
                    Some("a.b") => Err(RoxError::new("invalid_args", "not a GitHub repository")),
                    _ => {
                        let mut v = inspect_answer(Value::Null);
                        v["command"] = json!("/data/v1/apps/app.buwiz.fixture/1.1.0/bin/fx");
                        v["args"] = json!(["mcp"]);
                        v["env"] = json!({"FX_MANIFEST": "/data/m.json"});
                        v["package"] = json!("/data/v1/apps/app.buwiz.fixture/1.1.0");
                        Ok(v)
                    }
                },
                "/v1/host/plugins/install" => match &self.refuse {
                    Some(e) => Err(e.clone()),
                    None => {
                        *self.installed.lock().unwrap() = Some("app.buwiz.fixture".into());
                        Ok(json!({"ok": true}))
                    }
                },
                r if r.ends_with("/uninstall") => {
                    *self.installed.lock().unwrap() = None;
                    Ok(json!({"ok": true}))
                }
                _ => Ok(json!({"ok": true})),
            }
        }
    }

    impl Lifecycle {
        fn routes(&self) -> Vec<String> {
            self.routes.lock().unwrap().iter().map(|r| r.0.clone()).collect()
        }
    }

    #[test]
    fn add_fetches_a_name_or_repository_and_inspects_a_command_line() {
        for src in ["word", "Google Workspace", "word@v1.0.0", "owner/repo", "https://github.com/o/r/releases/tag/v1", "python3 server"] {
            assert!(release_source(src), "{src}");
        }
        for cmd in ["node /path/to/plugin.mjs", "FX=1 node x.mjs", "/apps/fx mcp", "./fx", "~/fx", "'/Applications/Buwiz Bot.app/x'", "a/b c"] {
            assert!(!release_source(cmd), "{cmd}");
        }
        let b = Lifecycle::default();
        let r = add(&b, "Google Workspace").unwrap();
        assert_eq!((r.command.as_str(), r.args.clone(), r.package.as_deref()), ("/data/v1/apps/app.buwiz.fixture/1.1.0/bin/fx", vec!["mcp".to_owned()], Some("/data/v1/apps/app.buwiz.fixture/1.1.0")));
        assert_eq!(r.env, Some(json!({"FX_MANIFEST": "/data/m.json"})));
        assert_eq!(install_body(&r)["package"], "/data/v1/apps/app.buwiz.fixture/1.1.0", "Install sends the package back");
        assert_eq!(b.routes(), ["POST /v1/host/plugins/fetch"]);
        assert_eq!(add(&b, "word").unwrap_err().message, "buwiz-app/word has no release yet");
        // not a repository to the broker: inspected as a command
        // a command with plain-word arguments reads as a name too: no such
        // release, so it is inspected as the command line it is
        let b = Lifecycle::default();
        assert_eq!(add(&b, "python3 server").unwrap().command, "python3");
        assert_eq!(b.routes(), ["POST /v1/host/plugins/fetch", "POST /v1/host/plugins/inspect"]);
        let r = add(&Lifecycle::default(), "fx mcp").unwrap();
        assert_eq!((r.command.as_str(), r.args.clone()), ("fx", vec!["mcp".to_owned()]));
        // neither a release nor a command that starts: the release's word
        assert_eq!(add(&Lifecycle::default(), "nosuch arg").unwrap_err().message, "buwiz-app/nosuch-arg has no release yet");
        // a single word with no release stays the name's answer, no spawn
        let b = Lifecycle::default();
        assert_eq!(add(&b, "word").unwrap_err().code, "not_found");
        assert_eq!(b.routes(), ["POST /v1/host/plugins/fetch"]);
        let b = Lifecycle::default();
        add(&b, "node /path/to/plugin.mjs").unwrap();
        assert_eq!(b.routes(), ["POST /v1/host/plugins/inspect"]);
        assert!(install_body(&Review::default()).get("package").is_none());
    }

    #[test]
    fn lcy_01_install_shows_every_review_line_and_cancel_changes_nothing() {
        let b = Lifecycle::default();
        let r = inspect(&b, "node", &["fixture.mjs".to_owned()], None).unwrap();
        // every line of 13.1 step 3 the broker gave is in the review
        let want = inspect_answer(Value::Null)["review"]["lines"].clone();
        assert_eq!(json!(r.lines), want);
        assert!(r.lines.iter().any(|l| l.contains("inline 480×320")));
        assert!(r.lines.iter().any(|l| l.starts_with("Routine 'Mail' reads")));
        assert_eq!((r.title.as_str(), r.publisher.as_str(), r.version.as_str()), ("Fixture", "Goldcoders", "1.1.0"));
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.action(), "Install");
        // Cancel: the review is dropped; nothing but the inspect happened
        drop(r);
        assert_eq!(b.routes(), ["POST /v1/host/plugins/inspect"]);
        assert!(b.installed.lock().unwrap().is_none());
        // Install installs exactly what was reviewed
        let r = inspect(&b, "node", &["fixture.mjs".to_owned()], None).unwrap();
        assert_eq!(install(&b, &r).unwrap(), Installed::Done);
        let sent = b.routes.lock().unwrap().last().unwrap().1.clone();
        assert_eq!(sent["manifest_sha256"], "ab".repeat(32));
        assert!(sent.get("approve_permissions").is_none() && sent.get("allow_downgrade").is_none());
    }

    #[test]
    fn lcy_02_an_update_adding_permissions_needs_approval_and_one_removing_does_not() {
        let wider = Review::of(
            &inspect_answer(json!({"from_version": "1.0.0", "to_version": "1.1.0", "downgrade": false, "added_permissions": ["Fixture Board: can call fx_new"]})),
            "node",
            &[],
        );
        assert_eq!(wider.added, ["Fixture Board: can call fx_new"]);
        assert_eq!(wider.action(), "Approve and update");
        let b = Lifecycle::default();
        install(&b, &wider).unwrap();
        assert_eq!(b.routes.lock().unwrap()[0].1["approve_permissions"], true);
        let narrower = Review::of(
            &inspect_answer(json!({"from_version": "1.0.0", "to_version": "1.1.0", "downgrade": false, "added_permissions": []})),
            "node",
            &[],
        );
        assert_eq!(narrower.action(), "Update");
        let b = Lifecycle::default();
        install(&b, &narrower).unwrap();
        assert!(b.routes.lock().unwrap()[0].1.get("approve_permissions").is_none());
        // the broker refusing an unapproved widening asks again
        let b = Lifecycle { refuse: Some(RoxError::new("consent_declined", "adds permissions")), ..Default::default() };
        assert_eq!(install(&b, &narrower).unwrap(), Installed::NeedsApproval(Vec::new()));
    }

    #[test]
    fn lcy_03_a_downgrade_needs_install_older_version() {
        let older = Review::of(
            &inspect_answer(json!({"from_version": "1.2.0", "to_version": "1.1.0", "downgrade": true, "added_permissions": []})),
            "node",
            &[],
        );
        assert_eq!(older.action(), "Install older version");
        let b = Lifecycle::default();
        install(&b, &older).unwrap();
        assert_eq!(b.routes.lock().unwrap()[0].1["allow_downgrade"], true);
        let b = Lifecycle { refuse: Some(RoxError::new("invalid_args", "1.1.0 is older than the installed 1.2.0")), ..Default::default() };
        let mut silent = older.clone();
        silent.downgrade = false;
        assert_eq!(install(&b, &silent).unwrap(), Installed::NeedsDowngrade);
    }

    /// Only a person's Retry asks the broker to take the keystore's key.
    #[test]
    fn retry_asks_for_the_stored_key_and_an_enable_does_not() {
        let b = Lifecycle::default();
        switch(&b, "app.buwiz.fixture", PluginAction::Enable).unwrap();
        switch(&b, "app.buwiz.fixture", PluginAction::Retry).unwrap();
        let sent = b.routes.lock().unwrap().clone();
        let route = "POST /v1/host/plugins/app.buwiz.fixture/enable".to_owned();
        assert_eq!(sent, [(route.clone(), json!({})), (route, json!({"adopt_key": true}))]);
    }

    #[test]
    fn lcy_06_uninstall_removes_attachments_pins_storage_and_the_link_is_not_installed() {
        let base = std::env::temp_dir().join(format!("rox-uninstall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (records_dir, plugins_dir) = (base.join("rox"), base.join("rox/plugins"));
        // its own id: ending its sessions must not touch other tests' opens
        let id = "app.buwiz.uninstallfx";
        let entry = PluginEntry::from_json(&json!({"id": id, "short": "fx", "title": "Fixture", "version": "1.0.0"})).unwrap();
        let mut r = Records::default();
        r.attach("th1", &format!("@fx:item:{T}"), "Item One", &entry);
        r.pin(Pin { plugin: id.into(), app: "board".into(), placement: "full".into(), title: "Board".into(), ..Default::default() });
        r.save(&records_dir).unwrap();
        let store = storage::AppStorage::new(&plugins_dir, id, "board", 1024);
        store.set("k", "v").unwrap();
        assert!(plugins_dir.join(id).exists());
        let b = Lifecycle { installed: Mutex::new(Some(id.into())), ..Default::default() };
        let closed = Mutex::new(Vec::new());
        let removed = uninstall(&b, id, &records_dir, &plugins_dir, &|p| closed.lock().unwrap().push(p.to_owned())).unwrap();
        assert_eq!((removed.attachments, removed.pins), (1, 1));
        assert_eq!(*closed.lock().unwrap(), [id]);
        assert_eq!(b.routes(), [format!("POST /v1/host/plugins/{id}/uninstall")]);
        let after = Records::load(&records_dir);
        assert!(after.attachments.is_empty() && after.pins.is_empty());
        assert!(!plugins_dir.join(id).exists(), "its storage is deleted");
        // a later link shows "not installed" and does nothing else
        assert_eq!(links::route(&format!("buwiz://app/{id}/board"), &[]), links::Route::NotInstalled(id.into()));
        // a broker refusal removes nothing on Rox's side
        let mut r = Records::default();
        r.attach("th1", &format!("@fx:item:{T}"), "Item One", &entry);
        r.save(&records_dir).unwrap();
        struct Refuses;
        impl Broker for Refuses {
            fn request(&self, _: &str, _: &str, _: Option<Value>, _: Duration) -> Result<Value, RoxError> {
                Err(RoxError::new("plugin_unavailable", "down"))
            }
        }
        assert!(uninstall(&Refuses, id, &records_dir, &plugins_dir, &|_| {}).is_err());
        assert_eq!(Records::load(&records_dir).attachments.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_chat_cards_install_sends_its_body_and_its_answer_settles_the_notice() {
        let r = Review::of(&inspect_answer(Value::Null), "/apps/fx", &["mcp".to_owned()]);
        let typed = [("FX_KEY".to_owned(), "s3cret".to_owned())];
        let b = Lifecycle::default();
        assert_eq!(install_with(&b, chat_install_body(&r, &typed, None)).unwrap(), Installed::Done);
        let sent = b.routes.lock().unwrap()[0].clone();
        assert_eq!(sent.0, "POST /v1/host/plugins/install");
        assert_eq!(sent.1, json!({"command": "/apps/fx", "args": ["mcp"], "manifest_sha256": "ab".repeat(32), "env": {"FX_KEY": "s3cret"}}));
        let keys = ["FX_KEY".to_owned()];
        let b = Lifecycle::default();
        install_with(&b, chat_install_body(&r, &typed, Some(("dev.fx", &keys)))).unwrap();
        let sent = b.routes.lock().unwrap()[0].1.clone();
        assert_eq!((sent["env_from"].clone(), sent["env_from_keys"].clone(), sent.get("env")), (json!("dev.fx"), json!(["FX_KEY"]), None));
        // a refusal is the card's error
        let b = Lifecycle { refuse: Some(RoxError::new("invalid_args", "manifest changed")), ..Default::default() };
        assert_eq!(install_with(&b, chat_install_body(&r, &[], None)).unwrap_err().message, "manifest changed");
        let b = Lifecycle::default();
        host_api::settle_notice(&b, "c1", "app_install", "app.buwiz.fixture", "declined").unwrap();
        assert_eq!(
            b.routes.lock().unwrap()[0],
            ("POST /v1/host/notices/settle".to_owned(), json!({"chat": "c1", "kind": "app_install", "id": "app.buwiz.fixture", "state": "declined"}))
        );
    }

    #[test]
    fn a_command_line_takes_its_environment_first() {
        let (env, cmd, args) = command_line("HOME=/tmp/h BIR_X=1 /bin/bir mcp --flag").unwrap();
        assert_eq!(env, Some(json!({"HOME": "/tmp/h", "BIR_X": "1"})));
        assert_eq!((cmd.as_str(), args), ("/bin/bir", vec!["mcp".to_owned(), "--flag".to_owned()]));
        let (env, cmd, args) = command_line("node a=b").unwrap();
        assert_eq!((env, cmd.as_str(), args), (None, "node", vec!["a=b".to_owned()]));
        assert!(command_line("  ").is_none());
        // a path with a space (an app bundle, Program Files), quoted or
        // escaped, stays one word on every platform's path style
        let unix = "/opt/Buwiz Bot/bin/plugin";
        let (_, cmd, args) = command_line(&format!("\"{unix}\" mcp")).unwrap();
        assert_eq!((cmd.as_str(), args), (unix, vec!["mcp".to_owned()]));
        let (_, cmd, _) = command_line(&format!("{} mcp", unix.replace(' ', "\\ "))).unwrap();
        assert_eq!(cmd, unix);
        let windows = r"C:\Program Files\Buwiz Bot\plugin.exe";
        let (_, cmd, _) = command_line(&format!("\"{windows}\" mcp")).unwrap();
        assert_eq!(cmd, windows);
        // an unquoted Windows path keeps its separators
        let (_, cmd, _) = command_line(r"C:\tools\plugin.exe mcp").unwrap();
        assert_eq!(cmd, r"C:\tools\plugin.exe");
        // an env value with spaces (a JSON upstream) in single quotes
        let (env, cmd, _) = command_line(r#"GWS_UPSTREAM='{"command": "uvx"}' /bin/gws mcp"#).unwrap();
        assert_eq!((env, cmd.as_str()), (Some(json!({"GWS_UPSTREAM": "{\"command\": \"uvx\"}"})), "/bin/gws"));
        // double quotes take \" and \\; other escapes stay as typed
        let (_, _, args) = command_line(r#"x "a \"b\" \\ \n""#).unwrap();
        assert_eq!(args, vec![r#"a "b" \ \n"#.to_owned()]);
        // an unclosed quote or a trailing backslash is not a command line
        assert!(command_line("\"/bin/x mcp").is_none());
        assert!(command_line("/bin/x \\").is_none());
        // the environment goes with the install of what was reviewed
        let b = Lifecycle::default();
        let r = inspect(&b, "node", &[], Some(json!({"HOME": "/tmp/h"}))).unwrap();
        assert_eq!(b.routes.lock().unwrap()[0].1["env"], json!({"HOME": "/tmp/h"}));
        install(&b, &r).unwrap();
        assert_eq!(b.routes.lock().unwrap()[1].1["env"], json!({"HOME": "/tmp/h"}));
    }

    #[test]
    fn the_list_reads_state_version_publisher_tools_and_routines() {
        let l = Listed::of(&json!({
            "id": "app.buwiz.ebirforms", "short": "bir", "title": "BIR", "version": "1.0.0", "publisher": {"name": "Goldcoders", "key_id": "gc-2026-01"},
            "state": "failed", "code": "plugin_bad_signature", "message": "bad", "enabled": true, "generation": 4,
            "tools": [{"name": "bir_resolve", "description": "Resolve", "disabled": false}, {"name": "bir_file", "disabled": true}],
            "routines": [{"id": "bir-payments", "name": "Payments", "approved": false, "enabled": false, "available": false,
                "reason": "app.buwiz.gws is not installed", "reads_status": [{"plugin": "app.buwiz.gws", "tools": ["gws_search_mail"]}],
                "last_run": 1793194720000u64, "last_error": "bir_payment_search: offline"}],
            "apps": [{"id": "forms", "title": "BIR Forms", "placement": ["full", "inline"]}],
        }));
        assert_eq!((l.publisher.as_str(), l.state_label().as_str(), l.failed()), ("Goldcoders", "Failed", true));
        // running, but a setup its accounts need is missing: it says so
        let running = |setup: Value| Listed::of(&json!({"id": "app.buwiz.gws", "state": "running", "enabled": true, "setup": setup}));
        let missing = running(json!([{"provider": "google", "ready": false, "fields": [{"key": "GOOGLE_CLIENT_ID"}]}]));
        assert_eq!((missing.state_label().as_str(), missing.needs_setup()), ("Needs setup", true));
        let ready = running(json!([{"provider": "google", "ready": true, "fields": [{"key": "GOOGLE_CLIENT_ID", "set": true}]}]));
        assert_eq!((ready.state_label().as_str(), ready.needs_setup()), ("Running", false));
        assert_eq!(running(json!([])).state_label(), "Running", "no setup to need");
        assert_eq!(l.tools.iter().filter(|t| t.disabled).count(), 1);
        // the lifecycle tools are the broker's: not listed, not counted
        let reserved = Listed::of(&json!({"id": "app.buwiz.books", "tools": [
            {"name": "buwiz.provision"}, {"name": "buwiz.session.open"}, {"name": "rox.ticket.resolve"}, {"name": "books_today"}, {"name": "buwiz_x"}]}));
        assert_eq!(reserved.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["books_today", "buwiz_x"]);
        assert_eq!(l.routines[0].reads, ["app.buwiz.gws: gws_search_mail"]);
        assert_eq!((l.routines[0].last_run, l.routines[0].last_error.as_str()), (1793194720000.0, "bir_payment_search: offline"));
        assert_eq!((l.routines[0].home.as_str(), l.routines[0].last_items), ("", None));
        assert_eq!(l.apps, ["BIR Forms · full screen, inline"]);
        let p = log_file("app.buwiz.ebirforms").unwrap();
        assert!(p.ends_with("v1/logs/app.buwiz.ebirforms.log"), "{}", p.display());
        assert_eq!(log_file("../x"), None);
    }

    #[test]
    fn a_2_1_plugin_lists_its_accounts_and_skills_and_a_2_0_plugin_neither() {
        let old = Listed::of(&json!({"id": "app.buwiz.ebirforms", "title": "BIR", "tools": []}));
        assert!(old.accounts.is_empty() && old.skills.is_empty() && old.account_groups().is_empty());
        assert_eq!(old.signin_label("google"), "default");
        let l = Listed::of(&json!({"id": "app.buwiz.google-workspace", "title": "Google Workspace",
            "accounts": [
                {"account": "google", "label": "default", "provider": "google", "title": "Google account", "subject": "uriah@example.com", "state": "connected", "required": true},
                {"account": "google", "label": "work", "provider": "google", "title": "Google account", "subject": null, "state": "expired", "required": true},
                {"account": "drive", "label": "default", "provider": "oauth:https://auth.example.com", "title": "Drive", "subject": null, "state": "none", "required": false,
                    "setup": "Add your Google client in Setup"},
                {"account": "google", "label": "old", "provider": "google", "title": "Google account", "state": "removed"},
                {"label": "no account id"},
            ],
            "skills": [{"name": "triage-inbox", "description": "Sort the inbox"}, {"description": "no name"}],
        }));
        assert_eq!(l.accounts.len(), 3, "a removed row and one without an account id are dropped");
        assert_eq!((l.accounts[0].setup.as_str(), l.accounts[2].setup.as_str()), ("", "Add your Google client in Setup"));
        let a = &l.accounts[0];
        assert_eq!((a.subject.as_str(), a.status(), a.name(), a.required), ("uriah@example.com", "Connected", "Google account".to_owned(), true));
        let w = &l.accounts[1];
        assert_eq!((w.subject.as_str(), w.status(), w.name(), w.key()), ("", "Needs sign-in", "Google account (work)".to_owned(), "google/work".to_owned()));
        assert_eq!(l.accounts[2].status(), "Not signed in");
        let groups = l.account_groups();
        assert_eq!(groups.iter().map(|(id, rows)| (id.as_str(), rows.len())).collect::<Vec<_>>(), [("google", 2), ("drive", 1)]);
        assert!(l.account_connected("google") && !l.account_connected("drive"));
        assert_eq!(l.signin_label("google"), "default");
        assert_eq!(l.skills, [Skill { name: "triage-inbox".into(), description: "Sort the inbox".into() }]);
        // a failed sign-in: why, and whether Change (the setup) fixes it
        let failed = Listed::of(&json!({"id": "x.y.z", "accounts": [
            {"account": "google", "provider": "google", "state": "none", "error": "Google refused this client. Check the Client ID and Secret.", "fix": "setup"},
            {"account": "drive", "provider": "google", "state": "none", "error": "Bad code"}]}));
        assert_eq!((failed.accounts[0].error.as_str(), failed.accounts[0].fix_setup), ("Google refused this client. Check the Client ID and Secret.", true));
        assert_eq!((failed.accounts[1].error.as_str(), failed.accounts[1].fix_setup), ("Bad code", false));
        assert!(l.accounts[0].error.is_empty() && !l.accounts[0].fix_setup);
        // a broker that leaves out label and state: the default slot, not signed in
        let bare = Listed::of(&json!({"id": "x.y.z", "accounts": [{"account": "google", "provider": "google"}]}));
        assert_eq!((bare.accounts[0].label.as_str(), bare.accounts[0].state.as_str(), bare.accounts[0].name()), ("default", "none", "google".to_owned()));
    }

    #[test]
    fn a_plugins_setup_lists_its_fields_and_whether_each_is_set() {
        let l = Listed::of(&json!({"id": "app.buwiz.google-workspace", "setup": [
            {"provider": "google", "ready": false, "fields": [
                {"key": "GOOGLE_CLIENT_ID", "label": "Google Client ID", "placeholder": "x.apps.googleusercontent.com", "secret": false, "hint": "Shared", "set": true},
                {"key": "GOOGLE_CLIENT_SECRET", "label": "Google Client Secret", "placeholder": "GOCSPX-…", "secret": true, "hint": "", "set": false},
                {"label": "no key"},
            ]},
            {"provider": "oauth:https://auth.example.com", "ready": true, "fields": [{"key": "K"}]},
            {"provider": "empty", "ready": false, "fields": []},
        ]}));
        assert_eq!(l.setup.len(), 2, "a setup without fields is dropped");
        let g = l.setup_missing("google").expect("the Google client is missing");
        assert_eq!(g.fields.len(), 2);
        assert_eq!((g.fields[0].key.as_str(), g.fields[0].secret, g.fields[0].set), ("GOOGLE_CLIENT_ID", false, true));
        assert_eq!((g.fields[1].label.as_str(), g.fields[1].secret, g.fields[1].set), ("Google Client Secret", true, false));
        assert!(l.setup_missing("oauth:https://auth.example.com").is_none(), "ready");
        assert_eq!(l.first_missing_setup().map(|x| x.provider.as_str()), Some("google"));
        let ready = Listed::of(&json!({"id": "x.y.z", "setup": [{"provider": "google", "ready": true, "fields": [{"key": "K", "set": true}]}]}));
        assert!(ready.first_missing_setup().is_none() && ready.setup_missing("google").is_none());
        assert!(Listed::of(&json!({"id": "x.y.z"})).setup.is_empty());
        // a stored value of the wrong shape: not ready, and why
        let bad = Listed::of(&json!({"id": "x.y.z", "setup": [{"provider": "google", "ready": false, "error": "The saved Google Client ID is not a Client ID. Change it in Setup.", "fields": [{"key": "K", "set": true}]}]}));
        assert_eq!(bad.setup_missing("google").map(|x| x.error.as_str()), Some("The saved Google Client ID is not a Client ID. Change it in Setup."));
        assert_eq!(l.setup_missing("google").map(|x| x.error.as_str()), Some(""));
        assert_eq!(SetupField::of(&json!({"key": "K"})).label, "K", "the key labels a field without one");
        assert_eq!((g.fields[0].value.as_str(), g.fields[1].value.as_str()), ("", ""));
        let shown = SetupField::of(&json!({"key": "GOOGLE_CLIENT_ID", "set": true, "value": "cid"}));
        assert_eq!(shown.value, "cid", "the Client ID shows, to be changed");
        assert_eq!(SetupField::of(&json!({"key": "S", "secret": true, "set": true, "value": "leak"})).value, "", "a secret's value is never kept");
        assert_eq!(ready.first_setup().map(|x| x.provider.as_str()), Some("google"));
        assert!(Listed::of(&json!({"id": "x.y.z"})).first_setup().is_none());
        // adding it says the sign-in needs it; changing it, what a change does
        assert!(setup_intro("google", false).starts_with("Signing in needs your Google client first"));
        let change = setup_intro("google", true);
        assert!(!change.contains("Signing in needs") && change.contains("Leave the secret blank") && change.ends_with("Changing it signs Google accounts out."));
        assert!(setup_intro("oauth:x", true).starts_with("Change these values"));
    }

    #[test]
    fn an_account_command_alone_is_the_hosts_for_a_plugin_with_accounts_only() {
        let listed = vec![
            Listed::of(&json!({"id": "app.buwiz.google-workspace", "short": "gws", "account_commands": ["login", "logout", "switch", "setup", "bogus"]})),
            Listed::of(&json!({"id": "app.buwiz.ebirforms", "short": "ebirforms"})),
        ];
        assert_eq!(listed[0].account_commands, ["login", "logout", "switch", "setup"]);
        for c in ["login", "logout", "switch", "setup"] {
            assert_eq!(account_command_for(&format!(" @gws:{c} "), &listed), Some(("app.buwiz.google-workspace".to_owned(), c.to_owned())));
        }
        // with other text, another plugin's or no accounts: the model's, as typed
        for to_model in ["@gws:login please", "log me in @gws:login", "@ebirforms:login", "@books:login", "@gws:taxform:x", "@gws:bogus"] {
            assert_eq!(account_command_for(to_model, &listed), None, "{to_model}");
        }
        // the route it posts, and the use route
        let b = Lifecycle::default();
        host_api::account_command(&b, "app.buwiz.google-workspace", "c1", "switch").unwrap();
        host_api::account_action(&b, "app.buwiz.google-workspace", "google", "work", host_api::AccountAction::Use).unwrap();
        assert_eq!(
            *b.routes.lock().unwrap(),
            [
                ("POST /v1/host/plugins/app.buwiz.google-workspace/command".to_owned(), json!({"chat": "c1", "command": "switch"})),
                ("POST /v1/host/plugins/app.buwiz.google-workspace/accounts/google/use".to_owned(), json!({"label": "work"})),
            ]
        );
        assert!(host_api::account_command(&b, "app.buwiz.gws", "c1", "status").is_err() && host_api::account_command(&b, "../x", "c1", "login").is_err());
    }

    #[test]
    fn a_sign_in_waits_while_pending_and_ends_when_done_failed_or_gone() {
        let at = |rows: Value| Listed::of(&json!({"id": "x.y.z", "accounts": rows}));
        assert_eq!(signin_wait(None, "google", "default"), SigninWait::Waiting, "the list unread (the broker restarting)");
        assert_eq!(signin_wait(Some(&at(json!([{"account": "google", "label": "default", "state": "none", "pending": true}]))), "google", "default"), SigninWait::Waiting);
        assert_eq!(signin_wait(Some(&at(json!([{"account": "google", "label": "default", "state": "connected"}]))), "google", "default"), SigninWait::Connected);
        let failed = at(json!([{"account": "google", "label": "default", "state": "none", "error": "Sign-in failed: access_denied"}]));
        assert_eq!(signin_wait(Some(&failed), "google", "default"), SigninWait::Failed("Sign-in failed: access_denied".into()));
        // signing in again to a connected label: wait until its flow ends
        let again = at(json!([{"account": "google", "label": "default", "state": "connected", "pending": true}]));
        assert_eq!(signin_wait(Some(&again), "google", "default"), SigninWait::Waiting);
        // a failed re-sign-in: still connected, with the reason
        let failed_again = at(json!([{"account": "google", "label": "default", "state": "connected", "error": "Google refused this client. Check the Client ID and Secret."}]));
        assert_eq!(signin_wait(Some(&failed_again), "google", "default"), SigninWait::Failed("Google refused this client. Check the Client ID and Secret.".into()));
        // refused while the flow still waits: keep waiting
        let refused = at(json!([{"account": "google", "label": "default", "state": "none", "pending": true, "error": "The sign-in answer did not match. Sign in again."}]));
        assert_eq!(signin_wait(Some(&refused), "google", "default"), SigninWait::Waiting);
        // the broker restarted: the flow is gone, Sign in again
        assert_eq!(signin_wait(Some(&at(json!([{"account": "google", "label": "default", "state": "none"}]))), "google", "default"), SigninWait::Gone);
        assert_eq!(signin_wait(Some(&at(json!([]))), "google", "work"), SigninWait::Gone, "a new label's row went with its flow");
        assert_eq!(SIGNIN_WAIT, std::time::Duration::from_secs(600));
    }

    #[test]
    fn a_typed_login_starts_the_sign_in_its_answer_names() {
        assert_eq!(signin_of(&json!({"ok": true, "kind": "account_required", "signin": {"account": "google", "label": "work"}})), Some(("google".into(), "work".into())));
        assert_eq!(signin_of(&json!({"signin": {"account": "google"}})), Some(("google".into(), "default".into())));
        assert_eq!(signin_of(&json!({"ok": true, "kind": "account_required"})), None, "setup first, or an agent's card: no start");
        assert_eq!(signin_of(&json!({"signin": {"account": "../x", "label": "default"}})), None);
        assert_eq!(signin_of(&json!({"signin": {"account": "google", "label": "a b"}})), None);
    }

    #[test]
    fn the_active_sign_in_is_the_marked_one_else_the_first_connected() {
        let l = Listed::of(&json!({"id": "x.y.z", "accounts": [
            {"account": "google", "label": "default", "state": "connected", "subject": "d@example.com", "active": false},
            {"account": "google", "label": "work", "state": "connected", "subject": "w@example.com", "active": true},
            {"account": "google", "label": "old", "state": "expired"},
        ]}));
        assert_eq!(l.connected_of("google").iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["default", "work"]);
        assert_eq!(l.active_of("google").map(|a| a.subject.as_str()), Some("w@example.com"));
        let unmarked = Listed::of(&json!({"id": "x.y.z", "accounts": [{"account": "google", "label": "work", "state": "connected"}]}));
        assert_eq!(unmarked.active_of("google").map(|a| a.label.as_str()), Some("work"));
        assert!(Listed::of(&json!({"id": "x.y.z"})).active_of("google").is_none());
    }

    #[test]
    fn the_setup_check_finds_a_swap_and_wrong_shapes_and_swap_them_fixes_it() {
        use std::collections::BTreeMap;
        // the broker's google fields, as setup_view sends them
        let fields: Vec<SetupField> = [
            json!({"key": "GOOGLE_CLIENT_ID", "pattern": "^[0-9]+-[a-z0-9]+\\.apps\\.googleusercontent\\.com$", "pattern_hint": "A Client ID ends with .apps.googleusercontent.com"}),
            json!({"key": "GOOGLE_CLIENT_SECRET", "secret": true, "not_pattern": "\\.apps\\.googleusercontent\\.com$", "not_pattern_hint": "This looks like a Client ID: the secret usually starts with GOCSPX-",
                "swap_with": "GOOGLE_CLIENT_ID", "swap_hint": "The Client ID and Secret look swapped: the Client ID ends with .apps.googleusercontent.com, and the secret usually starts with GOCSPX-."}),
        ]
        .iter()
        .map(SetupField::of)
        .collect();
        let cid = concat!("123-abc.apps", ".googleusercontent.com");
        let typed = |i: &str, s: &str| BTreeMap::from([("GOOGLE_CLIENT_ID".to_owned(), i.to_owned()), ("GOOGLE_CLIENT_SECRET".to_owned(), s.to_owned())]);
        assert_eq!(setup_check(&fields, &typed(cid, "GOCSPX-x")), SetupCheck::default());
        assert_eq!(setup_check(&fields, &typed(cid, "an-older-secret")), SetupCheck::default());
        let mut vals = typed("GOCSPX-x", cid);
        let c = setup_check(&fields, &vals);
        assert!(c.swapped.as_deref().unwrap().starts_with("The Client ID and Secret look swapped"));
        assert!(!c.first().unwrap().contains(cid) && !c.first().unwrap().contains("GOCSPX-x"), "no value in the message");
        assert_eq!(c.field_error("GOOGLE_CLIENT_ID"), None, "while swapped, only Swap them speaks");
        swap_typed(&fields, &mut vals);
        assert_eq!(vals, typed(cid, "GOCSPX-x"), "Swap them trades the two");
        assert_eq!(setup_check(&fields, &vals).first(), None);
        // a wrong shape alone is said under its field
        let c = setup_check(&fields, &typed("my-client", ""));
        assert_eq!((c.swapped.clone(), c.field_error("GOOGLE_CLIENT_ID")), (None, Some("A Client ID ends with .apps.googleusercontent.com")));
        let c = setup_check(&fields, &typed(cid, concat!("456-x.apps", ".googleusercontent.com")));
        assert_eq!(c.field_error("GOOGLE_CLIENT_SECRET"), Some("This looks like a Client ID: the secret usually starts with GOCSPX-"));
        // a stored Client ID counts for the swap; only typed values for their own shape
        let mut stored = fields.clone();
        stored[0].value = "GOCSPX-x".into();
        let only_secret = BTreeMap::from([("GOOGLE_CLIENT_SECRET".to_owned(), cid.to_owned())]);
        assert!(setup_check(&stored, &only_secret).swapped.is_some());
        let mut v = only_secret.clone();
        swap_typed(&stored, &mut v);
        assert_eq!(v, typed(cid, "GOCSPX-x"));
        assert_eq!(setup_check(&fields, &BTreeMap::new()), SetupCheck::default());
        // Save sends only when no save of it runs and the checks pass
        assert_eq!(save_gate(false, &SetupCheck::default()), Ok(()));
        assert_eq!(save_gate(true, &SetupCheck::default()).unwrap_err(), "This setup is being saved already; try again in a moment");
        assert!(save_gate(false, &setup_check(&fields, &typed("GOCSPX-x", cid))).unwrap_err().starts_with("The Client ID and Secret look swapped"));
    }

    #[test]
    fn the_setup_save_sends_only_typed_values_to_its_route() {
        let b = Lifecycle::default();
        let typed = vec![("GOOGLE_CLIENT_ID".to_owned(), " cid ".to_owned()), ("GOOGLE_CLIENT_SECRET".to_owned(), "  ".to_owned())];
        host_api::save_setup(&b, "app.buwiz.google-workspace", &typed).unwrap();
        assert_eq!(*b.routes.lock().unwrap(), [("POST /v1/host/plugins/app.buwiz.google-workspace/setup".to_owned(), json!({"values": {"GOOGLE_CLIENT_ID": "cid"}}))]);
        let b = Lifecycle::default();
        assert!(host_api::save_setup(&b, "../x", &typed).is_err());
        assert!(host_api::save_setup(&b, "app.buwiz.gws", &[("a b".to_owned(), "v".to_owned())]).is_err());
        assert!(host_api::save_setup(&b, "app.buwiz.gws", &[("K".to_owned(), "".to_owned())]).is_err(), "nothing typed: nothing to send");
        assert!(b.routes().is_empty());
        host_api::clear_setup(&b, "app.buwiz.gws").unwrap();
        assert_eq!(*b.routes.lock().unwrap(), [("POST /v1/host/plugins/app.buwiz.gws/setup".to_owned(), json!({"clear": true}))]);
        assert!(host_api::clear_setup(&b, "../x").is_err());
    }

    #[test]
    fn each_account_button_sends_its_route_with_the_label_only() {
        use host_api::{AccountAction, account_action};
        let b = Lifecycle::default();
        account_action(&b, "app.buwiz.google-workspace", "google", "default", AccountAction::SignIn).unwrap();
        account_action(&b, "app.buwiz.google-workspace", "google", " work ", AccountAction::SignOut).unwrap();
        account_action(&b, "app.buwiz.google-workspace", "google", "work", AccountAction::Remove).unwrap();
        host_api::settle_notice(&b, "c1", "account_required", "app.buwiz.google-workspace", "declined").unwrap();
        assert_eq!(
            *b.routes.lock().unwrap(),
            [
                ("POST /v1/host/plugins/app.buwiz.google-workspace/accounts/google/signin".to_owned(), json!({"label": "default"})),
                ("POST /v1/host/plugins/app.buwiz.google-workspace/accounts/google/signout".to_owned(), json!({"label": "work"})),
                ("POST /v1/host/plugins/app.buwiz.google-workspace/accounts/google/remove".to_owned(), json!({"label": "work"})),
                (
                    "POST /v1/host/notices/settle".to_owned(),
                    json!({"chat": "c1", "kind": "account_required", "id": "app.buwiz.google-workspace", "state": "declined"})
                ),
            ]
        );
        // nothing reaches the broker for a bad plugin, account or label
        let b = Lifecycle::default();
        for (p, a, l) in [
            ("../x", "google", "default"),
            ("app.buwiz.gws", "Google", "default"),
            ("app.buwiz.gws", "g/../x", "default"),
            ("app.buwiz.gws", "google", "  "),
            ("app.buwiz.gws", "google", "work email"),
            ("app.buwiz.gws", "google", "-work"),
            ("app.buwiz.gws", "google", "a/b"),
        ] {
            assert!(account_action(&b, p, a, l, AccountAction::SignIn).is_err(), "{p} {a} {l:?}");
        }
        assert!(account_action(&b, "app.buwiz.gws", "google", &"x".repeat(61), AccountAction::SignIn).is_err());
        assert!(b.routes().is_empty());
        for ok in ["default", "work", "uriah@example.com", "a.b_c+d-e", "W2"] {
            assert!(host_api::account_label_ok(ok), "{ok}");
        }
        assert!(host_api::account_label_ok(&"x".repeat(60)) && !host_api::account_label_ok("é"));
        assert!(host_api::account_id_ok("google") && host_api::account_id_ok("g-2") && !host_api::account_id_ok("2g") && !host_api::account_id_ok(&"a".repeat(33)));
    }

    #[test]
    fn a_routine_threads_quiet_line_says_when_it_checked_and_what_it_found() {
        let l = Listed::of(&json!({"id": "app.buwiz.ebirforms", "routines": [{"id": "bir-payments", "name": "Payments",
            "every_s": 300, "enabled": true, "available": true, "thread": "t1", "home": "t9",
            "last_run": 1793194720000u64, "last_error": null, "last_items": 0}]}));
        let r = l.routines[0].clone();
        assert_eq!((r.home.as_str(), r.every_s, r.last_items), ("t9", 300.0, Some(0)));
        let at = |_: f64| "3:42 PM".to_owned();
        assert_eq!(r.quiet_line(at), ("Checks every 5 min · last checked 3:42 PM · nothing new".to_owned(), false));
        assert_eq!(Routine { last_items: Some(2), ..r.clone() }.quiet_line(at).0, "Checks every 5 min · last checked 3:42 PM · 2 new items");
        assert_eq!(Routine { last_items: None, ..r.clone() }.quiet_line(at).0, "Checks every 5 min · last checked 3:42 PM");
        assert_eq!(Routine { last_run: 0.0, ..r.clone() }.quiet_line(at).0, "Checks every 5 min · waiting for its first check");
        assert_eq!(
            Routine { last_error: "bir_payment_search: offline".into(), ..r.clone() }.quiet_line(at),
            ("Checks every 5 min · last check failed: bir_payment_search: offline".to_owned(), true)
        );
        assert_eq!(
            Routine { available: false, reason: "you have not approved what it reads".into(), ..r.clone() }.quiet_line(at),
            ("Can't check: you have not approved what it reads".to_owned(), true)
        );
        assert_eq!(Routine { enabled: false, ..r }.quiet_line(at).0, "Turned off");
    }
}
