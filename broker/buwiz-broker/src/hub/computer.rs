//! The Bots' computers: the records, wake and sleep, and the routes the Computer tab uses.
//!
//! A **computer** is its own record (`computers`), and one or more Bots sit at it: a **seat**
//! (`computer_seats`) ties a Bot to a computer and holds that Bot's screen, its own display, input
//! and folder. A Bot has at most one seat. Sleep is the computer's: it sleeps when no Bot at it has
//! used it for a while and nobody watches a screen, and waking it from any Bot wakes it for all.
//!
//! Only a person makes, attaches and wakes one: these routes answer the app's host token alone,
//! and the shell can't reach the port. The model acts through `tools/computer.rs`, on the screen
//! of the seat of its own Bot, found here from the Bot. It has no way to name another. A screen
//! isn't a security boundary, though: see `tools/computer.rs`.
//!
//! Secrets stay in `computers/<id>/machine.json`, a 0600 file in the data dir, which the shell's
//! sandbox closes. They aren't in app.db, and no route answers with them: the app gets a one-time
//! ticket ([`buwiz_computer::Gate`]). A person's own SSH key isn't one of them: it's in the keystore,
//! the Keychain for the owner's data dir, and goes with the computer.
//!
//! Files outlive everything but a person's delete: a box or holder gone missing is rebuilt on the
//! same volumes when it next wakes, and a lost record is rebuilt from what's left. A computer is
//! only ever removed for a person who deleted its last Bot, or just made and empty.
//!
//! A person and an agent share a screen by these rules, all held in [`Now`] and changed only under
//! its lock, so a click, a sweep and a run see the same thing:
//!
//! - The first run to touch a screen keeps it until it ends ([`Computers::hold`]). A second is
//!   refused, not queued.
//! - A person outranks every run. While they drive, the agent's calls wait ([`Computers::person_on`]);
//!   a person is never made to wait for the agent.
//! - A run can hand the screen to a person until a time limit ([`Computers::hand_over`]). Sleeping
//!   the computer or deleting the Bot ends that too.
//! - Nothing but a person's click sleeps a computer a run or a hand-over has.
//!
//! A remote computer ([`buwiz_computer::RemoteProvider`]) is the same record with a connection in
//! it, and only a person adds one, with a yes to [`CONSENT`]: the wall that makes a computer safe to
//! leave on isn't there, so that's theirs to say, and a Bot at one starts at Ask until a person says
//! otherwise ([`ask_first`]). Waking is connecting and sleeping is hanging up.
//! Removing or retiring one drops the connection and this record: nothing at the far end is told
//! anything. Its record lost, it's never swapped for a local one: a person removes it and adds it
//! again.
//!
//! What C6 hangs here: a seat's `role` (`member` today; a Bot lent another's computer would be
//! a `guest`, with what it may do decided by that side's switches) and a screen's `folder`, the
//! per-agent place artifacts are collected from.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::helper::keystore::Keystore;
use buwiz_computer::remote::{Asked, Probed};
use buwiz_computer::{provider_for, Gate, Machine, Mode, Power, Problem, Provider, ProviderKind, Reach, RemoteProvider, Screen, Secret, Sleep, Spec, Woke};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::ctx::HubCtx;
use super::tools::computer::KEY as COMPUTER_KEY;
use super::policy;
use super::routes::host;
use super::types::{self, HubRequest, HubResponse, Pattern::Exact as X, Pattern::Segments as S, Verb};
use super::Registry;

/// The own screens a box runs (`box-screen`'s `BOX_MAX_OWN_SCREENS`).
pub const MAX_SEATS: i64 = 4;
const IDLE_ENV: &str = "BUWIZ_COMPUTER_IDLE_SECS";
const DEEP_ENV: &str = "BUWIZ_COMPUTER_DEEP_SECS";
const QUIET_ENV: &str = "BUWIZ_COMPUTER_QUIET_SECS";
/// Idle this long, it sleeps. Asleep this long, it lets go of its memory.
const IDLE: Duration = Duration::from_secs(15 * 60);
const DEEP: Duration = Duration::from_secs(2 * 60 * 60);
/// A person with a drive viewer open and no input for this long has gone. Until then they're there,
/// between clicks: the agent mustn't land where they're about to press.
const QUIET: Duration = Duration::from_secs(60);
/// How long an agent's call waits for a person before it gives up, undone.
const PATIENCE: Duration = Duration::from_secs(600);
/// The longest a recording runs: voxl's `--max-record-seconds`, which the image leaves as it is.
/// Past this it ended by itself, and there's nothing to stop.
const RECORDING_LIFE: Duration = Duration::from_secs(300);
/// How long a person's ticket waits on a recording that's starting or stopping: as long as a stop
/// can take (`guest.rs`), and a little.
const RECORDING_PATIENCE: Duration = Duration::from_secs(130);
/// How often a waiting call looks again.
const PERSON_POLL: Duration = Duration::from_millis(100);
/// How often a wake looks for a sleep under way to finish.
const SLEEP_POLL: Duration = Duration::from_millis(20);
/// How long opencodex's word on which models see images holds: every model call asks.
const SIGHT_TTL: Duration = Duration::from_secs(60);

/// What a person says yes to when they add a remote computer. The form shows it, word for word.
pub const CONSENT: &str = "The AI will act on this machine with every right this login has. It can run commands, read and change files and use the screen there, and that includes deleting things. Nothing walls it off. The Bot's switches still apply, and with its computer switch at On it acts without asking each time. Sleep only disconnects, and Remove only forgets the connection: neither touches the machine. A copy of the SSH key is kept in this Mac's Keychain until you remove the computer.";
/// Who a switch's Ask is from, when it came with a remote computer and no person chose it.
const REMOTE_ACTOR: &str = "remote-computer";
const UNREAD: &str = "This computer's record couldn't be read. Try again in a moment.";
const LOST_REMOTE: &str = "This remote computer's saved connection is lost. Nothing happened to the machine: remove it and add it again.";

/// A Bot's place at a computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seat {
    pub bot: String,
    pub computer: String,
    pub role: String,
    /// `None` until the computer has woken with this Bot at it
    pub screen: Option<Screen>,
}

/// What the broker knows that isn't saved: the computers on their way up, and why one failed.
#[derive(Default)]
struct Now {
    waking: HashSet<String>,
    /// by computer id, or by Bot before it has one
    problems: HashMap<String, Problem>,
    wake_ms: HashMap<String, u64>,
    /// the computers going to sleep right now. A wake waits for it, and nothing new starts on one
    sleeping: HashSet<String>,
    /// by Bot: the run that has its screen, as (its [`Hold`]'s number, the computer)
    holds: HashMap<String, (u64, String)>,
    /// by Bot: the person who has its screen
    handovers: HashMap<String, Handover>,
    /// by Bot: the recording its model has, one at most
    recordings: HashMap<String, Recording>,
    /// by Bot: how many drive tickets are being made, from before their recording check to the
    /// session being open. No recording starts meanwhile.
    coming: HashMap<String, usize>,
    numbers: u64,
}

impl Now {
    fn number(&mut self) -> u64 {
        self.numbers += 1;
        self.numbers
    }

    /// Ends `bot`'s hand-over for `why`, if it has one.
    fn end_handover(&mut self, bot: &str, why: Ended) -> bool {
        let found = self.handovers.remove(bot);
        let ended = found.is_some();
        if let Some(h) = found {
            let _ = h.done.send(why);
        }
        ended
    }

    /// Whether a run or a person has a screen on `computer`. From what's here, not the seats: this
    /// has no read to fail.
    fn in_use(&self, computer: &str) -> bool {
        self.holds.values().any(|(_, c)| c == computer) || self.handovers.values().any(|h| h.computer == computer)
    }
}

/// What the model recorded on a screen.
#[derive(Clone)]
struct Recording {
    computer: String,
    screen: Screen,
    /// `None` while its start is on the way
    id: Option<u64>,
    /// a stop is on the way
    stopping: bool,
    began: Instant,
}

/// A screen a person has been given.
struct Handover {
    number: u64,
    computer: String,
    reason: String,
    until: Instant,
    done: tokio::sync::oneshot::Sender<Ended>,
}

/// How a hand-over ended, short of its time running out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// a person pressed Hand back
    HandedBack,
    /// a person slept the computer
    Asleep,
    /// a person took the Bot off it or deleted it
    Gone,
}

/// A run's claim on its Bot's screen, kept in the run: it ends with the run, however that ends.
pub struct Hold {
    computers: Arc<Computers>,
    bot: String,
    number: u64,
}

impl Hold {
    pub fn is_for(&self, bot: &str) -> bool {
        self.bot == bot
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut now = self.computers.now();
        if now.holds.get(&self.bot).is_some_and(|(n, _)| *n == self.number) {
            now.holds.remove(&self.bot);
        }
    }
}

