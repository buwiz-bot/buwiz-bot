//! Bot controls: which tools a Bot (or every plain chat, the Chats profile) has, what the
//! shell may run on this Mac, and the cards that change them.
//!
//! - Store: `bot_tools` (a switch is on, ask or off; no row is the profile's default,
//!   [`default_pos`]), `shell_mode` (no row is `bypass`), `shell_rule`,
//!   `shell_activity` (7 days, clipped and masked), `policy_change` (30 days for the notes,
//!   24 h in Recent changes and for Undo; each with the JSON of its inverse and the thread a
//!   Bot made it in), `policy_note` (what a Bot reads of the user's decisions on its next
//!   model call) and `policy_seen` (the last change each thread's model was told of). Tables
//!   in `store/schema.rs`.
//! - A [`Change`] tightens or widens. A model tool applies a tighten at once and only ever
//!   proposes a widen: a `tools_change` notice in the chat that a person answers through
//!   `POST /v1/host/tools/answer`. Nothing a model tool calls applies a widen.
//! - The gate (`gate.rs`, its shell reader `shell.rs`) and the sandbox (`sandbox.rs`) judge
//!   and run `shell_run`; `keys.rs` maps each tool to the switch it belongs to and says
//!   whether a call only reads; `ask.rs` parks a call on a switch at Ask until the user
//!   answers its `tool_ask` card.

pub mod ask;
pub mod gate;
pub mod keys;
pub mod sandbox;
pub mod shell;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::store::{uuidv7, Turn};
use crate::v1::V1Error;
use gate::Mode;

pub const DAY: i64 = 86_400_000;
const WEEK: i64 = 7 * DAY;
/// How long `policy_change` keeps a row: a thread idle this long still hears of it.
pub const KEEP: i64 = 30 * DAY;

/// The card answer for a click on a changed or settled proposal.
pub fn stale_text(bot_name: &str) -> String {
    format!("That changed since you looked. Ask {bot_name} again.")
}
pub const UNDO_STALE: &str = "Undo is no longer possible: it changed since.";
pub const PUT_BACK: &str = "Put back.";
/// The proposal note the model reads.
pub const NOT_IN_EFFECT: &str = "Not in effect until the user confirms.";
pub const SAID_NO: &str = "The user said no to this. Ask them in words first.";
/// What a chat without a Bot is called on its cards, in Activity and in the notes.
pub const NO_BOT_NAME: &str = "A plain chat";
/// The owner id of the profile every plain chat shares (never a Bot's id: those are
/// `b_…`), and what its cards and receipts call it.
pub const CHATS: &str = "@chats";
pub const CHATS_NAME: &str = "Plain chats";

/// A switch's position: on, ask before a call that changes something or reaches someone,
/// or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pos {
    On,
    Ask,
    Off,
}

