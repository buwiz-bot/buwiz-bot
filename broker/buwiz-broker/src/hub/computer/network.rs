//! A computer's browser network (D1): its browsing rules, the tunnel switch, and this Mac's side of
//! the tunnel.
//!
//! **Per computer, not per Bot.** A box has one proxy and one tunnel client: every Bot at a computer
//! gets that computer's rules, and every answer here says so. A change with `all` goes to every
//! computer.
//!
//! **The rules** are the box's own (`box_egress::rules`): `mode` all (every site but the blocked) or
//! listed (only the allowed), an allow and a block list, block always winning, an entry covering the
//! name and every name under it. They're kept here (`computer_network`), staged beside the box's
//! secrets before every start so it starts with them, and put in force while it runs
//! (`PUT /v1/egress/rules`). A set that can't be read here is never sent: the box then has none, and
//! refuses every site. The exit is `tunnel` while the switch is on, else `direct`: the box's proxy
//! dials the site itself, by the same rules, so the lists mean the same either way.
//!
//! **The tunnel's client** runs here, one per computer, with the box's own client, and judges each
//! stream by the same rules again before it dials. It runs while the switch is on and the computer
//! is awake, and stops on sleep, retire and switch-off. With the switch on and no client attached
//! the box's browser reaches no site (the box answers 503), and the status says why.
//!
//! **What isn't covered**, said in every answer: the browser on the computer's screens is; commands
//! the agent runs aren't (the network wall still applies to them). Cloud and remote computers browse
//! from their own network: their settings are refused, not stored. A local computer made before the
//! proxy can't take any of this until a person makes it again, which nothing here does by itself.
//!
//! Nothing here logs a site, and no answer names one the browser visited: the box's status is
//! counts and outcomes. The lists themselves are the person's own settings, shown back to them.
//!
//! **The network rule** (allow / ask / never, OpenGrok's `egress_policy`) is not built in this
//! slice: `/network rule` is refused in a sentence that says so, and nothing is stored for it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buwiz_computer::box_egress::client::{ClientError, ClientState, TunnelClientConfig};
use buwiz_computer::box_egress::destination::DestinationPolicy;
use buwiz_computer::box_egress::rules::{entry, Exit, Mode as ListMode, Rules, RulesCell, MAX_ENTRIES};
use buwiz_computer::box_egress::Allowlist;
use buwiz_computer::{Machine, Power, Problem, ProviderKind};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{bot_name, db_problem, Computers, HubCtx};

pub(super) const LIMIT_BROWSER: &str = "These rules cover the browser on this computer's screens. Commands the agent runs aren't covered: the network wall still applies to them.";
const LIMIT_HTTP: &str = "Plain http:// pages don't load in this computer's browser; https:// pages do.";
const CLOUD: &str = "This computer browses from its provider's network: browsing rules and the tunnel apply to computers on this Mac.";
const REMOTE: &str = "This computer browses from its own network: browsing rules and the tunnel apply to computers on this Mac.";
const REMAKE: &str = "This computer was made before browsing rules, so its browser can't take them. To use them, make the computer again: its files are kept, and the programs running on it and its open browser are lost. Nothing was changed.";
const REMAKE_TUNNEL: &str = "This computer was made without the tunnel, so its browser can't go out through this Mac. To use it, make the computer again: its files are kept, and the programs running on it and its open browser are lost. Nothing was changed.";
const OLD_IMAGE: &str = "This computer's software has no browser proxy, so its rules aren't in force: update the computer's image, then make the computer again.";
const UNREAD: &str = "This computer's browsing rules couldn't be read, so none were sent: its browser reaches no site until they can be.";
const RULE_NOT_BUILT: &str = "The network rule (allow, ask or never) isn't built yet, so nothing was changed. The allow and block lists and the tunnel switch work today.";

