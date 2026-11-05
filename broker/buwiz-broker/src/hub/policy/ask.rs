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
//! - A turn with no one to ask (a routine's, an agent's request) never waits: its call is
//!   refused and kept in Activity as `needs_ok`. On start, a card left pending by a
//!   previous run expires.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::{db, gate, log_ask, now, shell, Change};
use crate::hub::ctx::HubCtx;
use crate::hub::store::uuidv7;
use crate::hub::tools::Run;
use crate::hub::types::TurnCtx;
use crate::js;
use crate::v1::V1Error;

/// How long a card waits for its answer.
pub const WAIT: Duration = Duration::from_secs(600);

/// Another wait than [`WAIT`] (tests), in `ctx.ext`.
pub struct AskWait(pub Duration);

pub const NOT_NOW: &str = "The user chose Not now. Don't retry this call; ask them in words if you still need it.";
pub const NO_ANSWER: &str = "No answer, so this didn't run.";
pub const NOBODY: &str = "This needs the user's OK, and they weren't there.";
pub const WAITING: &str = "Waiting for you";

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
    /// what will happen: "Send a message to Bigmom?"
    pub title: String,
    /// short facts under it ("Subject: …")
    pub lines: Vec<String>,
    /// the exact payload
    pub preview: String,
    /// the turn read outside content (the guard asks even with the switch on)
    pub external: bool,
    /// the shell's "Always allow `<prefix>`"
    pub prefix: Option<String>,
    /// a press on a screen: the answer covers this one call, and leaves no grant
    pub once: bool,
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

/// Asks the user in this turn's chat and waits: → the answer that lets the call run, or
/// what the model is told instead.
pub async fn ask(run: &Run, a: Asking) -> Result<Answer, String> {
    let ctx = &run.ctx;
    let bot = (!a.bot.is_empty()).then(|| a.bot.clone());
    if !present(&run.turn) {
        log_ask(ctx, bot.as_deref(), shown(&a.tool, &a.title, &a.preview), &a.tool, "needs_ok");
        return Err(NOBODY.into());
    }
    let chat = run.thread_id();
    let id = format!("a_{}", uuidv7());
    let bot_name = super::bot_name(ctx, &a.bot);
    let mut n = json!({
        "kind": "tool_ask", "id": id, "bot": a.bot, "botName": bot_name, "chat": chat, "key": a.key, "tool": a.tool,
        "title": a.title, "lines": a.lines, "preview": a.preview, "state": "pending", "at": now(ctx),
    });
    if a.external {
        n["external"] = json!(true);
    }
    if let Some(p) = &a.prefix {
        n["prefix"] = json!(p);
    }
    if a.once {
        n["once"] = json!(true);
    }
    let (tx, mut rx) = oneshot::channel();
    waiting().insert(id.clone(), tx);
    let parked = Parked { ctx: ctx.clone(), chat: chat.clone(), id: id.clone(), bot, what: shown(&a.tool, &a.title, &a.preview).to_owned(), tool: a.tool.clone() };
    if !crate::hub::tools::miniapps::post_in(ctx, &chat, &n) {
        waiting().remove(&id);
        return Err("There is no chat to ask the user in, so this didn't run.".into());
    }
    run.set_job("waiting", WAITING);
    let wait = ctx.ext.get::<AskWait>().map_or(WAIT, |w| w.0);
    let answer = match tokio::time::timeout(wait, &mut rx).await {
        Ok(r) => r.ok(),
        // the answer that came as the wait ran out still counts
        Err(_) if !waiting().contains_key(&id) => rx.await.ok(),
        Err(_) => None,
    };
    drop(parked);
    let (p, d) = crate::hub::tools::activity::tool_activity(ctx, &a.tool, &json!({}));
    run.set_job(&p, &d);
    match answer {
        Some(Answer::No) => Err(NOT_NOW.into()),
        Some(a) => Ok(a),
        None => Err(NO_ANSWER.into()),
    }
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

/// `POST /v1/host/tools/ask {chat, id, answer}`: the person's answer on a pending card.
/// Human only (the host token).
pub fn host_answer(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let str_of = |k: &str| b.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    let (Some(chat), Some(id), Some(ans)) = (str_of("chat"), str_of("id"), str_of("answer")) else {
        return V1Error::err("invalid_args", "send {chat, id, answer: \"once\", \"chat\", \"rule\" or \"no\"}");
    };
    let Some(answer) = Answer::parse(ans) else { return V1Error::err("invalid_args", "answer is \"once\", \"chat\", \"rule\" or \"no\"") };
    let Some(n) = find(ctx, chat, id) else { return V1Error::err("not_found", "no such card in this chat") };
    if n["state"] != "pending" {
        return Err(V1Error::new("stale", NO_ANSWER).with_details(json!({"notice": n})));
    }
    if answer == Answer::Chat && n["once"] == true {
        return V1Error::err("invalid_args", "a press on a screen is allowed one call at a time");
    }
    let key = js::string_of(&n["key"]);
    let thread = ctx.store.read(|s| s.thread(chat).map(|t| t.id.clone())).unwrap_or_default();
    let prefix = n.get("prefix").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
    if answer == Answer::Rule && (key != "shell" || prefix.is_none()) {
        return V1Error::err("invalid_args", "only a shell card with a rule to save takes \"rule\"");
    }
    // the parked call, if it still waits (a restart, a stopped turn: it doesn't)
    let Some(tx) = waiting().remove(id) else {
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