/// A hand-over under way. Dropped with the call that waits on it, so a run that ends leaves no
/// person holding a screen that nobody will take back.
pub struct Handing {
    computers: Arc<Computers>,
    bot: String,
    number: u64,
    limit: Duration,
    ended: tokio::sync::oneshot::Receiver<Ended>,
}

impl Handing {
    /// Waits for it to end: `None` when the time ran out first. A Hand back that lands on the limit
    /// is decided by whichever takes the record out first.
    pub async fn ended(&mut self) -> Option<Ended> {
        match tokio::time::timeout(self.limit, &mut self.ended).await {
            Ok(ended) => ended.ok(),
            Err(_) if self.take_back() => None,
            Err(_) => (&mut self.ended).await.ok(),
        }
    }

    /// Whether this call was the one to end it; its drive session goes with it.
    fn take_back(&self) -> bool {
        let mine = {
            let mut now = self.computers.now();
            let mine = now.handovers.get(&self.bot).is_some_and(|h| h.number == self.number);
            if mine {
                now.handovers.remove(&self.bot);
            }
            mine
        };
        if mine {
            self.computers.close_viewer(&self.bot);
        }
        mine
    }
}

impl Drop for Handing {
    fn drop(&mut self) {
        self.take_back();
    }
}

/// The hub's computers, in `ctx.ext`.
pub struct Computers {
    provider: Arc<dyn Provider>,
    /// one for the broker: it holds the connections
    remote: Arc<RemoteProvider>,
    gate: tokio::sync::OnceCell<Option<Gate>>,
    now: Mutex<Now>,
    machines: Mutex<HashMap<String, Machine>>,
    idle: Duration,
    deep: Duration,
    quiet: Duration,
    patience: Duration,
    /// one at a time: making a computer and changing who sits where. Two first clicks mustn't
    /// make two computers. Global rather than per Bot, because a join changes two Bots' view of
    /// one computer; it is held for a `docker create` at most, and a wake runs outside it.
    seating: tokio::sync::Mutex<()>,
    /// which models see images, as opencodex says, and when it said
    sight: Mutex<Option<(Instant, HashMap<String, bool>)>>,
}

/// What a tool acts with: the computer, and the screen of the Bot it was found for.
pub struct Live {
    pub provider: Arc<dyn Provider>,
    pub machine: Machine,
    pub computer: String,
    pub bot: String,
    pub screen: Screen,
}

fn secs(name: &str, default: Duration) -> Duration {
    std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()).map_or(default, Duration::from_secs)
}

fn db_problem(e: String) -> Problem {
    Problem::Failed(e)
}

