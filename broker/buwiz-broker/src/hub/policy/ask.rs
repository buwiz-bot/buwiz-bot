//! Asking before acting (PR 2): a call that changes something or reaches someone, on a
//! switch at Ask (or any such call after the turn read outside content), waits for the
//! user's answer on a `tool_ask` card in the chat.
//!
//! - [`ask`] posts the notice `{kind: "tool_ask", id, bot, botName, chat, key, tool, title,
//!   lines, preview, external?, prefix?, state: "pending", at}` and parks the run on a
//!   oneshot in [`waiting`] until `POST /v1/host/tools/ask` ([`host_answer`]) answers it, for
//!   at most 10 minutes. The job's status says "Waiting for you".
//! - Answers: `once` runs this call; `chat` runs it and grants the key in this thread for
//!   the rest of the chat (`ask_grant`); `rule` (the shell only) saves an allow rule for the
//!   card's `prefix`, then runs it; `no` doesn't run it. The notice becomes `allowed`,
//!   `declined` or `expired`; Activity keeps each answer.
//! - On start, a card left pending by a previous run expires.
//!
//! A card outlives its run. With no one to ask (a routine's turn, a v1 surface's, or the app
//! away, [`UiLink::connected`](crate::hub::tools::ui::UiLink::connected)), or when the 10 minutes
//! run out, it becomes `waiting`: nothing is parked, so no model money goes on it. The model is
//! told the call hasn't run ([`WAITS`]) and its turn ends in the usual way.
//!
//! - A waiting card is answered `once` or `no`, and a yes grants nothing that lasts. `no` starts
//!   nothing. `once` starts a new turn in that chat, the person's own, with a [`Yes`] on it: that
//!   tool with that exact input, once, kept in that turn alone and nowhere a model can reach. It
//!   goes on the first call it covers, whichever way that call goes. The call still goes through
//!   every switch and the shell's gate, so a yes passes a card and no more.
//!   `computer_ask_person` is bound by the tool alone, and the hand-over uses the reason and time
//!   limit the person read, whatever the model sends: its other checks run fresh. A yes with
//!   none of that kept hands nothing over.
//! - The call itself is in `ask_waiting`, with the routine that made it. It survives a restart
//!   and expires after [`KEEP`]. A call holding a secret ([`tool_log::holds_secret`]) isn't
//!   kept: it's refused as before.
//! - A routine with a waiting card doesn't run ([`routine_waits`]): a 60-second one would stack
//!   up cards and spend. It has one such card at a time: a later call in that turn is told so
//!   ([`ONE_CARD`]).
//! - An agent's request never waits: its call is refused and kept in Activity as `needs_ok`. A
//!   yes runs as the person, and the request came from someone else, perhaps outside the Bot.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::{db, gate, log_ask, now, shell, Change, DAY};
use crate::hub::agent::tool_log;
use crate::hub::ctx::HubCtx;
use crate::hub::store::uuidv7;
use crate::hub::tools::computer::ASK_PERSON;
use crate::hub::tools::Run;
use crate::hub::traits::TurnRequest;
use crate::hub::types::TurnCtx;
use crate::js;
use crate::v1::V1Error;

/// How long a card waits for its answer.
pub const WAIT: Duration = Duration::from_secs(600);

/// How long a waiting card can be answered, in ms: a week away.
pub const KEEP: i64 = 7 * DAY;
/// A waiting card's row follows it at once: one with none this long after, in ms, never had it.
const CARD_TO_ROW: i64 = 60_000;
/// How often the broker looks for a waiting card past [`KEEP`], in ms.
const SWEEP: u64 = 3_600_000;

/// Another wait than [`WAIT`] (tests), in `ctx.ext`.
pub struct AskWait(pub Duration);

