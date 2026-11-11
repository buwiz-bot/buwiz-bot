//! The Computer tab's Network section on a headless engine: what a person reads, and that its
//! switches and buttons are what a screen reader says they are. Nothing reaches a server: the
//! backend is a port nothing listens on.

use std::time::Duration;

use buwiz_office_kit::testing::{Headless, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};
use serde_json::json;

use crate::network::{show, Net};

const CHILD: &str = "BUWIZ_NETWORK_UI_TEST";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("network_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

fn local(tunnel: bool, client: &str) -> Net {
    Net::parse(&json!({"ok": true, "state": "computer", "supported": true, "tunnelMade": true, "tunnel": tunnel, "mode": "all", "allow": [], "block": ["facebook.com"], "awake": true,
        "client": {"state": client}, "counts": {"direct": 4, "blocked": 2},
        "notes": ["These are this computer's rules: every Bot on it (Ann, Bea) gets them.", "These rules cover the browser on this computer's screens. Commands the agent runs aren't covered: the network wall still applies to them."]}))
}

#[component]
fn Section() -> impl IntoElement {
    view()
        .size_full()
        .child(view().label("say local").w(px(40.0)).h(px(20.0)).on_press(|| show("b1", local(false, "off"))))
        .child(view().label("say tunnel down").w(px(40.0)).h(px(20.0)).on_press(|| show("b1", local(true, "attaching"))))
        .child(view().label("say cloud").w(px(40.0)).h(px(20.0)).on_press(|| {
            show("b1", Net::parse(&json!({"state": "computer", "supported": false, "why": "This computer browses from its provider's network: browsing rules and the tunnel apply to computers on this Mac."})))
        }))
        .child(crate::network::section())
        .child(text("probe ready=1").text_size(px(8.0)))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Section>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(700.0), px(900.0));
    run(config)
}

fn shows(app: &mut Headless, word: &str) -> bool {
    app.pump(Duration::from_millis(60));
    app.texts().iter().any(|t| t.contains(word))
}

#[test]
fn the_section_says_whose_rules_they_are_what_they_cover_and_has_real_controls() {
    if !in_child("the_section_says_whose_rules_they_are_what_they_cover_and_has_real_controls") {
        return;
    }
    let mut app = start();
    app.press("say local");
    assert!(shows(&mut app, "Tunnel off: the computer dials sites itself."), "{:?}", app.texts());
    assert!(shows(&mut app, "every Bot on it (Ann, Bea) gets them"), "{:?}", app.texts());
    assert!(shows(&mut app, "Commands the agent runs aren't covered"), "{:?}", app.texts());
    assert!(shows(&mut app, "facebook.com") && shows(&mut app, "4 connections made, 2 refused"), "{:?}", app.texts());
    let (role, _, checked) = app.access("Tunnel through this Mac, off");
    assert_eq!(role, AccessibilityRole::Switch);
    assert_eq!(checked, Some(nsh_core::Checked::False));
    assert_eq!(app.access("Only allowed sites, off").0, AccessibilityRole::Switch);
    for button in ["Allow this site", "Block this site", "Remove facebook.com from Blocked"] {
        assert_eq!(app.access(button).0, AccessibilityRole::Button, "{button}");
    }
    app.press("say tunnel down");
    assert!(shows(&mut app, "can't reach any site until it is"), "{:?}", app.texts());
    assert_eq!(app.access("Tunnel through this Mac, on").2, Some(nsh_core::Checked::True));
    app.press("say cloud");
    assert!(shows(&mut app, "browses from its provider's network"), "{:?}", app.texts());
    assert!(!app.has("Tunnel through this Mac, off") && !app.has("Tunnel through this Mac, on"), "no dead switch on a cloud computer");
}
