//! The tests' native screens, registered for the fixture plugin
//! ([`register`]): a board that uses the bridge as a TSX test app does, one
//! whose view panics and one whose build does.

use buwiz_app::{ABI, Action, Bridge, PickOptions, ScreenDef};
use nsh_core::{element::Root, prelude::*};
use serde_json::{Value, json};

pub const PLUGIN: &str = "app.buwiz.fixture";

static SCREENS: &[ScreenDef] = &[
    ScreenDef { id: "fx.board", abi: ABI, build: board },
    ScreenDef { id: "fx.panics", abi: ABI, build: panics },
    ScreenDef { id: "fx.build-panics", abi: ABI, build: build_panics },
];

/// Registers the screens, once.
pub fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| super::screens::register(PLUGIN, SCREENS));
}

/// Shows `vp <width> <placement> | <pressed> | <out>`: the viewport, the
/// last footer button pressed, and once its calls are answered `done
/// <granted capabilities> <echo> <a denied tool's code>`. It sets a Save
/// button, fits to 130 and says ready first.
fn board(bridge: Bridge) -> Root {
    buwiz_app::root(bridge, |b| {
        let vp = b.viewport();
        let pressed = signal(String::from("-"));
        b.on_action(move |id| pressed.set(id.to_owned()));
        let out = signal(String::from("waiting"));
        let b2 = b.clone();
        b.spawn(async move {
            let b = b2;
            let set = b.actions(vec![Action::new("save", "Save").primary()]).await;
            let _ = b.fit(130.0);
            b.ready();
            let caps = [
                ("storage", b.storage().is_some()),
                ("attach", b.attach("item", "x").is_some()),
                ("pick", b.pick_file(PickOptions::default()).is_some()),
                ("copy", b.copy("x").is_some()),
                ("preview", b.preview("fx_doc", Value::Null, None).is_some()),
            ];
            let caps: Vec<&str> = caps.iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect();
            let echo = match b.call("fx_echo", json!({"code": "1601C"})).await {
                Ok(v) => v.to_string(),
                Err(e) => e.code,
            };
            let secret = match b.call("fx_secret", json!({})).await {
                Ok(_) => "ran".to_owned(),
                Err(e) => e.code,
            };
            out.set(format!("done {} {} {echo} {secret}", set.is_ok(), if caps.is_empty() { "-".to_owned() } else { caps.join(",") }));
        });
        text(move || {
            let v = vp.get();
            format!("vp {} {:?} | {} | {}", v.width, v.placement, pressed.get(), out.get())
        })
    })
}

fn panics(bridge: Bridge) -> Root {
    buwiz_app::root(bridge, |_| -> AnyElement { panic!("the ledger broke") })
}

fn build_panics(_: Bridge) -> Root {
    panic!("no ledger to build")
}