impl Computers {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self { provider, remote: Arc::new(RemoteProvider::new()), gate: tokio::sync::OnceCell::new(), now: Mutex::default(), machines: Mutex::default(), seating: tokio::sync::Mutex::new(()), idle: secs(IDLE_ENV, IDLE), deep: secs(DEEP_ENV, DEEP), quiet: secs(QUIET_ENV, QUIET), patience: PATIENCE, sight: Mutex::default() }
    }

    /// With this remote provider (tests).
    #[cfg(test)]
    pub fn with_remote(mut self, remote: RemoteProvider) -> Self {
        self.remote = Arc::new(remote);
        self
    }

    pub fn of(ctx: &HubCtx) -> Option<Arc<Self>> {
        ctx.ext.get::<Arc<Self>>().cloned()
    }

    fn now(&self) -> std::sync::MutexGuard<'_, Now> {
        self.now.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The provider of a saved computer: a kind this build can't run answers with its sentence.
    fn provider_of(&self, machine: &Machine) -> Arc<dyn Provider> {
        if machine.provider == ProviderKind::Remote {
            self.remote.clone()
        } else if machine.provider == self.provider.kind() {
            self.provider.clone()
        } else {
            provider_for(machine.provider)
        }
    }

    async fn gate(&self) -> Option<&Gate> {
        self.gate.get_or_init(|| async { Gate::start().await.ok() }).await.as_ref()
    }

    /// The gate, if a viewer was ever opened: with none, nobody can be on a screen.
    fn open_gate(&self) -> Option<&Gate> {
        self.gate.get().and_then(Option::as_ref)
    }

    fn close_viewer(&self, bot: &str) {
        if let Some(gate) = self.open_gate() {
            gate.close(bot);
        }
    }

    // ---- the records ----

    /// Where `bot` sits. A failed read is an error: "not seated" is what lets a Bot go with its
    /// seat left behind, or join a second computer.
    pub fn seat(ctx: &HubCtx, bot: &str) -> Result<Option<Seat>, Problem> {
        ctx.db.with(|c| c.query_row("SELECT bot_id, computer_id, role, screen FROM computer_seats WHERE bot_id=?", [bot], seat_row).optional()).map_err(db_problem)
    }

    /// Who sits at `computer`. A failed read is an error, never an empty list: "nobody else is
    /// here" is what removes a computer.
    pub fn seats(ctx: &HubCtx, computer: &str) -> Result<Vec<Seat>, Problem> {
        ctx.db
            .with(|c| {
                let mut q = c.prepare("SELECT bot_id, computer_id, role, screen FROM computer_seats WHERE computer_id=? ORDER BY attached_at, bot_id")?;
                let rows = q.query_map([computer], seat_row)?;
                rows.collect()
            })
            .map_err(db_problem)
    }

    fn try_ids_where(ctx: &HubCtx, clause: &str) -> Result<Vec<String>, String> {
        ctx.db.with(|c| {
            let mut q = c.prepare(&format!("SELECT id FROM computers WHERE {clause} ORDER BY created_at, id"))?;
            let rows = q.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// For going through them: a failed read is nothing to do this time.
    fn ids_where(ctx: &HubCtx, clause: &str) -> Vec<String> {
        Self::try_ids_where(ctx, clause).unwrap_or_default()
    }

    fn ids(ctx: &HubCtx) -> Vec<String> {
        Self::ids_where(ctx, "gone_at IS NULL")
    }

    /// Whether `id` is a computer this broker has: never a path anyone sent.
    fn known(ctx: &HubCtx, id: &str) -> bool {
        valid_id(id) && Self::ids(ctx).iter().any(|i| i == id)
    }

    fn dir(ctx: &HubCtx, id: &str) -> PathBuf {
        ctx.data.join("computers").join(id)
    }

    /// Whole or not at all: the record goes to a new file, then takes the old one's place.
    fn save_machine(ctx: &HubCtx, id: &str, machine: &Machine) -> Result<(), Problem> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let failed = |e: std::io::Error| Problem::Failed(e.to_string());
        let (path, next) = (Self::dir(ctx, id).join("machine.json"), Self::dir(ctx, id).join("machine.json.new"));
        let text = serde_json::to_vec(machine).map_err(|e| Problem::Failed(e.to_string()))?;
        let _ = std::fs::remove_file(&next);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&next).map_err(failed)?;
        f.write_all(&text).map_err(failed)?;
        f.sync_all().map_err(failed)?;
        std::fs::rename(&next, &path).map_err(failed)
    }

    /// The saved record. Missing or unreadable is [`Problem::Gone`]: the computer can't be
    /// reached without it, and a click makes a new one.
    fn machine(&self, ctx: &HubCtx, id: &str) -> Result<Machine, Problem> {
        if !valid_id(id) {
            return Err(Problem::Gone);
        }
        if let Some(m) = self.machines.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            return Ok(m.clone());
        }
        let text = std::fs::read(Self::dir(ctx, id).join("machine.json")).map_err(|_| Problem::Gone)?;
        let mut m: Machine = serde_json::from_slice(&text).map_err(|_| Problem::Gone)?;
        // a key that can't be read now may be readable later: it isn't remembered as lost
        if matches!(m.reach, Reach::Ssh { .. }) {
            let key = keys(ctx).ok().and_then(|k| k.get_computer_key(id).ok().flatten()).map(Secret::new);
            let found = key.is_some();
            if let Some(link) = m.link.as_mut() {
                link.key = key;
            }
            if !found {
                return Ok(m);
            }
        }
        self.machines.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_owned(), m.clone());
        Ok(m)
    }

    fn set_screen(ctx: &HubCtx, bot: &str, screen: Option<&Screen>) {
        let text = screen.and_then(|s| serde_json::to_string(s).ok());
        let _ = ctx.db.with(|c| c.execute("UPDATE computer_seats SET screen=? WHERE bot_id=?", rusqlite::params![text, bot]));
    }

    fn add_seat(ctx: &HubCtx, bot: &str, computer: &str, role: &str) -> Result<(), Problem> {
        let at = policy::now(ctx);
        ctx.db.with(|c| c.execute("INSERT INTO computer_seats(bot_id, computer_id, role, attached_at) VALUES(?,?,?,?)", rusqlite::params![bot, computer, role, at])).map(|_| ()).map_err(db_problem)
    }

    /// The computer was used: it stays up.
    pub fn touch(ctx: &HubCtx, computer: &str) {
        let at = policy::now(ctx);
        let _ = ctx.db.with(|c| c.execute("UPDATE computers SET last_used=? WHERE id=?", rusqlite::params![at, computer]));
    }

    // ---- wake and sleep ----

    /// A new computer, asleep, with no Bot at it yet.
    async fn make(&self, ctx: &HubCtx, bot: &str) -> Result<String, Problem> {
        let id = format!("cmp_{}", buwiz_computer::random_hex(8)?);
        let dir = Self::dir(ctx, &id);
        std::fs::create_dir_all(&dir).map_err(|e| Problem::Failed(e.to_string()))?;
        let spec = Spec { id: id.clone(), dir: dir.clone() };
        let made = self.provider.create(&spec).await;
        let machine = match made {
            Ok(m) => m,
            Err(e) => {
                // past its checks it may have left a part behind, and with no record nothing
                // would find it: the sweep does, by its label
                if !matches!(e, Problem::ProviderDown(_) | Problem::ProviderMissing(_) | Problem::ImageMissing(_) | Problem::NotBuilt(_)) {
                    let at = policy::now(ctx);
                    let _ = ctx.db.with(|c| c.execute("INSERT INTO computers(id, provider, created_at, created_for, gone_at) VALUES(?,?,?,?,?)", rusqlite::params![id, self.provider.kind().as_str(), at, bot, at]));
                } else {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                return Err(e);
            }
        };
        // from here a container exists: no record means nothing would ever remove it
        if let Err(e) = Self::record(ctx, &id, &machine, bot) {
            let _ = self.provider.destroy(&machine).await;
            let _ = ctx.db.with(|c| c.execute("DELETE FROM computers WHERE id=?", [&id]));
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        self.machines.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), machine);
        Ok(id)
    }

    fn record(ctx: &HubCtx, id: &str, machine: &Machine, bot: &str) -> Result<(), Problem> {
        Self::save_machine(ctx, id, machine)?;
        let at = policy::now(ctx);
        let reach = serde_json::to_string(&machine.reach).ok();
        ctx.db.with(|c| c.execute("INSERT INTO computers(id, provider, created_at, created_for, last_used, reach) VALUES(?,?,?,?,?,?)", rusqlite::params![id, machine.provider.as_str(), at, bot, at, reach])).map(|_| ()).map_err(db_problem)
    }

    /// Where `id` was, as its row says.
    fn reach_of(ctx: &HubCtx, id: &str) -> Result<Option<Reach>, Problem> {
        let text: Option<Option<String>> = ctx.db.with(|c| c.query_row("SELECT reach FROM computers WHERE id=?", [id], |r| r.get(0)).optional()).map_err(db_problem)?;
        Ok(text.flatten().and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// Whether `id`'s row says it's remote. A failed read is an error: "not remote" is what brings
    /// the local provider's hands to it.
    fn is_remote(ctx: &HubCtx, id: &str) -> Result<bool, Problem> {
        let word: Option<String> = ctx.db.with(|c| c.query_row("SELECT provider FROM computers WHERE id=?", [id], |r| r.get(0)).optional()).map_err(db_problem)?;
        Ok(word.as_deref() == Some(ProviderKind::Remote.as_str()))
    }

    /// Whether `id` is marked for removal: nothing may bring it up again.
    fn retired(ctx: &HubCtx, id: &str) -> Result<bool, Problem> {
        Self::try_ids_where(ctx, "gone_at IS NOT NULL").map(|ids| ids.iter().any(|i| i == id)).map_err(db_problem)
    }

    /// A person clicked: makes the computer if `bot` has none, or if its record is lost (the Bots
    /// at it move to the new one), and starts waking it. Comes back at once; the status says how
    /// it goes.
    pub async fn click(self: &Arc<Self>, ctx: &Arc<HubCtx>, bot: &str) -> Result<(), Problem> {
        self.now().problems.remove(bot);
        let (id, recovered) = match self.computer_for_click(ctx, bot).await {
            Ok(found) => found,
            Err(e) => {
                self.now().problems.insert(bot.to_owned(), e.clone());
                return Err(e);
            }
        };
        if !self.now().waking.insert(id.clone()) {
            return Ok(());
        }
        let (me, ctx) = (self.clone(), ctx.clone());
        tokio::spawn(async move {
            // what's running still has the old secrets. A real stop, so it can finish its writes,
            // and here, with the seats' lock long released
            if recovered {
                if let Ok(machine) = me.machine(&ctx, &id) {
                    let _ = me.provider_of(&machine).sleep(&machine, Sleep::Deep).await;
                }
            }
            let _ = me.finish_wake(&ctx, &id).await;
        });
        Ok(())
    }

    /// The computer to wake, and whether it was recovered with new secrets.
    async fn computer_for_click(&self, ctx: &HubCtx, bot: &str) -> Result<(String, bool), Problem> {
        let seating = self.seating.lock().await;
        let Some(seat) = Self::seat(ctx, bot)? else {
            let id = self.make(ctx, bot).await?;
            if let Err(e) = Self::add_seat(ctx, bot, &id, "member") {
                // just made: nothing is on it. The provider isn't waited on under the lock
                self.mark_retired(ctx, &id);
                drop(seating);
                self.finish_retire(ctx, &id).await;
                return Err(e);
            }
            return Ok((id, false));
        };
        // a record that reads is all it takes: the wake rebuilds whatever went, on the same files
        if !matches!(self.machine(ctx, &seat.computer), Err(Problem::Gone)) {
            return Ok((seat.computer, false));
        }
        // nothing here can stand in for a machine a person connected
        if Self::is_remote(ctx, &seat.computer)? {
            return Err(Problem::Remote(LOST_REMOTE.to_owned()));
        }
        // its record is lost: what's left of the computer gets a new one, files and all
        let spec = Spec { id: seat.computer.clone(), dir: Self::dir(ctx, &seat.computer) };
        if let Some(machine) = self.provider.recover(&spec, Self::reach_of(ctx, &seat.computer)?.as_ref()).await? {
            std::fs::create_dir_all(&spec.dir).map_err(|e| Problem::Failed(e.to_string()))?;
            Self::save_machine(ctx, &seat.computer, &machine)?;
            self.machines.lock().unwrap_or_else(|e| e.into_inner()).insert(seat.computer.clone(), machine);
            self.now().problems.remove(&seat.computer);
            return Ok((seat.computer, true));
        }
        // nothing of it is left to find
        let id = self.make(ctx, bot).await?;
        if let Err(e) = ctx.db.with(|c| c.execute("UPDATE computer_seats SET computer_id=?, screen=NULL WHERE computer_id=?", [&id, &seat.computer])) {
            self.mark_retired(ctx, &id);
            drop(seating);
            self.finish_retire(ctx, &id).await;
            return Err(db_problem(e));
        }
        self.mark_retired(ctx, &seat.computer);
        Ok((id, false))
    }

    /// Up, with every Bot's screen. `id` is in `waking`.
    async fn finish_wake(&self, ctx: &HubCtx, id: &str) -> Result<(), Problem> {
        let began = Instant::now();
        let woke = self.wake_all(ctx, id).await;
        {
            let mut now = self.now();
            now.waking.remove(id);
            match &woke {
                Ok(()) => {
                    now.problems.remove(id);
                    now.wake_ms.insert(id.to_owned(), u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
                }
                Err(e) => {
                    now.problems.insert(id.to_owned(), e.clone());
                }
            }
        }
        if woke.is_ok() {
            Self::touch(ctx, id);
            let _ = ctx.db.with(|c| c.execute("UPDATE computers SET slept_at=NULL WHERE id=?", [id]));
            self.clear_empty_screens(ctx, id).await;
        }
        woke
    }

    /// A Bot that left while it slept has its screen and slot taken down: after the wake, which
    /// nobody should wait on for this.
    async fn clear_empty_screens(&self, ctx: &HubCtx, id: &str) {
        let Ok(machine) = self.machine(ctx, id) else { return };
        let provider = self.provider_of(&machine);
        let Ok((_, machine)) = provider.look(&machine).await else { return };
        // not knowing who sits here is no reason to take a screen down
        let Ok(seats) = Self::seats(ctx, id) else { return };
        for bot in provider.screens(&machine).await.unwrap_or_default() {
            if !seats.iter().any(|s| s.bot == bot) {
                let _ = provider.screen_down(&machine, &bot).await;
            }
        }
    }

    async fn wake_all(&self, ctx: &HubCtx, id: &str) -> Result<(), Problem> {
        // `id` is in `waking`, so no sleep starts now: a pause that lands after this wake's check
        // would leave it asleep under a person who just woke it
        while self.now().sleeping.contains(id) {
            tokio::time::sleep(SLEEP_POLL).await;
        }
        // `id` is in `waking` by now, and the sweep leaves a waking computer alone: whichever of
        // this check and a retire comes first, nothing is brought up after it's removed
        match Self::retired(ctx, id) {
            Ok(false) => {}
            Ok(true) => return Err(Problem::Gone),
            // refused all the same, and not called gone: it may be perfectly well
            Err(_) => return Err(Problem::Failed("whether it can wake couldn't be checked. Try again in a moment".to_owned())),
        }
        let machine = self.machine(ctx, id)?;
        let provider = self.provider_of(&machine);
        let woke = provider.wake(&machine).await?;
        // resumed, the screens it had are as they were: only a Bot that joined meanwhile needs one
        let seats = Self::seats(ctx, id)?;
        if woke == Woke::Started {
            // what recorded went with the viewers: a stop now would fail, and keep a person out
            let mut now = self.now();
            for seat in &seats {
                now.recordings.remove(&seat.bot);
            }
        }
        let waiting: Vec<_> = seats.into_iter().filter(|s| woke == Woke::Started || s.screen.is_none()).collect();
        if waiting.is_empty() {
            return Ok(());
        }
        let (_, machine) = provider.look(&machine).await?;
        for seat in waiting {
            let screen = provider.screen_up(&machine, &seat.bot).await?;
            Self::set_screen(ctx, &seat.bot, Some(&screen));
        }
        Ok(())
    }

    /// Wakes `id` and waits: a routine a person let use it, with nobody there to click.
    pub async fn wake_and_wait(&self, ctx: &HubCtx, id: &str) -> Result<(), Problem> {
        if !self.now().waking.insert(id.to_owned()) {
            return Err(Problem::Asleep);
        }
        self.finish_wake(ctx, id).await
    }

    /// A person put it to sleep: once it has, whatever a person was doing on it ends. A sleep that
    /// fails leaves them where they were.
    pub async fn sleep(&self, ctx: &HubCtx, id: &str, depth: Sleep) -> Result<(), Problem> {
        let marked = {
            let mut now = self.now();
            now.sleeping.insert(id.to_owned());
            Sleeping(self, id.to_owned())
        };
        self.put_to_sleep(ctx, id, depth, marked).await
    }

    /// The sweep's sleep: no, if anything needs it. That check and the mark that stops a run, a
    /// hand-over or a wake starting are one step. `Ok(false)` is a no.
    async fn sleep_if_unused(&self, ctx: &HubCtx, id: &str, depth: Sleep) -> Result<bool, Problem> {
        let marked = {
            let mut now = self.now();
            if now.in_use(id) || now.waking.contains(id) {
                return Ok(false);
            }
            now.sleeping.insert(id.to_owned());
            Sleeping(self, id.to_owned())
        };
        self.put_to_sleep(ctx, id, depth, marked).await.map(|()| true)
    }

    async fn put_to_sleep(&self, ctx: &HubCtx, id: &str, depth: Sleep, _marked: Sleeping<'_>) -> Result<(), Problem> {
        let machine = self.machine(ctx, id)?;
        let provider = self.provider_of(&machine);
        // nothing the model recorded is left to pick up where it stopped
        let recording: Vec<(String, Recording)> = self.now().recordings.iter().filter(|(_, r)| r.computer == id && r.id.is_some()).map(|(bot, r)| (bot.clone(), r.clone())).collect();
        for (bot, r) in recording {
            if let Some(number) = r.id {
                if provider.record_stop(&machine, &r.screen, number).await.is_ok() {
                    self.now().recordings.remove(&bot);
                }
            }
        }
        provider.sleep(&machine, depth).await?;
        if let Some(gate) = self.gate().await {
            gate.close_computer(&machine.viewer_password);
        }
        {
            let mut now = self.now();
            let handed: Vec<String> = now.handovers.iter().filter(|(_, h)| h.computer == id).map(|(bot, _)| bot.clone()).collect();
            for bot in handed {
                now.end_handover(&bot, Ended::Asleep);
            }
        }
        let at = if depth == Sleep::Light { Some(policy::now(ctx)) } else { None };
        ctx.db.with(|c| c.execute("UPDATE computers SET slept_at=? WHERE id=?", rusqlite::params![at, id])).map(|_| ()).map_err(db_problem)
    }

    /// One look at every computer: the ones nobody needs go to sleep, then deeper.
    pub async fn sleep_the_idle(&self, ctx: &HubCtx) {
        for id in Self::ids_where(ctx, "gone_at IS NOT NULL") {
            // a wake in flight could put back what this removes: the next look gets it
            if !self.now().waking.contains(&id) {
                self.finish_retire(ctx, &id).await;
            }
        }
        let at = policy::now(ctx);
        for id in Self::ids(ctx) {
            if self.now().waking.contains(&id) {
                continue;
            }
            let Ok(machine) = self.machine(ctx, &id) else { continue };
            let row: Option<(i64, Option<i64>)> = ctx.db.with(|c| c.query_row("SELECT last_used, slept_at FROM computers WHERE id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?))).optional()).ok().flatten();
            let Some((used, slept)) = row else { continue };
            let provider = self.provider_of(&machine);
            match provider.power(&machine).await {
                Ok(Power::Awake) if at - used >= millis(self.idle) && !self.watched(ctx, &id).await => {
                    let _ = self.sleep_if_unused(ctx, &id, Sleep::Light).await;
                }
                Ok(Power::Asleep) if slept.is_some_and(|s| at - s >= millis(self.deep)) => {
                    let _ = self.sleep_if_unused(ctx, &id, Sleep::Deep).await;
                }
                _ => {}
            }
        }
    }

    async fn watched(&self, ctx: &HubCtx, id: &str) -> bool {
        let Some(gate) = self.gate().await else { return false };
        // not knowing counts as watched: it stays up
        Self::seats(ctx, id).map_or(true, |seats| seats.iter().any(|s| gate.watched(&s.bot)))
    }

    // ---- seats ----

    /// A person put `bot` at `computer`, which already serves other Bots.
    pub async fn attach(&self, ctx: &HubCtx, bot: &str, computer: &str) -> Result<(), Problem> {
        let _seating = self.seating.lock().await;
        if Self::seat(ctx, bot)?.is_some() {
            return Err(Problem::Failed("this Bot already has a computer".to_owned()));
        }
        if !Self::known(ctx, computer) {
            return Err(Problem::Gone);
        }
        let machine = self.machine(ctx, computer)?;
        if Self::seats(ctx, computer)?.len() as i64 >= MAX_SEATS {
            return Err(Problem::Failed(format!("a computer has room for {MAX_SEATS} Bots")));
        }
        // the seat comes last: a Bot that can't use the computer isn't seated at it
        let provider = self.provider_of(&machine);
        let (power, machine) = provider.look(&machine).await?;
        let screen = if power == Power::Awake { Some(provider.screen_up(&machine, bot).await?) } else { None };
        // before the seat: there's no moment it's at a remote computer with the switch at On
        if machine.link.is_some() {
            ask_first(ctx, bot)?;
        }
        if let Err(e) = Self::add_seat(ctx, bot, computer, "member") {
            drop_ask(ctx, bot);
            if screen.is_some() {
                let _ = provider.screen_down(&machine, bot).await;
            }
            return Err(e);
        }
        if let Some(screen) = screen {
            Self::set_screen(ctx, bot, Some(&screen));
        }
        Ok(())
    }

    /// A person took `bot` off a computer others stay at. The last Bot can't leave this way: that
    /// would delete the computer's files, and only deleting the Bot says so and asks.
    pub async fn leave(&self, ctx: &HubCtx, bot: &str) -> Result<(), Problem> {
        self.take_off(ctx, bot, false).await
    }

    /// `bot` is off its computer because a person deleted it. The last Bot's going marks the
    /// computer for removal, and the sweep removes it and what it kept
    /// ([`Computers::sleep_the_idle`]), so that removal isn't waited on: with Docker off or hung a
    /// Bot still goes, and nobody else's click waits behind it. Taking a Bot off a shared computer
    /// does ask the provider to bring its screen down, outside the lock and best effort. Fails if
    /// the seats can't be read: nothing is removed on a guess.
    pub async fn detach(&self, ctx: &HubCtx, bot: &str) -> Result<(), Problem> {
        self.take_off(ctx, bot, true).await
    }

    /// The check that it's the last Bot and its going are one step, under the lock: two Bots
    /// leaving at once can't each see the other still there.
    async fn take_off(&self, ctx: &HubCtx, bot: &str, may_be_last: bool) -> Result<(), Problem> {
        let seat = {
            let _seating = self.seating.lock().await;
            let Some(seat) = Self::seat(ctx, bot)? else { return Ok(()) };
            let last = Self::seats(ctx, &seat.computer)?.len() <= 1;
            if last && !may_be_last {
                return Err(Problem::Failed("the last Bot at a computer can't leave it: its files would go".to_owned()));
            }
            ctx.db.with(|c| c.execute("DELETE FROM computer_seats WHERE bot_id=?", [bot])).map_err(db_problem)?;
            drop_ask(ctx, bot);
            if last {
                self.mark_retired(ctx, &seat.computer);
            }
            let mut now = self.now();
            now.end_handover(bot, Ended::Gone);
            now.recordings.remove(bot);
            drop(now);
            (!last).then_some(seat)
        };
        if let Some(gate) = self.gate().await {
            gate.close(bot);
        }
        // asleep, its screen comes down at the next wake (`clear_empty_screens`)
        if let Some(seat) = seat {
            if let Ok(machine) = self.machine(ctx, &seat.computer) {
                let provider = self.provider_of(&machine);
                if let Ok((Power::Awake, machine)) = provider.look(&machine).await {
                    let _ = provider.screen_down(&machine, bot).await;
                }
            }
        }
        Ok(())
    }

    /// `id` has no Bots left, and a person agreed to that: it goes as soon as the provider can.
    fn mark_retired(&self, ctx: &HubCtx, id: &str) {
        let at = policy::now(ctx);
        let _ = ctx.db.with(|c| c.execute("UPDATE computers SET gone_at=? WHERE id=?", rusqlite::params![at, id]));
        let _ = ctx.db.with(|c| c.execute("DELETE FROM computer_seats WHERE computer_id=?", [id]));
    }

    async fn finish_retire(&self, ctx: &HubCtx, id: &str) {
        // read while the row is there. Not knowing is a try next time: the row is what finds the key
        let Ok(remote) = Self::is_remote(ctx, id) else { return };
        let removed = match self.machine(ctx, id) {
            Ok(m) => self.provider_of(&m).destroy(&m).await,
            Err(_) => match (Self::is_remote(ctx, id), Self::reach_of(ctx, id)) {
                // its record is all there is here
                (Ok(true), _) => Ok(()),
                (Ok(false), Ok(reach)) => self.provider.sweep(id, reach.as_ref()).await,
                // the next look tries again, with the name that finds its files
                (Err(e), _) | (_, Err(e)) => Err(e),
            },
        };
        if matches!(removed, Ok(()) | Err(Problem::NotBuilt(_) | Problem::Gone)) {
            // the key first: with its row gone nothing would ever find it again
            if remote && !keys(ctx).is_ok_and(|k| k.remove_computer_key(id).is_ok()) {
                return;
            }
            let _ = ctx.db.with(|c| c.execute("DELETE FROM computers WHERE id=?", [id]));
            self.machines.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
            self.now().problems.remove(id);
            if valid_id(id) {
                let _ = std::fs::remove_dir_all(Self::dir(ctx, id));
            }
        }
    }

    // ---- remote computers ----

    /// A person's test of what they typed: who the far end says it is, or that it works.
    pub async fn probe_remote(&self, ctx: &HubCtx, asked: &Asked) -> Result<Probed, Problem> {
        let dir = Scratch::new(ctx)?;
        self.remote.probe(&asked.machine(&dir.0)?).await
    }

    /// A person added a remote computer for `bot`: tested again here, then seated at it, asleep. The
    /// far end is reached outside the seats' lock, so a slow machine holds nobody else's click.
    pub async fn add_remote(&self, ctx: &HubCtx, bot: &str, asked: &Asked) -> Result<String, Problem> {
        if Self::seat(ctx, bot)?.is_some() {
            return Err(Problem::Remote(HAS_ONE.to_owned()));
        }
        let id = format!("cmp_{}", buwiz_computer::random_hex(8)?);
        let dir = Self::dir(ctx, &id);
        std::fs::create_dir_all(&dir).map_err(|e| Problem::Failed(e.to_string()))?;
        let added = self.seat_remote(ctx, bot, &id, &dir, asked).await;
        if added.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        added.map(|()| id)
    }

    async fn seat_remote(&self, ctx: &HubCtx, bot: &str, id: &str, dir: &std::path::Path, asked: &Asked) -> Result<(), Problem> {
        let machine = asked.machine(dir)?;
        if let Probed::Trust { .. } = self.remote.probe(&machine).await? {
            return Err(Problem::Remote("A person has to trust this computer's key first: test it, check what it shows, and say yes.".to_owned()));
        }
        let _seating = self.seating.lock().await;
        if Self::seat(ctx, bot)?.is_some() {
            return Err(Problem::Remote(HAS_ONE.to_owned()));
        }
        let store = keys(ctx)?;
        if let Some(key) = machine.link.as_ref().and_then(|l| l.key.as_ref()) {
            store.put_computer_key(id, key.expose()).map_err(Problem::Remote)?;
        }
        let seated = Self::record(ctx, id, &machine, bot).and_then(|()| ask_first(ctx, bot)).and_then(|()| Self::add_seat(ctx, bot, id, "member"));
        if let Err(e) = seated {
            let _ = ctx.db.with(|c| c.execute("DELETE FROM computers WHERE id=?", [id]));
            drop_ask(ctx, bot);
            let _ = store.remove_computer_key(id);
            return Err(e);
        }
        self.machines.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_owned(), machine);
        Ok(())
    }

    /// A person removed `bot`'s remote computer: the connection and this record go, for every Bot
    /// at it. Nothing on that machine changes. Marked first, so a wake under way stops or is swept.
    pub async fn remove_remote(&self, ctx: &HubCtx, bot: &str) -> Result<(), Problem> {
        let seat = Self::seat(ctx, bot)?.ok_or(Problem::Gone)?;
        let id = seat.computer;
        if !Self::is_remote(ctx, &id)? {
            return Err(Problem::Remote("Only a remote computer is removed here.".to_owned()));
        }
        let bots: Vec<String> = Self::seats(ctx, &id)?.into_iter().map(|s| s.bot).collect();
        {
            let _seating = self.seating.lock().await;
            self.mark_retired(ctx, &id);
        }
        // what the model recorded stops, and a hand-over ends. A lost record has neither
        let _ = self.sleep(ctx, &id, Sleep::Deep).await;
        for bot in &bots {
            drop_ask(ctx, bot);
            let mut now = self.now();
            now.end_handover(bot, Ended::Gone);
            now.recordings.remove(bot);
            drop(now);
            self.close_viewer(bot);
        }
        if !self.now().waking.contains(&id) {
            self.finish_retire(ctx, &id).await;
        }
        Ok(())
    }

    /// Whether `bot` sits at a remote computer, and if so whether it has a screen. `None` for a
    /// local one. Not knowing counts as remote, with a screen: what that takes away is what the wall
    /// would have given, and a model that can't see gets nothing either.
    pub fn remote_at(&self, ctx: &HubCtx, bot: &str) -> Option<bool> {
        let seat = match Self::seat(ctx, bot) {
            Ok(seat) => seat?,
            Err(_) => return Some(true),
        };
        match self.machine(ctx, &seat.computer) {
            Ok(machine) => machine.link.is_some().then(|| machine.has_screen()),
            Err(_) => Self::is_remote(ctx, &seat.computer).unwrap_or(true).then_some(true),
        }
    }

    // ---- reading ----

    /// `bot`'s computer ready to act on, or why not. Only an awake computer is: the model doesn't
    /// wake one. Nothing gets past here on a computer with a recorded problem, or one whose
    /// provider's guard fails; that guard is how a box from before this broker's start gets its
    /// wall before anyone uses it.
    pub async fn live(&self, ctx: &HubCtx, bot: &str) -> Result<Live, Problem> {
        let seat = Self::seat(ctx, bot)?.ok_or(Problem::Asleep)?;
        {
            let now = self.now();
            if now.waking.contains(&seat.computer) || now.sleeping.contains(&seat.computer) {
                return Err(Problem::Asleep);
            }
            if let Some(p) = now.problems.get(&seat.computer) {
                return Err(p.clone());
            }
        }
        let machine = self.machine(ctx, &seat.computer)?;
        let provider = self.provider_of(&machine);
        // the one read of the computer for this request: what's below acts on it
        let (power, machine) = provider.look(&machine).await?;
        match (power, seat.screen) {
            (Power::Awake, Some(screen)) => {
                if let Err(p) = provider.guard(&machine).await {
                    self.now().problems.insert(seat.computer.clone(), p.clone());
                    return Err(p);
                }
                Ok(Live { provider, machine, computer: seat.computer, bot: bot.to_owned(), screen })
            }
            (Power::Absent, _) => Err(Problem::Gone),
            _ => Err(Problem::Asleep),
        }
    }

    // ---- a person and an agent ----

    /// The run with this call has `bot`'s screen until the [`Hold`] goes, or `Err`: another run
    /// has it, or the computer is on its way to sleep. The one step that decides who is first.
    pub fn hold(self: &Arc<Self>, bot: &str, computer: &str) -> Result<Hold, Problem> {
        let mut now = self.now();
        if now.sleeping.contains(computer) {
            return Err(Problem::Asleep);
        }
        if now.holds.contains_key(bot) {
            return Err(Problem::Held);
        }
        let number = now.number();
        now.holds.insert(bot.to_owned(), (number, computer.to_owned()));
        Ok(Hold { computers: self.clone(), bot: bot.to_owned(), number })
    }

    /// Whether a person has `bot`'s screen right now: it was handed to them, or they're driving it.
    pub fn person_on(&self, bot: &str) -> bool {
        self.now().handovers.contains_key(bot) || self.open_gate().is_some_and(|g| g.driving(bot, self.quiet))
    }

    /// Waits for the person to leave `bot`'s screen, and says whether it had to.
    /// [`Problem::PersonOn`] when they're still there after [`PATIENCE`].
    pub async fn person_gone(&self, bot: &str) -> Result<bool, Problem> {
        let began = Instant::now();
        let mut waited = false;
        while self.person_on(bot) {
            if began.elapsed() >= self.patience {
                return Err(Problem::PersonOn);
            }
            waited = true;
            tokio::time::sleep(PERSON_POLL).await;
        }
        Ok(waited)
    }

    /// What the model recorded stops before a person is on the screen, so that what they type
    /// isn't in a file the model can read. A start on its way is waited for: it stops itself, seeing
    /// a person ([`Computers::recording_started`]). A stop that fails keeps them off it, for no
    /// longer than the recording can last.
    pub async fn end_recording(&self, live: &Live) -> Result<Option<String>, Problem> {
        let waiting = Instant::now();
        loop {
            let id = {
                let mut now = self.now();
                let Some(r) = now.recordings.get_mut(&live.bot) else { return Ok(None) };
                match (r.id, r.stopping) {
                    (_, _) if r.began.elapsed() >= RECORDING_LIFE => {
                        now.recordings.remove(&live.bot);
                        return Ok(None);
                    }
                    (Some(id), false) => {
                        r.stopping = true;
                        Some(id)
                    }
                    _ => None,
                }
            };
            let Some(id) = id else {
                if waiting.elapsed() >= RECORDING_PATIENCE {
                    return Err(Problem::Failed("a recording on this screen wouldn't finish starting or stopping".to_owned()));
                }
                tokio::time::sleep(SLEEP_POLL).await;
                continue;
            };
            let stopped = live.provider.record_stop(&live.machine, &live.screen, id).await;
            self.recording_stopped(&live.bot, stopped.is_ok());
            return stopped.map(Some);
        }
    }

    /// The model wants to record. One at a time, so none is left running with nothing to stop it,
    /// and none while a person is on their way or can drive: `Err` is the model's answer.
    pub fn begin_recording(&self, live: &Live) -> Result<(), &'static str> {
        let mut now = self.now();
        if now.coming.contains_key(&live.bot) || now.handovers.contains_key(&live.bot) || self.drive_open(&live.bot) {
            return Err("A person has this screen open to drive, so it can't be recorded now. Try again once they've left.");
        }
        if now.recordings.get(&live.bot).is_some_and(|r| r.began.elapsed() < RECORDING_LIFE) {
            return Err("A recording of this screen is already running. Stop it first.");
        }
        now.recordings.insert(live.bot.clone(), Recording { computer: live.computer.clone(), screen: live.screen.clone(), id: None, stopping: false, began: Instant::now() });
        Ok(())
    }

    /// The recording [`Computers::begin_recording`] allowed has started as `id`. `false` when a
    /// person came meanwhile: the caller stops it now, and says how that went.
    pub fn recording_started(&self, bot: &str, id: u64) -> bool {
        let mut now = self.now();
        let person = now.coming.contains_key(bot) || now.handovers.contains_key(bot) || self.drive_open(bot);
        if let Some(r) = now.recordings.get_mut(bot) {
            r.id = Some(id);
            r.stopping = person;
        }
        !person
    }

    /// Its start failed: there's nothing to stop.
    pub fn recording_failed(&self, bot: &str) {
        self.now().recordings.remove(bot);
    }

    /// A stop came back. One that failed leaves the recording to be stopped again.
    pub fn recording_stopped(&self, bot: &str, stopped: bool) {
        let mut now = self.now();
        if stopped {
            now.recordings.remove(bot);
        } else if let Some(r) = now.recordings.get_mut(bot) {
            r.stopping = false;
        }
    }

    /// The model stopped a recording by `id`: only the one it has counts.
    pub fn model_stopped_recording(&self, bot: &str, id: u64) {
        let mut now = self.now();
        if now.recordings.get(bot).is_some_and(|r| r.id == Some(id)) {
            now.recordings.remove(bot);
        }
    }

    /// Whether a person's viewer of `bot`'s screen can drive it, input or not.
    pub fn drive_open(&self, bot: &str) -> bool {
        self.open_gate().and_then(|g| g.mode(bot)) == Some(Mode::Drive)
    }

    /// Gives `live`'s screen to a person, for `limit` from now. One at a time, and not on a computer
    /// going to sleep. Their viewer is theirs to open: the Computer tab's status says to.
    pub fn hand_over(self: &Arc<Self>, live: &Live, reason: &str, limit: Duration) -> Result<Handing, Problem> {
        let mut now = self.now();
        if now.sleeping.contains(&live.computer) {
            return Err(Problem::Asleep);
        }
        if now.handovers.contains_key(&live.bot) {
            return Err(Problem::Failed("this screen is with a person already".to_owned()));
        }
        let number = now.number();
        let (done, ended) = tokio::sync::oneshot::channel();
        now.handovers.insert(live.bot.clone(), Handover { number, computer: live.computer.clone(), reason: reason.to_owned(), until: Instant::now() + limit, done });
        Ok(Handing { computers: self.clone(), bot: live.bot.clone(), number, limit, ended })
    }

    /// A person handed `bot`'s screen back. `false` when nothing was theirs to hand back.
    pub fn hand_back(&self, bot: &str) -> bool {
        let ended = self.now().end_handover(bot, Ended::HandedBack);
        if ended {
            self.close_viewer(bot);
        }
        ended
    }

    /// Whether `model` sees images, as opencodex's catalog says. `None` when it doesn't say: a
    /// catalog that can't be reached, or a model not in it.
    pub async fn sees(&self, opencodex: &str, model: &str) -> Option<bool> {
        let fresh = |t: &Instant| t.elapsed() < SIGHT_TTL;
        if let Some((_, known)) = self.sight.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(t, _)| fresh(t)) {
            return sight_in(known, model);
        }
        let catalog = super::model::images::get_json(&format!("{opencodex}/v1/models")).await.ok().map(|j| sight_of(&j)).unwrap_or_default();
        let answer = sight_in(&catalog, model);
        *self.sight.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), catalog));
        answer
    }

    /// Pins the catalog (tests).
    #[cfg(test)]
    pub fn set_sight(&self, known: HashMap<String, bool>) {
        *self.sight.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), known));
    }

    /// The Computer tab's status for `bot`. No secret, no address.
    pub async fn status(&self, ctx: &HubCtx, bot: &str) -> Value {
        let sleep_after = self.idle.as_secs();
        let seat = match Self::seat(ctx, bot) {
            Ok(seat) => seat,
            Err(p) => return json!({"state": "error", "code": p.code(), "message": p.sentence(), "sleepAfterS": sleep_after}),
        };
        let Some(seat) = seat else {
            let problem = self.now().problems.get(bot).cloned();
            return match problem {
                Some(p) => json!({"state": "error", "code": p.code(), "message": p.sentence(), "sleepAfterS": sleep_after}),
                None => json!({"state": "absent", "sleepAfterS": sleep_after}),
            };
        };
        let id = seat.computer.clone();
        let bots: Vec<Value> = Self::seats(ctx, &id).unwrap_or_default().iter().map(|s| json!({"id": s.bot, "name": bot_name(ctx, &s.bot)})).collect();
        let (waking, problem, wake_ms) = {
            let now = self.now();
            (now.waking.contains(&id), now.problems.get(&id).cloned(), now.wake_ms.get(&id).copied())
        };
        let mut out = json!({"computer": id, "bots": bots, "sleepAfterS": sleep_after, "display": seat.screen.as_ref().map(|s| s.display.clone())});
        let record = self.machine(ctx, &id);
        match &record {
            Ok(m) if m.link.is_some() => out["remote"] = json!({"screen": m.has_screen(), "where": where_of(&m.reach)}),
            Err(_) if matches!(Self::is_remote(ctx, &id), Ok(true)) => out["remote"] = json!({"screen": false, "where": "", "lost": true}),
            _ => {}
        }
        if let Some(gate) = self.open_gate() {
            out["viewer"] = json!(gate.mode(bot).map(Mode::as_str));
            out["driving"] = json!(gate.driving(bot, self.quiet));
        }
        {
            let now = self.now();
            out["working"] = json!(now.holds.contains_key(bot));
            if let Some(h) = now.handovers.get(bot).filter(|h| h.computer == id) {
                out["takeover"] = json!({"reason": h.reason, "secondsLeft": h.until.saturating_duration_since(Instant::now()).as_secs()});
            }
        }
        if let Some(ms) = wake_ms {
            out["wakeMs"] = json!(ms);
        }
        let state = if waking {
            "waking".to_owned()
        } else if let Some(p) = problem {
            out["code"] = json!(p.code());
            out["message"] = json!(p.sentence());
            "error".to_owned()
        } else {
            match record {
                Ok(m) => {
                    out["provider"] = json!(m.provider.label());
                    match self.provider_of(&m).power(&m).await {
                        Ok(Power::Broken(why)) => {
                            out["message"] = json!(why);
                            "error".to_owned()
                        }
                        // awake with no screen for this Bot yet: a click brings it up
                        Ok(Power::Awake) if seat.screen.is_none() => "asleep".to_owned(),
                        Ok(p) => p.word().to_owned(),
                        Err(p) => {
                            out["code"] = json!(p.code());
                            out["message"] = json!(p.sentence());
                            "error".to_owned()
                        }
                    }
                }
                Err(p) => {
                    out["code"] = json!(p.code());
                    out["message"] = json!(match Self::is_remote(ctx, &id) {
                        Ok(true) => LOST_REMOTE.to_owned(),
                        Ok(false) => p.sentence(),
                        // neither the record nor its row: what kind it is isn't known, so it isn't said
                        Err(_) => UNREAD.to_owned(),
                    });
                    "error".to_owned()
                }
            }
        };
        out["state"] = json!(state);
        out
    }

    /// The computers another Bot could join, for the tab's picker.
    pub fn list(ctx: &HubCtx) -> Value {
        let rows: Vec<Value> = Self::ids(ctx)
            .into_iter()
            .map(|id| {
                let seats = Self::seats(ctx, &id).unwrap_or_default();
                let names: Vec<String> = seats.iter().map(|s| bot_name(ctx, &s.bot)).collect();
                let remote = Self::is_remote(ctx, &id).unwrap_or(true);
                json!({"id": id, "bots": names, "full": seats.len() as i64 >= MAX_SEATS, "remote": remote})
            })
            .collect();
        json!({"computers": rows})
    }
}

