//! The viewer's bar on a headless engine: what a person sees and what a press asks for. Nothing
//! reaches a server: the backend is a port nothing listens on.

use std::time::Duration;

use buwiz_office_kit::testing::{Headless, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};

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
