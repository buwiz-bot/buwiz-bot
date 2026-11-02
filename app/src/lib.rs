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

mod agents;
mod appdetails;
mod avatar;
mod botdetails;
mod botmodal;
pub mod broker;
mod cards;
mod chat;
mod client;
mod composer;
mod computer;
mod ctx;
mod dotgrid;
mod files;
mod history;
mod hot;
mod icons;
pub mod identity;
pub mod keys;
mod links;
pub mod live;
mod mascot;
mod md;
mod mermaid;
mod office;
pub mod mini;
pub mod mini_v1;
mod models;
mod palette;
mod pane;
mod settings;
mod slash;
mod plugins;
mod plugin_review;
mod routines;
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

/// The app's display name (the window title, the top pill, the empty
/// state, the composer's "Message …" outside an agent's chat), so a test
/// or dev instance never passes for the real app: see [`pick_app_name`].
pub fn app_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let env = std::env::var("APP_NAME").ok();
        let bundle = main_bundle_name();
        pick_app_name(
            env.as_deref(),
            bundle.as_ref().map(|(id, name)| (id.as_str(), name.as_str())),
            env!("CARGO_PKG_VERSION"),
        )
    })
}

/// The name [`app_name`] shows: `APP_NAME` from the environment; else the
/// name of the app bundle the binary runs in (`bundle`: its id and
/// display name), when it is one of Buwiz Bot's (`com.buwiz.bot`, or a
/// `dev.buwiz.bot.*` test bundle of `scripts/buwiz-test`); else, a build run
/// outside any of them (`cargo run`, a worktree's binary), "Buwiz Bot
/// dev-v<version>".
pub fn pick_app_name(env: Option<&str>, bundle: Option<(&str, &str)>, version: &str) -> String {
    if let Some(n) = env.map(str::trim).filter(|n| !n.is_empty()) {
        return n.to_owned();
    }
    match bundle {
        Some((id, name)) if (id == identity::APP_ID || id.starts_with("dev.buwiz.bot.")) && !name.trim().is_empty() => {
            name.trim().to_owned()
        }
        _ => format!("Buwiz Bot dev-v{version}"),
    }
}

/// The main bundle's id and `CFBundleDisplayName` (else `CFBundleName`),
/// when the binary runs from inside an app bundle.
#[cfg(target_os = "macos")]
fn main_bundle_name() -> Option<(String, String)> {
    use nsh_platform_macos::objc2_foundation::{NSBundle, NSString, ns_string};
    let bundle = NSBundle::mainBundle();
    let id = bundle.bundleIdentifier()?.to_string();
    let name = [ns_string!("CFBundleDisplayName"), ns_string!("CFBundleName")].into_iter().find_map(|key| {
        bundle.objectForInfoDictionaryKey(key)?.downcast::<NSString>().ok().map(|s| s.to_string())
    })?;
    Some((id, name))
}

/// Elsewhere (iOS, packaged by `ios.sh` as com.buwiz.bot) there are no
/// test or dev instances to tell apart: the app's own name.
#[cfg(not(target_os = "macos"))]
fn main_bundle_name() -> Option<(String, String)> {
    Some((identity::APP_ID.to_owned(), "Buwiz Bot".to_owned()))
}

/// The font files the root registers before it renders (`Root::font`):
/// the office views' icon font, when the app bundle has it. The office
/// documents' bundled fonts (`office::fonts`) start registering with
/// CoreText here too, straight from their files, off the main thread (a
/// document's worker waits for them before it opens one).
pub fn fonts() -> Vec<Vec<u8>> {
    std::thread::spawn(office::fonts::register);
    office::ui::icons::font_data().into_iter().collect()
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
    // `buwiz://chat/<thread>[?m=<turn>]` / `buwiz://chat/new?draft=<text>` /
    // `buwiz://plugin/<name>` (the plugin page's Share link) /
    // `buwiz://app/<server>/<app>` / `buwiz://view?path=<file>` (a mini app's
    // print preview, `links::viewable` only) opened from another app
    // or a mini app (the dev bundle declares the scheme): the root never
    // remounts, so this handler lives as long as the app. `open_os_link`
    // makes nothing until the person acts in Rox (a new chat or a mini app
    // asks first).
    // Other URLs are ignored here — the router would hand them to the
    // system, which for `buwiz://` sends them straight back.
    links::make_view_dir();
    on_open_url(|url| {
        let thread = url.strip_prefix("buwiz://chat/").map(|r| r.split('?').next().unwrap_or(""));
        if thread.is_some_and(|t| !t.trim_end_matches('/').is_empty())
            || url.starts_with("buwiz://plugin/")
            || url.starts_with("buwiz://app/")
            || url.starts_with("buwiz://view")
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

#[cfg(test)]
mod app_name_tests {
    use super::pick_app_name;

    #[test]
    fn the_installed_bundle_is_buwiz_bot() {
        assert_eq!(pick_app_name(None, Some(("com.buwiz.bot", "Buwiz Bot")), "0.1.0"), "Buwiz Bot");
    }

    #[test]
    fn a_test_bundle_shows_its_own_name() {
        let got = pick_app_name(None, Some(("dev.buwiz.bot.test-books", "Buwiz Bot test-books")), "0.1.0");
        assert_eq!(got, "Buwiz Bot test-books");
    }

    #[test]
    fn a_build_outside_buwiz_bots_bundles_is_a_dev_build() {
        assert_eq!(pick_app_name(None, None, "0.1.0"), "Buwiz Bot dev-v0.1.0");
        // another app's bundle (nsh's NshDev host) names no Buwiz Bot
        assert_eq!(pick_app_name(None, Some(("com.longbridge.nsh.host", "NshDev")), "0.2.0"), "Buwiz Bot dev-v0.2.0");
        assert_eq!(pick_app_name(None, Some(("com.buwiz.bot", "  ")), "0.1.0"), "Buwiz Bot dev-v0.1.0");
    }

    #[test]
    fn app_name_from_the_environment_wins() {
        assert_eq!(pick_app_name(Some(" Ada "), Some(("com.buwiz.bot", "Buwiz Bot")), "0.1.0"), "Ada");
        assert_eq!(pick_app_name(Some(""), None, "0.1.0"), "Buwiz Bot dev-v0.1.0");
    }
}
