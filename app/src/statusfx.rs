//! Status text effects (`app/statusfx.tsx`): the shimmering "working"
//! line under a run — text with a sweep highlight animating across it.
//! The Rust version is a pulse: the phrase dims and brightens on a
//! keyframe while the run is live.

use nsh_core::ui::prelude::*;

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

/// Shimmering status text — `Thinking…`-style lines under agent runs.
pub(crate) fn shimmer(label: impl Fn() -> String + 'static) -> impl IntoElement {
    text(move || status_label(&label()))
        .text_size(px(12.0))
        .text_color(theme::DIM)
        .animation(KeyframeAnimation::new(Keyframes::Pulse).duration(1400.0))
}

/// A plain (non-animated) status line.
pub(crate) fn status_line(label: impl Fn() -> String + 'static) -> impl IntoElement {
    text(move || label())
        .text_size(px(12.0))
        .text_color(theme::FAINT)
}
