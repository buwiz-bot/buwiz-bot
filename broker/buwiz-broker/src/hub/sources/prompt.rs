//! What the model is told of the data sources: under the contract's DATA SOURCES bullet, one
//! block per source that is ready for the Bot, each action as the call a widget writes.
//!
//! ```text
//!   reminders — the user's Apple Reminders
//!      source("reminders").call("list") → {reminders:[{name,list,due,flagged}]} (or {error})
//!      source("reminders").call("add", {name,list?,due?})  — changes data
//! ```

use super::{listed, Origin, Ready, Source};
use crate::hub::ctx::HubCtx;

/// What stands for the list when no source is ready.
pub const NONE: &str = "  (none right now: build the widget from its own state, and never invent a data source)";

/// The blocks for `owner`'s ready sources (a Bot's id, or `@chats`).
pub async fn block(ctx: &HubCtx, owner: &str) -> String {
    let ready: Vec<Source> = listed(ctx, owner).await.into_iter().filter(|s| s.ready == Ready::Ready).collect();
    text(&ready)
}

/// The blocks for these sources.
pub fn text(sources: &[Source]) -> String {
    if sources.is_empty() {
        return NONE.to_owned();
    }
    sources.iter().map(source_block).collect::<Vec<_>>().join("\n")
}

fn source_block(s: &Source) -> String {
    let plugin = matches!(s.origin, Origin::Plugin { .. });
    let what = if s.about.is_empty() { &s.title } else { &s.about };
    let mut out = format!("  {} — {what}", s.name);
    for a in &s.actions {
        let args = if a.args.is_empty() { String::new() } else { format!(", {}", a.args) };
        let call = format!("source(\"{}\").call(\"{}\"{args})", s.name, a.name);
        // a plugin's own words say what its action does; a built-in's `about` is for the card
        let says = if plugin { a.about.as_str() } else { "" };
        let tail = match (a.write, a.returns.is_empty(), says.is_empty()) {
            (true, _, true) => "  — changes data".to_owned(),
            (true, _, false) => format!("  — {says}; changes data"),
            (false, false, _) => format!(" → {}", a.returns),
            (false, true, false) => format!("  — {says}"),
            (false, true, true) => String::new(),
        };
        out.push_str(&format!("\n     {call}{tail}"));
    }
    out
}