impl Pos {
    pub fn parse(s: &str) -> Option<Pos> {
        match s {
            "on" => Some(Pos::On),
            "ask" => Some(Pos::Ask),
            "off" => Some(Pos::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Pos::On => "on",
            Pos::Ask => "ask",
            Pos::Off => "off",
        }
    }

    /// How much it lets through: a move up is a widen.
    fn rank(self) -> u8 {
        match self {
            Pos::Off => 0,
            Pos::Ask => 1,
            Pos::On => 2,
        }
    }
}

/// The switches that start off for every profile: looking at the screen, and acting on it, are
/// turned on per Bot, each by itself. A person's data is on it.
pub const OFF_BY_DEFAULT: [&str; 2] = ["screens", "screen_actions"];

/// The switches that start at Ask for every profile: a command runs as the person, so nothing
/// runs without a person's yes until they turn a Bot's shell On.
pub const ASK_BY_DEFAULT: [&str; 1] = ["shell"];

/// A switch's position with no row: [`OFF_BY_DEFAULT`]'s are off, [`ASK_BY_DEFAULT`]'s ask; a
/// Bot's others are on; the Chats profile has the web and office documents on and everything
/// else at Ask.
pub fn default_pos(owner: &str, key: &str) -> Pos {
    if OFF_BY_DEFAULT.contains(&key) {
        Pos::Off
    } else if ASK_BY_DEFAULT.contains(&key) {
        Pos::Ask
    } else if owner != CHATS || matches!(key, "web" | "office") {
        Pos::On
    } else {
        Pos::Ask
    }
}

/// `Date.now()` on the hub's clock.
pub fn now(ctx: &HubCtx) -> i64 {
    ctx.clock.now() as i64
}

fn db<T>(ctx: &HubCtx, f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> Result<T, String> {
    ctx.db.with(f)
}

// ---- the store ----

/// A row's position: its `state`, else (a row from before it) `enabled`.
fn pos_of(enabled: Option<i64>, state: Option<String>) -> Pos {
    state.as_deref().and_then(Pos::parse).unwrap_or(if enabled == Some(0) { Pos::Off } else { Pos::On })
}

/// `owner`'s `key` (a Bot's id or [`CHATS`]): its row, else the profile's default.
pub fn tool_state(ctx: &HubCtx, owner: &str, key: &str) -> Pos {
    let r: Option<(Option<i64>, Option<String>)> = db(ctx, |c| c.query_row("SELECT enabled, state FROM bot_tools WHERE bot_id=? AND key=?", [owner, key], |r| Ok((r.get(0)?, r.get(1)?))).optional()).ok().flatten();
    match r {
        Some((e, s)) => pos_of(e, s),
        // the unit tests of what came before Ask run their plain chats with every switch on;
        // the ones of Ask ask for the real defaults
        #[cfg(test)]
        None if owner == CHATS && !OFF_BY_DEFAULT.contains(&key) && ctx.ext.get::<RealChatsDefaults>().is_none() => Pos::On,
        None => default_pos(owner, key),
    }
}

/// In `ctx.ext`: a unit test's plain chats have the Chats profile's real defaults.
#[cfg(test)]
pub struct RealChatsDefaults;

/// Whether `bot`'s `key` is not off.
pub fn tool_on(ctx: &HubCtx, bot: &str, key: &str) -> bool {
    tool_state(ctx, bot, key) != Pos::Off
}

/// `owner`'s stored switches: key → (position, actor, at).
pub fn tool_rows(ctx: &HubCtx, owner: &str) -> Vec<(String, Pos, String, i64)> {
    db(ctx, |c| {
        let mut q = c.prepare("SELECT key, enabled, state, actor, at FROM bot_tools WHERE bot_id=?")?;
        let r = q.query_map([owner], |r| Ok((r.get::<_, String>(0)?, pos_of(r.get(1)?, r.get(2)?), r.get::<_, Option<String>>(3)?.unwrap_or_default(), r.get::<_, Option<i64>>(4)?.unwrap_or(0))))?;
        r.collect()
    })
    .unwrap_or_default()
}

/// The keys switched off for `owner`, by a row or by default.
pub fn off_keys(ctx: &HubCtx, owner: &str) -> HashSet<String> {
    let rows = tool_rows(ctx, owner);
    let default = OFF_BY_DEFAULT.iter().filter(|k| !rows.iter().any(|r| r.0 == **k)).map(|k| (*k).to_owned());
    let off: Vec<String> = default.collect();
    rows.into_iter().filter(|r| r.1 == Pos::Off).map(|r| r.0).chain(off).collect()
}

/// The Mac's shell mode.
pub fn mode(ctx: &HubCtx) -> Mode {
    let m: Option<String> = db(ctx, |c| c.query_row("SELECT mode FROM shell_mode WHERE id=1", [], |r| r.get(0)).optional()).ok().flatten();
    Mode::from_stored(m.as_deref().unwrap_or("bypass"))
}

/// A stored rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub kind: String,
    pub pattern: String,
    pub risk: Option<String>,
    pub path: Option<String>,
}

/// The rules of `kind` (`allow` or `block`), oldest first.
pub fn rules(ctx: &HubCtx, kind: &str) -> Vec<Rule> {
    db(ctx, |c| {
        let mut q = c.prepare("SELECT kind, pattern, risk, path FROM shell_rule WHERE kind=? ORDER BY at, id")?;
        let r = q.query_map([kind], |r| Ok(Rule { kind: r.get(0)?, pattern: r.get(1)?, risk: r.get(2)?, path: r.get(3)? }))?;
        r.collect()
    })
    .unwrap_or_default()
}

fn rule(ctx: &HubCtx, kind: &str, pattern: &str) -> Option<Rule> {
    rules(ctx, kind).into_iter().find(|r| r.pattern == pattern)
}

fn patterns(ctx: &HubCtx, kind: &str) -> Vec<String> {
    rules(ctx, kind).into_iter().map(|r| r.pattern).collect()
}

/// The gate's verdict for `command` under the stored mode and rules.
pub fn judge(ctx: &HubCtx, command: &str) -> gate::Verdict {
    gate::decide(mode(ctx), &patterns(ctx, "allow"), &patterns(ctx, "block"), command)
}

/// [`judge`] under `mode` instead of the stored one (a switch at Ask reads the rules as mode
/// `only` does).
pub fn judge_in(ctx: &HubCtx, mode: Mode, command: &str) -> gate::Verdict {
    gate::decide(mode, &patterns(ctx, "allow"), &patterns(ctx, "block"), command)
}

/// The allow `pattern`'s inert line ("blocked by `rm`"), when a block overrides it.
pub fn inert_of(ctx: &HubCtx, pattern: &str) -> Option<String> {
    gate::inert_by(&patterns(ctx, "block"), pattern).map(|b| format!("blocked by `{b}`"))
}

/// One `shell_run` call into Activity (pruned to 7 days). `unasked`: the Bot ran it after
/// reading outside content, and the user's message did not name it (a proposal's
/// `unrequested`).
pub fn log_activity(ctx: &HubCtx, bot: Option<&str>, command: &str, result: &str, outcome: &str, unasked: bool) {
    let at = now(ctx);
    let cmd = gate::mask_and_clip(command);
    let result = gate::clip(&gate::mask_and_clip(result), 200);
    let r = db(ctx, |c| {
        c.execute("DELETE FROM shell_activity WHERE at < ?", [at - WEEK])?;
        c.execute("INSERT INTO shell_activity(at, bot_id, command, result, outcome, unasked) VALUES(?,?,?,?,?,?)", rusqlite::params![at, bot, cmd, result, outcome, unasked as i64])
    });
    if let Err(e) = r {
        eprintln!("buwiz-broker: shell activity: {e}");
    }
}

fn bot_name(ctx: &HubCtx, bot: &str) -> String {
    if bot.is_empty() {
        return NO_BOT_NAME.to_owned();
    }
    if bot == CHATS {
        return CHATS_NAME.to_owned();
    }
    ctx.store.read(|s| s.bot(bot).map(|b| b.name.clone())).unwrap_or_else(|| bot.to_owned())
}

/// An asked call into Activity: `what` (the command, or the card's title), the `tool`, and
/// its answer: `allowed`, `allowed_chat`, `declined`, `expired` or `needs_ok` (no one was
/// there to ask).
pub fn log_ask(ctx: &HubCtx, bot: Option<&str>, what: &str, tool: &str, outcome: &str) {
    let at = now(ctx);
    let what = gate::mask_and_clip(what);
    let r = db(ctx, |c| {
        c.execute("DELETE FROM shell_activity WHERE at < ?", [at - WEEK])?;
        c.execute("INSERT INTO shell_activity(at, bot_id, command, result, outcome, unasked, tool) VALUES(?,?,?,?,?,0,?)", rusqlite::params![at, bot, what, "", outcome, tool])
    });
    if let Err(e) = r {
        eprintln!("buwiz-broker: ask activity: {e}");
    }
}

/// Activity, newest first: `[{at, bot, botName, command, outcome, result, unasked, tool}]`
/// (the last 7 days). `outcome` is `ran` (exit 0), `failed` (it ran; `result` is `exit 1`,
/// `exit timeout` and the like), `refused_block` or `refused_mode` for a command; an asked
/// call's answer ([`log_ask`]) for a row with a `tool`.
pub fn activity(ctx: &HubCtx, bot: Option<&str>) -> Vec<Value> {
    let since = now(ctx) - WEEK;
    type Row = (i64, Option<String>, String, String, String, bool, String);
    let rows: Vec<Row> = db(ctx, |c| {
        let mut q = c.prepare("SELECT at, bot_id, command, outcome, result, unasked, tool FROM shell_activity WHERE at >= ? ORDER BY at DESC, rowid DESC LIMIT 200")?;
        let r = q.query_map([since], |r| {
            let text = |i: usize| r.get::<_, Option<String>>(i).map(Option::unwrap_or_default);
            Ok((r.get(0)?, r.get(1)?, text(2)?, text(3)?, text(4)?, r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0, text(6)?))
        })?;
        r.collect()
    })
    .unwrap_or_default();
    rows.into_iter()
        .filter(|r| bot.is_none_or(|b| r.1.as_deref() == Some(b)))
        .map(|(at, b, command, outcome, result, unasked, tool)| {
            let b = b.unwrap_or_default();
            // a row from before `failed`: a run that did not exit 0
            let outcome = if outcome == "ran" && result.starts_with("exit ") && result != "exit 0" { "failed".to_owned() } else { outcome };
            let mut v = json!({"at": at, "bot": b, "botName": bot_name(ctx, &b), "command": command, "outcome": outcome, "result": result, "unasked": unasked});
            if !tool.is_empty() {
                v["tool"] = json!(tool);
            }
            v
        })
        .collect()
}

// ---- the user's decisions, as the Bot reads them ----

/// A note `thread`'s next model call reads (and drops).
pub fn note(ctx: &HubCtx, thread: &str, text: &str) {
    note_as(ctx, thread, &uuidv7(), text);
}

/// [`note`] with the id of the change it tells of (`u_…`), so the change is not told again.
fn note_as(ctx: &HubCtx, thread: &str, id: &str, text: &str) {
    let Some(t) = ctx.store.read(|s| s.thread(thread).map(|t| t.id.clone())) else { return };
    let at = now(ctx);
    let _ = db(ctx, |c| {
        c.execute("DELETE FROM policy_note WHERE at < ?", [at - DAY])?;
        c.execute("INSERT INTO policy_note(id, at, thread, text) VALUES(?,?,?,?)", rusqlite::params![id, at, t, text])
    });
}

/// `thread`'s notes, oldest first, then one line of the changes the user made since its
/// last call (the sheet's included), as model messages (each once): the form agent messages
/// take (`agents/run.rs` `history_for`). `bot` is the Bot answering there (`None` in a
/// chat without one): another Bot's switches are not its business.
pub fn take_notes(ctx: &HubCtx, thread: &str, bot: Option<&str>) -> Vec<Value> {
    let notes: Vec<(String, String)> = db(ctx, |c| {
        let mut q = c.prepare("SELECT id, text FROM policy_note WHERE thread=? ORDER BY at, rowid")?;
        let r = q.query_map([thread], |r| Ok((r.get(0)?, r.get(1)?)))?;
        r.collect()
    })
    .unwrap_or_default();
    for (id, _) in &notes {
        let _ = db(ctx, |c| c.execute("DELETE FROM policy_note WHERE id=?", [id]));
    }
    let told: HashSet<&str> = notes.iter().map(|n| n.0.as_str()).collect();
    let (user, bots) = unseen_changes(ctx, thread, bot, &told);
    let mut texts: Vec<String> = notes.iter().map(|n| n.1.clone()).collect();
    if !user.is_empty() {
        texts.push(format!("Since your last message, the user changed: {}", user.join(" ")));
    }
    if !bots.is_empty() {
        texts.push(format!("Since your last message, in other chats: {}", bots.join(" ")));
    }
    texts.into_iter().map(|t| json!({"role": "user", "content": format!("[Buwiz Bot, the app (not a message from the user):]\n{t}")})).collect()
}

/// What changed since `thread`'s marker (oldest first): (the user's, a Bot's in another
/// thread, each as "Hex made this change: …"). The Mac's reach every thread, a Bot's
/// switches only that Bot's; a Bot's change is never news in the thread it made it in. The
/// marker moves past them; a thread without one starts now, with no backlog. A change in
/// `told` (a card answer's note) is not repeated.
fn unseen_changes(ctx: &HubCtx, thread: &str, bot: Option<&str>, told: &HashSet<&str>) -> (Vec<String>, Vec<String>) {
    let at = now(ctx);
    let last: Option<String> = db(ctx, |c| c.query_row("SELECT last FROM policy_seen WHERE thread=?", [thread], |r| r.get(0)).optional()).ok().flatten();
    let Some(last) = last else {
        let _ = db(ctx, |c| c.execute("INSERT OR REPLACE INTO policy_seen(thread, last, at) VALUES(?,?,?)", rusqlite::params![thread, format!("u_{}", uuidv7()), at]));
        return (Vec::new(), Vec::new());
    };
    // ids are `u_` and a v7 uuid: in the order they were made
    type Row = (String, Option<String>, String, String);
    let rows: Vec<Row> = db(ctx, |c| {
        let mut q = c.prepare("SELECT id, bot_id, COALESCE(said, text), actor FROM policy_change WHERE id > ? AND (actor='human' OR (thread IS NOT NULL AND thread != ?)) ORDER BY id")?;
        let r = q.query_map([&last, thread], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?.unwrap_or_default())))?;
        r.collect()
    })
    .unwrap_or_default();
    let Some(newest) = rows.last().map(|r| r.0.clone()) else { return (Vec::new(), Vec::new()) };
    let _ = db(ctx, |c| c.execute("UPDATE policy_seen SET last=?, at=? WHERE thread=?", rusqlite::params![newest, at, thread]));
    let (mut user, mut bots) = (Vec::new(), Vec::new());
    // a plain chat's switches are the Chats profile's
    let me = bot.unwrap_or(CHATS);
    for (id, b, text, actor) in rows {
        if text.is_empty() || told.contains(id.as_str()) || b.as_deref().is_some_and(|b| b != me) {
            continue;
        }
        match actor.strip_prefix("bot:") {
            Some(who) => bots.push(format!("{} made this change: {text}", bot_name(ctx, who))),
            None => user.push(text),
        }
    }
    (user, bots)
}

