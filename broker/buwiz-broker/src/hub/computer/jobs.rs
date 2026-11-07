//! Jobs: a command a Bot leaves running on its computer, and the computer staying up for it.
//!
//! The words sent to the computer are [`buwiz_computer::job`]'s, through the plain `run`. This is
//! the record: a `computer_jobs` row each, so a job outlives a restart of this broker, and an
//! entry in `Now` for each one running. That entry is what `Now::in_use` reads, so "a job needs
//! this computer" and the sleep mark are one locked step, as they are for a run or a hand-over.
//!
//! A row carries its limit, and the sweep ([`Computers::sleep_the_idle`]) looks at each running job
//! (`Computers::poll_jobs`). The model isn't there while it runs, so it costs no model money.
//!
//! - A person's Sleep still wins: it freezes the job, and waking the computer lets it go on. The
//!   tab's button says so. A job doesn't count as running while it's frozen.
//! - Past its limit a job is stopped at the next look that can reach it. Until that works it stays
//!   a job: on the tab, in the count, and holding its computer up. One frozen past its limit, or one
//!   whose stop failed [`STOP_TRIES`] times (a person is told, once), stays on the record and lets
//!   the computer and the Mac go, and each look still tries. A deep sleep that takes it is its end,
//!   by its limit.
//! - The end of a job is one `UPDATE ... WHERE state='running'`: the sweep and a model's status call
//!   can meet there, and only one posts the notice. A notice is free. A model turn follows only with
//!   `computer_routines` On, at most one, and it waits for a run under way in that chat. It's owed in
//!   the row, so a restart doesn't lose it, and it's checked again as it starts, so a person
//!   who turned routines off in between isn't charged for it.
//!
//! While a job runs, and isn't frozen, the Mac is kept awake ([`super::awake`]).
//!
//! A job can be over on the record and still be on the computer: its Bot left while the computer
//! slept, or a start's end wasn't heard. Its row owes a cancel ([`Computers::settle_stops`]), by the
//! Bot's folder and so with no seat needed, paid for that computer as a wake finishes or a Bot
//! leaves, and for all of them by each sweep, until the computer has said it's done. A computer that
//! was up and didn't take it [`STOP_TRIES`] times is given up on, and a person told. So is a remote
//! computer that goes with a cancel unpaid: nothing there is destroyed with it. A computer that
//! sleeps isn't a failure: what's frozen waits for its wake.
//!
//! What a model named a job stays out of the unattended turn it starts: it may have copied it from
//! a page, and nobody is there. Only a person's notice shows it.
//!
//! No row holds the command: what's run stays in the computer, and a job's name is the model's words
//! for a person to read.

use std::sync::Arc;
use std::time::Duration;

use buwiz_computer::job::{self, Seen};
use buwiz_computer::{Machine, Power, Problem, Provider, Screen};
use rusqlite::params;
use serde_json::{json, Value};

use super::{db_problem, Computers, Live};
use crate::hub::ctx::HubCtx;
use crate::hub::policy::{self, Pos};
use crate::hub::tools::computer::{inside, ROUTINES_KEY};
use crate::hub::tools::miniapps::post_in;
use crate::hub::traits::TurnRequest;
use crate::hub::types::TurnCtx;

/// How long a job may run when the model doesn't say, and the most it can say. These stop a
/// forgotten job holding a 4 GiB box.
pub const LIMIT: Duration = Duration::from_secs(4 * 60 * 60);
pub const MAX_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);
pub const MIN_LIMIT: Duration = Duration::from_secs(60);
/// How many a Bot may have running at once.
pub const PER_BOT: usize = 2;
/// How many cancels a computer that's up can fail before a person is told instead.
const STOP_TRIES: i64 = 20;
/// How long an ended job's row stays, for its status.
const KEPT_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// A look at a job or a start of one: long enough for a busy box, short enough for the sweep.
const LOOK_MS: u64 = 15_000;
const MAX_NAME: usize = 80;
/// How many of its jobs a status lists, newest first.
const LISTED: usize = 10;
/// How much of what it printed a status gives the model.
const OUT_BYTES: usize = 20_000;

