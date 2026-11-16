//! What a person allowed a generated widget to use of a data source, one answer per widget
//! and source: `read` (the source's actions that only read), `write` (every action) or `no`.
//!
//! A call with no answer posts one `widget_ask` notice in the widget's chat and answers
//! `not_granted`; it holds nothing. While that card waits, another call posts no second one
//! and gets the same id. The answer comes back through `POST /v1/host/widget/grant`, which
//! writes the row (`widget_grants`) and settles the notice. A row keeps the action names
//! the person was shown: a call for an action not among them asks again.

use std::sync::Mutex;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::ticket::{base_of, call_path, is_widget};
use super::{all, CallError, Resolved, Source};
use crate::hub::ctx::HubCtx;
use crate::hub::policy::{self, CHATS};
use crate::hub::store::uuidv7;
use crate::v1::V1Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Read,
    Write,
    No,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Write => "write",
            Mode::No => "no",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "read" => Some(Mode::Read),
            "write" => Some(Mode::Write),
            "no" => Some(Mode::No),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    pub source: String,
    pub mode: Mode,
    /// the action names the person was shown
    pub actions: Vec<String>,
    pub at: i64,
}

/// What a grant says of a call.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Allowed,
    /// the person said no (or allowed only reading): do not ask again by itself
    Refused(String),
    Ask,
}

/// What `grant` (if any) makes of a call to `action`: no answer, or an answer given before
/// the source had this action, asks; `no` refuses; `read` covers the actions that only read;
/// `write` covers all.
pub fn verdict(grant: Option<&Grant>, action: &super::Action) -> Verdict {
    let Some(g) = grant.filter(|g| g.actions.contains(&action.name)) else { return Verdict::Ask };
    match g.mode {
        Mode::No => Verdict::Refused("the person said no to this widget using this source".into()),
        Mode::Read if action.write => Verdict::Refused("the person allowed this widget to read this source, not to change it".into()),
        Mode::Read | Mode::Write => Verdict::Allowed,
    }
}

fn parse_actions(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

/// The answer for `widget`'s `source`, if there is one.
pub fn row(ctx: &HubCtx, widget: &str, source: &str) -> Option<Grant> {
    ctx.db
        .with(|c| {
            c.query_row("SELECT source, mode, actions, at FROM widget_grants WHERE widget=? AND source=?", [widget, source], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))
                .optional()
        })
        .ok()
        .flatten()
        .and_then(|(source, mode, actions, at)| Some(Grant { source, mode: Mode::parse(&mode)?, actions: parse_actions(&actions), at }))
}

