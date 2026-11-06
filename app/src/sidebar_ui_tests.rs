//! The sidebar on a headless engine (`nsh-testing`), to see what survives:
//! a row's node id gone from `live` means the row, and the avatar in it,
//! was built again, so its animation started over.
//!
//! The store is one per process, so each test runs in a child of its own.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use buwiz_office_kit::testing::{Headless, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};

use crate::live::store;
use crate::store::RoxBot;

const CHILD: &str = "BUWIZ_SIDEBAR_UI_TEST";
const BOTS: usize = 27;
static STEP: AtomicUsize = AtomicUsize::new(0);

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("sidebar_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

fn seed() {
    let st = store();
    st.side_shown.set(true);
    st.side_mode.set("full".to_owned());
    st.bots.set(
        (0..BOTS)
            .map(|i| RoxBot {
                id: format!("b{i}"),
                name: format!("Bot {i}"),
                thread_id: format!("t{i}"),
                shape: "blob".to_owned(),
                color: "#1884FC".to_owned(),
                ..Default::default()
            })
            .collect(),
    );
}

/// Every Bot one state on, a step apart, as `status-all` does.
fn step() {
    let names: Vec<&'static str> = crate::mark::STATES.iter().map(|(n, _)| *n).filter(|n| *n != "idle").collect();
    let k = STEP.fetch_add(1, Ordering::Relaxed);
    for i in 0..BOTS {
        let name = names[(i * 2 + k) % names.len()];
        store().agent_set(format!("t{i}"), Some(format!("b{i}")), Some((name.to_owned(), name.to_owned())));
    }
}

#[component]
fn Side() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view()
        .size_full()
        .child(
            view()
                .flex_row()
                .h(px(24.0))
                .child(view().label("seed").w(px(40.0)).h_full().on_press(seed))
                .child(view().label("step").w(px(40.0)).h_full().on_press(step))
                .child(view().label("rail").w(px(40.0)).h_full().on_press(|| store().side_mode.set("rail".to_owned())))
                .child(text("probe ready=1").text_size(px(8.0))),
        )
        .child(view().flex_1().child(crate::sidebar::sidebar()))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Side>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1200.0), px(900.0));
    run(config)
}

/// A status write repaints the avatars and rebuilds none of them: a rebuilt
/// avatar starts its animation over, and with several Bots running every
/// avatar did, each time any one of them changed state.
#[test]
fn a_status_write_rebuilds_no_avatar() {
    if !in_child("a_status_write_rebuilds_no_avatar") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.pump(Duration::from_millis(400));
    assert!(app.live.len() > BOTS * 3, "only {} nodes: the rows never showed", app.live.len());
    // the first write puts the status rows up, which is new, not rebuilt
    app.press("step");
    app.pump(Duration::from_millis(100));
    for k in 0..60 {
        let before = app.live.clone();
        app.press("step");
        app.pump(Duration::from_millis(40));
        let gone = before.difference(&app.live).count();
        assert_eq!(gone, 0, "step {k}: {gone} nodes rebuilt");
    }
}

/// A sweep down the rail crosses a tile every few pixels, and each one puts
/// its hover card up and takes it down: the tiles themselves stay.
#[test]
fn a_hover_rebuilds_no_avatar() {
    if !in_child("a_hover_rebuilds_no_avatar") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.press("rail");
    app.pump(Duration::from_millis(400));
    let before = app.live.clone();
    for k in 0..40 {
        let row = format!("Bot {}", k % 8);
        app.hover(&row, true);
        for x in [6.0, 14.0, 22.0, 30.0] {
            app.hover_at(&row, x, 12.0);
        }
        app.hover(&row, false);
    }
    let gone = before.difference(&app.live).count();
    assert_eq!(gone, 0, "{gone} nodes rebuilt by hovering");
}
