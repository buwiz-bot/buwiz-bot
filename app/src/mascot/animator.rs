//! The per-avatar animation state: which activity state plays and for how
//! long, blinks, where the eyes look, a gesture in progress, a shape
//! morph, and the short hand-off between poses when any of those change.
//! `motion_pose` is a pure function of the state and the time, so the same
//! inputs always give the same frame.

use std::f32::consts::{PI, TAU};

use super::figure::{clamp_look, Body, Figure, Pose, REST_LOOK};
use super::outline::Shape;
use super::states::{Gesture, Motion, StateSpec};

/// Seconds a hand-off between two poses takes.
const HANDOFF_SECONDS: f32 = 0.35;
/// Seconds a shape morph takes.
const MORPH_SECONDS: f32 = 0.6;
/// Seconds one blink takes.
const BLINK_SECONDS: f64 = 0.17;
/// How quickly the eyes catch up with where they should look (per second).
const GAZE_RATE: f32 = 9.0;
/// How quickly emphasis fades in and out (per second).
const EMPHASIS_RATE: f32 = 10.0;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn wave(seconds: f32, period: f32, offset: f32) -> f32 {
    (TAU * seconds / period + offset).sin()
}

/// The pose of `state` at `seconds` into it. `offset` (radians) shifts the
/// slow cycles so a list of avatars doesn't move in lockstep; `compact`
/// drops the outline wobble small avatars can't show.
pub(crate) fn motion_pose(state: &StateSpec, seconds: f64, offset: f32, compact: bool) -> Pose {
    let s = (seconds as f32) * state.tempo;
    let mut pose = Pose { eye_open: state.lids, ..Pose::REST };
    match state.motion {
        Motion::Rest => {
            pose.stretch = 1.0 + 0.012 * wave(s, 4.2, offset);
        }
        Motion::Ponder => {
            let turn = TAU * s / 5.5 + offset;
            pose.look = [0.15 + 0.55 * turn.cos(), 0.55 + 0.3 * turn.sin()];
            pose.lean = 0.05 * (turn + 1.0).sin();
            pose.stretch = 1.0 + 0.01 * wave(s, 3.0, offset);
        }
        Motion::Busy => {
            let bob = (PI * s * 1.5).sin().abs();
            pose.shift[1] = -0.035 * bob;
            pose.stretch = 1.0 + 0.04 * (bob - 0.4);
            pose.look = [0.3 + 0.12 * wave(s, 3.1, offset), -0.45];
        }
        Motion::Scan => {
            let line_seconds = 1.8;
            let progress = (s / line_seconds).fract();
            let line = (s / line_seconds).floor() as i64 % 3;
            let x = if progress < 0.82 {
                -0.65 + 1.4 * smoothstep(0.0, 0.82, progress)
            } else {
                0.75 - 1.4 * smoothstep(0.82, 1.0, progress)
            };
            pose.look = [x, 0.05 - 0.22 * line as f32];
            pose.lean = 0.04 * x;
        }
        Motion::Listen => {
            let sway = wave(s, 2.4, offset);
            pose.lean = 0.06 * sway;
            pose.shift[0] = 0.025 * sway;
            pose.stretch = 1.0 + 0.015 * wave(s, 1.2, offset);
            pose.look = [0.1 + 0.15 * sway, 0.25];
        }
        Motion::Talk => {
            let syllable = (TAU * 3.0 * s).sin().max(0.0);
            let phrase = smoothstep(0.2, 0.6, 0.5 + 0.5 * wave(s, 1.7, offset));
            let speak = syllable * phrase;
            pose.stretch = 0.98 + 0.05 * speak;
            pose.shift[1] = -0.01 * speak;
            pose.look = [0.45, 0.35];
        }
        Motion::Wait => {
            let cycle = (s / 7.0 + offset / TAU).rem_euclid(1.0);
            let aside = smoothstep(0.42, 0.5, cycle) - smoothstep(0.92, 1.0, cycle);
            pose.look = [-0.55 + 1.15 * aside, 0.15 + 0.1 * aside];
            pose.stretch = 1.0 + 0.015 * wave(s, 5.0, offset);
        }
        Motion::Alert => {
            let cycle = (s / 2.2).fract();
            let hop = if cycle < 0.18 { (PI * cycle / 0.18).sin() } else { 0.0 };
            let land = if (0.18..0.3).contains(&cycle) { (PI * (cycle - 0.18) / 0.12).sin() } else { 0.0 };
            pose.shift[1] = -0.04 - 0.12 * hop;
            pose.stretch = 1.0 + 0.1 * hop - 0.08 * land;
            pose.look = [0.0, 0.2];
            pose.eye_size = 1.12;
        }
        Motion::Cheer => {
            let cycle = (s / 0.85).fract();
            let air = if cycle < 0.7 { (PI * cycle / 0.7).sin() } else { 0.0 };
            let land = if cycle >= 0.7 { (PI * (cycle - 0.7) / 0.3).sin() } else { 0.0 };
            pose.shift[1] = -0.16 * air;
            pose.stretch = 1.0 + 0.1 * air - 0.14 * land;
            pose.lean = 0.05 * wave(s, 1.7, offset);
            pose.look = [0.45, 0.55];
        }
        Motion::Agree => {
            let nod = (TAU * s / 1.1).sin().max(0.0);
            pose.shift[1] = 0.05 * nod;
            pose.stretch = 1.0 - 0.04 * nod;
            pose.look = [0.35, 0.4 - 0.6 * nod];
            pose.eye_open = state.lids * (1.0 - 0.25 * nod);
        }
        Motion::Droop => {
            let drift = wave(s, 6.5, offset);
            pose.shift[1] = 0.06;
            pose.stretch = 0.93 + 0.012 * wave(s, 5.0, offset);
            pose.swell = 0.97;
            pose.lean = 0.035 * drift;
            pose.look = [-0.2 + 0.15 * drift, -0.65];
            pose.eye_tilt = 0.12;
        }
        Motion::Fume => {
            pose.shift[0] = 0.014 * (TAU * 9.0 * s).sin();
            pose.stretch = 1.0 + 0.025 * wave(s, 1.4, offset);
            pose.look = [0.1, 0.05];
            pose.eye_tilt = -0.18;
            pose.ripple = 0.012;
            pose.ripple_phase = 7.0 * s;
        }
        Motion::Doze => {
            pose.stretch = 1.0 + 0.035 * wave(s, 3.8, offset);
            pose.lean = 0.04 * wave(s, 7.6, offset);
            pose.look = [0.3, -0.35];
        }
        Motion::Wake => {
            let cycle = (s / 3.2).fract();
            let open = smoothstep(0.0, 0.3, cycle) * (1.0 - smoothstep(0.9, 1.0, cycle));
            pose.eye_open = state.lids * (0.1 + 0.9 * open);
            let reach = if (0.35..0.65).contains(&cycle) { (PI * (cycle - 0.35) / 0.3).sin() } else { 0.0 };
            pose.stretch = 1.0 + 0.12 * reach;
            pose.look = [0.6 * (TAU * cycle).sin(), 0.4];
        }
        Motion::Lean => {
            let push_cycle = (s / 1.3).fract();
            let push = if push_cycle < 0.3 { (PI * push_cycle / 0.3).sin() } else { 0.0 };
            pose.lean = 0.13 + 0.03 * wave(s, 1.3, offset);
            pose.shift = [0.05 + 0.03 * wave(s, 1.3, offset), -0.05 * push];
            pose.look = [0.9, 0.15];
        }
    }
    if compact {
        pose.ripple = 0.0;
    }
    pose
}

