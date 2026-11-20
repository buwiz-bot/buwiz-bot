//! The Agent Hub off macOS: the press rules and held calls only. No agent
//! drives a screen here, so nothing listens and a held press is declined.

#[path = "agent_hub/hold.rs"]
pub mod hold;
#[path = "agent_hub/press.rs"]
pub mod press;

use buwiz_agent::OnceId;
use hold::{Answer, Card};

pub fn opened(_n: u64) {}

pub fn closed(_n: u64) {}

pub fn show_card(_n: u64, _once: OnceId, _c: Card, done: Box<dyn FnOnce(Answer) + Send>) {
    done(Answer::Changed)
}

pub fn hide_card(_once: OnceId) {}

pub fn start() {}
