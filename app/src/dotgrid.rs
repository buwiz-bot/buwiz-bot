//! The generation placeholder (ChatGPT's image-generation look): the
//! output's rectangle as a grid of faint dots that fill in blue — a blob
//! from the top-left and one from the bottom-right growing toward each
//! other — with a `NN%` pill in the bottom-right corner. Nothing reports
//! real progress, so it is simulated from how long the placeholder has
//! shown: `1 - e^(-t/τ)`, held under 95% until the result replaces it.

use std::f32::consts::TAU;

use nsh_core::canvas::{CanvasTextAlign, CanvasTextBaseline};
use nsh_core::ui::prelude::*;

use crate::live::store;
use crate::theme;

/// Dot pitch, and a dot's radius at rest and filled.
const PITCH: f32 = 15.0;
const R_IDLE: f32 = 0.8;
const R_FULL: f32 = 2.5;
/// Fill levels: dots are batched into one path per level (one fill per
/// level a frame, not one per dot).
const LEVELS: usize = 16;
/// The front's soft edge and the per-dot jitter of where it passes, as
/// fractions of the diagonal.
const SOFT: f32 = 0.05;
const JITTER: f32 = 0.035;
/// The twinkle's reach in fill levels (0..1), so a stalled bar still lives.
const TWINKLE: f32 = 0.07;
/// The ceiling simulated progress waits under for the real result.
const CAP: f64 = 0.95;
/// Seconds to `1 - 1/e` (63%): an image reaches ~80% at 40–45s, a mini
/// app at ~25s.
pub(crate) const TAU_IMAGE: f64 = 25.0;
pub(crate) const TAU_APP: f64 = 15.0;

/// The dot grid filling its parent. `key` names the placeholder in
/// `Store::art_started`: the same key (a turn id) keeps its progress
/// through a rebuilt row or a recycled cell.
pub(crate) fn dot_grid(key: String, tau: f64) -> impl IntoElement {
    let started = *store()
        .art_started
        .borrow_mut()
        .entry(key)
        .or_insert_with(std::time::Instant::now);
    let bounds = signal(Bounds::<Points>::default());
    let font = "600 11px -apple-system, system-ui, sans-serif";
    // The store's shared ~30fps clock drives the paint; the progress is
    // read off the wall clock, so a dropped frame never slows it.
    canvas(move || {
        let t = store().tick() as f32;
        let area = bounds.get().size;
        let (w, h) = (area.width.0, area.height.0);
        if w < 1.0 || h < 1.0 {
            return Painter::new();
        }
        let p = (1.0 - (-started.elapsed().as_secs_f64() / tau).exp()).min(CAP);
        let (idle, fill) = (color(theme::DOT_IDLE), color(theme::DOT_FILL));
        let front = front_of(p as f32);
        let diag = (w * w + h * h).sqrt();
        let cols = ((w - 2.0 * R_FULL) / PITCH).floor().max(0.0) as i32 + 1;
        let rows = ((h - 2.0 * R_FULL) / PITCH).floor().max(0.0) as i32 + 1;
        let x0 = (w - (cols - 1) as f32 * PITCH) / 2.0;
        let y0 = (h - (rows - 1) as f32 * PITCH) / 2.0;
        // Each dot's level, bucketed: its distance to the nearer corner
        // (top-left or bottom-right) against the front, jittered per dot,
        // plus its own slow twinkle.
        let mut levels: Vec<Vec<(f32, f32)>> = vec![Vec::new(); LEVELS];
        for iy in 0..rows {
            for ix in 0..cols {
                let (x, y) = (x0 + ix as f32 * PITCH, y0 + iy as f32 * PITCH);
                let near = (x.hypot(y)).min((w - x).hypot(h - y)) / diag;
                let d = near + (hash(ix, iy, 1) * 2.0 - 1.0) * JITTER;
                let s = smoothstep((front - d) / SOFT + 0.5);
                let speed = 0.6 + 0.8 * hash(ix, iy, 2);
                let tw = (t * speed * 1.6 + hash(ix, iy, 3) * TAU).sin();
                let s = (s + tw * TWINKLE).clamp(0.0, 1.0);
                levels[(s * (LEVELS - 1) as f32).round() as usize].push((x, y));
            }
        }
        let mut painter = Painter::new();
        for (k, dots) in levels.iter().enumerate() {
            if dots.is_empty() {
                continue;
            }
            let s = k as f32 / (LEVELS - 1) as f32;
            let r = R_IDLE + (R_FULL - R_IDLE) * s.powf(0.8);
            painter = painter.fill_color(mix(idle, fill, s)).begin_path();
            for &(x, y) in dots {
                painter = painter.move_to(x + r, y).arc(x, y, r, 0.0, TAU);
            }
            painter = painter.fill();
        }
        // The percentage pill, bottom-right: the accent on a lifted plate.
        let label = format!("{}%", (p * 100.0).floor() as i32);
        let pw = measure_text(font, &label).0 + 14.0;
        let (ph, inset) = (20.0, 4.0);
        let (px_, py_) = (w - pw - inset, h - ph - inset);
        painter
            .fill_color(color(theme::PILL_BG))
            .begin_path()
            .round_rect(
                Bounds { origin: point(px(px_), px(py_)), size: size(px(pw), px(ph)) },
                px(ph / 2.0),
            )
            .fill()
            .font(font)
            .text_align(CanvasTextAlign::Center)
            .text_baseline(CanvasTextBaseline::Middle)
            .fill_color(fill)
            .fill_text(&label, px_ + pw / 2.0, py_ + ph / 2.0)
    })
    .on_layout(move |b: &Bounds<Points>| bounds.set(*b))
    .pointer_events(PointerEvents::None)
    .size_full()
}