/// The state's still frame: its pose at the very start, without wobble.
/// Used for paused and reduced-motion avatars.
pub(crate) fn still_pose(state: &StateSpec) -> Pose {
    motion_pose(state, 0.0, 0.0, true)
}

/// A gesture in progress.
#[derive(Copy, Clone, Debug)]
struct GesturePlay {
    gesture: Gesture,
    elapsed: f64,
    repeats: f64,
}

impl GesturePlay {
    fn length(&self) -> f64 {
        self.gesture.seconds() * self.repeats
    }

    fn apply(&self, pose: &mut Pose) {
        let overall = (self.elapsed / self.length()).clamp(0.0, 1.0) as f32;
        let local = if overall >= 1.0 { 1.0 } else { (overall * self.repeats as f32).fract() };
        match self.gesture {
            Gesture::Spin => {
                pose.lean += TAU * smoothstep(0.0, 1.0, local);
            }
            Gesture::Bounce => {
                let air = (PI * local).sin();
                pose.shift[1] -= 0.22 * air;
                pose.stretch *= 1.0 + 0.1 * air;
            }
            Gesture::Nod => {
                let nod = (TAU * 2.0 * local).sin().max(0.0);
                pose.shift[1] += 0.06 * nod;
                pose.look[1] -= 0.5 * nod;
                pose.eye_open *= 1.0 - 0.3 * nod;
            }
        }
    }
}