const NOTICE: &str = "computer_job";
/// What a notice's `state` is for a job nothing could stop.
const UNSTOPPED: &str = "unstopped";
const TOO_MANY: &str = "Two jobs are running on this Bot's computer already, and that's the most it can have. Wait for one to end, or stop one with computer_job_stop.";

/// A running job, as [`super::Now`] holds it.
#[derive(Clone, Debug)]
pub(super) struct Running {
    pub(super) bot: String,
    pub(super) computer: String,
    pub(super) thread: String,
    /// its start isn't finished: there's nothing to look at yet
    starting: bool,
    /// its computer sleeps, so it's stopped where it was
    pub(super) frozen: bool,
    /// its limit passed and it couldn't be stopped: it no longer holds anything up
    pub(super) past: bool,
}

/// A `computer_jobs` row.
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub bot: String,
    computer: String,
    thread: String,
    pub name: String,
    started: i64,
    until: i64,
    pub state: String,
    pub exit: Option<i32>,
}

/// How a job ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum End {
    Exited(i32),
    Stopped,
    Limit,
    /// nothing says how, and the computer may have restarted under it
    Lost,
    /// its Bot left the computer, and a cancel is owed
    Left,
}

impl End {
    fn word(self) -> &'static str {
        match self {
            Self::Exited(_) => "done",
            Self::Stopped => "stopped",
            Self::Limit => "limit",
            Self::Lost => "lost",
            Self::Left => "left",
        }
    }
}

/// What a job's notice says. The model never reads it: notices stay out of its history.
pub fn said(name: &str, state: &str, exit: Option<i64>) -> String {
    let what = if name.is_empty() { "Job".to_owned() } else { format!("Job \"{name}\"") };
    match (state, exit) {
        ("done", Some(0)) => format!("{what} finished."),
        ("done", Some(code)) => format!("{what} failed with exit code {code}."),
        ("stopped", _) => format!("{what} was stopped."),
        ("limit", _) => format!("{what} reached its time limit and was stopped."),
        ("left", _) => format!("{what} is being stopped: its Bot left that computer."),
        (UNSTOPPED, _) => format!("{what} couldn't be stopped, so it may still be running on that computer."),
        _ => format!("{what} ended and left no exit code. Its computer may have restarted."),
    }
}

const COLUMNS: &str = "id, bot_id, computer_id, thread, name, started_at, until_at, state, exit_code";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job { id: r.get(0)?, bot: r.get(1)?, computer: r.get(2)?, thread: r.get(3)?, name: r.get(4)?, started: r.get(5)?, until: r.get(6)?, state: r.get(7)?, exit: r.get(8)? })
}

fn rows(ctx: &HubCtx, filter: &str, value: &str) -> Vec<Job> {
    let read = ctx.db.with(|c| {
        let mut q = c.prepare(&format!("SELECT {COLUMNS} FROM computer_jobs {filter} ORDER BY started_at, id"))?;
        let found = q.query_map([value], row)?;
        found.collect::<rusqlite::Result<Vec<_>>>()
    });
    read.unwrap_or_else(|e| {
        eprintln!("buwiz-broker: computer_jobs: {e}");
        Vec::new()
    })
}

fn running(ctx: &HubCtx) -> Vec<Job> {
    rows(ctx, "WHERE state=?", "running")
}

async fn command(live: &Live, line: &str) -> Result<String, Problem> {
    live.provider.run(&live.machine, &live.screen, line, LOOK_MS).await.map(|o| o.stdout)
}

impl Computers {
    /// On start: the jobs the last broker left are running here too. Rows long ended go.
    pub fn load_jobs(&self, ctx: &HubCtx) {
        let oldest = policy::now(ctx) - KEPT_MS;
        if let Err(e) = ctx.db.with(|c| c.execute("DELETE FROM computer_jobs WHERE state<>'running' AND turn_owed=0 AND stop_owed=0 AND ended_at<?", [oldest])) {
            eprintln!("buwiz-broker: computer_jobs: {e}");
        }
        let found = running(ctx);
        let mut now = self.now();
        for j in found {
            now.jobs.insert(j.id, Running { bot: j.bot, computer: j.computer, thread: j.thread, starting: false, frozen: false, past: false });
        }
        drop(now);
        self.sync_awake();
    }

