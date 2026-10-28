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

mod appdetails;
mod avatar;
mod botdetails;
mod botmodal;
mod cards;
mod chat;
mod client;
mod composer;
mod ctx;
mod dotgrid;
mod files;
mod history;
mod hot;
mod icons;
pub mod keys;
mod links;
pub mod live;
mod mark;
mod md;
mod mermaid;
pub mod mini;
pub mod mini_v1;
mod models;
mod palette;
mod pane;
mod settings;
mod slash;
mod plugins;
mod sidebar;
mod statusfx;
mod store;
mod theme;
mod toast;
mod vault;
mod versions;
mod widgets;
pub mod zoom;


use nsh_core::prelude::*;

/// As in `hot.rs`: colors built of plain `Hsla` literals — `to_hsla` /
/// `clamp`/`max` in const eval are not in ra-jit's evaluator yet, and a
/// function whose consts cannot be evaluated goes to cargo, not a patch.
const fn tone(light: Hsla, dark: Hsla) -> AdaptiveColor {
    AdaptiveColor { light, dark }
}
const fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h, s, l, a }
}

/// The app's display name: `APP_NAME` from the environment, else "Rox"
/// (the window title, the top pill, the empty state, the composer's
/// "Message …" outside an agent's chat).
pub fn app_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::env::var("APP_NAME")
            .ok()
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Rox".to_owned())
    })
}

const BG: AdaptiveColor = tone(hsla(0.17, 0.10, 0.95, 1.0), hsla(0.60, 0.11, 0.07, 1.0));

/// The app's UI set for nsh tooling (`nsh.json` `"rust"`).
#[native_module("rox/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The shell: the hot area in a `dynamic` that rebuilds on each patch.
/// This view runs once, when the app starts — its signals therefore
/// outlive every remount, so the live pill's count keeps climbing.
#[component]
pub fn App() -> impl IntoElement {
    let remount = signal(0u64);
    let note = signal(String::new());
    live::install(remount, note);
    live::init();
    // `rox://chat/<thread>[?m=<turn>]` / `rox://chat/new?draft=<text>` /
    // `rox://plugin/<name>` (the plugin page's Share link) /
    // `rox://app/<server>/<app>` / `rox://view?path=<file>` (a mini app's
    // print preview, `links::viewable` only) opened from another app
    // or a mini app (the dev bundle declares the scheme): the root never
    // remounts, so this handler lives as long as the app. `open_os_link`
    // makes nothing until the person acts in Rox (a new chat or a mini app
    // asks first).
    // Other URLs are ignored here — the router would hand them to the
    // system, which for `rox://` sends them straight back.
    links::make_view_dir();
    on_open_url(|url| {
        let thread = url.strip_prefix("rox://chat/").map(|r| r.split('?').next().unwrap_or(""));
        if thread.is_some_and(|t| !t.trim_end_matches('/').is_empty())
            || url.starts_with("rox://plugin/")
            || url.starts_with("rox://app/")
            || url.starts_with("rox://view")
        {
            live::store().open_os_link(url);
        } else {
            log::warn!("rox: ignoring opened URL {url}");
        }
    });

    view()
        .flex_1()
        .h_full()
        .bg(BG)
        .child(
            view().flex_1().child(each(
                move || vec![remount.get()],
                |&n| n,
                move |_, _| hot::hot_area(remount, note),
            )),
        )
        // ⌘= / ⌘- / ⌘0: the window's zoom (outside the hot area, so a
        // remount never drops it).
        .child(zoom::zoom_host())
        // the keymap's AppKit side (menu keys, actions, the recorder,
        // printing) and its bindings
        .child(keys::host())
        .child(keys::bindings())
        // A..Z on the newest open choice card
        .child(keys::choice_keys())
}
