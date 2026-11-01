//! Status text effects (`app/statusfx.tsx`): the shimmering "working"
//! line under a run — text with a gradient highlight sweeping across it,
//! drawn on a canvas exactly as Grok does.

use std::cell::Cell;

use nsh_core::canvas::{CanvasGradient, CanvasTextAlign, CanvasTextBaseline};
use nsh_core::prelude::*;

use crate::mark;
use crate::theme;

/// `STATUS_LABEL` from `app/statusfx.tsx` — a run state's display string.
pub(crate) fn status_label(state: &str) -> String {
    match state {
        "idle" => "",
        "listening" => "Listening…",
        "thinking" => "Thinking…",
        "streaming" => "Streaming…",
        "searching" => "Searching…",
        "reading" => "Reading…",
        "working" => "Working…",
        "coding" => "Coding…",
        "running" => "Running…",
        "computing" => "Computing…",
        "controlling" => "Controlling…",
        "waiting" => "Waiting…",
        "blocked" => "Blocked",
        "finished" => "Done",
        "failed" => "Failed",
        "sleeping" => "Sleeping",
        "waking" => "Waking…",
        "happy" => "Happy",
        "excited" => "Excited",
        "laughing" => "Laughing",
        "proud" => "Proud",
        "curious" => "Curious…",
        "playful" => "Playful",
        "shy" => "Shy",
        "surprised" => "Surprised",
        "suspicious" => "Suspicious",
        "angry" => "Angry",
        "confused" => "Confused",
        "sad" => "Sad",
        "drowsy" => "Drowsy",
        "bored" => "Bored",
        "scared" => "Scared",
        "celebrate" => "Celebrate",
        "approving" => "Approving",
        "orbit" => "Orbit",
        "radar" => "Radar",
        "progress" => "Progress",
        "spawning" => "Spawning…",
        "loading" => "Loading…",
        "dictating" => "Dictating…",
        "sending" => "Sending…",
        "receiving" => "Receiving…",
        "uploading" => "Uploading…",
        "writing" => "Writing…",
        "alerting" => "Alert",
        "bouncing" => "Bouncing",
        "powering-down" => "Powering down…",
        "humming" => "Humming",
        "notifying" => "Notification",
        "dragging" => "Dragging",
        other => return title_case(other),
    }
    .to_owned()
}