    /// `live`'s Bot starts `command`, for `limit` at most: the job's row, or what the model is told.
    /// Before anything is written it's in `Now`, so no sleep starts under it.
    pub async fn start_job(&self, ctx: &HubCtx, live: &Live, thread: &str, name: &str, command: &str, limit: Duration) -> Result<Job, String> {
        let id = format!("j_{}", buwiz_computer::random_hex(8).map_err(|p| p.sentence())?);
        {
            let mut now = self.now();
            if now.sleeping.contains(&live.computer) {
                return Err(Problem::Asleep.sentence());
            }
            if now.jobs.values().filter(|j| j.bot == live.bot).count() >= PER_BOT {
                return Err(TOO_MANY.to_owned());
            }
            now.jobs.insert(id.clone(), Running { bot: live.bot.clone(), computer: live.computer.clone(), thread: thread.to_owned(), starting: true, frozen: false, past: false });
        }
        let at = policy::now(ctx);
        let job = Job { id: id.clone(), bot: live.bot.clone(), computer: live.computer.clone(), thread: thread.to_owned(), name: name.to_owned(), started: at, until: at + super::millis(limit), state: "running".to_owned(), exit: None };
        // the row first: a job with no row is one nothing would ever stop
        if let Err(e) = ctx.db.with(|c| c.execute(&format!("INSERT INTO computer_jobs({COLUMNS}) VALUES(?,?,?,?,?,?,?,'running',NULL)"), params![job.id, job.bot, job.computer, job.thread, job.name, job.started, job.until])) {
            self.now().jobs.remove(&id);
            return Err(db_problem(e).sentence());
        }
        if let Err(p) = launch(live, &id, command).await {
            let why = self.abandon(ctx, live, &id, &p.sentence()).await;
            self.now().jobs.remove(&id);
            self.sync_awake();
            return Err(why);
        }
        if let Some(j) = self.now().jobs.get_mut(&id) {
            j.starting = false;
        }
        self.sync_awake();
        Ok(job)
    }

    /// A start that failed, or that nobody heard the end of: what it sent may still run, so `id` is
    /// cancelled before its row goes. What couldn't be cancelled keeps its row, owing that. →
    /// what the model is told.
    async fn abandon(&self, ctx: &HubCtx, live: &Live, id: &str, why: &str) -> String {
        if cancelled(live.provider.as_ref(), &live.machine, &live.screen, id).await {
            let _ = ctx.db.with(|c| c.execute("DELETE FROM computer_jobs WHERE id=?", [id]));
            return format!("{why} Nothing of it was left running.");
        }
        let at = policy::now(ctx);
        let _ = ctx.db.with(|c| c.execute("UPDATE computer_jobs SET state='lost', ended_at=?, stop_owed=1 WHERE id=?", params![at, id]));
        format!("{why} It may have started all the same, and the computer couldn't be reached to stop it. It'll be stopped as soon as it can be, so don't start it again yet.")
    }

    /// Pays the cancels ended jobs owe, on `computer` or, from the sweep, on every one that's up
    /// and can hear them. A person's click waits on this, so it's only ever the computer they
    /// clicked. Each stays owed until the computer has answered, or [`STOP_TRIES`] times it hasn't.
    pub(super) async fn settle_stops(&self, ctx: &HubCtx, computer: Option<&str>) {
        let owed = match computer {
            Some(id) => rows(ctx, "WHERE stop_owed=1 AND computer_id=?", id),
            None => rows(ctx, "WHERE stop_owed=1 AND state<>?", "running"),
        };
        for j in owed {
            {
                let now = self.now();
                if now.waking.contains(&j.computer) || now.sleeping.contains(&j.computer) {
                    continue;
                }
            }
            let Ok(machine) = self.machine(ctx, &j.computer) else { continue };
            let provider = self.provider_of(&machine);
            let paid = match provider.look(&machine).await {
                // the wall comes before anything runs there, as it does for a tool
                Ok((Power::Awake, machine)) => provider.guard(&machine).await.is_ok() && cancelled(provider.as_ref(), &machine, &folder_of(&j.bot), &j.id).await,
                // nothing of it is left to stop
                Ok((Power::Absent, _)) => true,
                // asleep, or not there to ask: no try was made
                _ => continue,
            };
            if paid {
                let _ = ctx.db.with(|c| c.execute("UPDATE computer_jobs SET stop_owed=0 WHERE id=?", [&j.id]));
                continue;
            }
            let _ = ctx.db.with(|c| c.execute("UPDATE computer_jobs SET stop_tries=stop_tries+1 WHERE id=?", [&j.id]));
            if matches!(ctx.db.with(|c| c.query_row("SELECT stop_tries FROM computer_jobs WHERE id=?", [&j.id], |r| r.get::<_, i64>(0))), Ok(n) if n >= STOP_TRIES) {
                give_up_on(ctx, &j);
            }
        }
    }