pub const NOT_NOW: &str = "The user chose Not now. Don't retry this call; ask them in words if you still need it.";
pub const NO_ANSWER: &str = "No answer, so this didn't run.";
pub const NOBODY: &str = "This needs the user's OK, and they weren't there.";
pub const WAITING: &str = "Waiting for you";
/// What the model is told of a call left on a waiting card.
pub const WAITS: &str = "This hasn't run. It's waiting on a card in this chat for the user's OK, and they may not answer for a long time. Don't call it again. Finish what you can without it, then say what's waiting.";
/// What the model is told of a routine's call when a card of that routine's already waits.
pub const ONE_CARD: &str = "This hasn't run, and no card was made for it: a card from this routine is already waiting for the user's OK. Don't call it again. Finish what you can without it, then say what's waiting.";
const WAITING_ANSWERS: &str = "a waiting card is answered \"once\" or \"no\"";
const BUSY: &str = "This chat is still working. Answer again when it's done.";

/// The user's answer on a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Once,
    Chat,
    Rule,
    No,
}

impl Answer {
    pub fn parse(s: &str) -> Option<Answer> {
        match s {
            "once" => Some(Answer::Once),
            "chat" => Some(Answer::Chat),
            "rule" => Some(Answer::Rule),
            "no" => Some(Answer::No),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Answer::Once => "once",
            Answer::Chat => "chat",
            Answer::Rule => "rule",
            Answer::No => "no",
        }
    }

    /// Its Activity outcome.
    fn outcome(self) -> &'static str {
        match self {
            Answer::Once | Answer::Rule => "allowed",
            Answer::Chat => "allowed_chat",
            Answer::No => "declined",
        }
    }
}

type Waiting = HashMap<String, oneshot::Sender<Answer>>;

/// The parked calls by card id (ids are unique across hubs, so one map serves them all).
fn waiting() -> MutexGuard<'static, Waiting> {
    static W: OnceLock<Mutex<Waiting>> = OnceLock::new();
    W.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// What a card asks.
#[derive(Clone, Debug, Default)]
pub struct Asking {
    /// the acting Bot (`""` in a plain chat)
    pub bot: String,
    pub key: String,
    pub tool: String,
    /// the call's exact input, what a yes on a waiting card is bound to
    pub input: Option<Value>,
    /// what will happen: "Send a message to Sam?"
    pub title: String,
    /// short facts under it ("Subject: …")
    pub lines: Vec<String>,
    /// the exact payload
    pub preview: String,
    /// the turn read outside content (the guard asks even with the switch on)
    pub external: bool,
    /// the shell's "Always allow `<prefix>`"
    pub prefix: Option<String>,
}

/// What Activity calls an asked call: the command for the shell, else the card's title.
fn shown<'a>(tool: &str, title: &'a str, preview: &'a str) -> &'a str {
    if tool == "shell_run" && !preview.is_empty() {
        preview
    } else {
        title
    }
}

/// Whether someone can answer a card in this turn: not a routine's, a v1 surface's or an
/// agent's request.
pub fn present(turn: &TurnCtx) -> bool {
    turn.routine.is_none() && turn.surface.is_none() && turn.acting_bot.is_none()
}

/// Whether the user allowed `key` for the rest of chat `thread`.
pub fn granted(ctx: &HubCtx, thread: &str, key: &str) -> bool {
    db(ctx, |c| c.query_row("SELECT 1 FROM ask_grant WHERE thread=? AND key=?", [thread, key], |_| Ok(())).optional()).ok().flatten().is_some()
}

fn grant(ctx: &HubCtx, thread: &str, key: &str) -> Result<(), String> {
    db(ctx, |c| c.execute("INSERT OR REPLACE INTO ask_grant(thread, key, at) VALUES(?,?,?)", rusqlite::params![thread, key, now(ctx)])).map(|_| ())
}

/// Every `tool_ask` notice `pick` takes (in `chat`, or every chat) gets `f`.
fn edit(ctx: &HubCtx, chat: Option<&str>, pick: impl Fn(&Value) -> bool, f: impl Fn(&mut Value)) -> usize {
    let n = ctx
        .store
        .write(|s| {
            let chats: Vec<String> = match chat {
                Some(c) => s.thread(c).map(|t| vec![t.id.clone()]).unwrap_or_default(),
                None => s.list.iter().map(|t| t.id.clone()).collect(),
            };
            let mut n = 0;
            for c in chats {
                let Some(t) = s.thread_mut(&c) else { continue };
                for u in &mut t.turns {
                    if let Some(x) = u.notice.as_mut().filter(|x| x["kind"] == "tool_ask" && pick(x)) {
                        f(x);
                        n += 1;
                    }
                }
            }
            n
        })
        .unwrap_or(0);
    if n > 0 {
        ctx.store.write_index();
    }
    n
}