fn title_case(s: &str) -> String {
    s.replace('-', " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `ShimmerText` (statusfx.tsx:309) — the run's status line drawn on a
/// canvas: `measureText` sizes the band, a linear-gradient highlight
/// sweeps across every 1.9s, `fillText` clamps to the canvas width. The
/// canvas is `flexGrow: 1; minWidth: 60; height: size+8`. Grok's
/// `color`/`hi` (`#f5f5f58a`, `tc("#f5f5f5")`) are `MUTED`/`TEXT` here.
/// The label is verbatim (`agent().label` is already display text).
pub(crate) fn shimmer(label: impl Fn() -> String + 'static) -> impl IntoElement {
    shimmer_at(label, 11.0, theme::MUTED, theme::TEXT)
}

/// `shimmer` with Grok's `size`/`color`/`hi` (11+MUTED+TEXT for the
/// status row).
pub(crate) fn shimmer_at(
    label: impl Fn() -> String + 'static,
    size: f32,
    base: AdaptiveColor,
    hi: AdaptiveColor,
) -> impl IntoElement {
    let font = format!("{size}px -apple-system, system-ui, sans-serif");
    // The store's shared ~30fps clock — one commit per frame, not one per
    // shimmer (statusfx's `startFx` is the same shared-clock idea).
    let bounds = signal(Bounds::<Points>::default());
    canvas(move || {
        let t = crate::live::store().tick();
        let text = label();
        let size = bounds.get().size;
        let w = (measure_text(&font, &text).0 as f64)
            .min(size.width.0 as f64)
            .max(0.0);
        let band = 30.0f64.max(w * 0.4);
        let cx = -band + ((t % 1.9) / 1.9) * (w + band * 2.0);
        let (base, hi) = (color(base), color(hi));
        let g = CanvasGradient::linear(
            point(px((cx - band) as f32), px(0.0)),
            point(px((cx + band) as f32), px(0.0)),
        )
        .add_color_stop(percentage(0.0), base)
        .add_color_stop(percentage(0.5), hi)
        .add_color_stop(percentage(1.0), base);
        Painter::new()
            .font(&font)
            .text_baseline(CanvasTextBaseline::Middle)
            .text_align(CanvasTextAlign::Left)
            .fill_style(g)
            .fill_text_max_width(&text, 0.0f32, size.height.0 / 2.0, size.width)
    })
    .on_layout(move |b: &Bounds<Points>| bounds.set(*b))
    .h(px(size + 8.0))
    .flex_1()
    .min_w(px(60.0))
}

/// A plain (non-animated) status line.
pub(crate) fn status_line(label: impl Fn() -> String + 'static) -> impl IntoElement {
    text(label)
        .text_size(px(12.0))
        .text_color(theme::FAINT)
}

// ---------------------------------------------------------------------------
// `morphKind`/`FxOverlay`/`StatusAvatar`/`StatusFx` — statusfx.tsx's canvas
// parts (the ambient `drawFx` around the mark, and the poked status blob).
// ---------------------------------------------------------------------------

/// `morphKind(state)` — a run state's ambient-fx kind (`"none"` when not
/// morph-backed).
pub(crate) fn morph_kind(state: &str) -> &'static str {
    mark::STATES
        .iter()
        .find(|(n, _)| *n == state)
        .and_then(|(_, d)| d.morph)
        .and_then(mark::vx)
        .unwrap_or("none")
}

/// `FxOverlay({fx,size,ink})` — `drawFx`'s ambient ring around the mark on
/// a `size*1.5` canvas overhanging the cell by `size*0.25`. `state` is the
/// run-state signal; `morphKind` decides what (if anything) is drawn.
pub(crate) fn fx_overlay(
    state: std::rc::Rc<dyn Fn() -> String>,
    size: f32,
    ink: String,
) -> impl IntoElement {
    fx_overlay_fn(state, size, ink)
}

/// `fx_overlay` with a state *source* — `botSt` closures and anything else
/// readable inside the paint (which subscribes to what it reads).
pub(crate) fn fx_overlay_fn(
    state: std::rc::Rc<dyn Fn() -> String>,
    size: f32,
    ink: String,
) -> impl IntoElement {
    let canvas_size = size * 1.5;
    let offset = -size * 0.25;
    let ink = ink;
    // `startFx` — the store's shared ~30fps overlay clock, read only
    // while an effect plays.
    canvas(move || {
        let fx = morph_kind(&state());
        let mut p = Painter::new();
        if fx != "none" {
            let t = crate::live::store().tick();
            p = mark::draw_fx(
                p,
                fx,
                canvas_size as f64,
                canvas_size as f64,
                t,
                &ink,
            );
        }
        p
    })
    .pointer_events(PointerEvents::None)
    .absolute()
    .left(px(offset))
    .top(px(offset))
    .size(px(canvas_size))
}

/// `StatusAvatar` — the status row's mark: it always plays the run state,
/// and a click pokes it through `UN` (spin → bounce → nod). `state` is the
/// run-state source (`agent().state`), read inside the mark's paint;
/// `emph_src` is the `emphasized` prop read (`emphBot() !== ""`).
pub(crate) fn status_avatar(
    shape: String,
    color: String,
    state_src: std::rc::Rc<dyn Fn() -> String>,
    size: f32,
    emph_src: std::rc::Rc<dyn Fn() -> bool>,
    seed: Option<String>,
) -> impl IntoElement {
    let pokes = crate::avatar::BlobPokes::new();
    let index = Cell::new(0usize);
    let pokes2 = pokes.clone();
    view()
        .size(px(size))
        .child(crate::avatar::bot_blob(crate::avatar::BotBlobProps {
            shape,
            color,
            size,
            state_src: Some(state_src),
            paused: false,
            emph_src: Some(emph_src),
            pokes: Some(pokes),
            seed,
            ..Default::default()
        }))
        .on_press(move || {
            let i = index.get();
            index.set(i + 1);
            pokes2.fire(mark::UN[i % mark::UN.len()], None);
        })
        .label("Agent activity mark")
}

/// The status row — mark + ambient fx + the shimmering label (`StatusFx`
/// in statusfx.tsx). `state_src` reads the run's state (the thread's
/// `agentRuns` entry). Hover sets `emphBot` to the run's bot
/// (`agent().bot ?? activeBot()?.id` — here `seed`), which emphasizes
/// this mark and the matching rail mark.
pub(crate) fn status_fx(
    shape: String,
    color: String,
    state_src: std::rc::Rc<dyn Fn() -> String>,
    size: f32,
    seed: Option<String>,
    label: impl Fn() -> String + 'static,
) -> impl IntoElement {
    let emph: std::rc::Rc<dyn Fn() -> bool> =
        std::rc::Rc::new(|| !crate::live::store().emph_bot.get().is_empty());
    let bot = seed.clone().unwrap_or_default();
    // `flexDirection: "row"` (App.tsx:2168): the mark, then its label, on
    // the column's left edge — a column centered the mark alone.
    // Full width: the label's canvas flex-grows into it (a hugging row
    // left it at its 60pt minimum, squeezing longer statuses).
    view()
        .flex_row()
        .w_full()
        .gap(px(7.0))
        .items_center()
        .on_mouse_enter(move |_: &MouseEvent| crate::live::store().emph_bot.set(bot.clone()))
        .on_mouse_leave(|_: &MouseEvent| crate::live::store().emph_bot.set(String::new()))
        .child(
            view()
                .size(px(size))
                .child(status_avatar(shape, color, state_src.clone(), size, emph, seed))
                .child(fx_overlay(state_src, size, theme::FAINT.dark.to_hex())),
        )
        .child(shimmer(label))
}