    /// `computer` is going for good. What ran on a box goes with it, but a remote one's far end
    /// stays: nothing there is destroyed, so whatever still owes a cancel is told of now, once.
    pub(super) async fn retire_jobs(&self, ctx: &HubCtx, computer: &str, remote: bool) {
        let at = policy::now(ctx);
        let _ = ctx.db.with(|c| c.execute("UPDATE computer_jobs SET state='lost', ended_at=?, stop_owed=1 WHERE state='running' AND computer_id=?", params![at, computer]));
        self.now().jobs.retain(|_, j| j.computer != computer);
        self.sync_awake();
        self.settle_stops(ctx, Some(computer)).await;
        if remote {
            for j in rows(ctx, "WHERE stop_owed=1 AND computer_id=?", computer) {
                give_up_on(ctx, &j);
            }
        }
    }

    /// `bot`'s jobs, oldest first.
    pub fn jobs_of(ctx: &HubCtx, bot: &str) -> Vec<Job> {
        rows(ctx, "WHERE bot_id=?", bot)
    }

    /// `id`, if it's `bot`'s: no model reaches another Bot's job.
    fn job_of(ctx: &HubCtx, bot: &str, id: &str) -> Option<Job> {
        rows(ctx, "WHERE id=?", id).pop().filter(|j| j.bot == bot)
    }

    /// The model's look at its jobs: `id`'s with what it printed, or all of them. A running job is
    /// looked at first, so what's said is the computer's word and not the last sweep's.
    pub async fn job_status(&self, ctx: &HubCtx, live: &Live, id: Option<&str>) -> Result<Value, String> {
        let Some(id) = id else {
            for j in Self::jobs_of(ctx, &live.bot).into_iter().filter(|j| j.state == "running") {
                self.look_at(ctx, live, &j, true).await;
            }
            let jobs: Vec<Value> = Self::jobs_of(ctx, &live.bot).iter().rev().take(LISTED).map(|j| facts(ctx, j)).collect();
            return Ok(json!({"jobs": jobs}));
        };
        let Some(j) = Self::job_of(ctx, &live.bot, id) else { return Err(format!("No job {id} on this computer.")) };
        if j.state == "running" {
            self.look_at(ctx, live, &j, true).await;
        }
        let j = Self::job_of(ctx, &live.bot, id).unwrap_or(j);
        let mut out = facts(ctx, &j);
        out["output"] = json!(command(live, &job::tail(id, OUT_BYTES)).await.map_err(|p| p.sentence())?);
        Ok(out)
    }

    /// The model stops `id`. Stopped is ended: no turn follows, the model is there.
    pub async fn stop_job(&self, ctx: &HubCtx, live: &Live, id: &str) -> Result<Value, String> {
        let Some(j) = Self::job_of(ctx, &live.bot, id) else { return Err(format!("No job {id} on this computer.")) };
        if j.state != "running" {
            return Ok(facts(ctx, &j));
        }
        if !answered(live.provider.as_ref(), &live.machine, &live.screen, &job::stop(id)).await {
            return Err("That job couldn't be stopped: part of it may still be running. Try again in a moment.".to_owned());
        }
        self.end_job(ctx, &j, End::Stopped, true);
        Ok(facts(ctx, &Self::job_of(ctx, &live.bot, id).unwrap_or(j)))
    }

