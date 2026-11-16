//! Registered data sources for generated widgets (#250). A widget reads and
//! changes the person's own data through one helper, `source("reminders").call("list")`,
//! never through a URL of its own: this registry says which sources there are, what each
//! can do, whether it works right now and whether the Bot's switch for it is on.
//!
//! - A source is one of the built-ins (`timeclock`, `reminders`, `linear`; their code is
//!   [`builtin`], [`reminders`] and [`linear`]) or a v1 plugin's: the plugin's tools whose
//!   `_meta["buwiz/audience"]` has `"widget"` ([`plugin`]), under the plugin's short name.
//! - [`all`] lists them for a Bot (a Bot's id, or [`crate::hub::policy::CHATS`]); [`resolve`]
//!   and [`run`] check and run one action; [`call`] does both.
//! - [`ticket`] says who may call (a widget's own ticket), [`grants`] what the person
//!   allowed, [`route`] answers `POST /w/<name>/<ticket>/call` with both, and [`prompt`]
//!   tells the model which sources the widget it writes may call.

pub mod builtin;
pub mod grants;
pub mod linear;
pub mod plugin;
pub mod prompt;
pub mod reminders;
pub mod route;
pub mod ticket;
#[cfg(test)]
mod tests;

use serde_json::{json, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::policy;
use crate::hub::types::{Pattern, TurnCtx, Verb};
use crate::hub::Registry;

/// The switch key of a built-in source (a plugin's is `v1:<plugin id>`).
pub const KEY_PREFIX: &str = "source:";

/// Whether a listed source can be used right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    Ready,
    NeedsKey,
    NeedsPermission,
    NeedsAccount,
}

impl Ready {
    pub fn as_str(self) -> &'static str {
        match self {
            Ready::Ready => "ready",
            Ready::NeedsKey => "needs_key",
            Ready::NeedsPermission => "needs_permission",
            Ready::NeedsAccount => "needs_account",
        }
    }
}

/// One thing a source does. `write` is true for an action that changes data.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub name: String,
    /// a short phrase a person reads on the permission card ("Add a reminder")
    pub about: String,
    pub write: bool,
    /// the arguments as the prompt shows them (`{name,list?,due?}`; empty for none)
    pub args: String,
    /// what a read answers, as the prompt shows it (empty: `about` stands in)
    pub returns: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    Builtin,
    /// a v1 plugin: its id and short name
    Plugin { id: String, short: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// what widget code writes: `source("<name>")`
    pub name: String,
    pub title: String,
    pub about: String,
    /// the Bot's switch for it: `source:<name>` or `v1:<plugin id>`
    pub key: String,
    pub ready: Ready,
    pub actions: Vec<Action>,
    /// the Bot's switch is off: not listed, and a call says `off_for_bot`
    pub off: bool,
    pub origin: Origin,
}

impl Source {
    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    /// The record `GET /sources` answers: `ready` says whether it can be used now, `state`
    /// why not.
    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "about": self.about,
            "ready": self.ready == Ready::Ready,
            "state": self.ready.as_str(),
            "actions": self.actions.iter().map(|a| json!({"name": a.name, "about": a.about, "write": a.write})).collect::<Vec<_>>(),
        })
    }
}

/// Why a call went no further: the code a widget reads, and a plain sentence.
#[derive(Clone, Debug, PartialEq)]
pub struct CallError {
    pub code: &'static str,
    pub message: String,
}

impl CallError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// Who a call is for: a widget's own code, or the model's `query_source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller<'a> {
    Widget(&'a str),
    Agent,
}

/// Every source this system has for `owner` (a Bot's id or `@chats`), in the order the prompt
/// lists them, those switched off for it marked `off`.
pub async fn all(ctx: &HubCtx, owner: &str) -> Vec<Source> {
    let mut out = builtin::all(ctx).await;
    out.extend(plugin::all(ctx));
    for s in &mut out {
        s.off = policy::tool_state(ctx, owner, &s.key) == policy::Pos::Off;
    }
    out
}

/// The sources listed for `owner`: [`all`] without those switched off for it.
pub async fn listed(ctx: &HubCtx, owner: &str) -> Vec<Source> {
    all(ctx, owner).await.into_iter().filter(|s| !s.off).collect()
}

/// The profile whose switches `turn` runs under: its Bot's id, else `@chats`.
pub fn owner_of_turn(ctx: &HubCtx, turn: &TurnCtx) -> String {
    ctx.store.read(|s| crate::hub::library::botctx::bot_of(s, turn).map(|b| b.id.clone())).unwrap_or_else(|| policy::CHATS.to_owned())
}

/// A source and one of its actions, found and ready.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub source: Source,
    pub action: Action,
}

