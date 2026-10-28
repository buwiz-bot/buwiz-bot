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
//!   uninstalls (the plugin's `rox.uninstall`, its key, its record), then
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
            tools: list(v, "tools")
                .iter()
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
        }
    }

    /// The state as the list says it.
    pub fn state_label(&self) -> String {
        match self.state.as_str() {
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

    /// Whether the list offers Retry and Open logs (14.4, 14.5).
    pub fn failed(&self) -> bool {
        self.state == "failed"
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
/// environment, then the command and its arguments (no quoting).
pub fn command_line(line: &str) -> Option<(Option<Value>, String, Vec<String>)> {
    let mut env = serde_json::Map::new();
    let mut words = line.split_whitespace().peekable();
    while let Some(w) = words.peek() {
        let Some((k, v)) = w.split_once('=') else { break };
        let name_ok = !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !name_ok || k.starts_with(|c: char| c.is_ascii_digit()) {
            break;
        }
        env.insert(k.to_owned(), json!(v));
        words.next();
    }
    let command = words.next()?.to_owned();
    Some(((!env.is_empty()).then(|| Value::Object(env)), command, words.map(str::to_owned).collect()))
}

/// `POST /v1/host/plugins/inspect` for `command args` with `env`: the
/// review (nothing is installed).
pub fn inspect(b: &dyn Broker, command: &str, args: &[String], env: Option<Value>) -> Result<Review, RoxError> {
    let v = host_api::inspect(b, command, args, env.clone())?;
    Ok(Review { env, ..Review::of(&v, command, args) })
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

/// Disable or enable (13.4, 13.5); a failed plugin's Retry is an enable.
pub fn set_enabled(b: &dyn Broker, plugin: &str, enabled: bool) -> Result<(), RoxError> {
    let action = if enabled { PluginAction::Enable } else { PluginAction::Disable };
    host_api::plugin_action(b, plugin, action)?;
    if !enabled {
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
    let token = crate::client::host_token_path(std::env::var("ROX_HOST_TOKEN_FILE").ok().as_deref(), &home);
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
            "manifest": {"id": "dev.goldcoders.fixture", "version": "1.1.0"},
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
            "warnings": ["/report is also a command of dev.goldcoders.acc; it will need /fx:report"],
        })
    }

    impl Broker for Lifecycle {
        fn request(&self, method: &str, route: &str, body: Option<Value>, _: Duration) -> Result<Value, RoxError> {
            self.routes.lock().unwrap().push((format!("{method} {route}"), body.clone().unwrap_or(Value::Null)));
            match route {
                "/v1/host/plugins/inspect" => Ok(inspect_answer(Value::Null)),
                "/v1/host/plugins/install" => match &self.refuse {
                    Some(e) => Err(e.clone()),
                    None => {
                        *self.installed.lock().unwrap() = Some("dev.goldcoders.fixture".into());
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

    #[test]
    fn lcy_06_uninstall_removes_attachments_pins_storage_and_the_link_is_not_installed() {
        let base = std::env::temp_dir().join(format!("rox-uninstall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (records_dir, plugins_dir) = (base.join("rox"), base.join("rox/plugins"));
        // its own id: ending its sessions must not touch other tests' opens
        let id = "dev.goldcoders.uninstallfx";
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
        assert_eq!(links::route(&format!("rox://app/{id}/board"), &[]), links::Route::NotInstalled(id.into()));
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
    fn a_command_line_takes_its_environment_first() {
        let (env, cmd, args) = command_line("HOME=/tmp/h BIR_X=1 /bin/bir mcp --flag").unwrap();
        assert_eq!(env, Some(json!({"HOME": "/tmp/h", "BIR_X": "1"})));
        assert_eq!((cmd.as_str(), args), ("/bin/bir", vec!["mcp".to_owned(), "--flag".to_owned()]));
        let (env, cmd, args) = command_line("node a=b").unwrap();
        assert_eq!((env, cmd.as_str(), args), (None, "node", vec!["a=b".to_owned()]));
        assert!(command_line("  ").is_none());
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
            "id": "dev.goldcoders.bir", "short": "bir", "title": "BIR", "version": "1.0.0", "publisher": {"name": "Goldcoders", "key_id": "gc-2026-01"},
            "state": "failed", "code": "plugin_bad_signature", "message": "bad", "enabled": true, "generation": 4,
            "tools": [{"name": "bir_resolve", "description": "Resolve", "disabled": false}, {"name": "bir_file", "disabled": true}],
            "routines": [{"id": "bir-payments", "name": "Payments", "approved": false, "enabled": false, "available": false,
                "reason": "dev.goldcoders.gws is not installed", "reads_status": [{"plugin": "dev.goldcoders.gws", "tools": ["gws_search_mail"]}],
                "last_run": 1793194720000u64, "last_error": "bir_payment_search: offline"}],
            "apps": [{"id": "forms", "title": "BIR Forms", "placement": ["full", "inline"]}],
        }));
        assert_eq!((l.publisher.as_str(), l.state_label().as_str(), l.failed()), ("Goldcoders", "Failed", true));
        assert_eq!(l.tools.iter().filter(|t| t.disabled).count(), 1);
        assert_eq!(l.routines[0].reads, ["dev.goldcoders.gws: gws_search_mail"]);
        assert_eq!((l.routines[0].last_run, l.routines[0].last_error.as_str()), (1793194720000.0, "bir_payment_search: offline"));
        assert_eq!((l.routines[0].home.as_str(), l.routines[0].last_items), ("", None));
        assert_eq!(l.apps, ["BIR Forms · full screen, inline"]);
        let p = log_file("dev.goldcoders.bir").unwrap();
        assert!(p.ends_with("v1/logs/dev.goldcoders.bir.log"), "{}", p.display());
        assert_eq!(log_file("../x"), None);
    }

    #[test]
    fn a_routine_threads_quiet_line_says_when_it_checked_and_what_it_found() {
        let l = Listed::of(&json!({"id": "dev.goldcoders.bir", "routines": [{"id": "bir-payments", "name": "Payments",
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