/// What a person keeps for one computer. A computer with no row has the defaults: tunnel off,
/// every site, empty lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub tunnel: bool,
    pub mode: ListMode,
    pub allow: Vec<String>,
    pub block: Vec<String>,
    /// the set's number, sent with it so the box's answer says this set is in force
    pub version: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { tunnel: false, mode: ListMode::All, allow: Vec::new(), block: Vec::new(), version: 0 }
    }
}

fn mode_word(mode: ListMode) -> &'static str {
    match mode {
        ListMode::All => "all",
        ListMode::Listed => "listed",
    }
}

fn list_of(text: &str) -> Result<Vec<String>, Problem> {
    serde_json::from_str(text).map_err(|_| Problem::Failed("a saved list doesn't read".to_owned()))
}

impl Settings {
    /// The saved settings. A row that doesn't read is an error, never the defaults: the defaults
    /// reach every site.
    pub fn load(ctx: &HubCtx, id: &str) -> Result<Self, Problem> {
        let row: Option<(i64, String, String, String, i64)> = ctx
            .db
            .with(|c| c.query_row("SELECT tunnel, mode, allow, block, version FROM computer_network WHERE computer_id=?", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional())
            .map_err(db_problem)?;
        let Some((tunnel, mode, allow, block, version)) = row else { return Ok(Self::default()) };
        let mode = match mode.as_str() {
            "all" => ListMode::All,
            "listed" => ListMode::Listed,
            _ => return Err(Problem::Failed("a saved mode doesn't read".to_owned())),
        };
        let settings = Self { tunnel: tunnel != 0, mode, allow: list_of(&allow)?, block: list_of(&block)?, version: u64::try_from(version).unwrap_or(0) };
        // what's sent must be what the box takes: a list it would refuse is no list
        settings.rules(Exit::Direct)?;
        Ok(settings)
    }

    fn save(&self, ctx: &HubCtx, id: &str) -> Result<(), Problem> {
        let (allow, block) = (serde_json::to_string(&self.allow).unwrap_or_default(), serde_json::to_string(&self.block).unwrap_or_default());
        let at = super::policy::now(ctx);
        ctx.db
            .with(|c| c.execute("INSERT OR REPLACE INTO computer_network(computer_id, tunnel, mode, allow, block, version, at) VALUES(?,?,?,?,?,?,?)", rusqlite::params![id, i64::from(self.tunnel), mode_word(self.mode), allow, block, i64::try_from(self.version).unwrap_or(i64::MAX), at]))
            .map(|_| ())
            .map_err(db_problem)
    }

    fn exit(&self, machine: &Machine) -> Exit {
        if self.tunnel && tunnel_made(machine) {
            Exit::Tunnel
        } else {
            Exit::Direct
        }
    }

    /// These settings as the box's rules, by its own checks.
    fn rules(&self, exit: Exit) -> Result<Rules, Problem> {
        Rules::new(self.version.max(1), exit, self.mode, &self.allow, &self.block).map_err(|e| Problem::Failed(e.0))
    }

    /// The rules document the box takes.
    fn doc(&self, machine: &Machine) -> Result<Value, Problem> {
        serde_json::from_str(&self.rules(self.exit(machine))?.to_json()).map_err(|e| Problem::Failed(e.to_string()))
    }
}

/// Made with the proxy, and so able to take rules.
fn proxy_made(machine: &Machine) -> bool {
    machine.egress.is_some()
}

fn tunnel_made(machine: &Machine) -> bool {
    machine.egress.as_ref().is_some_and(|e| e.tunnel && e.bearer.is_some())
}

/// Why this computer can't take browsing settings, as the sentence a person reads; `None` when it can.
fn unsupported(machine: &Machine) -> Option<&'static str> {
    match machine.provider {
        ProviderKind::Remote => Some(REMOTE),
        ProviderKind::Docker if proxy_made(machine) => None,
        ProviderKind::Docker => Some(REMAKE),
        _ => Some(CLOUD),
    }
}