fn find(ctx: &HubCtx, chat: &str, id: &str) -> Option<Value> {
    ctx.store.read(|s| s.thread(chat).and_then(|t| t.turns.iter().filter_map(|u| u.notice.as_ref()).find(|n| n["kind"] == "tool_ask" && n["id"] == id).cloned()))
}

fn settle(ctx: &HubCtx, chat: &str, id: &str, state: &str, answer: Option<Answer>) -> Option<Value> {
    let at = now(ctx);
    edit(ctx, Some(chat), |n| n["id"] == id, |n| {
        n["state"] = json!(state);
        n["at"] = json!(at);
        if let Some(a) = answer {
            n["answer"] = json!(a.as_str());
        }
    });
    find(ctx, chat, id)
}

/// On start: a card a previous run left pending can't be answered now.
pub fn expire_pending(ctx: &HubCtx) {
    let at = now(ctx);
    edit(ctx, None, |n| n["state"] == "pending", |n| {
        n["state"] = json!("expired");
        n["at"] = json!(at);
    });
}

/// A waiting card's call, as `ask_waiting` keeps it.
struct Kept {
    id: String,
    chat: String,
    tool: String,
    input: Option<Value>,
    at: i64,
}

fn kept(ctx: &HubCtx, filter: &str, value: &str) -> Vec<Kept> {
    let rows = db(ctx, |c| {
        let mut q = c.prepare(&format!("SELECT id, chat, tool, input, at FROM ask_waiting {filter}"))?;
        let bound: &[&dyn rusqlite::ToSql] = if filter.is_empty() { &[] } else { &[&value] };
        let r = q.query_map(bound, |r| Ok(Kept { id: r.get(0)?, chat: r.get(1)?, tool: r.get(2)?, input: r.get::<_, Option<String>>(3)?.and_then(|i| serde_json::from_str(&i).ok()), at: r.get(4)? }))?;
        r.collect::<rusqlite::Result<Vec<_>>>()
    });
    rows.unwrap_or_default()
}

fn forget(ctx: &HubCtx, id: &str) {
    if let Err(e) = db(ctx, |c| c.execute("DELETE FROM ask_waiting WHERE id=?", [id])) {
        eprintln!("buwiz-broker: ask_waiting: {e}");
    }
}

/// The routine a turn runs for, as [`routine_waits`] asks after it: a v1 routine by its instance.
fn routine_of(turn: &TurnCtx) -> Option<String> {
    turn.surface.as_ref().and_then(|s| s["instance"].as_str()).map(str::to_owned).or_else(|| turn.routine.clone())
}

/// What a person's yes on a waiting card covers: that tool with exactly that input, once. A
/// hand-over's is bound by the tool alone, and its input is what its card said.
///
/// Nothing outside this module can make one. Not by its fields:
///
/// ```compile_fail
/// let _ = buwiz_broker::hub::policy::ask::Yes { tool: String::new(), input: None };
/// ```
///
/// and not by its constructor:
///
/// ```compile_fail
/// let _ = buwiz_broker::hub::policy::ask::Yes::new(String::new(), None);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Yes {
    tool: String,
    input: Option<Value>,
}

impl Yes {
    fn new(tool: String, input: Option<Value>) -> Yes {
        Yes { tool, input }
    }

    fn covers(&self, tool: &str, input: Option<&Value>) -> bool {
        self.tool == tool && (tool == ASK_PERSON || self.input.as_ref() == input)
    }
}

/// A test's yes, as if a person had answered.
#[cfg(test)]
pub(crate) fn yes_for_test(tool: &str, input: Value) -> Yes {
    Yes::new(tool.to_owned(), Some(input))
}

/// A test's yes with nothing kept of its card.
#[cfg(test)]
pub(crate) fn bare_yes_for_test(tool: &str) -> Yes {
    Yes::new(tool.to_owned(), None)
}