const HAS_ONE: &str = "This Bot already has a computer.";

/// The keystore of this broker's data dir: the Keychain for the owner's, a file store for any other.
fn keys(ctx: &HubCtx) -> Result<Keystore, Problem> {
    let spec = std::env::var("BUWIZ_KEYSTORE").unwrap_or_default();
    let no_keychain = std::env::var("BUWIZ_NO_KEYCHAIN").as_deref() == Ok("1");
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    Keystore::for_data(&spec, no_keychain, &ctx.data, &home).map_err(Problem::Failed)
}

/// Puts `bot`'s computer switch at Ask, as a Bot at a remote computer starts. Off stays off.
fn ask_first(ctx: &HubCtx, bot: &str) -> Result<(), Problem> {
    if policy::tool_state(ctx, bot, COMPUTER_KEY) == policy::Pos::Off {
        return Ok(());
    }
    let at = policy::now(ctx);
    ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO bot_tools(bot_id, key, enabled, state, actor, at) VALUES(?,?,1,'ask',?,?)", rusqlite::params![bot, COMPUTER_KEY, REMOTE_ACTOR, at])).map(|_| ()).map_err(db_problem)
}

/// `bot` left a remote computer: that Ask goes with it. One a person set stays theirs.
fn drop_ask(ctx: &HubCtx, bot: &str) {
    let _ = ctx.db.with(|c| c.execute("DELETE FROM bot_tools WHERE bot_id=? AND key=? AND actor=?", rusqlite::params![bot, COMPUTER_KEY, REMOTE_ACTOR]));
}

