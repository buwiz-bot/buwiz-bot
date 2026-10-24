//! Rox: an all-Rust chat shell on nsh, edited live by ra-jit.
//!
//! The root is a thin shell ([`App`]): a `dynamic` mounts [`hot::hot_area`]
//! — which the daemon's patches cause to rebuild — and hands it the
//! remount/note/uptime signals for the floating stats pill it draws
//! (the uptime keeps counting across remounts).
//!
//! [`live::install`] is the only glue between ra-jit and the engine: it
//! polls the patch file on the engine thread, snapshots the `keep` signals
//! and bumps `remount`, which the dynamic reads. [`live::init`] anchors the
//! app's store under the root owner so the whole model survives remounts.

mod avatar;
mod botmodal;
mod chat;
mod client;
mod composer;
mod ctx;
mod files;
mod hot;
mod icons;
pub mod live;
mod mark;
mod md;
pub mod mini;
mod models;
mod palette;
mod pane;
mod plugins;
mod shelf;
mod sidebar;
mod statusfx;
mod store;
mod theme;
mod widgets;

use std::time::Duration;

use nsh_core::ui::prelude::*;

/// As in `hot.rs`: colors built of plain `Hsla` literals — `to_hsla` /
/// `clamp`/`max` in const eval are not in ra-jit's evaluator yet, and a
/// function whose consts cannot be evaluated goes to cargo, not a patch.
const fn tone(light: Hsla, dark: Hsla) -> AdaptiveColor {
    AdaptiveColor { light, dark }
}
const fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h, s, l, a }
}

const BG: AdaptiveColor = tone(hsla(0.17, 0.10, 0.95, 1.0), hsla(0.60, 0.11, 0.07, 1.0));

/// The app's UI set for nsh tooling (`nsh.json` `"rust"`).
#[native_module("rox/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The shell: the hot area in a `dynamic` that rebuilds on each patch.
/// This view runs once, when the app starts — its signals and its
/// interval therefore outlive every remount, which is what makes the
/// stats pill a witness that the process never stopped.
#[component]
pub fn App() -> impl IntoElement {
    let remount = signal(0u64);
    let note = signal("ra-jit live".to_owned());
    live::install(remount, note);
    let seconds = signal(0u64);
    set_interval(Duration::from_secs(1), move || seconds.update(|s| *s += 1));
    live::init();

    view()
        .flex_1()
        .h_full()
        .bg(BG)
        .child(
            view().flex_1().child(each(
                move || vec![remount.get()],
                |&n| n,
                move |_, _| hot::hot_area(remount, note, seconds),
            )),
        )
}