/// Between two steps that have to hold together (tests), in `ctx.ext`: it runs as if another
/// thread got in right there.
#[cfg(test)]
pub struct Between(pub Arc<dyn Fn(&Arc<HubCtx>) + Send + Sync>);

#[cfg(test)]
fn between(ctx: &Arc<HubCtx>) {
    if let Some(b) = ctx.ext.get::<Between>() {
        (b.0)(ctx);
    }
}

#[cfg(not(test))]
fn between(_: &Arc<HubCtx>) {}

/// The call `tool` with `input` is starting: the yes it's covered by goes now, whatever becomes
/// of the call. It's in hand until the call ends.
pub fn spend(run: &Run, tool: &str, input: &Value) {
    let mut state = run.state();
    if state.yes.as_ref().is_some_and(|y| y.covers(tool, Some(input))) {
        state.yes_in_hand = state.yes.take();
    }
}

/// What the card this call's yes answered asked for.
pub fn yes_terms(run: &Run) -> Option<Value> {
    run.state().yes_in_hand.as_ref().and_then(|y| y.input.clone())
}

/// Whether this call's yes kept nothing of what its card asked. A hand-over's terms are the
/// person's: without them there's nothing they said yes to.
pub fn yes_lost_its_terms(run: &Run) -> bool {
    run.state().yes_in_hand.as_ref().is_some_and(|y| y.input.is_none())
}

/// Whether a card's call can be kept for its answer.
fn keepable(a: &Asking) -> bool {
    !a.input.as_ref().is_some_and(tool_log::holds_secret)
}

/// Keeps card `id`'s call for its answer, the card being there already: a row with no waiting
/// card is one [`expire_waiting`] clears. → false when it can't be kept.
fn keep(run: &Run, a: &Asking, chat: &str, id: &str) -> bool {
    let input = a.input.as_ref().map(crate::js::stringify);
    let at = now(&run.ctx);
    let row = db(&run.ctx, |c| c.execute("INSERT INTO ask_waiting(id, chat, tool, input, routine, at) VALUES(?,?,?,?,?,?)", rusqlite::params![id, chat, a.tool, input, routine_of(&run.turn), at]));
    row.is_ok()
}

/// Past [`KEEP`], a waiting card expires; a row whose card went with its chat goes too.
pub fn expire_waiting(ctx: &HubCtx) {
    let oldest = now(ctx) - KEEP;
    for k in kept(ctx, "", "") {
        let live = find(ctx, &k.chat, &k.id).filter(|n| n["state"] == "waiting");
        if k.at > oldest && live.is_some() {
            continue;
        }
        if let Some(n) = live {
            settle(ctx, &k.chat, &k.id, "expired", None);
            log_ask(ctx, n["bot"].as_str().filter(|b| !b.is_empty()), shown(&k.tool, n["title"].as_str().unwrap_or(""), n["preview"].as_str().unwrap_or("")), &k.tool, "expired");
        }
        forget(ctx, &k.id);
    }
    clear_stranded(ctx);
}

/// A broker that stopped between a card and its row left a card nothing can answer: it expires.
/// Only one older than [`CARD_TO_ROW`], so a card just posted isn't taken for one. A failed read of
/// the rows clears nothing.
fn clear_stranded(ctx: &HubCtx) {
    let rows = db(ctx, |c| {
        let mut q = c.prepare("SELECT id FROM ask_waiting")?;
        let r = q.query_map([], |r| r.get::<_, String>(0))?;
        r.collect::<rusqlite::Result<std::collections::HashSet<_>>>()
    });
    let Ok(rows) = rows else { return };
    let at = now(ctx);
    let stranded = |n: &Value| n["kind"] == "tool_ask" && n["state"] == "waiting" && n["at"].as_i64().is_some_and(|t| at - t >= CARD_TO_ROW) && !n["id"].as_str().is_some_and(|id| rows.contains(id));
    let found: Vec<Value> = ctx.store.read(|s| s.list.iter().flat_map(|t| t.turns.iter()).filter_map(|u| u.notice.clone()).filter(|n| stranded(n)).collect());
    if found.is_empty() {
        return;
    }
    edit(ctx, None, stranded, |n| {
        n["state"] = json!("expired");
        n["at"] = json!(at);
    });
    for n in found {
        let tool = js::string_of(&n["tool"]);
        log_ask(ctx, n["bot"].as_str().filter(|b| !b.is_empty()), shown(&tool, n["title"].as_str().unwrap_or(""), n["preview"].as_str().unwrap_or("")), &tool, "expired");
    }
}