// ---- changes ----

/// One change to a Bot's tools or the Mac's shell rules.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// `bot` a Bot's id or [`CHATS`]; empty: only the switch every Bot shares (a skill's or
    /// server's, F3). `from` is the position it moves from (read when it was made).
    Tool { bot: String, key: String, state: Pos, from: Pos },
    RuleAdd { kind: String, pattern: String, risk: Option<String>, path: Option<String> },
    RuleRemove { kind: String, pattern: String },
    Mode(Mode),
}

impl Change {
    pub fn to_json(&self) -> Value {
        match self {
            Change::Tool { bot, key, state, from } => json!({"op": "tool", "bot": bot, "key": key, "state": state.as_str(), "from": from.as_str(), "enabled": *state != Pos::Off}),
            Change::RuleAdd { kind, pattern, risk, path } => json!({"op": "rule_add", "kind": kind, "pattern": pattern, "risk": risk, "path": path}),
            Change::RuleRemove { kind, pattern } => json!({"op": "rule_remove", "kind": kind, "pattern": pattern}),
            Change::Mode(m) => json!({"op": "mode", "mode": m.as_str()}),
        }
    }

    pub fn from_json(v: &Value) -> Option<Change> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Some(match v.get("op")?.as_str()? {
            "tool" => {
                // a row from before `state`: on or off
                let state = match s("state").as_deref().and_then(Pos::parse) {
                    Some(p) => p,
                    None if v.get("enabled")?.as_bool()? => Pos::On,
                    None => Pos::Off,
                };
                let from = s("from").as_deref().and_then(Pos::parse).unwrap_or(if state == Pos::Off { Pos::On } else { Pos::Off });
                Change::Tool { bot: s("bot").unwrap_or_default(), key: s("key")?, state, from }
            }
            "rule_add" => Change::RuleAdd { kind: s("kind")?, pattern: s("pattern")?, risk: s("risk"), path: s("path") },
            "rule_remove" => Change::RuleRemove { kind: s("kind")?, pattern: s("pattern")? },
            "mode" => Change::Mode(Mode::from_stored(&s("mode")?)),
            _ => return None,
        })
    }

    /// What it changes: one switch, one rule, or the mode.
    pub fn target(&self) -> String {
        match self {
            Change::Tool { bot, key, .. } => format!("tool:{bot}:{key}"),
            Change::RuleAdd { kind, pattern, .. } | Change::RuleRemove { kind, pattern } => format!("rule:{kind}:{pattern}"),
            Change::Mode(_) => "mode".into(),
        }
    }

    /// `owner`'s switch `key` to `state`, from where it is now.
    pub fn tool(ctx: &HubCtx, owner: &str, key: &str, state: Pos) -> Change {
        // a skill, server or mini app off for every Bot is off here too (F3)
        let from = if state != Pos::Off && !shared_on(ctx, key) {
            Pos::Off
        } else if owner.is_empty() {
            Pos::On
        } else {
            tool_state(ctx, owner, key)
        };
        Change::Tool { bot: owner.to_owned(), key: key.to_owned(), state, from }
    }

    /// Whether it lets more through: a switch toward on, an allow added, a block removed,
    /// bypass.
    pub fn widens(&self) -> bool {
        match self {
            Change::Tool { state, from, .. } => state.rank() > from.rank(),
            Change::RuleAdd { kind, .. } => kind == "allow",
            Change::RuleRemove { kind, .. } => kind == "block",
            Change::Mode(m) => *m == Mode::Bypass,
        }
    }

    /// Whether it is in effect already.
    pub fn noop(&self, ctx: &HubCtx) -> bool {
        match self {
            Change::Tool { bot, key, state, .. } => {
                let mine = bot.is_empty() || tool_state(ctx, bot, key) == *state;
                mine && (*state == Pos::Off || shared_on(ctx, key))
            }
            Change::RuleAdd { kind, pattern, .. } => rule(ctx, kind, pattern).is_some(),
            Change::RuleRemove { kind, pattern } => rule(ctx, kind, pattern).is_none(),
            Change::Mode(m) => mode(ctx) == *m,
        }
    }

    /// The change that puts it back, read before it applies.
    fn inverse(&self, ctx: &HubCtx) -> Change {
        match self {
            Change::Tool { bot, key, state, from } => Change::Tool { bot: bot.clone(), key: key.clone(), state: *from, from: *state },
            Change::RuleAdd { kind, pattern, .. } => Change::RuleRemove { kind: kind.clone(), pattern: pattern.clone() },
            Change::RuleRemove { kind, pattern } => {
                let r = rule(ctx, kind, pattern);
                Change::RuleAdd { kind: kind.clone(), pattern: pattern.clone(), risk: r.as_ref().and_then(|r| r.risk.clone()), path: r.and_then(|r| r.path) }
            }
            Change::Mode(_) => Change::Mode(mode(ctx)),
        }
    }

    fn apply(&self, ctx: &HubCtx, actor: &str) -> Result<(), String> {
        let at = now(ctx);
        match self {
            Change::Tool { bot, key, state, .. } => {
                let enabled = *state != Pos::Off;
                if !bot.is_empty() {
                    db(ctx, |c| c.execute("INSERT OR REPLACE INTO bot_tools(bot_id, key, enabled, state, actor, at) VALUES(?,?,?,?,?,?)", rusqlite::params![bot, key, enabled as i64, state.as_str(), actor, at]))?;
                }
                // turning a skill, server or mini app on for a Bot turns it on where it is off
                // for all (F3)
                if enabled {
                    if let Some(id) = key.strip_prefix("skill:") {
                        db(ctx, |c| c.execute("UPDATE skills SET enabled=1 WHERE id=?", [id]))?;
                    } else if let Some(id) = key.strip_prefix("mcp:") {
                        db(ctx, |c| c.execute("UPDATE mcp_servers SET enabled=1 WHERE id=?", [id]))?;
                    } else if let Some(id) = key.strip_prefix("v1:") {
                        enable_app(ctx, id);
                    }
                }
            }
            Change::RuleAdd { kind, pattern, risk, path } => {
                db(ctx, |c| c.execute("INSERT OR REPLACE INTO shell_rule(id, kind, pattern, risk, path, actor, at) VALUES(?,?,?,?,?,?,?)", rusqlite::params![format!("r_{}", uuidv7()), kind, pattern, risk, path, actor, at]))?;
            }
            Change::RuleRemove { kind, pattern } => {
                db(ctx, |c| c.execute("DELETE FROM shell_rule WHERE kind=? AND pattern=?", [kind, pattern]))?;
            }
            Change::Mode(m) => {
                db(ctx, |c| c.execute("INSERT OR REPLACE INTO shell_mode(id, mode, actor, at) VALUES(1,?,?,?)", rusqlite::params![m.as_str(), actor, at]))?;
            }
        }
        Ok(())
    }

    /// The receipt sentence (spec section 7).
    pub fn receipt(&self, ctx: &HubCtx) -> String {
        match self {
            Change::Tool { bot, key, state, .. } => {
                let what = keys::phrase_for(ctx, bot, key);
                match (bot.is_empty(), state) {
                    (true, _) => format!("{what} is on for every Bot."),
                    (false, Pos::On) => format!("{} can use {what} again.", bot_name(ctx, bot)),
                    (false, Pos::Ask) => format!("{} {} you before using {what}.", bot_name(ctx, bot), if bot == CHATS { "ask" } else { "asks" }),
                    (false, Pos::Off) => format!("{} can't use {what} now.", bot_name(ctx, bot)),
                }
            }
            Change::RuleAdd { kind, pattern, .. } if kind == "allow" => format!("Always allowed on this Mac: `{pattern}`."),
            Change::RuleAdd { pattern, .. } => format!("Blocked on this Mac: `{pattern}`."),
            // read after it applies: the mode it leaves
            Change::RuleRemove { kind, pattern } if kind == "allow" => match mode(ctx) {
                Mode::Bypass => format!("Removed `{pattern}` from Always allowed. It still runs while Bypass is on."),
                Mode::Only => format!("Removed `{pattern}` from Always allowed. Bots can't run it now unless you allow it again."),
            },
            Change::RuleRemove { pattern, .. } => format!("Stopped blocking `{pattern}` on this Mac."),
            Change::Mode(Mode::Only) => "Mode: Only allowed commands. Unlisted commands are refused.".into(),
            Change::Mode(Mode::Bypass) => "Mode: Bypass. Every command runs without asking. Blocks still apply.".into(),
        }
    }

    /// The note the Bot reads when a person says yes (`true`) or no to it.
    fn decision(&self, ctx: &HubCtx, yes: bool) -> String {
        match (self, yes) {
            (Change::Tool { bot, key, state, .. }, _) => {
                let what = keys::phrase_for(ctx, bot, key);
                let who = if bot.is_empty() { "every Bot".to_owned() } else { bot_name(ctx, bot) };
                let turn = match state {
                    Pos::On => "on",
                    Pos::Ask => "to Ask",
                    Pos::Off => "off",
                };
                if yes {
                    format!("The user turned {turn} {what} for {who}.")
                } else {
                    format!("The user said no to turning {turn} {what} for {who}.")
                }
            }
            (Change::RuleAdd { kind, pattern, .. }, true) if kind == "allow" => format!("The user allowed `{pattern}` on this Mac."),
            (Change::RuleAdd { kind, pattern, .. }, false) if kind == "allow" => format!("The user said no to always allowing `{pattern}` on this Mac."),
            (Change::RuleRemove { kind, pattern }, true) if kind == "block" => format!("The user stopped blocking `{pattern}` on this Mac."),
            (Change::RuleRemove { kind, pattern }, false) if kind == "block" => format!("The user said no to unblocking `{pattern}` on this Mac."),
            (Change::Mode(Mode::Bypass), true) => "The user set this Mac to Bypass: every command runs without asking; blocks still apply.".into(),
            (Change::Mode(Mode::Bypass), false) => "The user said no to Bypass on this Mac.".into(),
            (c, _) => format!("The user made this change: {}", c.receipt(ctx)),
        }
    }
}