    /// One look at every running job, from the sweep: the ones that ended are told of, the ones past
    /// their limit stopped. What it can't reach it leaves for the next look.
    pub(super) async fn poll_jobs(&self, ctx: &Arc<HubCtx>) {
        let at = policy::now(ctx);
        for j in running(ctx) {
            if self.now().jobs.get(&j.id).is_some_and(|r| r.starting) {
                continue;
            }
            let over = at >= j.until;
            match self.live(ctx, &j.bot).await {
                Ok(live) if live.computer == j.computer => {
                    self.set_frozen(&j.id, false);
                    if self.look_at(ctx, &live, &j, false).await || !over {
                        continue;
                    }
                    if answered(live.provider.as_ref(), &live.machine, &live.screen, &job::stop(&j.id)).await {
                        self.end_job(ctx, &j, End::Limit, false);
                    } else {
                        self.stop_failed(ctx, &j);
                    }
                }
                // nothing it ran is left. A record that can't be read says Gone too, and says
                // nothing of the box: that's for the next look
                Err(Problem::Gone) if self.machine(ctx, &j.computer).is_ok() => self.end_job(ctx, &j, End::Lost, false),
                // a limit is for what can be reached: frozen it stays, and lets the computer go
                Err(Problem::Asleep) => {
                    self.set_frozen(&j.id, true);
                    if over {
                        if let Some(r) = self.now().jobs.get_mut(&j.id) {
                            r.past = true;
                        }
                    }
                }
                _ if over => self.stop_failed(ctx, &j),
                _ => {}
            }
        }
        self.settle_stops(ctx, None).await;
        self.start_owed_turns(ctx);
        self.sync_awake();
    }

    /// `computer` was stopped for good: what was past its limit there ended with it, and that's what
    /// a person hears, not that it went missing. A remote one only hung up: what's there runs on.
    pub(super) fn deep_sleep_ended(&self, ctx: &HubCtx, computer: &str) {
        if !matches!(Self::is_remote(ctx, computer), Ok(false)) {
            return;
        }
        let at = policy::now(ctx);
        for j in rows(ctx, "WHERE state='running' AND computer_id=?", computer).into_iter().filter(|j| at >= j.until) {
            self.end_job(ctx, &j, End::Limit, false);
        }
    }

    /// `j` is past its limit and still there. It stays a job, and the next look tries again. Past
    /// [`STOP_TRIES`] of these it stops holding anything up, and a person hears, once.
    pub(super) fn stop_failed(&self, ctx: &HubCtx, j: &Job) {
        let _ = ctx.db.with(|c| c.execute("UPDATE computer_jobs SET stop_tries=stop_tries+1 WHERE id=?", [&j.id]));
        // not knowing how many isn't having had enough: it stays as it is until the next look
        let Ok(tries) = ctx.db.with(|c| c.query_row("SELECT stop_tries FROM computer_jobs WHERE id=?", [&j.id], |r| r.get::<_, i64>(0))) else { return };
        if let Some(r) = self.now().jobs.get_mut(&j.id) {
            r.past = tries >= STOP_TRIES;
        }
        if tries == STOP_TRIES {
            warn(ctx, j);
        }
    }

    /// Asks the computer how `j` stands, and ends it if it has: whether it did. `by_model` is a
    /// status call, whose turn hears of it there. A job still starting has nothing to see yet, and
    /// an empty folder isn't an end.
    async fn look_at(&self, ctx: &HubCtx, live: &Live, j: &Job, by_model: bool) -> bool {
        if self.now().jobs.get(&j.id).is_some_and(|r| r.starting) {
            return false;
        }
        let seen = command(live, &job::look(&j.id)).await.ok().and_then(|out| job::read_look(&out));
        let end = match seen {
            Some(Seen::Exited(code)) => End::Exited(code),
            Some(Seen::Gone) => End::Lost,
            Some(Seen::Running) | None => return false,
        };
        self.end_job(ctx, j, end, by_model);
        true
    }

    fn set_frozen(&self, id: &str, frozen: bool) {
        if let Some(j) = self.now().jobs.get_mut(id) {
            j.frozen = frozen;
        }
    }

    /// The computer slept or woke: its jobs stopped where they were, or go on.
    pub(super) fn freeze_jobs(&self, computer: &str, frozen: bool) {
        self.now().jobs.values_mut().filter(|j| j.computer == computer).for_each(|j| j.frozen = frozen);
        self.sync_awake();
    }