/// On start and every [`SWEEP`] after, so a card nobody comes back to doesn't say it still waits.
pub fn sweep_waiting(ctx: Arc<HubCtx>) {
    expire_waiting(&ctx);
    let next = ctx.clone();
    ctx.clock.set_timeout(SWEEP, Box::new(move || sweep_waiting(next)));
}

/// Whether `routine` (a v0 routine's id, a v1 instance's) has a card waiting for an answer.
pub fn routine_waits(ctx: &HubCtx, routine: &str) -> bool {
    expire_waiting(ctx);
    !kept(ctx, "WHERE routine=?", routine).is_empty()
}

/// The chats with a card waiting for an answer.
pub fn waiting_chats(ctx: &HubCtx) -> std::collections::HashSet<String> {
    let oldest = now(ctx) - KEEP;
    kept(ctx, "", "").into_iter().filter(|k| k.at > oldest).map(|k| k.chat).collect()
}

/// Leaves the call on a waiting card, parking nothing: → what the model is told.
fn wait(run: &Run, a: &Asking) -> String {
    let ctx = &run.ctx;
    let chat = run.thread_id();
    let bot = a.bot.as_str();
    let bot = (!bot.is_empty()).then_some(bot);
    // the same call again isn't another question
    let same = |k: &Kept| k.tool == a.tool && (a.tool == ASK_PERSON || k.input == a.input);
    if kept(ctx, "WHERE chat=?", &chat).iter().any(|k| same(k) && find(ctx, &chat, &k.id).is_some_and(|n| n["state"] == "waiting")) {
        return WAITS.to_owned();
    }
    let what = shown(&a.tool, &a.title, &a.preview);
    if routine_of(&run.turn).is_some_and(|r| routine_waits(ctx, &r)) {
        return ONE_CARD.to_owned();
    }
    log_ask(ctx, bot, what, &a.tool, "needs_ok");
    if !keepable(a) {
        return NOBODY.to_owned();
    }
    let (id, n) = card(ctx, a, &chat, "waiting");
    // an answer waits for this lock, so it finds the card and its row together
    let _together = waiting();
    if !crate::hub::tools::miniapps::post_in(ctx, &chat, &n) {
        return NOBODY.to_owned();
    }
    between(ctx);
    if !keep(run, a, &chat, &id) {
        settle(ctx, &chat, &id, "expired", None);
        return NOBODY.to_owned();
    }
    WAITS.to_owned()
}

/// A new card for `a` in `chat`: its id and notice.
fn card(ctx: &HubCtx, a: &Asking, chat: &str, state: &str) -> (String, Value) {
    let id = format!("a_{}", uuidv7());
    let mut n = json!({
        "kind": "tool_ask", "id": id, "bot": a.bot, "botName": super::bot_name(ctx, &a.bot), "chat": chat, "key": a.key, "tool": a.tool,
        "title": a.title, "lines": a.lines, "preview": a.preview, "state": state, "at": now(ctx),
    });
    if a.external {
        n["external"] = json!(true);
    }
    if let Some(p) = &a.prefix {
        n["prefix"] = json!(p);
    }
    (id, n)
}

/// Whether the yes in hand covers this card. One card takes it.
fn covered(run: &Run, a: &Asking) -> bool {
    run.state().yes_in_hand.take().is_some_and(|y| y.covers(&a.tool, a.input.as_ref()))
}

/// A parked call's card: on drop (a timeout, a stopped turn) one nobody answered expires.
struct Parked {
    ctx: Arc<HubCtx>,
    chat: String,
    id: String,
    bot: Option<String>,
    /// what Activity calls it ([`shown`])
    what: String,
    tool: String,
}