/// Where a remote computer is, as a person wrote it. Theirs to read: no route but the app's says it.
fn where_of(reach: &Reach) -> String {
    match reach {
        Reach::Url { url } => url.clone(),
        Reach::Ssh { host, user, port } => {
            let user = user.as_ref().map(|u| format!("{u}@")).unwrap_or_default();
            let port = port.map(|p| format!(":{p}")).unwrap_or_default();
            format!("{user}{host}{port}")
        }
        Reach::Container { name } => name.clone(),
    }
}

/// A folder for a test of a connection, in the data dir the shell can't read. Gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(ctx: &HubCtx) -> Result<Self, Problem> {
        let dir = ctx.data.join("computers").join(format!("{}{}", buwiz_computer::remote::ssh::PROBE_PREFIX, buwiz_computer::random_hex(8)?));
        std::fs::create_dir_all(&dir).map_err(|e| Problem::Failed(e.to_string()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A drive ticket is being made for `.1`: no recording starts until it's done, however that ends.
struct Coming<'a>(&'a Computers, String);

impl<'a> Coming<'a> {
    fn new(computers: &'a Computers, bot: &str) -> Self {
        *computers.now().coming.entry(bot.to_owned()).or_default() += 1;
        Self(computers, bot.to_owned())
    }
}

impl Drop for Coming<'_> {
    fn drop(&mut self) {
        let mut now = self.0.now();
        let left = now.coming.get_mut(&self.1).map(|n| {
            *n = n.saturating_sub(1);
            *n
        });
        if left == Some(0) {
            now.coming.remove(&self.1);
        }
    }
}

