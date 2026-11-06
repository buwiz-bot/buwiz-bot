//! How long an Agent Hub action may take, for the Bot and the broker together: each wait has to
//! outlast the one inside it, or a finished action reads as lost and gets tried again.

use std::time::Duration;

/// The input of an action: finding its target, the focus, the app settling.
pub const INPUT: Duration = Duration::from_secs(5);
/// A write's word from the broker.
pub const PRESS_WAIT: Duration = Duration::from_secs(25);
/// The Bot's wait for that word.
pub const PRESS_ROUTE: Duration = Duration::from_secs(27);
/// A signed call.
pub const CALL: Duration = Duration::from_secs(30);
/// How long a held call's card waits for the person, in the Bot.
pub const HOLD: Duration = Duration::from_secs(120);
/// How long an action's answer waits for what it started: a press, its card (read before it
/// shows and again at Allow), the person, then its call.
pub const CALLS_WAIT: Duration = Duration::from_secs(PRESS_ROUTE.as_secs() + 2 * CALL.as_secs() + HOLD.as_secs() + CALL.as_secs() + 3);
/// How long an action counts as running in the Bot: the longest one that can finish.
pub const ACT_LIMIT: Duration = Duration::from_secs(INPUT.as_secs() + CALLS_WAIT.as_secs() + 5);
/// How long the broker waits for an action's answer.
pub const ACT_WAIT: Duration = Duration::from_secs(ACT_LIMIT.as_secs() + 10);

/// The event on the Bot's stream that says the broker is opening a screen for the model.
pub const AGENT_OPENED: &str = "agent_opened";
/// That event as it goes down the stream.
pub const AGENT_OPENED_EVENT: &[u8] = b"event: agent_opened\ndata: {}\n\n";

const _: () = assert!(PRESS_WAIT.as_secs() < PRESS_ROUTE.as_secs());
const _: () = assert!(CALLS_WAIT.as_secs() > PRESS_ROUTE.as_secs() + 3 * CALL.as_secs() + HOLD.as_secs());
const _: () = assert!(ACT_LIMIT.as_secs() > INPUT.as_secs() + CALLS_WAIT.as_secs());
const _: () = assert!(ACT_WAIT.as_secs() > ACT_LIMIT.as_secs());

/// What uninstalling a mini app takes with it. Most hold no files of their own.
pub const UNINSTALL_REMOVES: &str = "pins, attachments and data";
/// The same for an app that keeps working copies: unsaved files are named
/// because they can't be fetched again.
pub const UNINSTALL_REMOVES_FILES: &str = "pins, attachments, data and unsaved files";
/// Apps with working copies, whose uninstall deletes them: Google Workspace's
/// `open-<id>/` goes in its `buwiz.uninstall` (its test
/// `uninstall_takes_every_token_and_every_file`).
const KEEPS_FILES: [&str; 1] = ["app.buwiz.google-workspace"];

/// What uninstalling `plugin` takes with it, as every place that asks says it.
pub fn uninstall_removes(plugin: &str) -> &'static str {
    if KEEPS_FILES.contains(&plugin) { UNINSTALL_REMOVES_FILES } else { UNINSTALL_REMOVES }
}
