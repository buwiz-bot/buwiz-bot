//! Edit Avatar's tabs on a headless engine: what a person sees and what a press does. Nothing
//! reaches a server: the backend is a port nothing listens on.

use std::time::Duration;

use buwiz_office_kit::testing::{Headless, read, run, until};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};

use crate::live::store;
use crate::store::AvatarCand;

const CHILD: &str = "BUWIZ_EDITOR_UI_TEST";
const EMPTY: &str = "Avatars you design show up here.";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("editor_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

#[component]
fn Editor() -> impl IntoElement {
    let st = store();
    st.set_threads(serde_json::json!({
        "list": [{"id": "t1", "title": "One", "bot": {"id": "b1", "name": "One"}}],
        "bots": [{"id": "b1", "name": "One", "threadId": "t1", "shape": "blob", "color": "#1884FC"}],
    }));
    st.active_id.set("t1".to_owned());
    st.right_open.set(true);
    st.open_avatar_editor();
    view()
        .size_full()
        .child(text(|| {
            let shape = store().active_bot().map(|b| b.shape).unwrap_or_default();
            format!("probe shape={shape}")
        }))
        .child(view().label("add a design").w(px(40.0)).h(px(40.0)).on_press(|| {
            store().design_saved.set(vec![AvatarCand {
                name: "Mine".to_owned(),
                shape: "cloud".to_owned(),
                color: "#FFFFFF".to_owned(),
                why: String::new(),
            }]);
        }))
        .child(crate::pane::editor_overlay())
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Editor>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1200.0), px(900.0));
    let mut app = run(config);
    app.wait_for("Own tab");
    app
}

fn shows(app: &Headless, line: &str) -> bool {
    app.texts().contains(&line)
}

/// Your avatars are on Own, not on Bot, and there's no Design tab to find.
#[test]
fn own_lists_your_avatars_and_bot_doesnt() {
    if !in_child("own_lists_your_avatars_and_bot_doesnt") {
        return;
    }
    let mut app = start();
    assert!(!app.has("Design tab"), "a Design tab");
    assert!(app.has("Upload tab") && app.has("Bot tab"), "the other tabs");
    app.press("add a design");
    app.pump(Duration::from_millis(100));
    assert!(!app.has("Use your Mine design"), "on the Bot tab");
    app.press("Own tab");
    app.wait_for("Use your Mine design");
    app.press("Bot tab");
    app.wait_for("Reset avatar");
    assert!(!app.has("Use your Mine design"), "back on the Bot tab");
}

/// With nothing of your own, Own says so instead of being blank.
#[test]
fn own_with_nothing_says_so() {
    if !in_child("own_with_nothing_says_so") {
        return;
    }
    let mut app = start();
    app.press("Own tab");
    for _ in 0..1200 {
        if shows(&app, EMPTY) {
            break;
        }
        app.pump(Duration::from_millis(50));
    }
    assert!(shows(&app, EMPTY), "no line: {:?}", app.texts());
    app.press("add a design");
    app.wait_for("Use your Mine design");
    assert!(!shows(&app, EMPTY), "the line stays with a design there");
}

/// A tap on Own puts the avatar on the Bot, and Revert brings the old one back.
#[test]
fn a_tap_on_own_uses_it_and_revert_undoes_it() {
    if !in_child("a_tap_on_own_uses_it_and_revert_undoes_it") {
        return;
    }
    let mut app = start();
    assert_eq!(read(&app).get("shape").map(String::as_str), Some("blob"));
    app.press("add a design");
    app.press("Own tab");
    app.wait_for("Use your Mine design");
    assert!(!app.has("Revert avatar"), "nothing to revert yet");
    app.press("Use your Mine design");
    until(&mut app, "the design on the Bot", |p| p.get("shape").is_some_and(|s| s == "cloud"));
    app.wait_for("Revert avatar");
    app.press("Revert avatar");
    until(&mut app, "the old avatar back", |p| p.get("shape").is_some_and(|s| s == "blob"));
}