/// Switches mini app `id` on for every Bot: it starts and is verified again (in a task: the
/// plugin host's enable waits for the process).
fn enable_app(ctx: &HubCtx, id: &str) {
    let (Some(host), Ok(rt)) = (ctx.plugins.clone(), tokio::runtime::Handle::try_current()) else { return };
    let id = id.to_owned();
    rt.spawn(async move {
        match host.enable(&id).await {
            Ok(v) if v.get("ok") == Some(&Value::Bool(true)) => {}
            Ok(v) => eprintln!("buwiz-broker: mini app {id} did not start: {}", crate::js::string_of(&v["state"])),
            Err(e) => eprintln!("buwiz-broker: mini app {id}: {}", e.message),
        }
    });
}

/// Whether the switch every Bot shares for `key` is on (a skill's, a server's, a mini app's;
/// others have none).
fn shared_on(ctx: &HubCtx, key: &str) -> bool {
    let q = |sql: &str, id: &str| db(ctx, |c| c.query_row(sql, [id], |r| r.get::<_, Option<i64>>(0)).optional()).ok().flatten().flatten();
    if let Some(id) = key.strip_prefix("skill:") {
        return q("SELECT enabled FROM skills WHERE id=?", id) != Some(0);
    }
    if let Some(id) = key.strip_prefix("mcp:") {
        return q("SELECT enabled FROM mcp_servers WHERE id=?", id) != Some(0);
    }
    if let Some(id) = key.strip_prefix("v1:") {
        return crate::hub::tools::v1::view(ctx).row(id).is_none_or(|r| r.enabled);
    }
    true
}