    /// `j` is over. Only the first to say so posts. Its computer gets the time an idle one gets, for
    /// whoever comes to read what it left.
    pub(super) fn end_job(&self, ctx: &HubCtx, j: &Job, end: End, by_model: bool) {
        let exit = if let End::Exited(code) = end { Some(code) } else { None };
        let owed = i64::from(!by_model && end != End::Stopped);
        let at = policy::now(ctx);
        let won = ctx.db.with(|c| c.execute("UPDATE computer_jobs SET state=?, exit_code=?, ended_at=?, turn_owed=? WHERE id=? AND state='running'", params![end.word(), exit, at, owed, j.id]));
        if matches!(won, Ok(1)) {
            Self::touch(ctx, &j.computer);
            let n = json!({"kind": NOTICE, "id": j.id, "bot": j.bot, "name": j.name, "state": end.word(), "exit": exit});
            post_in(ctx, &j.thread, &n);
        }
        self.now().jobs.remove(&j.id);
        self.sync_awake();
    }

    /// The turns ended jobs owe, one each, in chats that aren't busy. Off, it's forgotten.
    fn start_owed_turns(&self, ctx: &Arc<HubCtx>) {
        for j in rows(ctx, "WHERE turn_owed=1 AND state<>?", "running") {
            let on = policy::tool_state(ctx, &j.bot, ROUTINES_KEY) == Pos::On;
            // the chat is this turn's from here: a person's message or a yes can't start beside it
            let job = if on { ctx.jobs.create_if_idle(&j.thread) } else { None };
            if on && job.is_none() {
                continue;
            }
            // two can get here at once: the one that clears it starts the turn
            let won = matches!(ctx.db.with(|c| c.execute("UPDATE computer_jobs SET turn_owed=0 WHERE id=? AND turn_owed=1", [&j.id])), Ok(1));
            let (Some(job), Some(runner), true) = (job, ctx.turns.clone(), won) else {
                if let Some(job) = job {
                    ctx.jobs.with(job, |r| r.done = true);
                }
                continue;
            };
            let turn = TurnCtx { thread: j.thread.clone(), job: Some(job), routine: Some(format!("job:{}", j.id)), ..TurnCtx::default() };
            // its name is the model's, perhaps from a page: nobody is there to read this turn
            let ended = said("", &j.state, j.exit.map(i64::from));
            let prompt = format!("(A job you left running on your computer ended, and the user isn't waiting on this message: {ended} Its id is {}. Read how it went with computer_job_status, do what it was for, and report briefly.)", j.id);
            let req = TurnRequest { turn, shown: format!("Job · {ended}"), prompt, mark: None };
            let ctx = ctx.clone();
            tokio::spawn(async move {
                if let Err(e) = runner.run(ctx.clone(), req).await {
                    crate::hub::agent::runs::failed(&ctx, job, &e);
                }
            });
        }
    }

    /// `bot` left: nothing would look after its jobs now, so they end, and each owes a cancel. Who
    /// pays it is the caller's to say: this waits on no computer.
    pub(super) fn end_jobs_of(&self, ctx: &HubCtx, bot: &str) {
        for j in Self::jobs_of(ctx, bot).into_iter().filter(|j| j.state == "running") {
            self.end_job(ctx, &j, End::Left, true);
            let _ = ctx.db.with(|c| c.execute("UPDATE computer_jobs SET stop_owed=1 WHERE id=?", [&j.id]));
        }
    }

    /// `computer` has gone: its rows go with it.
    pub(super) fn forget_jobs(ctx: &HubCtx, computer: &str) {
        let _ = ctx.db.with(|c| c.execute("DELETE FROM computer_jobs WHERE computer_id=?", [computer]));
    }

