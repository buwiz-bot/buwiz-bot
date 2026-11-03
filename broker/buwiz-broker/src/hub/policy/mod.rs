//! Bot controls (PR 1): which tools a Bot has, what the shell may run on this Mac, and the
//! cards that change them.
//!
//! - Store: `bot_tools` (no row is on), `shell_mode` (no row is `bypass`), `shell_rule`,
//!   `shell_activity` (7 days, clipped and masked), `policy_change` (24 h, each with the JSON
//!   of its inverse) and `policy_note` (what a Bot reads of the user's decisions on its next
//!   model call). Tables in `store/schema.rs`.
//! - A [`Change`] tightens or widens. A model tool applies a tighten at once and only ever
//!   proposes a widen: a `tools_change` notice in the chat that a person answers through
//!   `POST /v1/host/tools/answer`. Nothing a model tool calls applies a widen.
//! - The gate (`gate.rs`, its shell reader `shell.rs`) and the sandbox (`sandbox.rs`) judge
//!   and run `shell_run`; `keys.rs` maps each tool to the switch it belongs to.

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

/// The card answer for a click on a changed or settled proposal.
pub fn stale_text(bot_name: &str) -> String {
    format!("That changed since you looked. Ask {bot_name} again.")
}
pub const UNDO_STALE: &str = "Undo is no longer possible: it changed since.";
pub const PUT_BACK: &str = "Put back.";
/// The proposal note the model reads.
pub const NOT_IN_EFFECT: &str = "Not in effect until the user confirms.";
pub const SAID_NO: &str = "The user said no to this. Ask them in words first.";
/// What a chat without a Bot is called on its cards.
pub const NO_BOT_NAME: &str = "The assistant";

/// `Date.now()` on the hub's clock.
pub fn now(ctx: &HubCtx) -> i64 {
    ctx.clock.now() as i64
}

fn db<T>(ctx: &HubCtx, f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> Result<T, String> {
    ctx.db.with(f)
}

// ---- the store ----

/// Whether `bot`'s `key` is on (no row is on).
pub fn tool_on(ctx: &HubCtx, bot: &str, key: &str) -> bool {
    let r: Option<i64> = db(ctx, |c| c.query_row("SELECT enabled FROM bot_tools WHERE bot_id=? AND key=?", [bot, key], |r| r.get(0)).optional()).ok().flatten();
    r != Some(0)
}

/// `bot`'s stored switches: key → (on, actor, at).
pub fn tool_rows(ctx: &HubCtx, bot: &str) -> Vec<(String, bool, String, i64)> {
    db(ctx, |c| {
        let mut q = c.prepare("SELECT key, enabled, actor, at FROM bot_tools WHERE bot_id=?")?;
        let r = q.query_map([bot], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<i64>>(3)?.unwrap_or(0))))?;
        r.collect()
    })
    .unwrap_or_default()
}