/// Applies `change` as `actor` and records it with its undo: → (undo id, receipt). `thread`
/// is the chat it was made in (a Bot's tool call, a card's answer); `None` for the sheet.
pub fn commit(ctx: &HubCtx, change: &Change, actor: &str, thread: Option<&str>) -> Result<(String, String), String> {
    let inverse = change.inverse(ctx);
    change.apply(ctx, actor)?;
    let text = change.receipt(ctx);
    let id = format!("u_{}", uuidv7());
    record(ctx, &Recorded { id: &id, actor, thread, said: None, undoes: None }, change, &text, Some(&inverse))?;
    settle_moot(ctx);
    Ok((id, text))
}

/// Pending cards whose change is in effect now (made in the sheet, or on another card) become
/// `stale` in every chat, so each chat's next turns read drops them.
fn settle_moot(ctx: &HubCtx) {
    edit_notices(ctx, None, |n| n["state"] == "pending" && change_of(n).filter(Change::widens).is_some_and(|c| c.noop(ctx)), |u| {
        if let Some(n) = u.notice.as_mut() {
            n["state"] = json!("stale");
        }
    });
}

fn bot_of_change(c: &Change) -> Option<&str> {
    match c {
        Change::Tool { bot, .. } if !bot.is_empty() => Some(bot),
        _ => None,
    }
}

/// A `policy_change` row's own fields.
struct Recorded<'a> {
    id: &'a str,
    actor: &'a str,
    thread: Option<&'a str>,
    /// what the model reads of it, when not `text`
    said: Option<&'a str>,
    /// an Undo's row: the change it undid (Recent changes marks that one instead)
    undoes: Option<&'a str>,
}

fn record(ctx: &HubCtx, r: &Recorded, change: &Change, text: &str, undo: Option<&Change>) -> Result<(), String> {
    let at = now(ctx);
    db(ctx, |c| {
        c.execute("DELETE FROM policy_change WHERE at < ?", [at - KEEP])?;
        c.execute(
            "INSERT INTO policy_change(id, at, actor, bot_id, text, said, undo, target, thread, undoes) VALUES(?,?,?,?,?,?,?,?,?,?)",
            rusqlite::params![r.id, at, r.actor, bot_of_change(change), text, r.said, undo.map(|u| u.to_json().to_string()), change.target(), r.thread, r.undoes],
        )
    })?;
    forget_gone_threads(ctx);
    Ok(())
}

/// Drops the `policy_seen` markers of threads that no longer exist (with the pruning above).
fn forget_gone_threads(ctx: &HubCtx) {
    let marked: Vec<String> = db(ctx, |c| {
        let mut q = c.prepare("SELECT thread FROM policy_seen")?;
        let r = q.query_map([], |r| r.get(0))?;
        r.collect()
    })
    .unwrap_or_default();
    let gone: Vec<String> = ctx.store.read(|s| if s.list.is_empty() { Vec::new() } else { marked.into_iter().filter(|t| s.thread(t).is_none()).collect() });
    for t in gone {
        let _ = db(ctx, |c| c.execute("DELETE FROM policy_seen WHERE thread=?", [&t]));
    }
}

/// Whether `target` changed after `at`.
fn changed_since(ctx: &HubCtx, target: &str, at: i64) -> bool {
    db(ctx, |c| c.query_row("SELECT 1 FROM policy_change WHERE target=? AND at>?", rusqlite::params![target, at], |_| Ok(())).optional()).ok().flatten().is_some()
}

/// Undo `id` as the person: `Err` (stale) after 24 h, once used, or when its target moved.
pub fn undo(ctx: &HubCtx, id: &str) -> Result<(), String> {
    let row: Option<(i64, Option<String>, Option<String>)> = db(ctx, |c| c.query_row("SELECT at, undo, target FROM policy_change WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()).ok().flatten();
    let stale = || Err(UNDO_STALE.to_owned());
    let Some((at, Some(inv), Some(target))) = row else { return stale() };
    if at < now(ctx) - DAY {
        return stale();
    }
    let later = db(ctx, |c| c.query_row("SELECT 1 FROM policy_change WHERE target=? AND id>?", [&target, id], |_| Ok(())).optional()).ok().flatten().is_some();
    let Some(inverse) = serde_json::from_str::<Value>(&inv).ok().as_ref().and_then(Change::from_json) else { return stale() };
    if later || inverse.noop(ctx) {
        return stale();
    }
    inverse.apply(ctx, "human")?;
    db(ctx, |c| c.execute("UPDATE policy_change SET undo=NULL WHERE id=?", [id]))?;
    let said = format!("Undid a change. {}", inverse.receipt(ctx));
    record(ctx, &Recorded { id: &format!("u_{}", uuidv7()), actor: "human", thread: None, said: Some(&said), undoes: Some(id) }, &inverse, PUT_BACK, None)?;
    settle_moot(ctx);
    Ok(())
}

/// Recent changes (24 h), newest first: `[{at, actor, text, undo?, undone?}]`, `undo` while
/// it is still possible. An Undo adds no row: the change it undid says `undone: true`.
pub fn recent(ctx: &HubCtx) -> Vec<Value> {
    let since = now(ctx) - DAY;
    type Row = (String, i64, String, String, Option<String>, Option<String>, Option<String>);
    let rows: Vec<Row> = db(ctx, |c| {
        let mut q = c.prepare("SELECT id, at, actor, text, undo, target, undoes FROM policy_change WHERE at >= ? ORDER BY id DESC LIMIT 100")?;
        let r = q.query_map([since], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?.unwrap_or_default(), r.get(4)?, r.get(5)?, r.get(6)?))
        })?;
        r.collect()
    })
    .unwrap_or_default();
    let undone: HashSet<String> = rows.iter().filter_map(|r| r.6.clone()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    rows.into_iter()
        .filter_map(|(id, at, actor, text, inv, target, undoes)| {
            // newest first: only a target's latest change can still be undone (an Undo's
            // own row counts, though it is not listed; one from before `undoes` neither)
            let latest = target.is_some_and(|t| seen.insert(t));
            if undoes.is_some() || text == PUT_BACK {
                return None;
            }
            let mut v = json!({"at": at, "actor": actor, "text": text});
            if undone.contains(&id) {
                v["undone"] = json!(true);
                return Some(v);
            }
            let undoable = latest && inv.as_deref().and_then(|i| serde_json::from_str::<Value>(i).ok()).as_ref().and_then(Change::from_json).is_some_and(|c| !c.noop(ctx));
            if undoable {
                v["undo"] = json!(id);
            }
            Some(v)
        })
        .collect()
}