/// A site a person named, as the box's lists hold it: lower case, no trailing dot, an international
/// name in its `xn--` form. The refusal is a sentence that doesn't repeat what was typed.
pub fn site(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let ascii = if raw.is_ascii() {
        raw.to_owned()
    } else {
        match url::Host::parse(raw) {
            Ok(url::Host::Domain(d)) => d,
            _ => return Err("That isn't a site name. Name a site, like example.com.".to_owned()),
        }
    };
    let name = entry(&ascii).map_err(|why| format!("That isn't a site name: {why}."))?;
    if !name.contains('.') {
        return Err("That isn't a site name. Name a site, like example.com.".to_owned());
    }
    Ok(name)
}

/// One change a person asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Tunnel(bool),
    Allow(String),
    Unallow(String),
    Block(String),
    Unblock(String),
    Mode(ListMode),
    /// the network rule: refused, not built in this slice
    Rule,
}

impl Change {
    /// `{op, value}` as the app sends it. The error is the sentence to show.
    pub fn from_body(body: &Value) -> Result<Self, String> {
        let op = body.get("op").and_then(Value::as_str).unwrap_or_default();
        let value = body.get("value").and_then(Value::as_str).unwrap_or_default().trim();
        match op {
            "tunnel" => match value {
                "on" => Ok(Self::Tunnel(true)),
                "off" => Ok(Self::Tunnel(false)),
                _ => Err("Say on or off.".to_owned()),
            },
            "allow" => site(value).map(Self::Allow),
            "unallow" => site(value).map(Self::Unallow),
            "block" => site(value).map(Self::Block),
            "unblock" => site(value).map(Self::Unblock),
            "mode" => match value {
                "all" => Ok(Self::Mode(ListMode::All)),
                "listed" => Ok(Self::Mode(ListMode::Listed)),
                _ => Err("The mode is all (every site but the blocked ones) or listed (only the allowed ones).".to_owned()),
            },
            "rule" => Ok(Self::Rule),
            _ => Err("That isn't a network change.".to_owned()),
        }
    }

    /// Applied to `s`: the sentence saying what changed, or why nothing did.
    fn apply(&self, s: &mut Settings, machine: &Machine) -> Result<String, String> {
        let add = |list: &mut Vec<String>, site: &str, name: &str| -> Result<String, String> {
            if list.iter().any(|e| e == site) {
                return Ok(format!("{site} is already on the {name} list."));
            }
            if list.len() >= MAX_ENTRIES {
                return Err(format!("The {name} list is full ({MAX_ENTRIES} sites). Nothing was changed."));
            }
            list.push(site.to_owned());
            Ok(format!("{site} and every name under it are on the {name} list."))
        };
        let remove = |list: &mut Vec<String>, site: &str, name: &str| {
            let before = list.len();
            list.retain(|e| e != site);
            if list.len() == before {
                format!("{site} isn't on the {name} list. Nothing was changed.")
            } else {
                format!("{site} is off the {name} list.")
            }
        };
        match self {
            Self::Rule => Err(RULE_NOT_BUILT.to_owned()),
            Self::Tunnel(true) if !tunnel_made(machine) => Err(REMAKE_TUNNEL.to_owned()),
            Self::Tunnel(on) => {
                s.tunnel = *on;
                Ok(if *on { "The tunnel is on: this computer's browser goes out through this Mac.".to_owned() } else { "The tunnel is off: the computer dials sites itself, by the same rules.".to_owned() })
            }
            Self::Allow(site) => {
                let mut said = add(&mut s.allow, site, "allow")?;
                if s.block.iter().any(|b| site == b || site.ends_with(&format!(".{b}"))) {
                    said.push_str(" It's blocked too, and block wins: it still won't load.");
                } else if s.mode == ListMode::All {
                    said.push_str(" The mode is all, so every site but the blocked ones loads anyway; the allow list counts in listed mode.");
                }
                Ok(said)
            }
            Self::Unallow(site) => Ok(remove(&mut s.allow, site, "allow")),
            Self::Block(site) => add(&mut s.block, site, "block"),
            Self::Unblock(site) => Ok(remove(&mut s.block, site, "block")),
            Self::Mode(mode) => {
                s.mode = *mode;
                Ok(match mode {
                    ListMode::All => "Every site loads except the blocked ones.".to_owned(),
                    ListMode::Listed if s.allow.is_empty() => "Only allowed sites load, and none is allowed yet: the browser reaches no site until you allow one.".to_owned(),
                    ListMode::Listed => "Only allowed sites load, and never a blocked one.".to_owned(),
                })
            }
        }
    }
}

