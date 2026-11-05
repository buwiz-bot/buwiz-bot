//! Who a mini app's call speaks for: the person, or the agent.
//!
//! A press reaches `buwiz.call` through the app's own async code, so a
//! call can't be tied to the press that caused it. What is known is the
//! input around it, so that decides:
//!
//! - With nothing watching the input, every call is the agent's: no one
//!   can say the person made it.
//! - Until an agent has acted, every call is the person's.
//! - An agent's action, on any surface, makes every surface the agent's:
//!   what it presses may open another surface, and a menu item it fires
//!   has none. A surface opened later starts as the agent's too. The broker
//!   opening a screen for the model is such an action ([`agent_opened`]).
//! - A surface comes back to the person only by the person's own press or
//!   key in that surface, once the agent is quiet ([`quiet`]). Typing in
//!   the chat gives nothing in a mini app back.
//! - Before that, the person's input changes nothing: the call goes out as
//!   the agent's. A wrong refusal costs a second press; a wrong "person"
//!   would run a person-only tool.
//!
//! - While a write of the agent's is out, or its card is up, nothing is
//!   quiet, for as long as it takes. [`MAX_CHAIN`] is for polling reads alone.
//!
//! **What this can't do.** A call can't be tied to the press that caused it,
//! so an app's delayed call goes by the moment it is made. If the AI presses
//! and the app sits on a timer past the settle with nothing in flight, then
//! the person's own press in that surface takes it back, and what fires next
//! goes out as the person's, though the AI started it. Nothing here can see
//! that: the app holds the timer, and no call is out to watch. It takes an
//! app that delays a write after a press with no call between, and the
//! person pressing in that surface in the gap. Closing it needs the app to
//! say which press a call belongs to.
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
/// How long after the agent's last action its reads can keep it from being
/// quiet: a screen that polls would otherwise never settle. As long as one
/// call may last. A write has no such cap.
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
    /// Input reaches this: nothing below means anything without it.
    watching: bool,
    last_action: Option<Instant>,
    last_call_end: Option<Instant>,
    calls: usize,
    /// Calls of the agent's that may write: out, or waiting on their card.
    writes: usize,
    last_write_end: Option<Instant>,
    /// The surfaces the person has taken back since the agent's last action.
    person: HashSet<u64>,
}

impl Attribution {
    /// Input reaches this from now on.
    fn watch(&mut self) {
        self.watching = true;
    }

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

    /// A call of the agent's may write from now on.
    pub fn write_began(&mut self) {
        self.writes += 1;
    }

    /// That call is over, or turned out to read.
    pub fn write_ended(&mut self, now: Instant) {
        self.writes = self.writes.saturating_sub(1);
        self.last_write_end = Some(now);
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
        if self.writes > 0 || since(action) < SETTLE || self.last_write_end.is_some_and(|t| since(t) < SETTLE) {
            return false;
        }
        since(action) >= MAX_CHAIN || (self.calls == 0 && self.last_call_end.is_none_or(|t| since(t) >= SETTLE))
    }