impl Drop for Parked {
    fn drop(&mut self) {
        if waiting().remove(&self.id).is_some() {
            settle(&self.ctx, &self.chat, &self.id, "expired", None);
            log_ask(&self.ctx, self.bot.as_deref(), &self.what, &self.tool, "expired");
        }
    }
}

/// Asks the user in this turn's chat and waits, or leaves the card waiting for them: → the
/// answer that lets the call run, or what the model is told instead.
pub async fn ask(run: &Run, a: Asking) -> Result<Answer, String> {
    let ctx = &run.ctx;
    if covered(run, &a) {
        return Ok(Answer::Once);
    }
    let bot = (!a.bot.is_empty()).then(|| a.bot.clone());
    if run.turn.acting_bot.is_some() {
        log_ask(ctx, bot.as_deref(), shown(&a.tool, &a.title, &a.preview), &a.tool, "needs_ok");
        return Err(NOBODY.into());
    }
    if !present(&run.turn) || !ctx.ui.connected() {
        return Err(wait(run, &a));
    }
    let chat = run.thread_id();
    let (id, n) = card(ctx, &a, &chat, "pending");
    let (tx, mut rx) = oneshot::channel();
    waiting().insert(id.clone(), tx);
    let parked = Parked { ctx: ctx.clone(), chat: chat.clone(), id: id.clone(), bot, what: shown(&a.tool, &a.title, &a.preview).to_owned(), tool: a.tool.clone() };
    if !crate::hub::tools::miniapps::post_in(ctx, &chat, &n) {
        waiting().remove(&id);
        return Err("There is no chat to ask the user in, so this didn't run.".into());
    }
    run.set_job("waiting", WAITING);
    let wait = ctx.ext.get::<AskWait>().map_or(WAIT, |w| w.0);
    let mut left = false;
    let answer = match tokio::time::timeout(wait, &mut rx).await {
        Ok(r) => r.ok(),
        // the answer that came as the wait ran out still counts
        Err(_) => match leave_waiting(run, &a, &chat, &id) {
            Left::Answered => rx.await.ok(),
            Left::Waiting => {
                left = true;
                None
            }
            Left::Expires => None,
        },
    };
    drop(parked);
    let (p, d) = crate::hub::tools::activity::tool_activity(ctx, &a.tool, &json!({}));
    run.set_job(&p, &d);
    match answer {
        Some(Answer::No) => Err(NOT_NOW.into()),
        Some(a) => Ok(a),
        None if left => Err(WAITS.into()),
        None => Err(NO_ANSWER.into()),
    }
}

/// What became of a card whose 10 minutes ran out.
enum Left {
    /// An answer got there first.
    Answered,
    Waiting,
    /// Its call can't be kept, so it stays parked for `Parked` to expire.
    Expires,
}

/// Turns the card a parked call gave up on into a waiting one. The map's lock is held across it,
/// so an answer finds it either parked or waiting.
fn leave_waiting(run: &Run, a: &Asking, chat: &str, id: &str) -> Left {
    let mut parked = waiting();
    if !parked.contains_key(id) {
        return Left::Answered;
    }
    if !keepable(a) {
        return Left::Expires;
    }
    // the card first: a row with no waiting card is one a sweep clears
    settle(&run.ctx, chat, id, "waiting", None);
    between(&run.ctx);
    if !keep(run, a, chat, id) {
        return Left::Expires;
    }
    parked.remove(id);
    log_ask(&run.ctx, (!a.bot.is_empty()).then_some(a.bot.as_str()), shown(&a.tool, &a.title, &a.preview), &a.tool, "needs_ok");
    Left::Waiting
}