/// One avatar's animation.
pub(crate) struct Animator {
    state: &'static StateSpec,
    body: Body,
    /// Seconds into the current state.
    clock: f64,
    /// Per-avatar offset of the slow cycles.
    offset: f32,
    random: u32,
    /// Seconds until the next blink, or into the current one when
    /// `blinking`.
    blink_timer: f64,
    blinking: bool,
    blink_again: bool,
    /// The pose being left and how far the hand-off has come.
    previous: Option<Pose>,
    handoff: f32,
    gaze_target: Option<[f32; 2]>,
    gaze: [f32; 2],
    emphasis: f32,
    emphasis_target: f32,
    gesture: Option<GesturePlay>,
    resting: bool,
    compact: bool,
}

impl Animator {
    pub(crate) fn new(shape: Shape, state: &'static StateSpec, seed: u32, resting: bool, compact: bool) -> Animator {
        let random = seed.max(1);
        let mut animator = Animator {
            state,
            body: Body::of(shape),
            clock: 0.0,
            offset: (seed % 6283) as f32 / 1000.0,
            random,
            blink_timer: 0.0,
            blinking: false,
            blink_again: false,
            previous: None,
            handoff: 1.0,
            gaze_target: None,
            gaze: REST_LOOK,
            emphasis: 0.0,
            emphasis_target: 0.0,
            gesture: None,
            resting,
            compact,
        };
        animator.blink_timer = animator.next_blink_wait();
        animator.gaze = animator.desired_look();
        animator
    }

    #[cfg(test)]
    pub(crate) fn shape(&self) -> Shape {
        self.body.to
    }

    fn next_random(&mut self) -> f64 {
        // xorshift32
        let mut x = self.random;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random = x;
        x as f64 / u32::MAX as f64
    }

    fn next_blink_wait(&mut self) -> f64 {
        2.2 + 3.8 * self.next_random()
    }

    /// The pose before blinks, gaze, emphasis and gestures.
    fn base_pose(&self) -> Pose {
        let target = if self.resting {
            still_pose(self.state)
        } else {
            motion_pose(self.state, self.clock, self.offset, self.compact)
        };
        match &self.previous {
            Some(previous) => previous.mix(&target, smoothstep(0.0, 1.0, self.handoff)),
            None => target,
        }
    }

    fn desired_look(&self) -> [f32; 2] {
        clamp_look(self.gaze_target.unwrap_or_else(|| self.base_pose().look))
    }

    fn begin_handoff(&mut self) {
        self.previous = Some(self.base_pose());
        self.handoff = 0.0;
    }

    pub(crate) fn set_state(&mut self, state: &'static StateSpec) {
        if std::ptr::eq(state, self.state) {
            return;
        }
        self.begin_handoff();
        self.state = state;
        self.clock = 0.0;
    }

    pub(crate) fn set_resting(&mut self, resting: bool) {
        if resting != self.resting {
            self.begin_handoff();
            self.resting = resting;
        }
    }

