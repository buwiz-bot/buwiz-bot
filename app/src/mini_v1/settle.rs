//! When an inline card's app is done building (6.5): the engine lays an
//! app out over several frames, so a card shown at once has its buttons
//! pop in and its height jump. The card holds its app hidden until the
//! layout settles, with a placeholder over it once the hold is long
//! enough to see, then reveals it. An app that asks `inline.wait_ready`
//! loads its data after it draws, so its hold is long by design: its
//! placeholder is up from the mount.
//!
//! Settled is the layout's signature (every visible frame's origin and
//! size, hashed) unchanged for [`STABLE_TICKS`] ticks [`TICK`] apart; an
//! app whose manifest asks `inline.wait_ready` also waits for its
//! `buwiz.ready()`. Neither may hold a card for long: a wall-clock cap from
//! the mount ([`CAP`], [`CAP_READY`]) reveals it whatever it does.
//!
//! The decisions are pure (modelled on the bridge's `Fitter`); `mini.rs`
//! ticks it and the card draws what it says.

use std::time::{Duration, Instant};

/// How often a settling card's layout is read.
pub const TICK: Duration = Duration::from_millis(33);
/// Ticks in a row whose signature matched the one before: settled.
pub const STABLE_TICKS: u32 = 2;
/// A hold shorter than this shows nothing: a fast card never flashes a
/// placeholder. A `wait_ready` app's shows at once.
pub const SHOW_AFTER: Duration = Duration::from_millis(100);
/// A placeholder once up stays this long: shorter is a flicker.
pub const MIN_SHOWN: Duration = Duration::from_millis(250);
/// The longest hold, from the mount.
pub const CAP: Duration = Duration::from_millis(1500);
/// The longest hold of an app that waits for its `buwiz.ready()`.
pub const CAP_READY: Duration = Duration::from_millis(3000);
/// The placeholder's fade out, in ms.
pub const FADE_MS: f32 = 175.0;

/// What a tick changed, for the card to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The hold is long enough to see: the placeholder goes up. A
    /// `wait_ready` app's card put it up at its mount and hears nothing.
    Placeholder,
    /// Settled (or capped): the app shows.
    Settled,
}

/// How the placeholder gives way to the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// Fades out over [`FADE_MS`], its grid animated.
    Fade,
    /// Reduce Motion: a plain swap, the grid still.
    Swap,
}

/// The reveal under the person's Reduce Motion setting.
pub fn reveal(reduce_motion: bool) -> Reveal {
    if reduce_motion { Reveal::Swap } else { Reveal::Fade }
}

/// One mounted card's hold. A mount makes a new one; the app's loads in
/// the same mount (a reload, a plugin running again) leave it be.
#[derive(Debug)]
pub struct Settle {
    mounted: Instant,
    wait_ready: bool,
    ready: bool,
    last: Option<u64>,
    stable: u32,
    /// Since when the placeholder is up.
    shown: Option<Instant>,
    settled: bool,
}

impl Settle {
    /// A card mounted at `now`: held.
    pub fn new(now: Instant) -> Settle {
        Settle { mounted: now, wait_ready: false, ready: false, last: None, stable: 0, shown: None, settled: false }
    }

    /// The app loaded into the card, its manifest asking `wait_ready` or
    /// not. Once the card has settled this changes nothing: a reload
    /// replaces the app in place, the card stays shown. Before, the new
    /// app's layout starts over. A `wait_ready` app's card has had its
    /// placeholder up since the mount.
    pub fn loaded(&mut self, wait_ready: bool) {
        if self.settled {
            return;
        }
        self.wait_ready = wait_ready;
        if wait_ready {
            self.shown.get_or_insert(self.mounted);
        }
        self.ready = false;
        self.last = None;
        self.stable = 0;
    }

    /// The app called `buwiz.ready()`: its data is in. What it draws with it
    /// comes in the next frames, so the layout counts as stable only from
    /// here.
    pub fn ready(&mut self) {
        if self.settled || self.ready {
            return;
        }
        self.ready = true;
        self.stable = 0;
    }

    /// Whether the app is still held.
    pub fn holding(&self) -> bool {
        !self.settled
    }