// ---- the tools_change card ----

fn op_of(c: &Change) -> &'static str {
    match c {
        Change::Tool { state: Pos::On, .. } => "tool_on",
        Change::Tool { state: Pos::Ask, .. } => "tool_ask",
        Change::Tool { .. } => "tool_off",
        Change::RuleAdd { .. } => "rule_add",
        Change::RuleRemove { .. } => "rule_remove",
        Change::Mode(_) => "mode",
    }
}

/// The change a `tools_change` notice carries.
pub fn change_of(n: &Value) -> Option<Change> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let rule = n.get("rule").cloned().unwrap_or(Value::Null);
    Some(match n.get("op")?.as_str()? {
        op @ ("tool_on" | "tool_ask" | "tool_off") => {
            let state = match op {
                "tool_on" => Pos::On,
                "tool_ask" => Pos::Ask,
                _ => Pos::Off,
            };
            // a card from before `from`: on was off, off was on
            let from = s(n, "from").as_deref().and_then(Pos::parse).unwrap_or(if state == Pos::Off { Pos::On } else { Pos::Off });
            Change::Tool { bot: s(n, "bot").unwrap_or_default(), key: s(n, "key")?, state, from }
        }
        "rule_add" => Change::RuleAdd { kind: s(&rule, "kind")?, pattern: s(&rule, "pattern")?, risk: s(&rule, "risk"), path: s(&rule, "path") },
        "rule_remove" => Change::RuleRemove { kind: s(&rule, "kind")?, pattern: s(&rule, "pattern")? },
        "mode" => Change::Mode(Mode::from_stored(&s(n, "mode")?)),
        _ => return None,
    })
}

/// A `tools_change` notice for `change` by `bot` (`""` in a chat without one), in `state`.
fn notice_of(ctx: &HubCtx, change: &Change, bot: &str, state: &str) -> Value {
    let mut n = json!({"kind": "tools_change", "id": format!("p_{}", uuidv7()), "bot": bot, "botName": bot_name(ctx, bot), "op": op_of(change)});
    let mut steps = 1;
    match change {
        Change::Tool { key, from, .. } => {
            n["key"] = json!(key);
            n["label"] = json!(keys::label_of(ctx, key).map(|l| l.0).unwrap_or_else(|| key.clone()));
            n["from"] = json!(from.as_str());
        }
        Change::RuleAdd { kind, pattern, risk, path } => {
            let mut r = json!({"kind": kind, "pattern": pattern});
            if let Some(risk) = risk {
                r["risk"] = json!(risk);
                if kind == "allow" && risk == "any" {
                    steps = 2;
                }
            }
            if let Some(p) = path {
                r["path"] = json!(p);
            }
            n["rule"] = r;
            if kind == "allow" {
                if let Some(i) = inert_of(ctx, pattern) {
                    n["inert"] = json!(i);
                }
            }
        }
        Change::RuleRemove { kind, pattern } => {
            let mut r = json!({"kind": kind, "pattern": pattern});
            if let Some(risk) = rule(ctx, kind, pattern).and_then(|r| r.risk) {
                r["risk"] = json!(risk);
            }
            n["rule"] = r;
        }
        Change::Mode(m) => {
            n["mode"] = json!(m.as_str());
            if *m == Mode::Bypass {
                steps = 2;
            }
        }
    }
    n["steps"] = json!(steps);
    n["state"] = json!(state);
    n["at"] = json!(now(ctx));
    n
}

/// A pending card's title, as the turn's text (spec section 7, A1 to A5).
fn title_of(ctx: &HubCtx, change: &Change, bot: &str) -> String {
    let who = bot_name(ctx, bot);
    match change {
        Change::Tool { bot, key, state: Pos::Ask, .. } => format!("Let {who} use {} again, asking you first?", keys::phrase_for(ctx, bot, key)),
        // the switch every Bot shares, from a plain chat (F3)
        Change::Tool { bot, key, .. } if bot.is_empty() => format!("Turn {} back on?", keys::phrase_of(ctx, key)),
        Change::Tool { bot, key, .. } => format!("Turn {} back on for {who}?", keys::phrase_for(ctx, bot, key)),
        Change::RuleAdd { pattern, .. } => format!("Always allow `{pattern}` on this Mac?"),
        Change::RuleRemove { pattern, .. } => format!("Stop blocking `{pattern}` on this Mac?"),
        Change::Mode(_) => format!("Let {who} run every command on this Mac without asking?"),
    }
}

fn post(ctx: &HubCtx, chat: &str, text: &str, n: &Value) -> bool {
    match ctx.store.append_turn(chat, Turn::notice(text, n.clone())) {
        Ok(Some(_)) => {
            ctx.store.write_index();
            true
        }
        Ok(None) => false,
        Err(e) => {
            eprintln!("buwiz-broker: tools_change: the store's write: {e}");
            true
        }
    }
}

/// Every `tools_change` notice that `pick` takes gets `f`; how many changed.
fn edit_notices(ctx: &HubCtx, chat: Option<&str>, pick: impl Fn(&Value) -> bool, f: impl Fn(&mut Turn)) -> usize {
    let r = ctx.store.write(|s| {
        let chats: Vec<String> = match chat {
            Some(c) => s.thread(c).map(|t| vec![t.id.clone()]).unwrap_or_default(),
            None => s.list.iter().map(|t| t.id.clone()).collect(),
        };
        let mut n = 0;
        for c in chats {
            let Some(t) = s.thread_mut(&c) else { continue };
            for u in &mut t.turns {
                if u.notice.as_ref().is_some_and(|x| x["kind"] == "tools_change" && pick(x)) {
                    f(u);
                    n += 1;
                }
            }
        }
        n
    });
    let n = r.unwrap_or(0);
    if n > 0 {
        ctx.store.write_index();
    }
    n
}