    pub(crate) fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
    }

    /// A new body; with `morph` the old one blends into it.
    pub(crate) fn set_shape(&mut self, shape: Shape, morph: bool) {
        if shape == self.body.to && self.body.blend >= 1.0 {
            return;
        }
        self.body = if morph {
            Body { from: self.body.to, to: shape, blend: 0.0 }
        } else {
            Body::of(shape)
        };
    }

    pub(crate) fn set_emphasis(&mut self, on: bool) {
        self.emphasis_target = if on { 1.0 } else { 0.0 };
    }

    /// Point the eyes at `target` (x right, y up, clamped into the unit
    /// disk), or back to what the state does with `None`.
    pub(crate) fn look_at(&mut self, target: Option<[f64; 2]>) {
        self.gaze_target = target.map(|[x, y]| clamp_look([x as f32, y as f32]));
    }

    /// Puts the eyes where they should look now, skipping the ease (a new
    /// avatar starts there).
    pub(crate) fn settle_gaze(&mut self) {
        self.gaze = self.desired_look();
    }

    /// Play `gesture` `repeats` times (at least once), replacing one in
    /// progress.
    pub(crate) fn play(&mut self, gesture: Gesture, repeats: f64) {
        let repeats = if repeats.is_finite() { repeats.clamp(1.0, 4.0) } else { 1.0 };
        self.gesture = Some(GesturePlay { gesture, elapsed: 0.0, repeats });
    }

    #[cfg(test)]
    pub(crate) fn is_playing(&self) -> bool {
        self.gesture.is_some()
    }

    /// Nothing is moving: a resting avatar with no gesture, blink, morph,
    /// hand-off, or eyes or emphasis still on their way.
    pub(crate) fn is_settled(&self) -> bool {
        let look = self.desired_look();
        self.resting
            && self.gesture.is_none()
            && !self.blinking
            && self.previous.is_none()
            && self.body.blend >= 1.0
            && (self.gaze[0] - look[0]).abs() < 0.003
            && (self.gaze[1] - look[1]).abs() < 0.003
            && (self.emphasis - self.emphasis_target).abs() < 0.003
    }

    /// Moves everything `dt` seconds on. A resting avatar's state clock and
    /// blinks hold still; its eyes, morphs and gestures still finish.
    pub(crate) fn advance(&mut self, dt: f64) {
        if dt <= 0.0 || !dt.is_finite() {
            return;
        }
        let step = dt as f32;
        if !self.resting {
            self.clock += dt;
            self.advance_blink(dt);
        } else {
            self.blinking = false;
        }
        if self.previous.is_some() {
            self.handoff += step / HANDOFF_SECONDS;
            if self.handoff >= 1.0 {
                self.previous = None;
                self.handoff = 1.0;
            }
        }
        if self.body.blend < 1.0 {
            self.body.blend = (self.body.blend + step / MORPH_SECONDS).min(1.0);
            if self.body.blend >= 1.0 {
                self.body = Body::of(self.body.to);
            }
        }
        if let Some(play) = &mut self.gesture {
            play.elapsed += dt;
            if play.elapsed >= play.length() {
                self.gesture = None;
            }
        }
        let ease = |rate: f32| 1.0 - (-rate * step).exp();
        let look = self.desired_look();
        let k = ease(GAZE_RATE);
        self.gaze = [self.gaze[0] + (look[0] - self.gaze[0]) * k, self.gaze[1] + (look[1] - self.gaze[1]) * k];
        if (self.gaze[0] - look[0]).abs() < 0.002 && (self.gaze[1] - look[1]).abs() < 0.002 {
            self.gaze = look;
        }
        self.emphasis += (self.emphasis_target - self.emphasis) * ease(EMPHASIS_RATE);
        if (self.emphasis - self.emphasis_target).abs() < 0.002 {
            self.emphasis = self.emphasis_target;
        }
    }

    fn advance_blink(&mut self, dt: f64) {
        if self.state.lids < 0.15 {
            self.blinking = false;
            return;
        }
        if self.blinking {
            self.blink_timer += dt;
            if self.blink_timer >= BLINK_SECONDS {
                self.blinking = false;
                if self.blink_again {
                    self.blink_again = false;
                    self.blink_timer = 0.1;
                } else {
                    self.blink_timer = self.next_blink_wait();
                }
            }
        } else {
            self.blink_timer -= dt;
            if self.blink_timer <= 0.0 {
                self.blinking = true;
                self.blink_again = self.next_random() < 0.18;
                self.blink_timer = 0.0;
            }
        }
    }

    /// The frame to draw now.
    pub(crate) fn pose(&self) -> Pose {
        let mut pose = self.base_pose();
        pose.look = self.gaze;
        if self.blinking {
            let closed = (PI * (self.blink_timer / BLINK_SECONDS) as f32).sin();
            pose.eye_open *= 1.0 - 0.94 * closed;
        }
        pose.swell *= 1.0 + 0.06 * self.emphasis;
        pose.eye_size *= 1.0 + 0.05 * self.emphasis;
        if let Some(play) = &self.gesture {
            play.apply(&mut pose);
        }
        pose
    }

    pub(crate) fn figure(&self) -> Figure {
        Figure::build(self.body, &self.pose())
    }
}

