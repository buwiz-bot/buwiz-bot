//! The activity states an avatar can show, the motion each plays, the
//! ambient effect drawn around it, and the one-shot gestures a click
//! fires.

/// How the body and eyes move while a state lasts. Several states share a
/// motion; `tempo` and `lids` on the state tell them apart.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Motion {
    /// Slow breathing, eyes ahead.
    Rest,
    /// Eyes drift in a slow loop up and around; the head tilts along.
    Ponder,
    /// A steady small bob, eyes down on the work.
    Busy,
    /// Eyes sweep left to right line by line, as if reading.
    Scan,
    /// A gentle side-to-side sway, eyes on the viewer.
    Listen,
    /// Quick soft squashes in bursts, like syllables.
    Talk,
    /// Slow breathing and a glance aside now and then.
    Wait,
    /// Eyes wide; a quick small hop every couple of seconds.
    Alert,
    /// Happy hops with squash and stretch, eyes squinted.
    Cheer,
    /// Repeated small nods.
    Agree,
    /// Sunk low and narrowed, eyes down and drooping.
    Droop,
    /// A fine tremble, eyes narrowed and slanted in.
    Fume,
    /// Eyes shut, deep slow breaths.
    Doze,
    /// Eyes open slowly, then a stretch, in a loop.
    Wake,
    /// Leaning toward the right as if heading off, with a small push.
    Lean,
}

/// What is drawn around an avatar while a state lasts.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Aura {
    /// Two small dots circling the body.
    Orbit,
    /// A ring that grows out of the body and fades.
    Pulse,
    /// A radar arc sweeping around.
    Sweep,
    /// A spinning arc that grows and shrinks.
    Progress,
    /// Small four-point twinkles popping up around the body.
    Twinkle,
    /// Three dots rising off the upper right, like a thought.
    Thought,
}

impl Aura {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Aura::Orbit => "orbit",
            Aura::Pulse => "pulse",
            Aura::Sweep => "sweep",
            Aura::Progress => "progress",
            Aura::Twinkle => "twinkle",
            Aura::Thought => "thought",
        }
    }
}

/// One activity state.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct StateSpec {
    /// The state's name, as runs and `/status:<name>` use it.
    pub(crate) name: &'static str,
    pub(crate) motion: Motion,
    /// Speed of the motion (1 is its normal pace).
    pub(crate) tempo: f32,
    /// How open the eyes are when not blinking (1 is fully).
    pub(crate) lids: f32,
    pub(crate) aura: Option<Aura>,
}

const fn spec(name: &'static str, motion: Motion, tempo: f32, lids: f32, aura: Option<Aura>) -> StateSpec {
    StateSpec { name, motion, tempo, lids, aura }
}

use Aura::*;
use Motion::*;

/// Every state, `idle` first. `/status:demo` walks them in this order.
pub(crate) const STATES: [StateSpec; 50] = [
    spec("idle", Rest, 1.0, 1.0, None),
    spec("listening", Listen, 1.0, 1.0, Some(Pulse)),
    spec("thinking", Ponder, 1.0, 1.0, Some(Thought)),
    spec("streaming", Talk, 1.0, 1.0, None),
    spec("searching", Scan, 1.3, 1.0, Some(Sweep)),
    spec("reading", Scan, 1.0, 0.9, None),
    spec("working", Busy, 1.0, 1.0, None),
    spec("coding", Busy, 1.35, 0.9, None),
    spec("running", Busy, 1.6, 1.0, None),
    spec("computing", Busy, 1.2, 0.85, Some(Orbit)),
    spec("controlling", Lean, 1.0, 1.0, Some(Orbit)),
    spec("waiting", Wait, 1.0, 0.85, None),
    spec("blocked", Alert, 0.8, 1.0, Some(Pulse)),
    spec("finished", Cheer, 1.0, 0.7, Some(Twinkle)),
    spec("failed", Droop, 1.0, 0.7, None),
    spec("sleeping", Doze, 1.0, 0.0, None),
    spec("waking", Wake, 1.0, 1.0, None),
    spec("happy", Cheer, 0.9, 0.75, None),
    spec("excited", Cheer, 1.4, 1.0, Some(Twinkle)),
    spec("laughing", Talk, 1.5, 0.45, None),
    spec("proud", Agree, 0.8, 0.75, None),
    spec("curious", Ponder, 1.3, 1.1, None),
    spec("playful", Cheer, 1.2, 0.9, None),
    spec("shy", Droop, 0.8, 0.8, None),
    spec("surprised", Alert, 1.0, 1.2, None),
    spec("suspicious", Ponder, 0.6, 0.5, None),
    spec("angry", Fume, 1.0, 0.55, None),
    spec("confused", Ponder, 0.8, 1.0, Some(Thought)),
    spec("sad", Droop, 0.7, 0.65, None),
    spec("drowsy", Doze, 0.8, 0.35, None),
    spec("bored", Wait, 0.6, 0.6, None),
    spec("scared", Alert, 1.6, 1.2, None),
    spec("celebrate", Cheer, 1.25, 0.7, Some(Twinkle)),
    spec("approving", Agree, 1.0, 0.8, None),
    spec("orbit", Ponder, 1.0, 1.0, Some(Orbit)),
    spec("radar", Scan, 1.2, 1.0, Some(Sweep)),
    spec("progress", Busy, 1.0, 1.0, Some(Progress)),
    spec("spawning", Busy, 1.2, 1.0, Some(Pulse)),
    spec("loading", Busy, 0.8, 1.0, Some(Progress)),
    spec("dictating", Listen, 1.3, 1.0, Some(Pulse)),
    spec("sending", Lean, 1.0, 1.0, None),
    spec("receiving", Listen, 0.8, 1.0, Some(Orbit)),
    spec("uploading", Lean, 0.8, 1.0, Some(Progress)),
    spec("writing", Busy, 1.1, 0.9, None),
    spec("alerting", Alert, 1.0, 1.1, Some(Pulse)),
    spec("bouncing", Cheer, 1.0, 1.0, None),
    spec("powering-down", Doze, 0.6, 0.2, None),
    spec("humming", Listen, 0.7, 0.6, None),
    spec("notifying", Alert, 1.2, 1.0, Some(Pulse)),
    spec("dragging", Lean, 1.2, 1.0, None),
];