/// Pending proposals older than 24 h become `expired`.
pub fn sweep_expired(ctx: &HubCtx) {
    let old = now(ctx) - DAY;
    edit_notices(ctx, None, |n| n["state"] == "pending" && n["at"].as_i64().is_some_and(|at| at < old), |u| {
        if let Some(n) = u.notice.as_mut() {
            n["state"] = json!("expired");
        }
    });
}

/// Posts the `applied` receipt card of a tighten the Bot made itself.
pub fn post_applied(ctx: &HubCtx, chat: &str, bot: &str, change: &Change, undo: &str, text: &str) {
    let mut n = notice_of(ctx, change, bot, "applied");
    n["undo"] = json!(undo);
    n["text"] = json!(text);
    post(ctx, chat, text, &n);
}

/// What a proposal knows of the turn that made it.
pub struct Ask<'a> {
    pub chat: &'a str,
    /// the acting Bot (`""` for a chat without one)
    pub bot: &'a str,
    pub unrequested: bool,
    /// whether the user's last message names the key, pattern or program
    pub named: bool,
}

/// Proposes the widen `change` in `ask.chat`: → the notice, or why not (the 24 h cooldown
/// after a Not now, a chat that is gone). One pending per (bot, op, target): an older one is
/// `superseded`.
pub fn propose(ctx: &HubCtx, ask: &Ask, change: &Change) -> Result<Value, String> {
    debug_assert!(change.widens());
    sweep_expired(ctx);
    let target = change.target();
    let op = op_of(change);
    let since = now(ctx) - DAY;
    let same = |n: &Value| n["bot"] == ask.bot && n["op"] == op && change_of(n).is_some_and(|c| c.target() == target);
    let declined = ctx.store.read(|s| {
        s.thread(ask.chat).is_some_and(|t| t.turns.iter().filter_map(|u| u.notice.as_ref()).any(|n| n["kind"] == "tools_change" && same(n) && n["state"] == "declined" && n["at"].as_i64().is_some_and(|at| at >= since)))
    });
    if declined && !ask.named {
        return Err(SAID_NO.into());
    }
    if ctx.store.read(|s| s.thread(ask.chat).is_none()) {
        return Err("no chat to ask in".into());
    }
    edit_notices(ctx, None, |n| n["state"] == "pending" && same(n), |u| {
        if let Some(n) = u.notice.as_mut() {
            n["state"] = json!("superseded");
        }
    });
    let mut n = notice_of(ctx, change, ask.bot, "pending");
    n["unrequested"] = json!(ask.unrequested);
    post(ctx, ask.chat, &title_of(ctx, change, ask.bot), &n);
    Ok(n)
}

fn find_notice(ctx: &HubCtx, chat: &str, id: &str) -> Option<Value> {
    ctx.store.read(|s| s.thread(chat).and_then(|t| t.turns.iter().filter_map(|u| u.notice.as_ref()).find(|n| n["kind"] == "tools_change" && n["id"] == id).cloned()))
}

fn set_notice(ctx: &HubCtx, chat: &str, id: &str, n: &Value, text: Option<&str>) {
    edit_notices(ctx, Some(chat), |x| x["id"] == id, |u| {
        u.notice = Some(n.clone());
        if let Some(t) = text {
            u.text = Some(t.to_owned());
        }
    });
}

/// `POST /v1/host/tools/answer {chat, id, answer}`: the person's yes or no on a pending
/// proposal, the state read again at the click.
pub fn answer(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let str_of = |k: &str| b.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let (Some(chat), Some(id), Some(ans)) = (str_of("chat"), str_of("id"), str_of("answer")) else { return V1Error::err("invalid_args", "send {chat, id, answer: \"yes\" or \"no\"}") };
    if ans != "yes" && ans != "no" {
        return V1Error::err("invalid_args", "answer is \"yes\" or \"no\"");
    }
    if ctx.store.read(|s| s.thread(chat).is_none()) {
        return V1Error::err("not_found", format!("no chat {}", crate::js::slice_utf16(chat, 80)));
    }
    sweep_expired(ctx);
    let Some(mut n) = find_notice(ctx, chat, id) else { return V1Error::err("not_found", "no such proposal in this chat") };
    let bot = n["bot"].as_str().unwrap_or("").to_owned();
    let stale = |n: &mut Value, set: bool| {
        if set {
            n["state"] = json!("stale");
            set_notice(ctx, chat, id, n, None);
        }
        V1Error::new("stale", stale_text(&bot_name(ctx, &bot))).with_details(json!({"notice": n}))
    };
    if n["state"] != "pending" {
        return Err(stale(&mut n, false));
    }
    let Some(change) = change_of(&n).filter(Change::widens) else { return V1Error::err("invalid_args", "this card has no change to answer") };
    let thread = ctx.store.read(|s| s.thread(chat).map(|t| t.id.clone())).unwrap_or_default();
    if ans == "no" {
        n["state"] = json!("declined");
        n["at"] = json!(now(ctx));
        set_notice(ctx, chat, id, &n, None);
        note(ctx, &thread, &change.decision(ctx, false));
        return Ok(json!({"ok": true, "notice": n}));
    }
    let at = n["at"].as_i64().unwrap_or(0);
    let gone = match &change {
        Change::Tool { bot, key, .. } => (!bot.is_empty() && bot != CHATS && ctx.store.read(|s| s.bot(bot).is_none())) || !keys::known(ctx, key),
        Change::RuleAdd { kind, pattern, .. } => kind == "allow" && matches!(gate::risk(pattern, &ctx.data), gate::Risk::Never(_)),
        _ => false,
    };
    if gone || change.noop(ctx) || changed_since(ctx, &change.target(), at) {
        return Err(stale(&mut n, true));
    }
    let (undo, text) = commit(ctx, &change, "human", Some(&thread)).map_err(|e| V1Error::new("internal", e))?;
    n["state"] = json!("applied");
    n["at"] = json!(now(ctx));
    n["undo"] = json!(undo);
    n["text"] = json!(text);
    set_notice(ctx, chat, id, &n, Some(&text));
    note_as(ctx, &thread, &undo, &change.decision(ctx, true));
    Ok(json!({"ok": true, "notice": n}))
}

// ---- the person's own edits (the sheet) ----

fn applied(ctx: &HubCtx, change: &Change) -> Result<Value, V1Error> {
    if change.noop(ctx) {
        return Ok(json!({"ok": true, "text": change.receipt(ctx), "undo": Value::Null}));
    }
    let (undo, text) = commit(ctx, change, "human", None).map_err(|e| V1Error::new("internal", e))?;
    Ok(json!({"ok": true, "text": text, "undo": undo}))
}

/// The profile a request names: a Bot's id, or `chats` (or [`CHATS`]) for every plain chat.
pub fn owner_of(name: &str) -> &str {
    if name == "chats" {
        CHATS
    } else {
        name
    }
}