/// A stable seed from a string (a bot id): FNV-1a, never zero.
pub(crate) fn seed_from(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mascot::states::{state, STATES};

    #[test]
    fn every_state_animates_without_nan() {
        for spec in &STATES {
            for frame in 0..240 {
                let pose = motion_pose(spec, frame as f64 / 30.0, 1.3, false);
                let numbers = [
                    pose.shift[0], pose.shift[1], pose.stretch, pose.lean, pose.swell, pose.look[0],
                    pose.look[1], pose.eye_open, pose.eye_size, pose.eye_tilt,
                ];
                assert!(numbers.iter().all(|n| n.is_finite()), "{} frame {frame}", spec.name);
                assert!(pose.stretch > 0.8 && pose.stretch < 1.2, "{}", spec.name);
                assert!(pose.shift[1] > -0.3 && pose.shift[1] < 0.15, "{}", spec.name);
            }
        }
    }

    #[test]
    fn motions_are_distinct() {
        // sample each motion family's poses over a few seconds; no two
        // families trace the same path
        let families: Vec<&StateSpec> = {
            let mut seen = Vec::new();
            let mut out = Vec::new();
            for spec in &STATES {
                if !seen.contains(&spec.motion) {
                    seen.push(spec.motion);
                    out.push(spec);
                }
            }
            out
        };
        assert_eq!(families.len(), 15);
        let trace = |spec: &StateSpec| -> Vec<f32> {
            (0..60)
                .flat_map(|i| {
                    let p = motion_pose(spec, i as f64 * 0.1, 0.0, true);
                    [p.shift[0], p.shift[1], p.stretch, p.lean, p.look[0], p.look[1], p.eye_open, p.eye_tilt]
                })
                .collect()
        };
        for (i, a) in families.iter().enumerate() {
            for b in &families[i + 1..] {
                let gap: f32 = trace(a).iter().zip(trace(b)).map(|(x, y)| (x - y).abs()).sum();
                assert!(gap > 1.0, "{} and {} move alike", a.name, b.name);
            }
        }
    }

    #[test]
    fn still_frames_are_deterministic() {
        for spec in &STATES {
            assert_eq!(still_pose(spec), still_pose(spec));
            let a = Figure::build(Body::of(Shape::Heart), &still_pose(spec));
            let b = Figure::build(Body::of(Shape::Heart), &still_pose(spec));
            assert_eq!(a.body, b.body);
            assert_eq!(a.eyes, b.eyes);
        }
        assert_eq!(still_pose(state("idle")), Pose::REST);
        assert!(still_pose(state("sleeping")).eye_open < 0.05);
    }

    #[test]
    fn resting_settles_and_gaze_follows() {
        let mut a = Animator::new(Shape::Blob, state("idle"), 7, true, false);
        assert!(a.is_settled());
        a.look_at(Some([-3.0, 0.0]));
        assert!(!a.is_settled());
        for _ in 0..90 {
            a.advance(1.0 / 30.0);
        }
        assert!(a.is_settled());
        assert_eq!(a.pose().look, [-1.0, 0.0]);
        a.look_at(None);
        for _ in 0..90 {
            a.advance(1.0 / 30.0);
        }
        assert_eq!(a.pose().look, REST_LOOK);
    }

    #[test]
    fn state_change_hands_off_smoothly() {
        let mut a = Animator::new(Shape::Star, state("idle"), 3, false, false);
        a.advance(0.5);
        let before = a.pose();
        a.set_state(state("failed"));
        let just_after = a.pose();
        assert!((before.shift[1] - just_after.shift[1]).abs() < 1e-4);
        for _ in 0..30 {
            a.advance(1.0 / 30.0);
        }
        assert!(a.pose().shift[1] > 0.05, "sunk into the failed droop");
    }

    #[test]
    fn gestures_play_once_and_end() {
        let mut a = Animator::new(Shape::Blob, state("idle"), 3, true, false);
        a.play(Gesture::Bounce, 1.0);
        assert!(a.is_playing() && !a.is_settled());
        a.advance(0.3);
        assert!(a.pose().shift[1] < -0.15, "mid-air");
        a.advance(0.4);
        assert!(!a.is_playing());
        a.play(Gesture::Spin, 2.0);
        a.advance(0.8);
        assert!(a.is_playing(), "two turns take twice as long");
        a.advance(0.81);
        assert!(!a.is_playing());
    }

    #[test]
    fn blinks_happen_while_live() {
        let mut a = Animator::new(Shape::Blob, state("working"), 11, false, false);
        let mut closed = false;
        for _ in 0..(30 * 8) {
            a.advance(1.0 / 30.0);
            closed |= a.pose().eye_open < 0.3;
        }
        assert!(closed);
    }

    #[test]
    fn morph_completes() {
        let mut a = Animator::new(Shape::Blob, state("idle"), 1, true, false);
        a.set_shape(Shape::House, true);
        assert!(!a.is_settled());
        a.advance(0.7);
        assert_eq!(a.shape(), Shape::House);
        assert!(a.is_settled());
    }

    #[test]
    fn seeds_are_stable() {
        assert_eq!(seed_from("bot-1"), seed_from("bot-1"));
        assert_ne!(seed_from("bot-1"), seed_from("bot-2"));
        assert_ne!(seed_from(""), 0);
    }
}