/// The shell card's "Always allow `<prefix>`": the program, with its subcommand for a tool
/// that has them (`git push`); none for a line of several commands or one that can never be
/// allowed.
pub fn prefix_of(ctx: &HubCtx, line: &str) -> Option<String> {
    const SUBCOMMANDS: &[&str] = &["git", "cargo", "npm", "pnpm", "yarn", "bun", "go", "swift", "docker", "kubectl", "brew", "gh", "pip", "pip3", "uv"];
    let parts = shell::read(line).simple_commands;
    let [one] = parts.as_slice() else { return None };
    let words = shell::read(one).plain?;
    let prog = words.first()?.clone();
    let sub = words.get(1).filter(|w| SUBCOMMANDS.contains(&prog.as_str()) && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    let p = match sub {
        Some(s) => format!("{prog} {s}"),
        None => prog,
    };
    match gate::risk(&p, &ctx.data) {
        gate::Risk::Never(_) => None,
        _ => Some(p),
    }
}

/// What the model reads in the turn a yes starts.
fn yes_prompt(tool: &str, input: Option<&Value>) -> String {
    if tool == ASK_PERSON {
        return format!("(The user answered a waiting card in this chat: yes, they'll take over your screen for what the card asked, and for as long as it said. Get the screen ready, then call {tool} once. What you pass it doesn't change that. Then finish what it was for, and say what happened.)");
    }
    let input = input.map(crate::js::stringify).unwrap_or_default();
    format!("(The user answered a waiting card in this chat: yes to one call, which hasn't run. Call {tool} now, with exactly this input, changing nothing:\n{input}\nThat yes covers that one call, once. Anything else still asks. Then finish what it was for, and say what happened.)")
}

/// Starts the turn a yes on card `n` asks for as job `job`: the person's own, in `thread`.
fn start_yes(ctx: &Arc<HubCtx>, runner: Arc<dyn crate::hub::traits::TurnRunner>, job: u64, thread: &str, n: &Value, k: Kept) {
    let prompt = yes_prompt(&k.tool, k.input.as_ref());
    let yes = Yes::new(k.tool, k.input);
    let turn = TurnCtx { thread: thread.to_owned(), job: Some(job), yes: Some(yes), ..TurnCtx::default() };
    let req = TurnRequest { turn, shown: format!("Yes: {}", js::string_of(&n["title"])), prompt, mark: None };
    let ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = runner.run(ctx.clone(), req).await {
            crate::hub::agent::runs::failed(&ctx, job, &e);
        }
    });
}

/// The answer on a waiting card. Its run is gone: `no` is the end of it, and a yes starts one.
fn answer_waiting(ctx: &Arc<HubCtx>, chat: &str, id: &str, answer: Answer) -> Result<Value, V1Error> {
    if !matches!(answer, Answer::Once | Answer::No) {
        return V1Error::err("invalid_args", WAITING_ANSWERS);
    }
    // a card just left waiting has its row by the time this lock is free
    drop(waiting());
    expire_waiting(ctx);
    let stale = || V1Error::new("stale", NO_ANSWER).with_details(json!({"notice": find(ctx, chat, id)}));
    let Some(k) = kept(ctx, "WHERE id=?", id).pop() else { return Err(stale()) };
    let thread = k.chat.clone();
    let turn = if answer == Answer::Once {
        let Some(runner) = ctx.turns.clone() else { return Err(V1Error::new("internal", "no turn runner")) };
        let Some(job) = ctx.jobs.create_if_idle(&thread) else { return Err(V1Error::new("busy", BUSY)) };
        Some((runner, job))
    } else {
        None
    };
    between(ctx);
    // two answers can cross: only the one that finds it still waiting counts
    let at = now(ctx);
    let state = if answer == Answer::No { "declined" } else { "allowed" };
    let won = edit(ctx, Some(chat), |n| n["id"] == id && n["state"] == "waiting", |n| {
        n["state"] = json!(state);
        n["at"] = json!(at);
        n["answer"] = json!(answer.as_str());
    });
    if won == 0 {
        if let Some((_, job)) = turn {
            ctx.jobs.with(job, |j| j.done = true);
        }
        return Err(stale());
    }
    forget(ctx, id);
    let n = find(ctx, chat, id).unwrap_or(Value::Null);
    let title = js::string_of(&n["title"]);
    log_ask(ctx, n["bot"].as_str().filter(|b| !b.is_empty()), shown(&k.tool, &title, &js::string_of(&n["preview"])), &k.tool, answer.outcome());
    let Some((runner, job)) = turn else {
        // it starts nothing: the model hears of it on its next turn in this chat
        super::note(ctx, &thread, &format!("The user chose Not now on the waiting card \"{title}\", so that call never ran. Don't retry it; ask them in words if you still need it."));
        return Ok(json!({"ok": true, "notice": n}));
    };
    start_yes(ctx, runner, job, &thread, &n, k);
    Ok(json!({"ok": true, "notice": n, "job": job, "thread": thread}))
}