/// `POST /v1/host/tools/set {bot | "chats", key, state}` (`state` on, ask or off; the old
/// `enabled` is on or off). The person's click: a widen applies too.
pub fn host_set(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let state = match (b.get("state").and_then(Value::as_str), b.get("enabled").and_then(Value::as_bool)) {
        (Some(s), _) => Pos::parse(s),
        (None, Some(true)) => Some(Pos::On),
        (None, Some(false)) => Some(Pos::Off),
        (None, None) => None,
    };
    let (Some(bot), Some(key), Some(state)) = (b.get("bot").and_then(Value::as_str).filter(|s| !s.is_empty()), b.get("key").and_then(Value::as_str).filter(|s| !s.is_empty()), state) else {
        return V1Error::err("invalid_args", "send {bot (a Bot's id, or \"chats\"), key, state: \"on\", \"ask\" or \"off\"}");
    };
    let owner = owner_of(bot);
    if owner != CHATS && ctx.store.read(|s| s.bot(owner).is_none()) {
        return V1Error::err("not_found", format!("no Bot {}", crate::js::slice_utf16(bot, 80)));
    }
    if !keys::known(ctx, key) {
        return V1Error::err("not_found", format!("no tool switch {}", crate::js::slice_utf16(key, 80)));
    }
    // the profile's threads read it on their next call (`take_notes`)
    applied(ctx, &Change::tool(ctx, owner, key, state))
}

/// `POST /v1/host/shell/mode {mode}`.
pub fn host_mode(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    match b.get("mode").and_then(Value::as_str) {
        Some(m @ ("only" | "bypass")) => applied(ctx, &Change::Mode(Mode::from_stored(m))),
        _ => V1Error::err("invalid_args", "mode is \"only\" or \"bypass\""),
    }
}

/// `POST /v1/host/shell/rules {op, kind, pattern}`. An allow whose risk is `never` is refused.
pub fn host_rules(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let s = |k: &str| b.get(k).and_then(Value::as_str);
    let (Some(op @ ("add" | "remove")), Some(kind @ ("allow" | "block")), Some(raw)) = (s("op"), s("kind"), s("pattern")) else {
        return V1Error::err("invalid_args", "send {op: \"add\" or \"remove\", kind: \"allow\" or \"block\", pattern}");
    };
    if op == "remove" {
        if rule(ctx, kind, raw).is_none() {
            return V1Error::err("not_found", "no such rule");
        }
        return applied(ctx, &Change::RuleRemove { kind: kind.into(), pattern: raw.into() });
    }
    let (pattern, starred) = gate::normalize(raw);
    if pattern.is_empty() {
        return V1Error::err("refused", "Not saved: the rule is empty.");
    }
    let risk = if kind == "allow" {
        match gate::risk(&pattern, &ctx.data) {
            gate::Risk::Never(why) => return V1Error::err("refused", why),
            r => Some(r.as_str().to_owned()),
        }
    } else {
        None
    };
    if rule(ctx, kind, &pattern).is_some() {
        return V1Error::err("exists", if kind == "allow" { "Already always allowed." } else { "Already blocked." });
    }
    let mut out = applied(ctx, &Change::RuleAdd { kind: kind.into(), pattern: pattern.clone(), risk, path: None })?;
    if starred {
        out["note"] = json!(gate::star_note(&pattern));
    }
    Ok(out)
}

/// `POST /v1/host/tools/undo {undo}`.
pub fn host_undo(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let Some(id) = b.get("undo").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return V1Error::err("invalid_args", "send {undo}") };
    undo(ctx, id).map_err(|e| if e == UNDO_STALE { V1Error::new("stale", e) } else { V1Error::new("internal", e) })?;
    Ok(json!({"ok": true, "text": PUT_BACK}))
}

/// The Mac's mode and rules: `{mode, allow: [{pattern, risk, inert?}], block: [{pattern}]}`.
pub fn mac_json(ctx: &HubCtx) -> Value {
    let blocks = patterns(ctx, "block");
    let allow: Vec<Value> = rules(ctx, "allow")
        .into_iter()
        .map(|r| {
            let mut v = json!({"pattern": r.pattern, "risk": r.risk.clone().unwrap_or_else(|| gate::risk(&r.pattern, &ctx.data).as_str().to_owned())});
            if let Some(b) = gate::inert_by(&blocks, &r.pattern) {
                v["inert"] = json!(format!("blocked by `{b}`"));
            }
            v
        })
        .collect();
    json!({"mode": mode(ctx).as_str(), "allow": allow, "block": blocks.iter().map(|p| json!({"pattern": p})).collect::<Vec<_>>()})
}

/// `owner`'s rows as `GET /v1/host/tools` answers them.
pub fn profile_rows(ctx: &HubCtx, owner: &str) -> Vec<Value> {
    keys::all_keys(ctx, owner)
        .iter()
        .map(|k| {
            let mut v = keys::row_json(ctx, k, tool_state(ctx, owner, k));
            // a plain chat messages agents as you, and makes none
            if owner == CHATS && k == "agents" {
                v["sub"] = json!("Messages your agents as you.");
            }
            v
        })
        .collect()
}

/// `GET /v1/host/tools?bot=<id>`: that Bot's switches; without a Bot (or `bot=chats`), the
/// Chats profile's (`profile: "chats"`, `bot` null).
pub fn host_view(ctx: &HubCtx, bot: Option<&str>) -> Result<Value, V1Error> {
    sweep_expired(ctx);
    let (who, profile, tools) = match bot.filter(|b| !b.is_empty()).map(owner_of).filter(|b| *b != CHATS) {
        Some(id) => {
            let Some(name) = ctx.store.read(|s| s.bot(id).map(|b| b.name.clone())) else { return V1Error::err("not_found", format!("no Bot {}", crate::js::slice_utf16(id, 80))) };
            (json!({"id": id, "name": name}), "bot", profile_rows(ctx, id))
        }
        None => (Value::Null, "chats", profile_rows(ctx, CHATS)),
    };
    let mut v = json!({"ok": true, "bot": who, "profile": profile, "tools": tools, "mac": mac_json(ctx), "activity": activity(ctx, None), "recent": recent(ctx)});
    // plain chats' memories, which the Chats tab lists
    if profile == "chats" {
        let ms: Vec<Value> = crate::hub::library::botctx::chats_memories(ctx).iter().map(|m| json!({"id": m["id"], "text": m["text"]})).collect();
        v["memories"] = json!(ms);
    }
    Ok(v)
}

/// `POST /v1/host/tools/memory/remove {id}`: the person forgets one of plain chats'
/// memories, at once.
pub fn host_memory_remove(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let Some(id) = b.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return V1Error::err("invalid_args", "send {id}") };
    match crate::hub::library::botctx::chats_memory_remove(ctx, id) {
        Ok(true) => Ok(json!({"ok": true, "text": "Forgot it."})),
        Ok(false) => V1Error::err("not_found", "no such memory"),
        Err(e) => Err(V1Error::new("internal", e)),
    }
}
