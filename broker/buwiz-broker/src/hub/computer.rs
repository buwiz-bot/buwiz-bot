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
//! ticket ([`buwiz_computer::Gate`]).
//!
//! Files outlive everything but a person's delete: a box or holder gone missing is rebuilt on the
//! same volumes when it next wakes, and a lost record is rebuilt from what's left. A computer is
//! only ever removed for a person who deleted its last Bot, or just made and empty.
//!
//! What C6 hangs here: a seat's `role` (`member` today; a Bot lent another's computer would be
//! a `guest`, with what it may do decided by that side's switches) and a screen's `folder`, the
//! per-agent place artifacts are collected from.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buwiz_computer::{provider_for, Gate, Machine, Power, Problem, Provider, ProviderKind, Reach, Screen, Sleep, Spec, Woke};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::ctx::HubCtx;
use super::policy;
use super::routes::host;
use super::types::{self, HubRequest, HubResponse, Pattern::Exact as X, Pattern::Segments as S, Verb};
use super::Registry;

/// The own screens a box runs (`box-screen`'s `BOX_MAX_OWN_SCREENS`).
pub const MAX_SEATS: i64 = 4;
const IDLE_ENV: &str = "BUWIZ_COMPUTER_IDLE_SECS";
const DEEP_ENV: &str = "BUWIZ_COMPUTER_DEEP_SECS";
/// Idle this long, it sleeps. Asleep this long, it lets go of its memory.
const IDLE: Duration = Duration::from_secs(15 * 60);
const DEEP: Duration = Duration::from_secs(2 * 60 * 60);
/// How long opencodex's word on which models see images holds: every model call asks.
const SIGHT_TTL: Duration = Duration::from_secs(60);

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
}