/// A card's body while a revise of it builds (Try Again, `revise_app`):
/// the dot grid over the old version, at the body's size. `app` keys the
/// card's Try Again status (`card_status`); `id` is the turn the server
/// flags `revising`. Each revise counts from 0 again: the start is
/// dropped once the card is idle.
pub(crate) fn revise_overlay(app: String, id: String) -> impl IntoElement {
    let key = format!("revise:{app}:{id}");
    let busy = computed(move || {
        store().card_status.with(|m| m.get(&app).is_some_and(|(_, note)| !note))
            || (!id.is_empty()
                && store().turns.with(|t| t.iter().any(|t| t.id == id && t.revising)))
    });
    {
        let key = key.clone();
        effect(move || {
            if !busy.get() {
                store().art_started.borrow_mut().remove(&key);
            }
        });
    }
    when(
        move || busy.get(),
        move || {
            view()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .bg(theme::CARD)
                .child(dot_grid(key.clone(), TAU_APP))
                .into_element()
        },
    )
}

/// How far each blob's front has come (in diagonals) for progress `p`:
/// two quarter-discs of radius `f` cover `π f²` of a square until they
/// meet at `f = 0.5`; past that the last corners fill roughly linearly.
fn front_of(p: f32) -> f32 {
    let meet = std::f32::consts::PI / 4.0;
    if p <= meet {
        (p / std::f32::consts::PI).sqrt()
    } else {
        0.5 + (p - meet) / (1.0 - meet) * (std::f32::consts::FRAC_1_SQRT_2 - 0.5)
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// `idle` → `fill` at `s`: the gray takes on the blue's hue as it fills.
fn mix(idle: Hsla, fill: Hsla, s: f32) -> Hsla {
    let l = |a: f32, b: f32| a + (b - a) * s;
    Hsla { h: fill.h, s: l(idle.s, fill.s), l: l(idle.l, fill.l), a: l(idle.a, fill.a) }
}

/// A stable 0..1 per dot and `salt`.
fn hash(x: i32, y: i32, salt: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ salt.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}