/// This Mac's side of a computer's tunnel: one client, while its switch is on and it's awake.
struct Running {
    url: String,
    rules: RulesCell,
    state: ClientState,
    /// how the client ended, once it has: its own typed outcome, never read from its words
    outcome: Arc<Mutex<Option<Result<(), ClientError>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// How often the sweep looks again at a computer whose client runs: a wake, a sleep, a switch and
/// a retire set it right as they happen, so this only catches what changed under them.
const LOOK_AGAIN: Duration = Duration::from_secs(300);

/// The tunnels' clients, and the turns changes to a computer's network take, by computer.
#[derive(Default)]
pub struct Tunnels {
    running: Mutex<HashMap<String, Running>>,
    /// one change to a computer's network at a time: load, save and push are one step
    turns: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// computers whose box didn't take their saved rules: tried again on the sweep while awake
    behind: Mutex<HashSet<String>>,
    /// when the sweep last looked at each running client
    looked: Mutex<HashMap<String, Instant>>,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a client is doing, in a word, from its state and its typed outcome.
fn client_word(finished: bool, outcome: Option<&Result<(), ClientError>>, attached: bool, erred: bool) -> &'static str {
    match (finished, outcome) {
        (true, Some(Err(ClientError::Unauthorized))) => "refused",
        (true, _) => "stopped",
        _ if attached => "attached",
        _ if erred => "retrying",
        _ => "attaching",
    }
}

impl Tunnels {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Running>> {
        locked(&self.running)
    }

    /// Computer `id`'s turn for a change to its network, waited for.
    async fn turn(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let turn = locked(&self.turns).entry(id.to_owned()).or_default().clone();
        turn.lock_owned().await
    }

    /// Stops computer `id`'s client, if one runs.
    pub fn stop(&self, id: &str) {
        self.lock().remove(id);
        locked(&self.looked).remove(id);
    }

    /// What the client is doing, in a word and a sentence, naming no site.
    pub fn status(&self, id: &str) -> Value {
        let map = self.lock();
        let Some(r) = map.get(id) else { return json!({"state": "off"}) };
        let snap = r.state.snapshot();
        let outcome = locked(&r.outcome).clone();
        let state = client_word(r.task.is_finished(), outcome.as_ref(), snap.attached, snap.last_error.is_some());
        json!({"state": state, "lastError": snap.last_error, "attaches": snap.attaches, "relayed": snap.opened, "refused": snap.refused})
    }

    fn running_at(&self, id: &str, url: &str) -> bool {
        self.lock().get(id).is_some_and(|r| r.url == url)
    }

    /// A client is running for `id` and the sweep looked at it lately: nothing to do on a tick.
    fn settled(&self, id: &str) -> bool {
        let live = self.lock().get(id).is_some_and(|r| !r.task.is_finished());
        live && locked(&self.looked).get(id).is_some_and(|at| at.elapsed() < LOOK_AGAIN)
    }

    fn update(&self, id: &str, rules: Rules) {
        if let Some(r) = self.lock().get(id) {
            r.rules.set(Some(rules));
        }
    }