/// Every answer for `widget`, by source.
pub fn rows(ctx: &HubCtx, widget: &str) -> Vec<Grant> {
    let read = ctx.db.with(|c| {
        let mut q = c.prepare("SELECT source, mode, actions, at FROM widget_grants WHERE widget=? ORDER BY source")?;
        let r = q.query_map([widget], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?;
        r.collect::<rusqlite::Result<Vec<_>>>()
    });
    read.unwrap_or_default().into_iter().filter_map(|(source, mode, actions, at)| Some(Grant { source, mode: Mode::parse(&mode)?, actions: parse_actions(&actions), at })).collect()
}

fn save(ctx: &HubCtx, widget: &str, source: &str, mode: Mode, actions: &[String]) -> Result<(), String> {
    let at = policy::now(ctx);
    ctx.db
        .with(|c| c.execute("INSERT OR REPLACE INTO widget_grants(widget, source, mode, actions, at) VALUES(?,?,?,?,?)", rusqlite::params![widget, source, mode.as_str(), json!(actions).to_string(), at]))
        .map(|_| ())
}

/// Forgets the answer: whether there was one.
fn remove(ctx: &HubCtx, widget: &str, source: &str) -> bool {
    ctx.db.with(|c| c.execute("DELETE FROM widget_grants WHERE widget=? AND source=?", [widget, source])).map(|n| n > 0).unwrap_or(false)
}

/// Where a widget lives: whose switches apply to it (its chat's Bot, else `@chats`), and the
/// chat that holds it, if any.
pub struct Home {
    pub owner: String,
    pub thread: Option<String>,
}

/// The chat that holds widget `base`'s card and that chat's Bot. A widget in no chat (opened
/// from the catalog only) belongs to `@chats`.
pub fn home(ctx: &HubCtx, base: &str) -> Home {
    ctx.store.read(|s| {
        let holder = s.list.iter().find(|t| t.turns.iter().any(|u| u.name.as_deref().and_then(base_of) == Some(base)));
        match holder {
            Some(t) => Home { owner: s.thread_bot(&t.id).map(|b| b.id.clone()).unwrap_or_else(|| CHATS.to_owned()), thread: Some(t.id.clone()) },
            None => Home { owner: CHATS.to_owned(), thread: None },
        }
    })
}

/// The chat a card for widget `base` goes in: the chat that holds it, else the most recent
/// chat of `@chats` (the active one, when no Bot owns it).
fn card_chat(ctx: &HubCtx, home: &Home) -> Option<String> {
    if let Some(t) = &home.thread {
        return Some(t.clone());
    }
    ctx.store.read(|s| {
        let plain = |id: &str| s.thread_bot(id).is_none();
        let active = s.active_thread();
        if plain(&active.id) {
            return Some(active.id.clone());
        }
        s.list.iter().rev().find(|t| plain(&t.id) && t.routine.is_none() && t.agents.is_none() && !t.archived).map(|t| t.id.clone()).or_else(|| Some(active.id.clone()))
    })
}

/// The title a person knows widget `base` by.
pub fn title_of(ctx: &HubCtx, base: &str) -> String {
    let asked = |sql: &str| ctx.db.with(|c| c.query_row(sql, [base], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten().flatten().filter(|t| !t.trim().is_empty());
    asked("SELECT title FROM apps WHERE name=?").or_else(|| asked("SELECT title FROM widgets WHERE name=?")).map(|t| t.chars().take(80).collect()).unwrap_or_else(|| base.to_owned())
}

/// Every `widget_ask` notice, with the chat holding it.
fn cards(ctx: &HubCtx) -> Vec<(String, Value)> {
    ctx.store.read(|s| s.list.iter().flat_map(|t| t.turns.iter().filter_map(|u| u.notice.as_ref()).filter(|n| n["kind"] == "widget_ask").map(|n| (t.id.clone(), n.clone()))).collect())
}

/// The id of the card still waiting for an answer about `widget`'s use of `source`.
pub fn pending_card(ctx: &HubCtx, widget: &str, source: &str) -> Option<String> {
    cards(ctx).into_iter().find(|(_, n)| n["state"] == "pending" && n["widget"] == widget && n["source"] == source).and_then(|(_, n)| n["id"].as_str().map(str::to_owned))
}

/// Posting is one step with finding that nothing waits: calls come in side by side.
fn asking() -> std::sync::MutexGuard<'static, ()> {
    static ASKING: Mutex<()> = Mutex::new(());
    ASKING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The card for `widget` and `source`: what it may read and what it may change, in the
/// words a person reads.
fn card(ctx: &HubCtx, home: &Home, chat: &str, widget: &str, s: &Source) -> Value {
    let phrases = |write: bool| s.actions.iter().filter(|a| a.write == write).map(|a| a.about.clone()).collect::<Vec<_>>();
    json!({
        "kind": "widget_ask", "id": format!("w_{}", uuidv7()), "chat": chat, "bot": home.owner, "botName": policy::bot_name(ctx, &home.owner),
        "widget": widget, "widgetTitle": title_of(ctx, widget), "source": s.name, "sourceTitle": s.title,
        "reads": phrases(false), "writes": phrases(true), "actions": s.actions.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
        "state": "pending", "at": policy::now(ctx),
    })
}

/// Asks the person (once) whether `widget` may use the resolved source: the id of the card
/// that waits, new or already there.
pub fn ask(ctx: &HubCtx, home: &Home, widget: &str, r: &Resolved) -> Result<String, CallError> {
    let _one = asking();
    if let Some(id) = pending_card(ctx, widget, &r.source.name) {
        return Ok(id);
    }
    let Some(chat) = card_chat(ctx, home) else { return Err(CallError::new("not_granted", "there is no chat to ask in")) };
    let n = card(ctx, home, &chat, widget, &r.source);
    if !crate::hub::tools::miniapps::post_in(ctx, &chat, &n) {
        return Err(CallError::new("not_granted", "there is no chat to ask in"));
    }
    Ok(n["id"].as_str().unwrap_or_default().to_owned())
}

// ———— the host routes ————

fn name_of(b: &Value) -> Result<String, V1Error> {
    match b.get("name").and_then(Value::as_str).and_then(base_of) {
        Some(base) => Ok(base.to_owned()),
        None => V1Error::err("invalid_args", "send {name}: a widget's name, R<digits>"),
    }
}

/// The answers of `base` as a list for the app: each with its source's title.
async fn titled(ctx: &HubCtx, base: &str, full: bool) -> Vec<Value> {
    let sources = all(ctx, &home(ctx, base).owner).await;
    rows(ctx, base)
        .iter()
        .map(|g| {
            let title = sources.iter().find(|s| s.name == g.source).map(|s| s.title.clone()).unwrap_or_else(|| g.source.clone());
            let mut v = json!({"source": g.source, "sourceTitle": title, "mode": g.mode.as_str()});
            if full {
                v["actions"] = json!(g.actions);
                v["at"] = json!(g.at);
            }
            v
        })
        .collect()
}

fn refused(error: impl Into<String>) -> Result<Value, V1Error> {
    Ok(json!({"ok": false, "error": error.into()}))
}

/// `POST /v1/host/widget/open {name}`: the widget's ticket, the path its code calls, and
/// what it was already allowed. Free of side effects (the ticket secret is made once).
pub async fn host_open(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let base = name_of(b)?;
    let dirs = crate::hub::pipeline::Dirs::of(&ctx.data);
    if !is_widget(&base) || !(dirs.gen.join(format!("{base}.tsx")).is_file() || dirs.widgets.join(format!("{base}.tsx")).is_file()) {
        return V1Error::err("not_found", format!("there is no widget {base}"));
    }
    let call = call_path(ctx, &base).map_err(|e| V1Error::new("internal", e))?;
    let ticket = call.split('/').nth(3).unwrap_or_default().to_owned();
    Ok(json!({"ok": true, "ticket": ticket, "call": call, "grants": titled(ctx, &base, false).await}))
}

/// `POST /v1/host/widget/grant {id, answer}`: the person's answer on a `widget_ask` card. A
/// `write` on a card with nothing to write is a `read`. A refusal is `{ok: false, error}`,
/// `error` a sentence.
pub fn host_grant(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let (Some(id), Some(answer)) = (b.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()), b.get("answer").and_then(Value::as_str).and_then(Mode::parse)) else {
        return refused("send {id, answer: \"write\", \"read\" or \"no\"}");
    };
    let at = policy::now(ctx);
    // the card goes from pending to the answer in one step: a second answer finds it settled
    let settled = ctx
        .store
        .write(|s| {
            for t in &mut s.list {
                for u in &mut t.turns {
                    if let Some(n) = u.notice.as_mut().filter(|n| n["kind"] == "widget_ask" && n["id"] == id) {
                        if n["state"] != "pending" {
                            return Some(Err("that question was already answered".to_owned()));
                        }
                        // nothing to change on the card: allowing it is allowing the reading
                        let answer = if answer == Mode::Write && n["writes"].as_array().is_some_and(Vec::is_empty) { Mode::Read } else { answer };
                        n["state"] = json!(answer.as_str());
                        n["at"] = json!(at);
                        return Some(Ok(n.clone()));
                    }
                }
            }
            None
        })
        .map_err(|e| V1Error::new("internal", e))?;
    let n = match settled {
        None => return Ok(json!({"ok": false, "error": "no such question"})),
        Some(Err(e)) => return Ok(json!({"ok": false, "error": e})),
        Some(Ok(n)) => n,
    };
    ctx.store.write_index();
    let shown: Vec<String> = n["actions"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
    let stored = Mode::parse(n["state"].as_str().unwrap_or_default()).unwrap_or(answer);
    save(ctx, n["widget"].as_str().unwrap_or_default(), n["source"].as_str().unwrap_or_default(), stored, &shown).map_err(|e| V1Error::new("internal", e))?;
    Ok(json!({"ok": true}))
}

/// `GET /v1/host/widget/grants?name=R37`: what the widget was answered, each with its
/// source's title (and the actions shown, and when).
pub async fn host_grants(ctx: &HubCtx, name: Option<&str>) -> Result<Value, V1Error> {
    let Some(base) = name.and_then(base_of) else { return refused("send ?name=: a widget's name, R<digits>") };
    Ok(json!({"ok": true, "grants": titled(ctx, base, true).await}))
}

/// `POST /v1/host/widget/revoke {name, source}`: the widget is asked again on its next call.
/// A card still waiting for it is settled as `no`, so a stale one cannot grant it later.
pub fn host_revoke(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let Some(base) = b.get("name").and_then(Value::as_str).and_then(base_of).map(str::to_owned) else { return refused("send {name, source}: name is a widget's, R<digits>") };
    let Some(source) = b.get("source").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return refused("send {name, source}") };
    let at = policy::now(ctx);
    let _ = ctx.store.write(|s| {
        for n in s.list.iter_mut().flat_map(|t| t.turns.iter_mut()).filter_map(|u| u.notice.as_mut()) {
            if n["kind"] == "widget_ask" && n["state"] == "pending" && n["widget"] == base && n["source"] == source {
                n["state"] = json!("no");
                n["at"] = json!(at);
            }
        }
    });
    ctx.store.write_index();
    Ok(json!({"ok": true, "removed": remove(ctx, &base, source)}))
}