/// The keys switched off for `bot`.
pub fn off_keys(ctx: &HubCtx, bot: &str) -> HashSet<String> {
    tool_rows(ctx, bot).into_iter().filter(|r| !r.1).map(|r| r.0).collect()
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

/// The allow `pattern`'s inert line ("blocked by `rm`"), when a block overrides it.
pub fn inert_of(ctx: &HubCtx, pattern: &str) -> Option<String> {
    gate::inert_by(&patterns(ctx, "block"), pattern).map(|b| format!("blocked by `{b}`"))
}

/// One `shell_run` call into Activity (pruned to 7 days).
pub fn log_activity(ctx: &HubCtx, bot: Option<&str>, command: &str, result: &str, outcome: &str) {
    let at = now(ctx);
    let cmd = gate::mask_and_clip(command);
    let result = gate::clip(&gate::mask_and_clip(result), 200);
    let r = db(ctx, |c| {
        c.execute("DELETE FROM shell_activity WHERE at < ?", [at - WEEK])?;
        c.execute("INSERT INTO shell_activity(at, bot_id, command, result, outcome) VALUES(?,?,?,?,?)", rusqlite::params![at, bot, cmd, result, outcome])
    });
    if let Err(e) = r {
        eprintln!("buwiz-broker: shell activity: {e}");
    }
}

fn bot_name(ctx: &HubCtx, bot: &str) -> String {
    if bot.is_empty() {
        return NO_BOT_NAME.to_owned();
    }
    ctx.store.read(|s| s.bot(bot).map(|b| b.name.clone())).unwrap_or_else(|| bot.to_owned())
}

/// Activity, newest first: `[{at, bot, botName, command, outcome}]` (the last 7 days).
pub fn activity(ctx: &HubCtx, bot: Option<&str>) -> Vec<Value> {
    let since = now(ctx) - WEEK;
    let rows: Vec<(i64, Option<String>, String, String)> = db(ctx, |c| {
        let mut q = c.prepare("SELECT at, bot_id, command, outcome FROM shell_activity WHERE at >= ? ORDER BY at DESC, rowid DESC LIMIT 200")?;
        let r = q.query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?.unwrap_or_default())))?;
        r.collect()
    })
    .unwrap_or_default();
    rows.into_iter()
        .filter(|r| bot.is_none_or(|b| r.1.as_deref() == Some(b)))
        .map(|(at, b, command, outcome)| {
            let b = b.unwrap_or_default();
            json!({"at": at, "bot": b, "botName": bot_name(ctx, &b), "command": command, "outcome": outcome})
        })
        .collect()
}

// ---- the user's decisions, as the Bot reads them ----

/// A note `thread`'s next model call reads (and drops).
pub fn note(ctx: &HubCtx, thread: &str, text: &str) {
    let Some(t) = ctx.store.read(|s| s.thread(thread).map(|t| t.id.clone())) else { return };
    let at = now(ctx);
    let _ = db(ctx, |c| {
        c.execute("DELETE FROM policy_note WHERE at < ?", [at - DAY])?;
        c.execute("INSERT INTO policy_note(id, at, thread, text) VALUES(?,?,?,?)", rusqlite::params![uuidv7(), at, t, text])
    });
}

/// `thread`'s notes, oldest first, as model messages (each once): the form agent messages
/// take (`agents/run.rs` `history_for`).
pub fn take_notes(ctx: &HubCtx, thread: &str) -> Vec<Value> {
    let texts: Vec<(String, String)> = db(ctx, |c| {
        let mut q = c.prepare("SELECT id, text FROM policy_note WHERE thread=? ORDER BY at, id")?;
        let r = q.query_map([thread], |r| Ok((r.get(0)?, r.get(1)?)))?;
        r.collect()
    })
    .unwrap_or_default();
    for (id, _) in &texts {
        let _ = db(ctx, |c| c.execute("DELETE FROM policy_note WHERE id=?", [id]));
    }
    texts.into_iter().map(|(_, t)| json!({"role": "user", "content": format!("[Buwiz Bot, the app (not a message from the user):]\n{t}")})).collect()
}

// ---- changes ----

/// One change to a Bot's tools or the Mac's shell rules.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// `bot` empty: only the switch every Bot shares (a skill's or server's, F3)
    Tool { bot: String, key: String, enabled: bool },
    RuleAdd { kind: String, pattern: String, risk: Option<String>, path: Option<String> },
    RuleRemove { kind: String, pattern: String },
    Mode(Mode),
}

impl Change {
    pub fn to_json(&self) -> Value {
        match self {
            Change::Tool { bot, key, enabled } => json!({"op": "tool", "bot": bot, "key": key, "enabled": enabled}),
            Change::RuleAdd { kind, pattern, risk, path } => json!({"op": "rule_add", "kind": kind, "pattern": pattern, "risk": risk, "path": path}),
            Change::RuleRemove { kind, pattern } => json!({"op": "rule_remove", "kind": kind, "pattern": pattern}),
            Change::Mode(m) => json!({"op": "mode", "mode": m.as_str()}),
        }
    }