    fn start(&self, id: &str, url: String, bearer: String, rules: Rules) {
        let (cell, state) = (RulesCell::new(Some(rules)), ClientState::default());
        // the shipped address policy: web ports, public addresses, never this Mac's network
        let cfg = TunnelClientConfig { url: url.clone(), bearer, allowlist: Allowlist::any(), destination: DestinationPolicy::default(), rules: cell.clone(), reconnect: true, state: state.clone() };
        let outcome = Arc::new(Mutex::new(None));
        let ended = outcome.clone();
        let task = tokio::spawn(async move {
            let got = buwiz_computer::box_egress::client::run(cfg).await;
            *locked(&ended) = Some(got);
        });
        self.lock().insert(id.to_owned(), Running { url, rules: cell, state, outcome, task });
        locked(&self.looked).insert(id.to_owned(), Instant::now());
    }
}

/// Whether the box's answer says these settings are the set in force.
fn in_force(egress: &Value, settings: &Settings) -> bool {
    egress.get("proxy_enabled") == Some(&json!(true)) && egress.pointer("/rules/version").and_then(Value::as_u64) == Some(settings.version.max(1))
}

impl Computers {
    /// Before a start: the rules the box reads first, from what's saved. Unreadable is none staged,
    /// and the box then reaches no site.
    pub(super) fn stage_network(&self, ctx: &HubCtx, id: &str, machine: &Machine) {
        if !proxy_made(machine) {
            return;
        }
        let doc = Settings::load(ctx, id).and_then(|s| s.doc(machine));
        match doc {
            Ok(doc) if buwiz_computer::docker::stage_rules(&machine.dir, doc.to_string().as_bytes()).is_ok() => {}
            _ => buwiz_computer::docker::unstage_rules(&machine.dir),
        }
    }

    /// After a wake: the saved rules in force (a resumed box kept what it had), and the tunnel's
    /// client as its switch says. Unreadable settings put "no site" in force.
    pub(super) async fn network_after_wake(&self, ctx: &HubCtx, id: &str) {
        let Ok(machine) = self.machine(ctx, id) else { return };
        if !proxy_made(&machine) {
            return;
        }
        {
            let _turn = self.tunnels.turn(id).await;
            let doc = Settings::load(ctx, id).and_then(|s| s.doc(&machine)).unwrap_or_else(|_| json!({"v": 1, "exit": "direct", "mode": "listed", "allow": [], "block": []}));
            let _ = self.push_rules(ctx, id, &machine, &doc).await;
        }
        self.reconcile_tunnel(ctx, id, false).await;
    }

    /// Puts `doc` in force on the awake computer `id`. One the box didn't take is logged, naming no
    /// site, and tried again on the sweep while the computer is awake ([`Computers::catch_up`]).
    async fn push_rules(&self, ctx: &HubCtx, id: &str, machine: &Machine, doc: &Value) -> Result<(), Problem> {
        let provider = self.provider_of(ctx, machine);
        match provider.set_egress_rules(machine, doc).await {
            Ok(_) => {
                locked(&self.tunnels.behind).remove(id);
                Ok(())
            }
            Err(p) => {
                eprintln!("buwiz-broker: computer {id} didn't take its browsing rules ({}): tried again while it's awake", p.code());
                locked(&self.tunnels.behind).insert(id.to_owned());
                Err(p)
            }
        }
    }

    /// The sweep: each awake computer whose box didn't take its saved rules gets them again.
    pub(super) async fn catch_up(&self, ctx: &HubCtx) {
        let behind: Vec<String> = locked(&self.tunnels.behind).iter().cloned().collect();
        for id in behind {
            let Ok(machine) = self.machine(ctx, &id) else {
                locked(&self.tunnels.behind).remove(&id);
                continue;
            };
            if self.now().sleeping.contains(&id) || !matches!(self.provider_of(ctx, &machine).power(&machine).await, Ok(Power::Awake)) {
                continue;
            }
            let _turn = self.tunnels.turn(&id).await;
            let doc = Settings::load(ctx, &id).and_then(|s| s.doc(&machine)).unwrap_or_else(|_| json!({"v": 1, "exit": "direct", "mode": "listed", "allow": [], "block": []}));
            let _ = self.push_rules(ctx, &id, &machine, &doc).await;
        }
    }