/// Marks a computer as going to sleep for as long as it's held, however that ends.
struct Sleeping<'a>(&'a Computers, String);

impl Drop for Sleeping<'_> {
    fn drop(&mut self) {
        self.0.now().sleeping.remove(&self.1);
    }
}

/// Which models of opencodex's catalog see images.
fn sight_of(catalog: &Value) -> HashMap<String, bool> {
    let models = catalog.get("data").and_then(Value::as_array).into_iter().flatten();
    models
        .filter_map(|m| {
            let caps = m.get("capabilities")?;
            let sees = caps.get("supports_vision").and_then(Value::as_bool).or_else(|| Some(caps.get("input_modalities")?.as_array()?.iter().any(|x| x == "image")))?;
            Some((m.get("id")?.as_str()?.to_owned(), sees))
        })
        .collect()
}

/// What the catalog says of `model`: by its id, else, for a bare name, by a provider's
/// `<provider>/<model>` entry. Providers that disagree count as not knowing.
fn sight_in(known: &HashMap<String, bool>, model: &str) -> Option<bool> {
    if let Some(sees) = known.get(model) {
        return Some(*sees);
    }
    let mut named = known.iter().filter(|(id, _)| id.rsplit('/').next() == Some(model)).map(|(_, sees)| *sees);
    let first = named.next()?;
    named.all(|sees| sees == first).then_some(first)
}