    /// A tick at `now` with the layout's signature, `None` while the app
    /// has no sized frame: what changed, if anything.
    pub fn tick(&mut self, now: Instant, signature: Option<u64>) -> Option<Phase> {
        if self.settled {
            return None;
        }
        match signature {
            Some(s) if self.last == Some(s) => self.stable += 1,
            _ => self.stable = 0,
        }
        self.last = signature;
        let age = now.saturating_duration_since(self.mounted);
        let cap = if self.wait_ready { CAP_READY } else { CAP };
        let stable = self.stable >= STABLE_TICKS && (!self.wait_ready || self.ready);
        // a placeholder up stays its minimum, past the cap too
        let seen = self.shown.is_none_or(|at| now.saturating_duration_since(at) >= MIN_SHOWN);
        if (stable || age >= cap) && seen {
            self.settled = true;
            return Some(Phase::Settled);
        }
        if self.shown.is_none() && age >= SHOW_AFTER {
            self.shown = Some(now);
            return Some(Phase::Placeholder);
        }
        None
    }
}

/// Where a card's hold is, as the card draws it: nothing over the held
/// app at first, the placeholder once the hold is long enough to see, its
/// fade once the app shows, then nothing again. Inline cards and generated
/// widget cards share it (`mini::hold_heard`, `hold_cleanup` and
/// `hold_overlay`; the widget's start and marking are `Shown::start` and
/// `Hold::revealed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    Quiet,
    Placeholder,
    Fading,
    Done,
}

impl Hold {
    /// A card's hold at its mount. `held` is whether it holds at all (a
    /// widget already shown once does not); `wait_ready` puts the
    /// placeholder up from the mount.
    pub fn start(held: bool, wait_ready: bool) -> Hold {
        match (held, wait_ready) {
            (false, _) => Hold::Done,
            (true, true) => Hold::Placeholder,
            (true, false) => Hold::Quiet,
        }
    }

    /// The hold after the card's `placeholder` or `settled` event, `None`
    /// when it stays. `Fading` is followed by `Done` after [`FADE_MS`].
    pub fn hear(self, event: &str, reduce_motion: bool) -> Option<Hold> {
        match (event, self) {
            ("placeholder", Hold::Quiet) => Some(Hold::Placeholder),
            // settled before the placeholder was due: nothing to take down
            ("settled", Hold::Quiet) => Some(Hold::Done),
            ("settled", Hold::Placeholder) => Some(match reveal(reduce_motion) {
                Reveal::Swap => Hold::Done,
                Reveal::Fade => Hold::Fading,
            }),
            _ => None,
        }
    }

    /// Whether the app has been unhidden: it fades in or is shown. This is
    /// the moment a widget counts as seen, not when the fade ends.
    pub fn revealed(self) -> bool {
        matches!(self, Hold::Fading | Hold::Done)
    }

    /// Whether the grid's progress (`Store::art_started`) is dropped on
    /// the move from `prev` to `now`: once, when the card is revealed. A
    /// card that is left held keeps it, so coming back goes on from there;
    /// one mounted already revealed never had it.
    pub fn drops_progress(prev: Option<Hold>, now: Hold) -> bool {
        now.revealed() && prev.is_some_and(|p| !p.revealed())
    }

    /// Whether the placeholder is over the app.
    pub fn covers(self) -> bool {
        matches!(self, Hold::Placeholder | Hold::Fading)
    }
}

/// The generated widgets (by module name) that have been shown once. A
/// widget card leaves its chat's screen when the chat is switched away
/// from and is mounted again on the way back: that is a remount, which
/// would hold again. A widget in this set does not.
#[derive(Debug, Default)]
pub struct Shown(std::collections::HashSet<String>);

impl Shown {
    /// `app` showed: its card's hold is over.
    pub fn mark(&mut self, app: &str) {
        self.0.insert(app.to_owned());
    }

    /// The hold a card mounting the widget `app` starts in.
    pub fn start(&self, app: &str) -> Hold {
        Hold::start(self.holds(app), false)
    }

    /// `app`'s card is now in `hold`: it counts as shown once revealed.
    pub fn note(&mut self, app: &str, hold: Hold) {
        if hold.revealed() {
            self.mark(app);
        }
    }