/// The hub's computers, in `ctx.ext`.
pub struct Computers {
    provider: Arc<dyn Provider>,
    gate: tokio::sync::OnceCell<Option<Gate>>,
    now: Mutex<Now>,
    machines: Mutex<HashMap<String, Machine>>,
    idle: Duration,
    deep: Duration,
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
        Self { provider, gate: tokio::sync::OnceCell::new(), now: Mutex::default(), machines: Mutex::default(), seating: tokio::sync::Mutex::new(()), idle: secs(IDLE_ENV, IDLE), deep: secs(DEEP_ENV, DEEP), sight: Mutex::default() }
    }

    pub fn of(ctx: &HubCtx) -> Option<Arc<Self>> {
        ctx.ext.get::<Arc<Self>>().cloned()
    }

    fn now(&self) -> std::sync::MutexGuard<'_, Now> {
        self.now.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The provider of a saved computer: a kind this build can't run answers with its sentence.
    fn provider_of(&self, machine: &Machine) -> Arc<dyn Provider> {
        if machine.provider == self.provider.kind() {
            self.provider.clone()
        } else {
            provider_for(machine.provider)
        }
    }

    async fn gate(&self) -> Option<&Gate> {
        self.gate.get_or_init(|| async { Gate::start().await.ok() }).await.as_ref()
    }

    // ---- the records ----

    pub fn seat(ctx: &HubCtx, bot: &str) -> Option<Seat> {
        ctx.db
            .with(|c| c.query_row("SELECT bot_id, computer_id, role, screen FROM computer_seats WHERE bot_id=?", [bot], seat_row).optional())
            .ok()
            .flatten()
    }

    pub fn seats(ctx: &HubCtx, computer: &str) -> Vec<Seat> {
        ctx.db
            .with(|c| {
                let mut q = c.prepare("SELECT bot_id, computer_id, role, screen FROM computer_seats WHERE computer_id=? ORDER BY attached_at, bot_id")?;
                let rows = q.query_map([computer], seat_row)?;
                rows.collect()
            })
            .unwrap_or_default()
    }

    fn ids_where(ctx: &HubCtx, clause: &str) -> Vec<String> {
        ctx.db
            .with(|c| {
                let mut q = c.prepare(&format!("SELECT id FROM computers WHERE {clause} ORDER BY created_at, id"))?;
                let rows = q.query_map([], |r| r.get(0))?;
                rows.collect()
            })
            .unwrap_or_default()
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
        let m: Machine = serde_json::from_slice(&text).map_err(|_| Problem::Gone)?;
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
    fn reach_of(ctx: &HubCtx, id: &str) -> Option<Reach> {
        let text: Option<String> = ctx.db.with(|c| c.query_row("SELECT reach FROM computers WHERE id=?", [id], |r| r.get(0)).optional()).ok().flatten().flatten();
        text.and_then(|t| serde_json::from_str(&t).ok())
    }

    /// Whether `id` is marked for removal: nothing may bring it up again.
    fn retired(ctx: &HubCtx, id: &str) -> bool {
        Self::ids_where(ctx, "gone_at IS NOT NULL").iter().any(|i| i == id)
    }

    /// A person clicked: makes the computer if `bot` has none, or if its record is lost (the Bots
    /// at it move to the new one), and starts waking it. Comes back at once; the status says how
    /// it goes.
    pub async fn click(self: &Arc<Self>, ctx: &Arc<HubCtx>, bot: &str) -> Result<(), Problem> {
        self.now().problems.remove(bot);
        let id = match self.computer_for_click(ctx, bot).await {
            Ok(id) => id,
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
            let _ = me.finish_wake(&ctx, &id).await;
        });
        Ok(())
    }

    async fn computer_for_click(&self, ctx: &HubCtx, bot: &str) -> Result<String, Problem> {
        let seating = self.seating.lock().await;
        let Some(seat) = Self::seat(ctx, bot) else {
            let id = self.make(ctx, bot).await?;
            if let Err(e) = Self::add_seat(ctx, bot, &id, "member") {
                // just made: nothing is on it. The provider isn't waited on under the lock
                self.mark_retired(ctx, &id);
                drop(seating);
                self.finish_retire(ctx, &id).await;
                return Err(e);
            }
            return Ok(id);
        };
        // a record that reads is all it takes: the wake rebuilds whatever went, on the same files
        if !matches!(self.machine(ctx, &seat.computer), Err(Problem::Gone)) {
            return Ok(seat.computer);
        }
        // its record is lost: what's left of the computer gets a new one, files and all
        let spec = Spec { id: seat.computer.clone(), dir: Self::dir(ctx, &seat.computer) };
        if let Some(machine) = self.provider.recover(&spec, Self::reach_of(ctx, &seat.computer).as_ref()).await? {
            std::fs::create_dir_all(&spec.dir).map_err(|e| Problem::Failed(e.to_string()))?;
            Self::save_machine(ctx, &seat.computer, &machine)?;
            self.machines.lock().unwrap_or_else(|e| e.into_inner()).insert(seat.computer.clone(), machine);
            self.now().problems.remove(&seat.computer);
            return Ok(seat.computer);
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
        Ok(id)
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
        let seats = Self::seats(ctx, id);
        for bot in provider.screens(&machine).await.unwrap_or_default() {
            if !seats.iter().any(|s| s.bot == bot) {
                let _ = provider.screen_down(&machine, &bot).await;
            }
        }
    }

    async fn wake_all(&self, ctx: &HubCtx, id: &str) -> Result<(), Problem> {
        // `id` is in `waking` by now, and the sweep leaves a waking computer alone: whichever of
        // this check and a retire comes first, nothing is brought up after it's removed
        if Self::retired(ctx, id) {
            return Err(Problem::Gone);
        }
        let machine = self.machine(ctx, id)?;
        let provider = self.provider_of(&machine);
        let woke = provider.wake(&machine).await?;
        // resumed, the screens it had are as they were: only a Bot that joined meanwhile needs one
        let seats = Self::seats(ctx, id);
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

    pub async fn sleep(&self, ctx: &HubCtx, id: &str, depth: Sleep) -> Result<(), Problem> {
        let machine = self.machine(ctx, id)?;
        if let Some(gate) = self.gate().await {
            for seat in Self::seats(ctx, id) {
                gate.close(&seat.bot);
            }
        }
        self.provider_of(&machine).sleep(&machine, depth).await?;
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
                    let _ = self.sleep(ctx, &id, Sleep::Light).await;
                }
                Ok(Power::Asleep) if slept.is_some_and(|s| at - s >= millis(self.deep)) => {
                    let _ = self.sleep(ctx, &id, Sleep::Deep).await;
                }
                _ => {}
            }
        }
    }

    async fn watched(&self, ctx: &HubCtx, id: &str) -> bool {
        let Some(gate) = self.gate().await else { return false };
        Self::seats(ctx, id).iter().any(|s| gate.watched(&s.bot))
    }

    // ---- seats ----

    /// A person put `bot` at `computer`, which already serves other Bots.
    pub async fn attach(&self, ctx: &HubCtx, bot: &str, computer: &str) -> Result<(), Problem> {
        let _seating = self.seating.lock().await;
        if Self::seat(ctx, bot).is_some() {
            return Err(Problem::Failed("this Bot already has a computer".to_owned()));
        }
        if !Self::known(ctx, computer) {
            return Err(Problem::Gone);
        }
        let machine = self.machine(ctx, computer)?;
        if Self::seats(ctx, computer).len() as i64 >= MAX_SEATS {
            return Err(Problem::Failed(format!("a computer has room for {MAX_SEATS} Bots")));
        }
        // the seat comes last: a Bot that can't use the computer isn't seated at it
        let provider = self.provider_of(&machine);
        let (power, machine) = provider.look(&machine).await?;
        let screen = if power == Power::Awake { Some(provider.screen_up(&machine, bot).await?) } else { None };
        if let Err(e) = Self::add_seat(ctx, bot, computer, "member") {
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
    /// ([`Computers::sleep_the_idle`]), so nothing here waits on the provider: with Docker off or
    /// hung a Bot still goes, and nobody else's click waits behind it.
    pub async fn detach(&self, ctx: &HubCtx, bot: &str) -> Result<(), Problem> {
        self.take_off(ctx, bot, true).await
    }

    /// The check that it's the last Bot and its going are one step, under the lock: two Bots
    /// leaving at once can't each see the other still there.
    async fn take_off(&self, ctx: &HubCtx, bot: &str, may_be_last: bool) -> Result<(), Problem> {
        let seat = {
            let _seating = self.seating.lock().await;
            let Some(seat) = Self::seat(ctx, bot) else { return Ok(()) };
            let last = Self::seats(ctx, &seat.computer).len() <= 1;
            if last && !may_be_last {
                return Err(Problem::Failed("the last Bot at a computer can't leave it: its files would go".to_owned()));
            }
            ctx.db.with(|c| c.execute("DELETE FROM computer_seats WHERE bot_id=?", [bot])).map_err(db_problem)?;
            if last {
                self.mark_retired(ctx, &seat.computer);
            }
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
        let removed = match self.machine(ctx, id) {
            Ok(m) => self.provider_of(&m).destroy(&m).await,
            Err(_) => self.provider.sweep(id, Self::reach_of(ctx, id).as_ref()).await,
        };
        if matches!(removed, Ok(()) | Err(Problem::NotBuilt(_) | Problem::Gone)) {
            let _ = ctx.db.with(|c| c.execute("DELETE FROM computers WHERE id=?", [id]));
            self.machines.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
            self.now().problems.remove(id);
            if valid_id(id) {
                let _ = std::fs::remove_dir_all(Self::dir(ctx, id));
            }
        }
    }

    // ---- reading ----

    /// `bot`'s computer ready to act on, or why not. Only an awake computer is: the model doesn't
    /// wake one. Nothing gets past here on a computer with a recorded problem, or one whose
    /// provider's guard fails; that guard is how a box from before this broker's start gets its
    /// wall before anyone uses it.
    pub async fn live(&self, ctx: &HubCtx, bot: &str) -> Result<Live, Problem> {
        let seat = Self::seat(ctx, bot).ok_or(Problem::Asleep)?;
        {
            let now = self.now();
            if now.waking.contains(&seat.computer) {
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
                Ok(Live { provider, machine, computer: seat.computer, screen })
            }
            (Power::Absent, _) => Err(Problem::Gone),
            _ => Err(Problem::Asleep),
        }
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
        let Some(seat) = Self::seat(ctx, bot) else {
            let problem = self.now().problems.get(bot).cloned();
            return match problem {
                Some(p) => json!({"state": "error", "code": p.code(), "message": p.sentence(), "sleepAfterS": sleep_after}),
                None => json!({"state": "absent", "sleepAfterS": sleep_after}),
            };
        };
        let id = seat.computer.clone();
        let bots: Vec<Value> = Self::seats(ctx, &id).iter().map(|s| json!({"id": s.bot, "name": bot_name(ctx, &s.bot)})).collect();
        let (waking, problem, wake_ms) = {
            let now = self.now();
            (now.waking.contains(&id), now.problems.get(&id).cloned(), now.wake_ms.get(&id).copied())
        };
        let mut out = json!({"computer": id, "bots": bots, "sleepAfterS": sleep_after, "display": seat.screen.as_ref().map(|s| s.display.clone())});
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
            match self.machine(ctx, &id) {
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
                    out["message"] = json!(p.sentence());
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
                let seats = Self::seats(ctx, &id);
                let names: Vec<String> = seats.iter().map(|s| bot_name(ctx, &s.bot)).collect();
                json!({"id": id, "bots": names, "full": seats.len() as i64 >= MAX_SEATS})
            })
            .collect();
        json!({"computers": rows})
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
    let Some(seat) = Computers::seat(&ctx, &bot) else { return types::json(StatusCode::OK, &computers.status(&ctx, &bot).await) };
    match computers.sleep(&ctx, &seat.computer, Sleep::Light).await {
        Ok(()) => types::json(StatusCode::OK, &computers.status(&ctx, &bot).await),
        Err(p) => refused(&p),
    }
}

/// A one-time URL for this Bot's screen.
async fn open(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let (computers, bot) = bot_route!(ctx, req);
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
    Computers::touch(&ctx, &live.computer);
    match gate.open(&bot, viewer) {
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
    routes.route(Verb::Get, S("/v1/host/computer/bot/*"), status);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/wake"), wake);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/sleep"), sleep);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/open"), open);
    routes.route(Verb::Get, S("/v1/host/computer/bot/*/screen"), screen);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/record"), record);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/attach"), attach);
    routes.route(Verb::Post, S("/v1/host/computer/bot/*/detach"), detach);
    routes.on_start(|ctx| {
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