    /// The jobs on `computer`, for the Computer tab: running, or frozen while it sleeps.
    pub(super) fn tab_jobs(&self, ctx: &HubCtx, computer: &str) -> Vec<Value> {
        let at = policy::now(ctx);
        let found = rows(ctx, "WHERE state='running' AND computer_id=?", computer);
        let shown: Vec<(Job, bool)> = {
            let now = self.now();
            found.into_iter().filter_map(|j| now.jobs.get(&j.id).filter(|r| !r.starting).map(|r| r.frozen).map(|frozen| (j, frozen))).collect()
        };
        shown.iter().map(|(j, frozen)| json!({"name": j.name, "bot": super::bot_name(ctx, &j.bot), "frozen": frozen, "secondsLeft": (j.until - at).max(0) / 1000})).collect()
    }

    /// What holds the Mac up follows the jobs: on while one runs, off when none does. Two of these
    /// can land out of order: the sweep's, each tick, puts that right.
    fn sync_awake(&self) {
        let wanted = self.now().jobs.values().any(|j| !j.frozen && !j.past);
        self.keep_awake.set(wanted);
    }

    /// This broker is on its way out: nothing of it stays.
    pub fn let_the_mac_sleep(&self) {
        self.keep_awake.set(false);
    }

    /// The chats a job is running for right now, for `GET /threads`. A frozen one isn't running.
    pub fn job_threads(&self) -> Vec<String> {
        self.now().jobs.values().filter(|j| !j.frozen).map(|j| j.thread.clone()).collect()
    }
}

/// `j`'s cancel won't be paid, and a person hears so once: in its chat, or if that went with its
/// Bot, in Activity.
fn give_up_on(ctx: &HubCtx, j: &Job) {
    if !matches!(ctx.db.with(|c| c.execute("UPDATE computer_jobs SET stop_owed=0 WHERE id=? AND stop_owed=1", [&j.id])), Ok(1)) {
        return;
    }
    warn(ctx, j);
}

/// A person hears that `j` may still be running.
fn warn(ctx: &HubCtx, j: &Job) {
    let n = json!({"kind": NOTICE, "id": j.id, "bot": j.bot, "name": j.name, "state": UNSTOPPED, "exit": Value::Null});
    if !post_in(ctx, &j.thread, &n) {
        policy::log_activity(ctx, None, &said(&j.name, UNSTOPPED, None), "", UNSTOPPED, false);
    }
}

/// What the model reads of a job.
fn facts(ctx: &HubCtx, j: &Job) -> Value {
    let mut out = json!({"id": j.id, "state": j.state, "folder": format!("{}/{}", job::FOLDER, j.id)});
    if !j.name.is_empty() {
        out["name"] = json!(j.name);
    }
    if let Some(code) = j.exit {
        out["exit_code"] = json!(code);
    }
    if j.state == "running" {
        out["seconds_left"] = json!((j.until - policy::now(ctx)).max(0) / 1000);
    }
    out
}

/// A Bot's place by its folder alone, for one with no seat or no screen: commands need no more.
fn folder_of(bot: &str) -> Screen {
    Screen { display: String::new(), viewer_port: 0, folder: format!("agents/{bot}"), profile: None }
}

/// Whether the computer ran `line` and said it worked.
async fn answered(provider: &dyn Provider, machine: &Machine, screen: &Screen, line: &str) -> bool {
    matches!(provider.run(machine, screen, line, LOOK_MS).await, Ok(out) if out.exit_code == Some(0))
}

/// Whether the computer said `id` is cancelled.
async fn cancelled(provider: &dyn Provider, machine: &Machine, screen: &Screen, id: &str) -> bool {
    answered(provider, machine, screen, &job::cancel(id)).await
}

/// Writes `run.sh` and starts it. Its folder is the Bot's own, links followed, as any file's is.
async fn launch(live: &Live, id: &str, line: &str) -> Result<(), Problem> {
    let Some(path) = inside(live, &job::script_path(id)).await? else { return Err(Problem::Failed("the jobs folder leaves this Bot's own".to_owned())) };
    live.provider.write_file(&live.machine, &path, job::script(id, line).as_bytes()).await?;
    let out = command(live, &job::start(id)).await?;
    if out.trim().parse::<u32>().is_err() {
        return Err(Problem::Failed("the job didn't start".to_owned()));
    }
    Ok(())
}

/// A job's name, or none: a few words of the model's.
pub fn name_of(input: &Value) -> String {
    let name = input.get("name").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    crate::js::slice_utf16(name, MAX_NAME).to_owned()
}