    /// Starts, keeps, updates or stops computer `id`'s tunnel client, by its switch and whether it's
    /// awake. `fresh` starts it again even if it's there, as after a person turned the switch on.
    pub(super) async fn reconcile_tunnel(&self, ctx: &HubCtx, id: &str, fresh: bool) {
        let Ok(machine) = self.machine(ctx, id) else {
            self.tunnels.stop(id);
            return;
        };
        let wanted = Settings::load(ctx, id).ok().filter(|s| s.tunnel && tunnel_made(&machine));
        let Some(settings) = wanted else {
            self.tunnels.stop(id);
            return;
        };
        let provider = self.provider_of(ctx, &machine);
        let awake = matches!(provider.look(&machine).await, Ok((Power::Awake, _))) && !self.now().sleeping.contains(id);
        let url = if awake { provider.tunnel_url(&machine).await.ok().flatten() } else { None };
        let (Some(url), Ok(rules), Some(bearer)) = (url, settings.rules(Exit::Tunnel), machine.egress.as_ref().and_then(|e| e.bearer.clone())) else {
            self.tunnels.stop(id);
            return;
        };
        if !fresh && self.tunnels.running_at(id, &url) {
            self.tunnels.update(id, rules);
            locked(&self.tunnels.looked).insert(id.to_owned(), Instant::now());
            return;
        }
        self.tunnels.stop(id);
        self.tunnels.start(id, url, bearer.expose().to_owned(), rules);
    }

    /// Every computer whose tunnel switch is on, set right: the sweep's tick and a start.
    pub(super) async fn reconcile_tunnels(&self, ctx: &HubCtx) {
        let on: Vec<String> = ctx
            .db
            .with(|c| {
                let mut q = c.prepare("SELECT computer_id FROM computer_network WHERE tunnel=1")?;
                let rows = q.query_map([], |r| r.get(0))?;
                rows.collect()
            })
            .unwrap_or_default();
        let running: Vec<String> = self.tunnels.lock().keys().cloned().collect();
        for id in on.iter().chain(running.iter().filter(|r| !on.contains(r))) {
            // a wake, a sleep, a switch and a retire set it right as they happen
            if !self.tunnels.settled(id) {
                self.reconcile_tunnel(ctx, id, false).await;
            }
        }
    }

    /// `bot`'s computer's network, for the pane and `/network`.
    pub async fn network_status(&self, ctx: &HubCtx, bot: &str) -> Value {
        let seat = match Self::seat(ctx, bot) {
            Ok(Some(seat)) => seat,
            Ok(None) => return json!({"ok": true, "state": "none", "message": "This Bot has no computer yet: its network settings come with its computer."}),
            Err(p) => return json!({"ok": false, "message": p.sentence()}),
        };
        self.network_of(ctx, &seat.computer).await
    }

