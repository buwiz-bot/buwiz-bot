//! The viewer's bar on a headless engine: what a person sees and what a press asks for. Nothing
//! reaches a server: the backend is a port nothing listens on.

use std::time::Duration;

use buwiz_office_kit::testing::{Headless, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};
use serde_json::json;

use crate::computer::{Status, Takeover};

const CHILD: &str = "BUWIZ_COMPUTER_UI_TEST";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("computer_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

fn say(viewer: &str, takeover: bool) {
    let takeover = takeover.then(|| Takeover { reason: "Sign in to the shop".to_owned(), seconds_left: 125 });
    crate::computer::show_status("b1", Status { state: "awake".to_owned(), viewer: viewer.to_owned(), takeover, ..Status::default() });
}

#[component]
fn Bar() -> impl IntoElement {
    view()
        .size_full()
        .child(view().label("watching").w(px(40.0)).h(px(20.0)).on_press(|| say("watch", false)))
        .child(view().label("driving").w(px(40.0)).h(px(20.0)).on_press(|| say("drive", false)))
        .child(view().label("handed").w(px(40.0)).h(px(20.0)).on_press(|| say("drive", true)))
        .child(view().label("another bot").w(px(40.0)).h(px(20.0)).on_press(|| crate::computer::show_status("b2", Status { viewer: "drive".to_owned(), ..Status::default() })))
        .child(crate::computer::drive_button("computer:b1"))
        .child(text(|| format!("probe ready=1 wants={}", crate::computer::wants_to_drive("b1"))).text_size(px(8.0)))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Bar>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(300.0));
    run(config)
}

fn shows(app: &mut Headless, word: &str) -> bool {
    app.pump(Duration::from_millis(60));
    app.texts().iter().any(|t| t.contains(word))
}

#[test]
fn the_bar_takes_control_gives_it_up_and_hands_back() {
    if !in_child("the_bar_takes_control_gives_it_up_and_hands_back") {
        return;
    }
    let mut app = start();
    app.press("watching");
    assert!(shows(&mut app, "Take control") && shows(&mut app, "Your clicks don't reach"), "{:?}", app.texts());
    assert!(shows(&mut app, "wants=false"));

    app.press("Take control");
    assert!(shows(&mut app, "wants=true"), "{:?}", app.texts());
    app.press("driving");
    assert!(shows(&mut app, "Stop driving") && shows(&mut app, "The agent waits"), "{:?}", app.texts());
    app.press("Take control");
    assert!(shows(&mut app, "wants=false"), "pressing it again gives control up");

    app.press("handed");
    assert!(shows(&mut app, "Hand back") && shows(&mut app, "Sign in to the shop · 2:05 left"), "{:?}", app.texts());
    app.press("Take control");
    assert!(shows(&mut app, "wants=false"), "Hand back asks for no ticket of its own");

    app.press("another bot");
    assert!(shows(&mut app, "Take control"), "another Bot's viewer says nothing of this one: {:?}", app.texts());
}

#[component]
fn RemoteForm() -> impl IntoElement {
    view()
        .size_full()
        .child(view().label("on b1").w(px(40.0)).h(px(20.0)).on_press(|| crate::computer::pane_for("b1")))
        .child(crate::computer::remote_form())
        .child(text(|| format!("probe ready=1 empty={}", crate::computer::form_is_empty())).text_size(px(8.0)))
}

fn wait_for(app: &mut Headless, word: &str) {
    for _ in 0..100 {
        if shows(app, word) {
            return;
        }
    }
    panic!("{word} never showed: {:?}", app.texts());
}

#[test]
fn the_form_trusts_a_key_only_on_a_press_and_adds_only_on_a_yes_and_keeps_no_secret() {
    if !in_child("the_form_trusts_a_key_only_on_a_press_and_adds_only_on_a_yes_and_keeps_no_secret") {
        return;
    }
    crate::computer::CANNED.lock().unwrap().0 = vec![
        json!({"ok": false, "code": "trust", "fingerprint": "SHA256:abc", "pin": "ssh-ed25519 AAAA"}),
        json!({"ok": true, "screen": false, "system": "Linux", "consent": "The AI will act on this machine."}),
        json!({"state": "asleep", "computer": "c", "remote": {"screen": false, "where": "me@h"}}),
    ];
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("on b1");
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    app.type_text("ssh://you@host or https://box.example.com", "ssh://me@h");
    app.type_text("SSH key file, like ~/.ssh/id_ed25519", "~/.ssh/id");
    app.type_text("The box's token (none for a plain SSH host)", "sesame");
    app.press("Test");
    wait_for(&mut app, "SHA256:abc");
    assert!(shows(&mut app, "Nothing has been sent yet"), "{:?}", app.texts());
    assert!(!shows(&mut app, "I understand"), "no yes before the key is trusted");
    app.press("I checked, trust it");
    wait_for(&mut app, "The AI will act on this machine.");
    assert!(shows(&mut app, "Linux answered") && shows(&mut app, "no screen"), "{:?}", app.texts());
    app.press("I understand, add it");
    wait_for(&mut app, "empty=true");
    let sent = std::mem::take(&mut crate::computer::CANNED.lock().unwrap().1);
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent[0].0.ends_with("/remote/test") && sent[0].1["token"] == "sesame" && sent[0].1["pin"] == "", "{sent:?}");
    assert_eq!(sent[1].1["pin"], "ssh-ed25519 AAAA", "the pin follows the press");
    assert!(sent[2].0.ends_with("/computer/bot/b1/remote") && sent[2].1["consent"] == true, "{sent:?}");
}

#[test]
fn cancelling_the_form_drops_what_was_typed_and_a_no_stays_on_it() {
    if !in_child("cancelling_the_form_drops_what_was_typed_and_a_no_stays_on_it") {
        return;
    }
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    app.type_text("The box's token (none for a plain SSH host)", "sesame");
    wait_for(&mut app, "empty=false");
    app.press("Test");
    // the backend here is a port nothing listens on
    wait_for(&mut app, "The broker didn't answer.");
    assert!(shows(&mut app, "Test"), "a person can try again: {:?}", app.texts());
    app.press("Cancel");
    wait_for(&mut app, "empty=true");
    assert!(shows(&mut app, "Connect a remote computer"));
}