/// `cmp_` and hex: what [`Computers::make`] writes.
fn valid_id(id: &str) -> bool {
    id.strip_prefix("cmp_").is_some_and(|h| !h.is_empty() && h.len() <= 32 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

fn bot_name(ctx: &HubCtx, bot: &str) -> String {
    ctx.store.read(|s| s.bot(bot).map(|b| b.name.clone())).unwrap_or_else(|| bot.to_owned())
}

fn seat_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Seat> {
    let screen: Option<String> = r.get(3)?;
    Ok(Seat { bot: r.get(0)?, computer: r.get(1)?, role: r.get(2)?, screen: screen.and_then(|s| serde_json::from_str(&s).ok()) })
}

// ---- the routes ----

fn refused(p: &Problem) -> HubResponse {
    types::json(StatusCode::CONFLICT, &json!({"ok": false, "code": p.code(), "message": p.sentence()}))
}

fn unavailable() -> HubResponse {
    types::json(StatusCode::SERVICE_UNAVAILABLE, &json!({"ok": false, "code": "no_computers", "message": "This broker has no computers."}))
}

/// The Bot of a route, if a person's app is asking and the Bot exists.
fn gate_route(ctx: &HubCtx, req: &HubRequest) -> Result<(Arc<Computers>, String), Box<HubResponse>> {
    if !host::authorized(&req.headers) {
        return Err(Box::new(host::unauthorized()));
    }
    let Some(computers) = Computers::of(ctx) else { return Err(Box::new(unavailable())) };
    let bot = req.params.first().cloned().unwrap_or_default();
    if ctx.store.read(|s| s.bot(&bot).is_none()) {
        return Err(Box::new(types::json(StatusCode::NOT_FOUND, &json!({"ok": false, "code": "not_found", "message": "No such Bot."}))));
    }
    Ok((computers, bot))
}

macro_rules! bot_route {
    ($ctx:expr, $req:expr) => {
        match gate_route(&$ctx, &$req) {
            Ok(v) => v,
            Err(r) => return *r,
        }
    };
}

async fn status(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    types::json(StatusCode::OK, &computers.status(&ctx, &bot).await)
}

async fn wake(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    // what went wrong is in the status, as the tab shows it
    let _ = computers.click(&ctx, &bot).await;
    types::json(StatusCode::OK, &computers.status(&ctx, &bot).await)
}

async fn sleep(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    let seat = match Computers::seat(&ctx, &bot) {
        Ok(seat) => seat,
        Err(p) => return refused(&p),
    };
    let Some(seat) = seat else { return types::json(StatusCode::OK, &computers.status(&ctx, &bot).await) };
    match computers.sleep(&ctx, &seat.computer, Sleep::Light).await {
        Ok(()) => types::json(StatusCode::OK, &computers.status(&ctx, &bot).await),
        Err(p) => refused(&p),
    }
}

/// A one-time URL for this Bot's screen, in the mode `{mode}` asks for: `watch` when it says
/// nothing, and a word that's neither gets nothing.
async fn open(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let mode = match body.get("mode") {
        None | Some(Value::Null) => Mode::Watch,
        Some(word) => match word.as_str().and_then(Mode::parse) {
            Some(mode) => mode,
            None => return refused(&Problem::Failed("a viewer is for watch or drive".to_owned())),
        },
    };
    let live = match computers.live(&ctx, &bot).await {
        Ok(l) => l,
        Err(p) => return refused(&p),
    };
    let viewer = match live.provider.viewer(&live.machine, &live.screen).await {
        Ok(Some(v)) => v,
        Ok(None) => return refused(&Problem::Unsupported("show a live desktop".to_owned())),
        Err(p) => return refused(&p),
    };
    let Some(gate) = computers.gate().await else { return refused(&Problem::Failed("no port for the viewer".to_owned())) };
    let coming = (mode == Mode::Drive).then(|| Coming::new(&computers, &bot));
    if coming.is_some() {
        if let Err(p) = computers.end_recording(&live).await {
            return refused(&p);
        }
    }
    Computers::touch(&ctx, &live.computer);
    match gate.open(&bot, viewer, mode) {
        Ok(url) => types::json(StatusCode::OK, &json!({"ok": true, "url": url})),
        Err(p) => refused(&p),
    }
}

/// This Bot's screen as a PNG. With `after`, it waits a moment for a change first and answers
/// 204 when there was none, so a still screen costs no pictures. Looking isn't using: a tile left
/// open doesn't keep a computer up.
async fn screen(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    let live = match computers.live(&ctx, &bot).await {
        Ok(l) => l,
        Err(p) => return refused(&p),
    };
    let mut seq = None;
    if let Some(since) = req.query("after").and_then(|a| a.parse::<u64>().ok()) {
        let wait = buwiz_computer::WaitFor { since: Some(since), ..buwiz_computer::WaitFor::default() };
        if let Ok(settled) = live.provider.settle(&live.machine, &live.screen, wait).await {
            seq = settled.seq;
            if !settled.changed && !settled.fallback {
                let mut res = types::json(StatusCode::NO_CONTENT, &Value::Null);
                *res.body_mut() = Full::new(Bytes::new()).map_err(|never| match never {}).boxed();
                return res;
            }
        }
    } else {
        seq = live.provider.mark(&live.machine, &live.screen).await;
    }
    match live.provider.screenshot(&live.machine, &live.screen).await {
        Ok(shot) => {
            let mut res = Response::new(Full::new(Bytes::from(shot.png)).map_err(|never| match never {}).boxed());
            res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
            if let Some(v) = seq.and_then(|s| HeaderValue::from_str(&s.to_string()).ok()) {
                res.headers_mut().insert("x-computer-seq", v);
            }
            res
        }
        Err(p) => refused(&p),
    }
}

/// The tab's Record: `{action: "start"}`, then `{action: "stop", id}`. The MP4 stays in the Bot's
/// folder on the computer.
async fn record(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    let live = match computers.live(&ctx, &bot).await {
        Ok(l) => l,
        Err(p) => return refused(&p),
    };
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let done = match (body.get("action").and_then(Value::as_str), body.get("id").and_then(Value::as_u64)) {
        (Some("start"), _) => live.provider.record_start(&live.machine, &live.screen).await.map(|id| json!({"ok": true, "id": id})),
        (Some("stop"), Some(id)) => live.provider.record_stop(&live.machine, &live.screen, id).await.map(|path| json!({"ok": true, "path": path})),
        _ => Err(Problem::Failed("say start, or stop with the id".to_owned())),
    };
    match done {
        Ok(v) => types::json(StatusCode::OK, &v),
        Err(p) => refused(&p),
    }
}

/// The person is done with a screen their Bot gave them.
async fn hand_back(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    if !computers.hand_back(&bot) {
        return refused(&Problem::Failed("this screen isn't with you".to_owned()));
    }
    types::json(StatusCode::OK, &computers.status(&ctx, &bot).await)
}

async fn attach(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let Some(computer) = body.get("computer").and_then(Value::as_str) else { return refused(&Problem::Failed("say which computer".to_owned())) };
    match computers.attach(&ctx, &bot, computer).await {
        Ok(()) => types::json(StatusCode::OK, &computers.status(&ctx, &bot).await),
        Err(p) => refused(&p),
    }
}

async fn detach(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    match computers.leave(&ctx, &bot).await {
        Ok(()) => types::json(StatusCode::OK, &computers.status(&ctx, &bot).await),
        Err(p) => refused(&p),
    }
}

/// What the form sent. Nothing of it is logged, and no answer repeats a secret.
fn asked(body: &Value) -> Asked {
    let text = |k: &str| body.get(k).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let secret = |k: &str| text(k).map(Secret::new);
    Asked { url: text("url").unwrap_or_default().to_owned(), token: secret("token"), viewer_password: secret("viewerPassword"), key_file: text("keyFile").map(PathBuf::from), pin: text("pin").map(str::to_owned) }
}

/// `{ok: true, screen, system}` for a computer that works, `{ok: false, code: "trust", fingerprint,
/// pin}` for one a person must look at first. Anything else is a refusal.
async fn test_remote(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    if !host::authorized(&req.headers) {
        return host::unauthorized();
    }
    let Some(computers) = Computers::of(&ctx) else { return unavailable() };
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    match computers.probe_remote(&ctx, &asked(&body)).await {
        Ok(Probed::Ready { boxed, system }) => types::json(StatusCode::OK, &json!({"ok": true, "screen": boxed, "system": system, "consent": CONSENT})),
        Ok(Probed::Trust { fingerprint, pin, vouched }) => types::json(StatusCode::OK, &json!({"ok": false, "code": "trust", "fingerprint": fingerprint, "pin": pin, "vouched": vouched})),
        Err(p) => refused(&p),
    }
}

/// Adds a remote computer for this Bot. `{consent: true}` is a person's yes to [`CONSENT`].
async fn add_remote(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    if body.get("consent").and_then(Value::as_bool) != Some(true) {
        return refused(&Problem::Remote(CONSENT.to_owned()));
    }
    match computers.add_remote(&ctx, &bot, &asked(&body)).await {
        Ok(_) => types::json(StatusCode::OK, &computers.status(&ctx, &bot).await),
        Err(p) => refused(&p),
    }
}

async fn remove_remote(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
    match computers.remove_remote(&ctx, &bot).await {
        Ok(()) => types::json(StatusCode::OK, &computers.status(&ctx, &bot).await),
        Err(p) => refused(&p),
    }
}

async fn list(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    if !host::authorized(&req.headers) {
        return host::unauthorized();
    }
    types::json(StatusCode::OK, &Computers::list(&ctx))
}

/// Puts the computers in `ctx.ext`, with their routes and the timer that puts them to sleep.
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    if ctx.ext.get::<Arc<Computers>>().is_none() {
        ctx.ext.insert(Arc::new(Computers::new(provider_for(ProviderKind::Docker))));
    }
    routes.route(Verb::Get, X("/v1/host/computers"), list);
    routes.route(Verb::Post, X("/v1/host/computer/remote/test"), test_remote);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/remote"), add_remote);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/remote/remove"), remove_remote);
    routes.route(Verb::Get, S("/v1/host/computer/bot/*"), status);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/wake"), wake);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/sleep"), sleep);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/open"), open);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/handback"), hand_back);
    routes.route(Verb::Get, S("/v1/host/computer/bot/*/screen"), screen);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/record"), record);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/attach"), attach);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/detach"), detach);
    routes.on_start(|ctx| {
        buwiz_computer::remote::ssh::clear_staged(&ctx.data.join("computers"));
        let Some(computers) = Computers::of(&ctx) else { return };
        let tick = (computers.idle / 2).clamp(Duration::from_secs(1), Duration::from_secs(30));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tick).await;
                computers.sleep_the_idle(&ctx).await;
            }
        });
    });
}

#[cfg(test)]
mod tests;