    async fn network_of(&self, ctx: &HubCtx, id: &str) -> Value {
        let bots: Vec<Value> = Self::seats(ctx, id).unwrap_or_default().iter().map(|s| json!({"id": s.bot, "name": bot_name(ctx, &s.bot)})).collect();
        let names: Vec<String> = bots.iter().filter_map(|b| b["name"].as_str().map(str::to_owned)).collect();
        let mut out = json!({"ok": true, "state": "computer", "computer": id, "bots": bots, "shared": names.len() > 1});
        let machine = match self.machine(ctx, id) {
            Ok(m) => m,
            Err(p) => {
                out["ok"] = json!(false);
                out["message"] = json!(p.sentence());
                return out;
            }
        };
        out["where"] = json!(match machine.provider {
            ProviderKind::Docker => "local",
            ProviderKind::Remote => "remote",
            _ => "cloud",
        });
        if let Some(why) = unsupported(&machine) {
            out["supported"] = json!(false);
            out["why"] = json!(why);
            out["notes"] = json!([why]);
            return out;
        }
        out["supported"] = json!(true);
        out["tunnelMade"] = json!(tunnel_made(&machine));
        let mut notes: Vec<String> = Vec::new();
        notes.push(if names.len() > 1 { format!("These are this computer's rules: every Bot on it ({}) gets them.", names.join(", ")) } else { "These are this computer's rules: any Bot that joins it gets them too.".to_owned() });
        let settings = match Settings::load(ctx, id) {
            Ok(s) => s,
            Err(_) => {
                out["ok"] = json!(false);
                out["message"] = json!(UNREAD);
                return out;
            }
        };
        out["tunnel"] = json!(settings.tunnel);
        out["mode"] = json!(mode_word(settings.mode));
        out["allow"] = json!(settings.allow);
        out["block"] = json!(settings.block);
        let provider = self.provider_of(ctx, &machine);
        let awake = matches!(provider.power(&machine).await, Ok(Power::Awake));
        out["awake"] = json!(awake);
        let client = self.tunnels.status(id);
        if awake {
            match provider.egress(&machine).await {
                Ok(egress) if egress.get("proxy_enabled") != Some(&json!(true)) => {
                    out["inForce"] = json!(false);
                    notes.push(OLD_IMAGE.to_owned());
                }
                Ok(egress) => {
                    let force = in_force(&egress, &settings);
                    out["inForce"] = json!(force);
                    out["canBrowse"] = egress.get("can_browse").cloned().unwrap_or(Value::Null);
                    out["counts"] = egress.get("counts").cloned().unwrap_or(Value::Null);
                    if !force {
                        notes.push("The computer isn't using these rules yet: it didn't take them when they were saved. They're tried again while it's awake.".to_owned());
                    }
                }
                Err(p) => {
                    out["inForce"] = Value::Null;
                    notes.push(format!("What the computer is using couldn't be read: {}", p.sentence()));
                }
            }
        } else {
            notes.push("The computer is asleep: these rules are in force from the moment it wakes.".to_owned());
        }
        // the one sentence for a tunnel that isn't carrying the browser: the app shows it as it is
        if settings.tunnel && client["state"] != "attached" {
            notes.push(match client["state"].as_str() {
                Some("refused") => "The tunnel is on and the computer refused this Mac's key for it, so the browser on this computer can't reach any site. Turn the tunnel off and on again; if it stays refused, make the computer again.".to_owned(),
                _ if !awake => "The tunnel is on: this Mac's side attaches when the computer wakes, and until then its browser can't reach any site.".to_owned(),
                _ => "The tunnel is on and this Mac's side isn't attached yet, so the browser on this computer can't reach any site until it is.".to_owned(),
            });
        }
        if settings.mode == ListMode::Listed && settings.allow.is_empty() {
            notes.push("Only allowed sites load, and none is allowed: the browser reaches no site.".to_owned());
        }
        notes.push(LIMIT_BROWSER.to_owned());
        notes.push(LIMIT_HTTP.to_owned());
        out["client"] = client;
        out["notes"] = json!(notes);
        out
    }

    /// One change, to `bot`'s computer or, with `all`, every computer. Each says what happened to it.
    pub async fn network_change(&self, ctx: &HubCtx, bot: &str, change: &Change, all: bool) -> Value {
        let ids = if all {
            Self::ids(ctx)
        } else {
            match Self::seat(ctx, bot) {
                Ok(Some(seat)) => vec![seat.computer],
                Ok(None) => return json!({"ok": false, "message": "This Bot has no computer yet: its network settings come with its computer."}),
                Err(p) => return json!({"ok": false, "message": p.sentence()}),
            }
        };
        let mut results = Vec::new();
        for id in &ids {
            let (ok, message) = self.change_one(ctx, id, change).await;
            let names: Vec<String> = Self::seats(ctx, id).unwrap_or_default().iter().map(|s| bot_name(ctx, &s.bot)).collect();
            results.push(json!({"computer": id, "bots": names, "ok": ok, "message": message}));
        }
        let ok = results.iter().all(|r| r["ok"] == json!(true));
        let mut out = if all { json!({"ok": ok, "results": results}) } else { json!({"ok": ok, "message": results.first().map(|r| r["message"].clone()).unwrap_or_default()}) };
        if !all {
            out["network"] = self.network_status(ctx, bot).await;
        }
        out
    }