    /// Who a call from `surface` speaks for now.
    pub fn actor(&self, surface: u64) -> Actor {
        if self.watching && (self.last_action.is_none() || self.person.contains(&surface)) { Actor::Person } else { Actor::Agent }
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

/// How many calls of the agent's are out, in this process.
pub fn agent_calls() -> usize {
    with(|a| a.calls)
}

/// A call of the agent's began (`true`) or ended, in this process.
pub fn agent_call(running: bool) {
    with(|a| if running { a.call_began() } else { a.call_ended(Instant::now()) });
}

/// A call of the agent's that may write began (`true`) or ended, in this
/// process.
pub fn agent_write(running: bool) {
    with(|a| if running { a.write_began() } else { a.write_ended(Instant::now()) });
}

/// The broker is opening a screen for the model: the agent is acting, though
/// no input of nsh's says so.
pub fn agent_opened() {
    with(|a| a.agent_acted(Instant::now()));
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
    with(Attribution::watch);
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORM: u64 = 3;
    const CHAT: u64 = 1;

    fn secs(t: Instant, s: u64) -> Instant {
        t + Duration::from_secs(s)
    }

    fn watched() -> Attribution {
        Attribution { watching: true, ..Attribution::default() }
    }

    /// What it does to this process's input: the rest of a surface's life is
    /// the tests here, on an [`Attribution`] of their own.
    #[test]
    fn a_screen_the_broker_opens_for_the_model_counts_as_the_agent_acting() {
        with(|a| a.last_action = None);
        agent_opened();
        assert!(with(|a| a.last_action.is_some()));
    }

    #[test]
    fn with_no_agent_every_call_is_the_persons() {
        assert_eq!(watched().actor(FORM), Actor::Person);
    }

    #[test]
    fn with_nothing_watching_every_call_is_the_agents() {
        let t = Instant::now();
        let mut a = Attribution::default();
        assert_eq!(a.actor(FORM), Actor::Agent);
        // nothing a stray report says makes it the person's
        a.person_pressed(FORM, t);
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.watch();
        assert_eq!(a.actor(CHAT), Actor::Person);
    }

    #[test]
    fn an_agents_action_takes_every_surface_even_one_opened_later() {
        let t = Instant::now();
        let mut a = watched();
        a.person_pressed(CHAT, t);
        a.agent_acted(t);
        assert_eq!(a.actor(FORM), Actor::Agent);
        assert_eq!(a.actor(CHAT), Actor::Agent);
        assert_eq!(a.actor(99), Actor::Agent);
    }

    #[test]
    fn only_the_persons_press_in_that_surface_takes_it_back() {
        let t = Instant::now();
        let mut a = watched();
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
        let mut a = watched();
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
        let mut a = watched();
        a.agent_acted(t);
        a.person_pressed(FORM, secs(t, 5));
        a.agent_acted(secs(t, 6));
        assert_eq!(a.actor(FORM), Actor::Agent);
    }

    #[test]
    fn a_call_in_flight_and_the_settle_after_it_hold_the_agents_chain() {
        let t = Instant::now();
        let mut a = watched();
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
        let mut a = watched();
        a.agent_acted(t);
        a.call_began();
        a.person_pressed(FORM, t + MAX_CHAIN - Duration::from_secs(1));
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.person_pressed(FORM, t + MAX_CHAIN);
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    /// Past the cap, a read that polls gives way, and a write doesn't.
    #[test]
    fn a_write_in_flight_keeps_the_agents_chain_however_long_it_takes() {
        let t = Instant::now();
        let mut a = watched();
        a.agent_acted(t);
        a.call_began();
        a.write_began();
        a.person_pressed(FORM, t + MAX_CHAIN);
        a.person_pressed(FORM, secs(t, 600));
        assert_eq!(a.actor(FORM), Actor::Agent, "a card can be up for minutes");
        a.write_ended(secs(t, 601));
        a.call_ended(secs(t, 601));
        // what comes after its answer gets its settle, past the cap too
        a.person_pressed(FORM, secs(t, 602));
        assert_eq!(a.actor(FORM), Actor::Agent);
        a.person_pressed(FORM, secs(t, 603));
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    #[test]
    fn a_call_that_turned_out_to_read_leaves_only_the_cap() {
        let t = Instant::now();
        let mut a = watched();
        a.agent_acted(t);
        a.call_began();
        a.write_began();
        a.write_ended(t);
        a.person_pressed(FORM, t + MAX_CHAIN);
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    /// The gap the module says it has: stated, so a change that closes it
    /// has to say so.
    #[test]
    fn a_delayed_call_with_nothing_in_flight_cant_be_told_from_the_persons() {
        let t = Instant::now();
        let mut a = watched();
        a.agent_acted(t);
        a.person_pressed(FORM, secs(t, 5));
        // the app's timer fires now, from the AI's press
        assert_eq!(a.actor(FORM), Actor::Person);
    }

    #[test]
    fn actors_are_named_as_the_broker_reads_them() {
        assert_eq!(Actor::Person.as_str(), "person");
        assert_eq!(Actor::Agent.as_str(), "agent");
    }
}