/// `POST /v1/host/tools/ask {chat, id, answer}`: the person's answer on a pending or waiting
/// card. Human only (the host token).
pub fn host_answer(ctx: &Arc<HubCtx>, b: &Value) -> Result<Value, V1Error> {
    let str_of = |k: &str| b.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let (Some(chat), Some(id), Some(ans)) = (str_of("chat"), str_of("id"), str_of("answer")) else {
        return V1Error::err("invalid_args", "send {chat, id, answer: \"once\", \"chat\", \"rule\" or \"no\"}");
    };
    let Some(answer) = Answer::parse(ans) else { return V1Error::err("invalid_args", "answer is \"once\", \"chat\", \"rule\" or \"no\"") };
    let Some(n) = find(ctx, chat, id) else { return V1Error::err("not_found", "no such card in this chat") };
    if n["state"] == "waiting" {
        return answer_waiting(ctx, chat, id, answer);
    }
    if n["state"] != "pending" {
        return Err(V1Error::new("stale", NO_ANSWER).with_details(json!({"notice": n})));
    }
    let key = js::string_of(&n["key"]);
    let thread = ctx.store.read(|s| s.thread(chat).map(|t| t.id.clone())).unwrap_or_default();
    let prefix = n.get("prefix").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
    if answer == Answer::Rule && (key != "shell" || prefix.is_none()) {
        return V1Error::err("invalid_args", "only a shell card with a rule to save takes \"rule\"");
    }
    // a person's own hands, one time: whatever a client sends, there's no standing yes
    if answer == Answer::Chat && key == crate::hub::tools::computer::ASK_KEY {
        return V1Error::err("invalid_args", "a hand-over is answered each time: \"once\" or \"no\"");
    }
    // the parked call, if it still waits (a restart, a stopped turn: it doesn't)
    let Some(tx) = waiting().remove(id) else {
        // its wait ran out as this came
        if find(ctx, chat, id).is_some_and(|n| n["state"] == "waiting") {
            return answer_waiting(ctx, chat, id, answer);
        }
        let n = settle(ctx, chat, id, "expired", None).unwrap_or(n);
        return Err(V1Error::new("stale", NO_ANSWER).with_details(json!({"notice": n})));
    };
    let saved = match answer {
        Answer::Rule => {
            let pattern = prefix.unwrap_or_default();
            match gate::risk(&pattern, &ctx.data) {
                gate::Risk::Never(why) => Err(V1Error::new("refused", why)),
                _ if super::rules(ctx, "allow").iter().any(|r| r.pattern == pattern) => Ok(()),
                r => {
                    let change = Change::RuleAdd { kind: "allow".into(), pattern, risk: Some(r.as_str().into()), path: None };
                    super::commit(ctx, &change, "human", Some(&thread)).map(|_| ()).map_err(|e| V1Error::new("internal", e))
                }
            }
        }
        Answer::Chat => grant(ctx, &thread, &key).map_err(|e| V1Error::new("internal", e)),
        _ => Ok(()),
    };
    if let Err(e) = saved {
        waiting().insert(id.to_owned(), tx);
        return Err(e);
    }
    let state = if answer == Answer::No { "declined" } else { "allowed" };
    let n = settle(ctx, chat, id, state, Some(answer)).unwrap_or(n);
    let bot = n["bot"].as_str().filter(|b| !b.is_empty());
    let tool = js::string_of(&n["tool"]);
    log_ask(ctx, bot, shown(&tool, &js::string_of(&n["title"]), &js::string_of(&n["preview"])), &tool, answer.outcome());
    if tx.send(answer).is_err() {
        // the turn stopped as the answer came
        let n = settle(ctx, chat, id, "expired", None).unwrap_or(n);
        return Err(V1Error::new("stale", NO_ANSWER).with_details(json!({"notice": n})));
    }
    Ok(json!({"ok": true, "notice": n}))
}