/// The state named `name`: empty means `idle`; a name not in `STATES`
/// (a new run phase) shows as `working`.
pub(crate) fn state(name: &str) -> &'static StateSpec {
    let name = name.trim();
    if name.is_empty() {
        return &STATES[0];
    }
    STATES
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| STATES.iter().find(|s| s.name == "working").unwrap_or(&STATES[0]))
}

/// Whether `name` is one of `STATES`.
pub(crate) fn is_state(name: &str) -> bool {
    STATES.iter().any(|s| s.name == name)
}

/// The ambient effect of the state named `name`, if it has one.
pub(crate) fn aura_of(name: &str) -> Option<Aura> {
    STATES.iter().find(|s| s.name == name).and_then(|s| s.aura)
}

/// One-shot moves a click on the status avatar plays, in turn.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Gesture {
    /// A full turn of the body.
    Spin,
    /// One hop with squash and stretch.
    Bounce,
    /// Two quick nods.
    Nod,
}

impl Gesture {
    pub(crate) const ALL: [Gesture; 3] = [Gesture::Spin, Gesture::Bounce, Gesture::Nod];

    /// Seconds one play takes.
    pub(crate) fn seconds(self) -> f64 {
        match self {
            Gesture::Spin => 0.8,
            Gesture::Bounce => 0.6,
            Gesture::Nod => 0.7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_idle_first() {
        assert_eq!(STATES[0].name, "idle");
        let mut seen = std::collections::HashSet::new();
        for s in STATES {
            assert!(seen.insert(s.name), "{} twice", s.name);
            assert!(s.tempo > 0.0 && (0.0..=1.3).contains(&s.lids), "{}", s.name);
        }
    }

    #[test]
    fn every_state_has_a_label() {
        for s in STATES {
            if s.name != "idle" {
                assert!(!crate::statusfx::status_label(s.name).is_empty(), "{}", s.name);
            }
        }
    }

    #[test]
    fn lookups() {
        assert_eq!(state("").name, "idle");
        assert_eq!(state("thinking").motion, Motion::Ponder);
        assert_eq!(state("sleeping").motion, Motion::Doze);
        assert_eq!(state("finished").motion, Motion::Cheer);
        assert_eq!(state("failed").motion, Motion::Droop);
        assert_eq!(state("some-new-phase").name, "working");
        assert!(is_state("powering-down") && !is_state("some-new-phase"));
        assert_eq!(aura_of("searching"), Some(Aura::Sweep));
        assert_eq!(aura_of("idle"), None);
        assert_eq!(aura_of("nope"), None);
    }

    #[test]
    fn every_run_phase_state_is_known() {
        // the states store.rs's phase map produces
        for name in [
            "thinking", "searching", "reading", "orbit", "writing", "coding", "progress", "running",
            "computing", "controlling", "working", "sending", "waiting", "streaming", "uploading",
            "finished", "failed",
        ] {
            assert!(is_state(name), "{name}");
        }
    }
}