    async fn change_one(&self, ctx: &HubCtx, id: &str, change: &Change) -> (bool, String) {
        // load, save and push as one step: a second change waits, and builds on this one
        let turn = self.tunnels.turn(id).await;
        let machine = match self.machine(ctx, id) {
            Ok(m) => m,
            Err(p) => return (false, p.sentence()),
        };
        if let Some(why) = unsupported(&machine) {
            return (false, why.to_owned());
        }
        let Ok(mut settings) = Settings::load(ctx, id) else { return (false, UNREAD.to_owned()) };
        let said = match change.apply(&mut settings, &machine) {
            Ok(said) => said,
            Err(why) => return (false, why),
        };
        let now = u64::try_from(super::policy::now(ctx)).unwrap_or(0);
        settings.version = (settings.version + 1).max(now);
        if let Err(p) = settings.save(ctx, id) {
            return (false, format!("Nothing was changed: the setting couldn't be saved ({}).", p.sentence()));
        }
        // the next start reads these first
        self.stage_network(ctx, id, &machine);
        let provider = self.provider_of(ctx, &machine);
        let applied = match provider.power(&machine).await {
            Ok(Power::Awake) => match settings.doc(&machine) {
                Ok(doc) => match self.push_rules(ctx, id, &machine, &doc).await {
                    Ok(()) => "It's in force now.".to_owned(),
                    Err(p) => format!("It's saved, but the computer didn't take it ({}), so it isn't in force yet: it's tried again while the computer is awake.", p.sentence()),
                },
                Err(p) => p.sentence(),
            },
            _ => "The computer is asleep: it's in force from the moment it wakes.".to_owned(),
        };
        drop(turn);
        self.reconcile_tunnel(ctx, id, matches!(change, Change::Tunnel(true))).await;
        (true, format!("{said} {applied}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_is_a_name_the_box_takes_and_nothing_else() {
        assert_eq!(site(" Facebook.COM. "), Ok("facebook.com".to_owned()));
        assert_eq!(site("*.example.com"), Ok("example.com".to_owned()));
        assert_eq!(site("bücher.de"), Ok("xn--bcher-kva.de".to_owned()));
        for bad in ["*", "", "  ", "localhost", "10.0.0.1", "https://example.com/x", "example.com:443", "exa mple.com", "*.*"] {
            let err = site(bad).unwrap_err();
            // a lone `*` is named by the box's own fixed sentence; nothing longer is ever repeated
            assert!(bad.trim().len() <= 1 || !err.contains(bad.trim()), "a refusal doesn't echo what was typed: {err}");
        }
    }

    #[test]
    fn a_clients_state_comes_from_its_typed_outcome_never_its_words() {
        assert_eq!(client_word(true, Some(&Err(ClientError::Unauthorized)), false, true), "refused");
        // words that say 401 in another error are not a refusal
        assert_eq!(client_word(true, Some(&Err(ClientError::Connect("HTTP 401-ish".into()))), false, true), "stopped");
        assert_eq!(client_word(false, None, true, false), "attached");
        assert_eq!(client_word(false, None, false, true), "retrying");
        assert_eq!(client_word(false, None, false, false), "attaching");
    }

    #[test]
    fn changes_read_from_the_app_and_unknown_words_are_refused() {
        assert_eq!(Change::from_body(&json!({"op": "tunnel", "value": "on"})), Ok(Change::Tunnel(true)));
        assert_eq!(Change::from_body(&json!({"op": "mode", "value": "listed"})), Ok(Change::Mode(ListMode::Listed)));
        assert_eq!(Change::from_body(&json!({"op": "block", "value": "www.facebook.com"})), Ok(Change::Block("www.facebook.com".into())));
        assert!(Change::from_body(&json!({"op": "allow", "value": "*"})).is_err());
        assert!(Change::from_body(&json!({"op": "tunnel", "value": "maybe"})).is_err());
        assert!(Change::from_body(&json!({"op": "explode"})).is_err());
    }
}
