//! How long an Agent Hub action may take, for the Bot and the broker together: each wait has to
//! outlast the one inside it, or a finished action reads as lost and gets tried again.

use std::time::Duration;

/// The input of an action: finding its target, the focus, the app settling.
pub const INPUT: Duration = Duration::from_secs(5);
/// A write's word from the broker, a person's answer included.
pub const PRESS_WAIT: Duration = Duration::from_secs(25);
/// The Bot's wait for that word.
pub const PRESS_ROUTE: Duration = Duration::from_secs(27);
/// A signed call.
pub const CALL: Duration = Duration::from_secs(30);
/// How long an action's answer waits for what it started: a press, then its call.
pub const CALLS_WAIT: Duration = Duration::from_secs(PRESS_ROUTE.as_secs() + CALL.as_secs() + 3);
/// How long an action counts as running in the Bot: the longest one that can finish.
pub const ACT_LIMIT: Duration = Duration::from_secs(INPUT.as_secs() + CALLS_WAIT.as_secs() + 5);
/// How long the broker waits for an action's answer.
pub const ACT_WAIT: Duration = Duration::from_secs(ACT_LIMIT.as_secs() + 10);

const _: () = assert!(PRESS_WAIT.as_secs() < PRESS_ROUTE.as_secs());
const _: () = assert!(CALLS_WAIT.as_secs() > PRESS_ROUTE.as_secs() + CALL.as_secs());
const _: () = assert!(ACT_LIMIT.as_secs() > INPUT.as_secs() + CALLS_WAIT.as_secs());
const _: () = assert!(ACT_WAIT.as_secs() > ACT_LIMIT.as_secs());
