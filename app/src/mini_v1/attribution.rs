//! Who a mini app's call speaks for: the person, or the agent.
//!
//! A press reaches `buwiz.call` through the app's own async code, so a
//! call can't be tied to the press that caused it. What is known is the
//! input around it, so that decides:
//!
//! - Until an agent has acted, every call is the person's.
//! - An agent's action, on any surface, makes every surface the agent's:
//!   what it presses may open another surface, and a menu item it fires
//!   has none. A surface opened later starts as the agent's too.
//! - A surface comes back to the person only by the person's own press or
//!   key in that surface, once the agent is quiet ([`quiet`]). Typing in
//!   the chat gives nothing in a mini app back.
//! - Before that, the person's input changes nothing: the call goes out as
//!   the agent's. A wrong refusal costs a second press; a wrong "person"
//!   would run a person-only tool.
//!
//! A call takes its actor when the app makes it, not when it is signed: a
//! queued call of the agent's stays the agent's. The broker turns the
//! actor into the origin the plugin sees, and reads still go out as the
//! app's: the screen stays alive (`broker/.../helper/sessions.rs`).

use std::{
    collections::HashSet,
    sync::Mutex,
    time::{Duration, Instant},
};

/// How long after the agent's last action, or the end of a call of its,
/// the person's input can't take a surface back: covers the debounce and
/// the next step of what it started.
const SETTLE: Duration = Duration::from_secs(2);
/// How long after the agent's last action its calls can keep it from
/// being quiet: a screen that polls would otherwise never settle. As long
/// as one call may last.
const MAX_CHAIN: Duration = Duration::from_secs(30);

/// Who a call speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Person,
    Agent,
}

impl Actor {
    /// As the signed envelope says it.
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::Person => "person",
            Actor::Agent => "agent",
        }
    }
}

/// The input seen so far.
#[derive(Debug, Default)]
pub struct Attribution {
    last_action: Option<Instant>,
    last_call_end: Option<Instant>,
    calls: usize,
    /// The surfaces the person has taken back since the agent's last action.
    person: HashSet<u64>,
}

impl Attribution {
    /// The agent is acting.
    pub fn agent_acted(&mut self, now: Instant) {
        self.last_action = Some(now);
        self.person.clear();
    }

    /// A call of the agent's left the app.
    pub fn call_began(&mut self) {
        self.calls += 1;
    }

    /// A call of the agent's is over.
    pub fn call_ended(&mut self, now: Instant) {
        self.calls = self.calls.saturating_sub(1);
        self.last_call_end = Some(now);
    }

    /// The person pressed or typed in `surface`.
    pub fn person_pressed(&mut self, surface: u64, now: Instant) {
        if self.quiet(now) {
            self.person.insert(surface);
        }
    }

    /// Whether what the agent started has had time to finish.
    fn quiet(&self, now: Instant) -> bool {
        let Some(action) = self.last_action else { return true };
        let since = |t: Instant| now.saturating_duration_since(t);
        if since(action) < SETTLE {
            return false;
        }
        since(action) >= MAX_CHAIN || (self.calls == 0 && self.last_call_end.is_none_or(|t| since(t) >= SETTLE))
    }

    /// Who a call from `surface` speaks for now.
    pub fn actor(&self, surface: u64) -> Actor {
        if self.last_action.is_none() || self.person.contains(&surface) { Actor::Person } else { Actor::Agent }
    }
}

static STATE: Mutex<Option<Attribution>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Attribution) -> R) -> R {
    f(STATE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default())
}

/// [`Attribution::actor`] of this process's input.
pub fn actor(surface: u64) -> Actor {
    with(|a| a.actor(surface))
}

/// A call of the agent's began (`true`) or ended, in this process.
pub fn agent_call(running: bool) {
    with(|a| if running { a.call_began() } else { a.call_ended(Instant::now()) });
}

/// Starts listening to nsh's input. On the main thread, before the first
/// window, so no action goes unheard.
#[cfg(target_os = "macos")]
pub fn watch(mtm: nsh_platform_macos::objc2::MainThreadMarker) {
    use nsh_platform_macos::Input;
    nsh_platform_macos::on_input(mtm, |input| match input {
        Input::Agent => with(|a| a.agent_acted(Instant::now())),
        Input::Person { surface } => with(|a| a.person_pressed(surface, Instant::now())),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORM: u64 = 3;
    const CHAT: u64 = 1;

    fn secs(t: Instant, s: u64) -> Instant {
        t + Duration::from_secs(s)
    }

    #[test]
    fn with_no_agent_every_call_is_the_persons() {
        let a = Attribution::default();
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    #[test]
    fn an_agents_action_takes_every_surface_even_one_opened_later() {
        let t = Instant::now();
        let mut a = Attribution::default();
        a.person_pressed(CHAT, t);
        a.agent_acted(t);
        assert_eq!(a.actor(FORM), Actor::Agent);
        assert_eq!(a.actor(CHAT), Actor::Agent);
        assert_eq!(a.actor(99), Actor::Agent);
    }

    #[test]
    fn only_the_persons_press_in_that_surface_takes_it_back() {
        let t = Instant::now();
        let mut a = Attribution::default();
        a.agent_acted(t);
        // nothing clears by itself
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.person_pressed(CHAT, secs(t, 60));
        assert_eq!(a.actor(CHAT), Actor::Person);
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.person_pressed(FORM, secs(t, 61));
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    #[test]
    fn a_press_in_the_settle_window_changes_nothing_and_needs_doing_again() {
        let t = Instant::now();
        let mut a = Attribution::default();
        a.agent_acted(t);
        a.person_pressed(FORM, t + SETTLE - Duration::from_millis(1));
        assert_eq!(a.actor(FORM), Actor::Agent);
        // the window passing doesn't bring that press back
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.person_pressed(FORM, t + SETTLE);
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    #[test]
    fn a_new_action_takes_back_what_the_person_held() {
        let t = Instant::now();
        let mut a = Attribution::default();
        a.agent_acted(t);
        a.person_pressed(FORM, secs(t, 5));
        a.agent_acted(secs(t, 6));
        assert_eq!(a.actor(FORM), Actor::Agent);
    }

    #[test]
    fn a_call_in_flight_and_the_settle_after_it_hold_the_agents_chain() {
        let t = Instant::now();
        let mut a = Attribution::default();
        a.agent_acted(t);
        a.call_began();
        // the action is long past, but its call is still out
        a.person_pressed(FORM, secs(t, 5));
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.call_ended(secs(t, 6));
        // what comes after the answer gets its settle too
        a.person_pressed(FORM, secs(t, 7));
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.person_pressed(FORM, secs(t, 8));
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    #[test]
    fn a_polling_screen_cant_keep_the_person_out_for_ever() {
        let t = Instant::now();
        let mut a = Attribution::default();
        a.agent_acted(t);
        a.call_began();
        a.person_pressed(FORM, t + MAX_CHAIN - Duration::from_secs(1));
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.person_pressed(FORM, t + MAX_CHAIN);
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    #[test]
    fn actors_are_named_as_the_broker_reads_them() {
        assert_eq!(Actor::Person.as_str(), "person");
        assert_eq!(Actor::Agent.as_str(), "agent");
    }
}