    /// Whether a card mounting `app` holds it.
    pub fn holds(&self, app: &str) -> bool {
        !self.0.contains(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Ticks `s` every [`TICK`] from `from` with the signatures `sigs`:
    /// each tick's time and what it changed.
    fn run(s: &mut Settle, t0: Instant, from: Duration, sigs: &[Option<u64>]) -> Vec<(Duration, Option<Phase>)> {
        sigs.iter()
            .enumerate()
            .map(|(i, sig)| {
                let at = from + TICK * i as u32;
                (at, s.tick(t0 + at, *sig))
            })
            .collect()
    }

    #[test]
    fn two_stable_ticks_settle_it() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        s.loaded(false);
        assert_eq!(s.tick(t0 + ms(10), Some(7)), None);
        assert_eq!(s.tick(t0 + ms(43), Some(7)), None, "one stable tick is not enough");
        assert!(s.holding());
        assert_eq!(s.tick(t0 + ms(76), Some(7)), Some(Phase::Settled));
        assert!(!s.holding());
        assert_eq!(s.tick(t0 + ms(109), Some(8)), None, "settled stays settled");
    }

    #[test]
    fn a_change_resets_the_count() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        let seen = run(&mut s, t0, ms(0), &[Some(1), Some(1), Some(2), Some(2), Some(2)]);
        let settled: Vec<_> = seen.iter().filter(|(_, p)| *p == Some(Phase::Settled)).collect();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].0, TICK * 4, "the change starts the count over");
    }

    #[test]
    fn no_sized_frame_never_counts_as_stable() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        assert_eq!(s.tick(t0, None), None);
        assert_eq!(s.tick(t0 + ms(33), None), None);
        assert_eq!(s.tick(t0 + ms(66), None), None);
        assert!(s.holding());
    }

    #[test]
    fn a_card_that_never_settles_shows_at_the_cap() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        s.loaded(false);
        let mut n = 0u64;
        let mut at = ms(0);
        let shown = loop {
            n += 1;
            if let Some(Phase::Settled) = s.tick(t0 + at, Some(n)) {
                break at;
            }
            at += TICK;
            assert!(at < ms(2000));
        };
        assert!(shown >= CAP && shown < CAP + TICK, "{shown:?}");
    }

    #[test]
    fn wait_ready_holds_for_the_app_up_to_three_seconds() {
        let t0 = Instant::now();
        // never ready: the longer cap
        let mut s = Settle::new(t0);
        s.loaded(true);
        assert_eq!(s.tick(t0 + ms(50), Some(1)), None);
        assert_eq!(s.tick(t0 + ms(150), Some(1)), None, "its placeholder has been up since the mount");
        assert_eq!(s.tick(t0 + ms(1600), Some(1)), None, "a stable skeleton is not ready");
        assert_eq!(s.tick(t0 + ms(2999), Some(1)), None);
        assert_eq!(s.tick(t0 + ms(3000), Some(1)), Some(Phase::Settled));

        // ready: then two stable ticks after it, counting from the call
        let mut s = Settle::new(t0);
        s.loaded(true);
        run(&mut s, t0, ms(0), &[Some(1), Some(1), Some(1), Some(1)]);
        s.ready();
        assert_ne!(s.tick(t0 + ms(200), Some(1)), Some(Phase::Settled), "the data's frames may not be drawn yet");
        assert_eq!(s.tick(t0 + ms(233), Some(2)), None);
        assert_eq!(s.tick(t0 + ms(266), Some(2)), None);
        assert_eq!(s.tick(t0 + ms(299), Some(2)), Some(Phase::Settled));
    }

    #[test]
    fn ready_without_wait_ready_changes_nothing() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        s.loaded(false);
        s.ready();
        assert_eq!(s.tick(t0, Some(1)), None);
        assert_eq!(s.tick(t0 + ms(33), Some(1)), None);
        assert_eq!(s.tick(t0 + ms(66), Some(1)), Some(Phase::Settled));
    }

    #[test]
    fn nothing_shows_under_100_ms() {
        let t0 = Instant::now();
        // a fast card: settled before the placeholder was due
        let mut s = Settle::new(t0);
        let seen = run(&mut s, t0, ms(0), &[Some(1), Some(1), Some(1), Some(1), Some(1), Some(1)]);
        assert!(!seen.iter().any(|(_, p)| *p == Some(Phase::Placeholder)));
        // a slow one: the placeholder at the first tick past 100 ms, once
        let mut s = Settle::new(t0);
        let seen = run(&mut s, t0, ms(0), &(0..10).map(Some).collect::<Vec<_>>());
        let up: Vec<_> = seen.iter().filter(|(_, p)| *p == Some(Phase::Placeholder)).map(|(at, _)| *at).collect();
        assert_eq!(up, [TICK * 4]);
        assert!(up[0] >= SHOW_AFTER);
    }

    #[test]
    fn wait_ready_shows_the_placeholder_from_the_mount_for_250_ms() {
        // a fast app: its data in and drawn well inside 250 ms
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        s.loaded(true);
        s.ready();
        let seen = run(&mut s, t0, ms(0), &[Some(1); 12]);
        assert!(!seen.iter().any(|(_, p)| *p == Some(Phase::Placeholder)), "the card put it up itself");
        let settled: Vec<_> = seen.iter().filter(|(_, p)| *p == Some(Phase::Settled)).map(|(at, _)| *at).collect();
        assert_eq!(settled, [TICK * 8], "the first tick 250 ms after the mount");
        assert!(settled[0] >= MIN_SHOWN);
    }

    #[test]
    fn a_placeholder_up_stays_250_ms() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        s.loaded(false);
        // still moving at 100 ms: the placeholder goes up, then it settles
        let seen = run(&mut s, t0, ms(0), &[Some(1), Some(2), Some(3), Some(4), Some(5), Some(5), Some(5)]);
        assert_eq!(seen[4], (TICK * 4, Some(Phase::Placeholder)));
        assert!(!seen.iter().any(|(_, p)| *p == Some(Phase::Settled)), "settled 66 ms in: held");
        assert_eq!(s.tick(t0 + TICK * 4 + ms(249), Some(5)), None);
        assert_eq!(s.tick(t0 + TICK * 4 + MIN_SHOWN, Some(5)), Some(Phase::Settled));

        // the cap waits for it too
        let mut s = Settle::new(t0);
        assert_eq!(s.tick(t0 + ms(1400), None), Some(Phase::Placeholder));
        assert_eq!(s.tick(t0 + ms(1500), None), None);
        assert_eq!(s.tick(t0 + ms(1650), None), Some(Phase::Settled));
    }

    #[test]
    fn the_cap_past_100_ms_skips_the_placeholder() {
        // a card first ticked late (its cell laid out late) settles: it
        // never puts the placeholder up only to take it down
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        assert_eq!(s.tick(t0 + ms(1600), None), Some(Phase::Settled));
        assert_eq!(s.tick(t0 + ms(1633), None), None);
    }

    #[test]
    fn reduce_motion_swaps() {
        assert_eq!(reveal(false), Reveal::Fade);
        assert_eq!(reveal(true), Reveal::Swap);
    }

    #[test]
    fn a_reload_does_not_rearm_and_a_remount_does() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        s.loaded(false);
        run(&mut s, t0, ms(0), &[Some(1), Some(1), Some(1)]);
        assert!(!s.holding());
        // a reload (the plugin ran again, Reload): a new app, same card
        s.loaded(true);
        assert!(!s.holding());
        assert_eq!(s.tick(t0 + ms(500), None), None);
        assert_eq!(s.tick(t0 + ms(3500), None), None);
        // a remount (Refresh to live, a rebuilt cell): a new hold
        let t1 = t0 + ms(4000);
        let mut s = Settle::new(t1);
        assert!(s.holding());
        assert_eq!(s.tick(t1 + ms(120), Some(4)), Some(Phase::Placeholder));
    }

    #[test]
    fn a_reload_before_settling_starts_the_layout_over() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        s.loaded(false);
        assert_eq!(s.tick(t0, Some(1)), None);
        assert_eq!(s.tick(t0 + ms(33), Some(1)), None);
        s.loaded(false);
        assert_eq!(s.tick(t0 + ms(66), Some(1)), None, "the old app's count is gone");
        assert_eq!(s.tick(t0 + ms(99), Some(1)), None);
        assert_eq!(s.tick(t0 + ms(132), Some(1)), Some(Phase::Settled));
    }

    #[test]
    fn a_widget_card_starts_held_and_reveals_when_settled() {
        let shown = Shown::default();
        let mut hold = Hold::start(shown.holds("R1"), false);
        assert_eq!(hold, Hold::Quiet);
        assert!(!hold.covers());
        // drive the settle machine as the card does
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        let seen = run(&mut s, t0, ms(0), &(0..10).map(Some).chain([Some(9); 12]).collect::<Vec<_>>());
        for (_, phase) in seen {
            let event = match phase {
                Some(Phase::Placeholder) => "placeholder",
                Some(Phase::Settled) => "settled",
                None => continue,
            };
            if let Some(next) = hold.hear(event, false) {
                hold = next;
            }
            if event == "placeholder" {
                assert!(hold.covers());
            }
        }
        assert_eq!(hold, Hold::Fading, "settled over a shown placeholder fades");
        assert!(hold.covers());
        assert!(!s.holding());
    }

    #[test]
    fn the_placeholder_does_not_show_under_100_ms_for_a_widget() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        let mut hold = Hold::start(true, false);
        for (_, phase) in run(&mut s, t0, ms(0), &[Some(1); 4]) {
            assert_ne!(phase, Some(Phase::Placeholder));
        }
        // settled before it was due: straight to Done, never covered
        assert_eq!(hold.hear("settled", false), Some(Hold::Done));
        hold = Hold::Done;
        assert!(!hold.covers());
    }

    #[test]
    fn reduce_motion_swaps_a_held_card() {
        assert_eq!(Hold::Placeholder.hear("settled", true), Some(Hold::Done));
        assert_eq!(Hold::Placeholder.hear("settled", false), Some(Hold::Fading));
    }

    #[test]
    fn events_after_the_reveal_change_nothing() {
        for h in [Hold::Fading, Hold::Done] {
            assert_eq!(h.hear("settled", false), None);
            assert_eq!(h.hear("placeholder", false), None);
        }
        assert_eq!(Hold::Placeholder.hear("placeholder", false), None);
    }

    #[test]
    fn a_wait_ready_inline_card_starts_with_its_placeholder() {
        assert_eq!(Hold::start(true, true), Hold::Placeholder);
        assert_eq!(Hold::start(true, false), Hold::Quiet);
    }

    #[test]
    fn a_reload_of_a_widget_does_not_rearm() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        run(&mut s, t0, ms(0), &[Some(1), Some(1), Some(1)]);
        // the widget's source changed (a revise): the same card, still shown
        s.loaded(false);
        assert!(!s.holding());
    }

    #[test]
    fn a_widgets_start_and_marking_follow_the_set() {
        let mut shown = Shown::default();
        // first mount holds
        assert_eq!(shown.start("R1"), Hold::Quiet);
        // not marked while covered or quiet
        for h in [Hold::Quiet, Hold::Placeholder] {
            shown.note("R1", h);
            assert_eq!(shown.start("R1"), Hold::Quiet, "{h:?}: a remount before the reveal holds again");
        }
        // marked the moment it is revealed, fade still pending
        shown.note("R1", Hold::Fading);
        assert_eq!(shown.start("R1"), Hold::Done, "a remount after it starts revealed");
        // another widget is unaffected
        assert_eq!(shown.start("R2"), Hold::Quiet);
        // a direct reveal (settled before the placeholder) marks too
        shown.note("R3", Hold::Done);
        assert_eq!(shown.start("R3"), Hold::Done);
    }

    #[test]
    fn the_grids_progress_is_kept_until_the_reveal() {
        // held, covered, left and remounted: never dropped
        assert!(!Hold::drops_progress(None, Hold::Quiet));
        assert!(!Hold::drops_progress(Some(Hold::Quiet), Hold::Placeholder));
        // dropped once, at the reveal
        assert!(Hold::drops_progress(Some(Hold::Placeholder), Hold::Fading));
        assert!(Hold::drops_progress(Some(Hold::Quiet), Hold::Done));
        assert!(!Hold::drops_progress(Some(Hold::Fading), Hold::Done));
        // a card mounted already revealed has nothing to drop
        assert!(!Hold::drops_progress(None, Hold::Done));
    }

    #[test]
    fn switching_chats_away_and_back_does_not_rearm_a_shown_widget() {
        let mut shown = Shown::default();
        assert!(shown.holds("R1"), "first mount holds");
        shown.mark("R1");
        // away (unmounted) and back (mounted again)
        assert_eq!(Hold::start(shown.holds("R1"), false), Hold::Done);
        assert!(shown.holds("R2"), "another widget still holds");
        // a widget left before it settled holds again on the way back
        assert_eq!(Hold::start(shown.holds("R2"), false), Hold::Quiet);
    }
}