    pub fn from_json(v: &Value) -> Option<Change> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Some(match v.get("op")?.as_str()? {
            "tool" => Change::Tool { bot: s("bot").unwrap_or_default(), key: s("key")?, enabled: v.get("enabled")?.as_bool()? },
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

    /// Whether it lets more through: a tool on, an allow added, a block removed, bypass.
    pub fn widens(&self) -> bool {
        match self {
            Change::Tool { enabled, .. } => *enabled,
            Change::RuleAdd { kind, .. } => kind == "allow",
            Change::RuleRemove { kind, .. } => kind == "block",
            Change::Mode(m) => *m == Mode::Bypass,
        }
    }

    /// Whether it is in effect already.
    pub fn noop(&self, ctx: &HubCtx) -> bool {
        match self {
            Change::Tool { bot, key, enabled } => {
                let mine = bot.is_empty() || tool_on(ctx, bot, key) == *enabled;
                mine && (!*enabled || shared_on(ctx, key))
            }
            Change::RuleAdd { kind, pattern, .. } => rule(ctx, kind, pattern).is_some(),
            Change::RuleRemove { kind, pattern } => rule(ctx, kind, pattern).is_none(),
            Change::Mode(m) => mode(ctx) == *m,
        }
    }

    /// The change that puts it back, read before it applies.
    fn inverse(&self, ctx: &HubCtx) -> Change {
        match self {
            Change::Tool { bot, key, enabled } => Change::Tool { bot: bot.clone(), key: key.clone(), enabled: !*enabled },
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
            Change::Tool { bot, key, enabled } => {
                if !bot.is_empty() {
                    db(ctx, |c| c.execute("INSERT OR REPLACE INTO bot_tools(bot_id, key, enabled, actor, at) VALUES(?,?,?,?,?)", rusqlite::params![bot, key, *enabled as i64, actor, at]))?;
                }
                // turning a skill or server on for a Bot turns it on where it is off for all (F3)
                if *enabled {
                    if let Some(id) = key.strip_prefix("skill:") {
                        db(ctx, |c| c.execute("UPDATE skills SET enabled=1 WHERE id=?", [id]))?;
                    } else if let Some(id) = key.strip_prefix("mcp:") {
                        db(ctx, |c| c.execute("UPDATE mcp_servers SET enabled=1 WHERE id=?", [id]))?;
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
            Change::Tool { bot, key, enabled } => {
                let what = keys::phrase_of(ctx, key);
                match (bot.is_empty(), enabled) {
                    (true, _) => format!("{what} is on for every Bot."),
                    (false, true) => format!("{} can use {what} again.", bot_name(ctx, bot)),
                    (false, false) => format!("{} can't use {what} now.", bot_name(ctx, bot)),
                }
            }
            Change::RuleAdd { kind, pattern, .. } if kind == "allow" => format!("Always allowed on this Mac: `{pattern}`."),
            Change::RuleAdd { pattern, .. } => format!("Blocked on this Mac: `{pattern}`."),
            Change::RuleRemove { kind, pattern } if kind == "allow" => format!("Removed `{pattern}` from Always allowed. Those commands ask first again."),
            Change::RuleRemove { pattern, .. } => format!("Stopped blocking `{pattern}` on this Mac."),
            Change::Mode(Mode::Only) => "Mode: Only allowed commands. Unlisted commands are refused.".into(),
            Change::Mode(Mode::Bypass) => "Mode: Bypass. Every command runs without asking. Blocks still apply.".into(),
        }
    }

    /// The note the Bot reads when a person says yes (`true`) or no to it.
    fn decision(&self, ctx: &HubCtx, yes: bool) -> String {
        match (self, yes) {
            (Change::Tool { bot, key, enabled }, _) => {
                let what = keys::phrase_of(ctx, key);
                let who = if bot.is_empty() { "every Bot".to_owned() } else { bot_name(ctx, bot) };
                let turn = if *enabled { "on" } else { "off" };
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

/// Whether the switch every Bot shares for `key` is on (a skill's, a server's; others have none).
fn shared_on(ctx: &HubCtx, key: &str) -> bool {
    let q = |sql: &str, id: &str| db(ctx, |c| c.query_row(sql, [id], |r| r.get::<_, Option<i64>>(0)).optional()).ok().flatten().flatten();
    if let Some(id) = key.strip_prefix("skill:") {
        return q("SELECT enabled FROM skills WHERE id=?", id) != Some(0);
    }
    if let Some(id) = key.strip_prefix("mcp:") {
        return q("SELECT enabled FROM mcp_servers WHERE id=?", id) != Some(0);
    }
    true
}

/// Applies `change` as `actor` and records it with its undo: → (undo id, receipt).
pub fn commit(ctx: &HubCtx, change: &Change, actor: &str) -> Result<(String, String), String> {
    let inverse = change.inverse(ctx);
    change.apply(ctx, actor)?;
    let text = change.receipt(ctx);
    let id = format!("u_{}", uuidv7());
    record(ctx, &id, actor, change, &text, Some(&inverse))?;
    Ok((id, text))
}

fn bot_of_change(c: &Change) -> Option<&str> {
    match c {
        Change::Tool { bot, .. } if !bot.is_empty() => Some(bot),
        _ => None,
    }
}

fn record(ctx: &HubCtx, id: &str, actor: &str, change: &Change, text: &str, undo: Option<&Change>) -> Result<(), String> {
    let at = now(ctx);
    db(ctx, |c| {
        c.execute("DELETE FROM policy_change WHERE at < ?", [at - DAY])?;
        c.execute(
            "INSERT INTO policy_change(id, at, actor, bot_id, text, undo, target) VALUES(?,?,?,?,?,?,?)",
            rusqlite::params![id, at, actor, bot_of_change(change), text, undo.map(|u| u.to_json().to_string()), change.target()],
        )
    })
    .map(|_| ())
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
    record(ctx, &format!("u_{}", uuidv7()), "human", &inverse, PUT_BACK, None)
}

/// Recent changes (24 h), newest first: `[{at, actor, text, undo?}]`, `undo` while it is
/// still possible.
pub fn recent(ctx: &HubCtx) -> Vec<Value> {
    let since = now(ctx) - DAY;
    let rows: Vec<(String, i64, String, String, Option<String>, Option<String>)> = db(ctx, |c| {
        let mut q = c.prepare("SELECT id, at, actor, text, undo, target FROM policy_change WHERE at >= ? ORDER BY id DESC LIMIT 100")?;
        let r = q.query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?.unwrap_or_default(), r.get(4)?, r.get(5)?)))?;
        r.collect()
    })
    .unwrap_or_default();
    let mut seen: HashSet<String> = HashSet::new();
    rows.into_iter()
        .map(|(id, at, actor, text, inv, target)| {
            // newest first: only a target's latest change can still be undone
            let latest = target.is_some_and(|t| seen.insert(t));
            let undoable = latest && inv.as_deref().and_then(|i| serde_json::from_str::<Value>(i).ok()).as_ref().and_then(Change::from_json).is_some_and(|c| !c.noop(ctx));
            let mut v = json!({"at": at, "actor": actor, "text": text});
            if undoable {
                v["undo"] = json!(id);
            }
            v
        })
        .collect()
}

// ---- the tools_change card ----

fn op_of(c: &Change) -> &'static str {
    match c {
        Change::Tool { enabled: true, .. } => "tool_on",
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
        op @ ("tool_on" | "tool_off") => Change::Tool { bot: s(n, "bot").unwrap_or_default(), key: s(n, "key")?, enabled: op == "tool_on" },
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
        Change::Tool { key, .. } => {
            n["key"] = json!(key);
            n["label"] = json!(keys::label_of(ctx, key).map(|l| l.0).unwrap_or_else(|| key.clone()));
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
        Change::Tool { key, .. } => format!("Turn {} back on for {who}?", keys::phrase_of(ctx, key)),
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
        Change::Tool { bot, key, .. } => (!bot.is_empty() && ctx.store.read(|s| s.bot(bot).is_none())) || !keys::known(ctx, key),
        Change::RuleAdd { kind, pattern, .. } => kind == "allow" && matches!(gate::risk(pattern, &ctx.data), gate::Risk::Never(_)),
        _ => false,
    };
    if gone || change.noop(ctx) || changed_since(ctx, &change.target(), at) {
        return Err(stale(&mut n, true));
    }
    let (undo, text) = commit(ctx, &change, "human").map_err(|e| V1Error::new("internal", e))?;
    n["state"] = json!("applied");
    n["at"] = json!(now(ctx));
    n["undo"] = json!(undo);
    n["text"] = json!(text);
    set_notice(ctx, chat, id, &n, Some(&text));
    note(ctx, &thread, &change.decision(ctx, true));
    Ok(json!({"ok": true, "notice": n}))
}

// ---- the person's own edits (the sheet) ----

fn applied(ctx: &HubCtx, change: &Change) -> Result<Value, V1Error> {
    if change.noop(ctx) {
        return Ok(json!({"ok": true, "text": change.receipt(ctx), "undo": Value::Null}));
    }
    let (undo, text) = commit(ctx, change, "human").map_err(|e| V1Error::new("internal", e))?;
    Ok(json!({"ok": true, "text": text, "undo": undo}))
}

/// `POST /v1/host/tools/set {bot, key, enabled}`.
pub fn host_set(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let (Some(bot), Some(key), Some(enabled)) = (b.get("bot").and_then(Value::as_str).filter(|s| !s.is_empty()), b.get("key").and_then(Value::as_str).filter(|s| !s.is_empty()), b.get("enabled").and_then(Value::as_bool)) else {
        return V1Error::err("invalid_args", "send {bot, key, enabled}");
    };
    let Some(thread) = ctx.store.read(|s| s.bot(bot).map(|x| x.thread_id.clone())) else { return V1Error::err("not_found", format!("no Bot {}", crate::js::slice_utf16(bot, 80))) };
    if !keys::known(ctx, key) {
        return V1Error::err("not_found", format!("no tool switch {}", crate::js::slice_utf16(key, 80)));
    }
    let change = Change::Tool { bot: bot.to_owned(), key: key.to_owned(), enabled };
    let was = change.noop(ctx);
    let out = applied(ctx, &change)?;
    if let (false, Some(t)) = (was, thread) {
        note(ctx, &t, &change.decision(ctx, true));
    }
    Ok(out)
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

/// `GET /v1/host/tools?bot=<id>`.
pub fn host_view(ctx: &HubCtx, bot: Option<&str>) -> Result<Value, V1Error> {
    sweep_expired(ctx);
    let (who, tools) = match bot.filter(|b| !b.is_empty()) {
        Some(id) => {
            let Some(name) = ctx.store.read(|s| s.bot(id).map(|b| b.name.clone())) else { return V1Error::err("not_found", format!("no Bot {}", crate::js::slice_utf16(id, 80))) };
            let off = off_keys(ctx, id);
            let tools: Vec<Value> = keys::all_keys(ctx, id).iter().map(|k| keys::row_json(ctx, k, !off.contains(k))).collect();
            (json!({"id": id, "name": name}), tools)
        }
        None => (Value::Null, Vec::new()),
    };
    Ok(json!({"ok": true, "bot": who, "tools": tools, "mac": mac_json(ctx), "activity": activity(ctx, None), "recent": recent(ctx)}))
}
