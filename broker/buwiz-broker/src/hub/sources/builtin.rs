//! The built-in sources: `timeclock` (the app's own clock-in and clock-out rows),
//! `reminders` (Apple Reminders, on Apple systems only) and `linear` (the user's Linear
//! issues, with their key). What each does is the code the `/data/*` routes had before,
//! moved: `leaf::timeclock`, [`super::reminders`], [`super::linear`].

use serde_json::{json, Value};

use super::{Action, Origin, Ready, Source, KEY_PREFIX};
use crate::hub::ctx::HubCtx;

/// Whether Apple Reminders exist on the system this broker runs on.
pub fn reminders_exist() -> bool {
    cfg!(any(target_os = "macos", target_os = "ios"))
}

fn action(name: &str, about: &str, write: bool, args: &str, returns: &str) -> Action {
    Action { name: name.into(), about: about.into(), write, args: args.into(), returns: returns.into() }
}

fn source(name: &str, title: &str, about: &str, ready: Ready, actions: Vec<Action>) -> Source {
    Source { name: name.into(), title: title.into(), about: about.into(), key: format!("{KEY_PREFIX}{name}"), ready, actions, off: false, origin: Origin::Builtin }
}

/// In `ctx.ext`: whether this system has Apple Reminders, instead of what the build says
/// (so a test is the same on every system).
#[derive(Clone, Copy)]
pub struct Apple(pub bool);

/// The built-ins that work on this system, with whether each is ready.
pub async fn all(ctx: &HubCtx) -> Vec<Source> {
    all_on(ctx, ctx.ext.get::<Apple>().map_or_else(reminders_exist, |a| a.0)).await
}

/// [`all`] on a system with (`apple`) or without Apple Reminders.
pub async fn all_on(ctx: &HubCtx, apple: bool) -> Vec<Source> {
    let mut out = vec![source(
        "timeclock",
        "Time clock",
        "a local clock-in/out tracker kept in the app",
        Ready::Ready,
        vec![
            action("status", "See whether you are clocked in, and today's sessions", false, "", r#"{status:"in"|"out", since:iso|null, todayMin:number, sessions:[{in,out,min}]}"#),
            action("in", "Clock you in", true, "", ""),
            action("out", "Clock you out", true, "", ""),
        ],
    )];
    if apple {
        // only a check that never asks the user: a status read, not a request
        let ready = if super::reminders::local(ctx).reminders_blocked().is_some() { Ready::NeedsPermission } else { Ready::Ready };
        out.push(source(
            "reminders",
            "Reminders",
            "the user's Apple Reminders",
            ready,
            vec![
                action("list", "Open reminders, 50 at most", false, "", "{reminders:[{name,list,due,flagged}]} (or {error})"),
                action("add", "Add a reminder", true, "{name,list?,due?}", ""),
                action("complete", "Complete a reminder", true, "{name}", ""),
            ],
        ));
    }
    let ready = if super::linear::key(ctx).await.is_some() { Ready::Ready } else { Ready::NeedsKey };
    out.push(source(
        "linear",
        "Linear",
        "the user's Linear issues (assigned, else recent workspace issues). Every issue has uid (internal id for mutations), id (FER-12 label), stateId",
        ready,
        vec![
            action("issues", "Your open Linear issues", false, "", r#"{scope:"assigned"|"workspace", issues:[{uid,id,title,state,stateId,priority,assignee,team,url}]} (or {error})"#),
            action("states", "The workflow states of your Linear teams", false, "", "{teams:[{team,states:[{id,name,type}]}]} (transition targets per team)"),
            action("assign", "Assign a Linear issue to you", true, "{uid}", ""),
        ],
    ));
    out
}

/// Runs one built-in action. `Err` is the failure's text; an `Ok` that holds an `{error}` is
/// the source's own way of saying no ([`super::settled`] turns both into `source_error`).
pub async fn run(ctx: &HubCtx, source: &str, action: &str, args: &Value) -> Result<Value, String> {
    let args = if args.is_null() { json!({}) } else { args.clone() };
    match (source, action) {
        ("timeclock", "status") => ctx.db.with(crate::leaf::timeclock::status_of),
        ("timeclock", "in") => ctx.db.with(crate::leaf::timeclock::clock_in_on),
        ("timeclock", "out") => ctx.db.with(crate::leaf::timeclock::clock_out_on),
        ("reminders", "list") => Ok(super::reminders::list(ctx).await),
        ("reminders", "add") => super::reminders::change(ctx, false, &args).await,
        ("reminders", "complete") => super::reminders::change(ctx, true, &args).await,
        ("linear", "issues") => super::linear::issues(ctx).await,
        ("linear", "states") => super::linear::states(ctx).await,
        ("linear", "assign") => super::linear::assign(ctx, &args).await,
        _ => Err(format!("{source} has no action {action}")),
    }
}

/// A built-in source's title and what it is, whether or not this system has it.
pub fn describe(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "timeclock" => Some(("Time clock", "the app's clock-in and clock-out tracker")),
        "reminders" if reminders_exist() => Some(("Reminders", "the user's Apple Reminders")),
        "linear" => Some(("Linear", "the user's Linear issues")),
        _ => None,
    }
}