/// The first checks of a call, in the contract's order: the source is listed for `owner`
/// (`unknown_source`, `off_for_bot`), has the action (`unknown_action`) and is ready
/// (`not_ready`).
pub async fn resolve(ctx: &HubCtx, owner: &str, source: &str, action: &str) -> Result<Resolved, CallError> {
    let Some(s) = all(ctx, owner).await.into_iter().find(|s| s.name == source) else {
        return Err(CallError::new("unknown_source", format!("there is no data source called {}", shown(source))));
    };
    if s.off {
        return Err(CallError::new("off_for_bot", format!("{} is switched off for this Bot", s.title)));
    }
    let Some(a) = s.action(action).cloned() else {
        return Err(CallError::new("unknown_action", format!("{} has no action called {}", s.title, shown(action))));
    };
    if s.ready != Ready::Ready {
        return Err(CallError::new("not_ready", format!("{} is not ready: {}", s.title, s.ready.as_str())));
    }
    Ok(Resolved { source: s, action: a })
}

/// A name from outside, short enough and plain enough for a sentence.
fn shown(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(40).collect();
    format!("\"{t}\"")
}

/// A failure's message as a widget gets it: at most 500 characters.
pub fn cut(s: &str) -> String {
    s.chars().take(500).collect()
}

/// The result a source answers, or why it did not: an `{error}` answer (a source's own way of
/// saying no) is a `source_error`.
fn settled(result: Result<Value, String>) -> Result<Value, CallError> {
    match result {
        Err(e) => Err(CallError::new("source_error", cut(&e))),
        Ok(v) => match v.get("error").filter(|e| crate::js::truthy(e)) {
            Some(e) => {
                let hint = v.get("hint").and_then(Value::as_str).map(|h| format!(" ({h})")).unwrap_or_default();
                Err(CallError::new("source_error", cut(&format!("{}{hint}", crate::js::string_of(e)))))
            }
            None => Ok(v),
        },
    }
}

/// Runs a resolved action. Whether the caller may is the caller's to have checked (a
/// widget's grant; the model's read-only rule).
pub async fn run(ctx: &HubCtx, caller: Caller<'_>, r: &Resolved, args: &Value) -> Result<Value, CallError> {
    match &r.source.origin {
        Origin::Builtin => settled(builtin::run(ctx, &r.source.name, &r.action.name, args).await),
        Origin::Plugin { id, short } => plugin::run(ctx, caller, id, short, &r.action.name, args).await,
    }
}

/// [`resolve`] and [`run`]: one action of one source for one Bot, nothing between (no grant,
/// no limit).
pub async fn call(ctx: &HubCtx, owner: &str, caller: Caller<'_>, source: &str, action: &str, args: &Value) -> Result<Value, CallError> {
    let r = resolve(ctx, owner, source, action).await?;
    run(ctx, caller, &r, args).await
}

/// `query_source`: the model reads one source for `owner`: `action` (default: the source's
/// first action that only reads) with `args`. Only actions that read, and for a plugin only
/// those it also offers the model (`"agent"` in its audience); what changes data is a
/// widget's to call, once the person allowed it.
pub async fn query(ctx: &HubCtx, owner: &str, source: &str, action: Option<&str>, args: &Value) -> Result<Value, CallError> {
    let action = match action {
        Some(a) => a.to_owned(),
        None => match listed(ctx, owner).await.iter().find(|s| s.name == source).and_then(|s| s.actions.iter().find(|a| !a.write)) {
            Some(a) => a.name.clone(),
            None => String::new(),
        },
    };
    let r = resolve(ctx, owner, source, &action).await?;
    if r.action.write {
        return Err(CallError::new("write_action", format!("{} changes data: only a widget the person allowed may call it", r.action.name)));
    }
    if let Origin::Plugin { id, short } = &r.source.origin {
        let tool = format!("{short}_{}", r.action.name);
        let for_agent = crate::hub::tools::v1::tool_def(ctx, id, &tool).is_some_and(|t| plugin::for_agent(&t));
        if !for_agent {
            return Err(CallError::new("widget_only", format!("{} is offered to widgets only", r.action.name)));
        }
    }
    run(ctx, Caller::Agent, &r, args).await
}

/// `GET /sources`: `{sources}` for `owner`, the ready ones and the listed ones that are not.
pub async fn sources_json(ctx: &HubCtx, owner: &str) -> Value {
    json!({ "sources": listed(ctx, owner).await.iter().map(Source::json).collect::<Vec<_>>() })
}

/// Registers `POST /w/<name>/<ticket>/call`.
pub fn install(routes: &mut Registry, _ctx: &mut HubCtx) {
    routes.route(Verb::Post, Pattern::Prefix("/w/"), route::call);
}
