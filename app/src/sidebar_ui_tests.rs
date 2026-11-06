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

const CHILD: &str = "BUWIZ_SIDEBAR_UI_TEST";

fn built() -> usize {
    crate::avatar::BUILT.load(Ordering::Relaxed)
}
const BOTS: usize = 27;
static STEP: AtomicUsize = AtomicUsize::new(0);
static REPLY: AtomicUsize = AtomicUsize::new(0);

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

/// `/threads` as the broker gives it, each Bot's last reply `desc`.
fn threads(desc: &str) -> serde_json::Value {
    let bots: Vec<_> = (0..BOTS)
        .map(|i| {
            serde_json::json!({"id": format!("b{i}"), "name": format!("Bot {i}"), "threadId": format!("t{i}"),
                "shape": "blob", "color": "#1884FC", "desc": desc})
        })
        .collect();
    serde_json::json!({"list": [], "bots": bots})
}

fn seed() {
    let st = store();
    st.side_shown.set(true);
    st.side_mode.set("full".to_owned());
    st.set_threads(threads("first words"));
}

/// A reply streaming: each refresh brings the Bots' longer `desc`.
fn reply() {
    let n = REPLY.fetch_add(1, Ordering::Relaxed);
    store().set_threads(threads(&format!("streamed {n} words")));
}

/// One Bot gets a new colour.
fn paint() {
    let mut v = threads("first words");
    v["bots"][3]["color"] = "#FC2C3E".into();
    store().set_threads(v);
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
                .child(view().label("reply").w(px(40.0)).h_full().on_press(reply))
                .child(view().label("paint").w(px(40.0)).h_full().on_press(paint))
                .child(view().label("group").w(px(40.0)).h_full().on_press(|| group(5)))
                .child(view().label("shrink").w(px(40.0)).h_full().on_press(|| group(3)))
                .child(view().label("work").w(px(40.0)).h_full().on_press(|| {
                    store().agent_set("t3".to_owned(), Some("b3".to_owned()), Some(("thinking".to_owned(), "Thinking".to_owned())));
                }))
                .child(view().label("rest").w(px(40.0)).h_full().on_press(|| store().agent_set("t3".to_owned(), Some("b3".to_owned()), None)))
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
    let start = app.live.clone();
    let made = built();
    for k in 0..120 {
        let before = app.live.clone();
        app.press("step");
        app.pump(Duration::from_millis(40));
        let gone = before.difference(&app.live).count();
        assert_eq!(gone, 0, "step {k}: {gone} nodes rebuilt");
    }
    // the last write's effects land too
    app.pump(Duration::from_millis(600));
    assert_eq!(start.difference(&app.live).count(), 0, "rebuilt after the last write");
    assert_eq!(built() - made, 0, "avatars built by status writes");
}

/// The first `n` Bots in a group, as the broker gives groups.
fn group(n: usize) {
    store().groups.set(vec![crate::store::RoxGroup {
        id: "g1".to_owned(),
        name: "Team".to_owned(),
        items: (0..n)
            .map(|i| crate::store::GroupItem { kind: "bot".to_owned(), id: format!("b{i}"), ..Default::default() })
            .collect(),
        ..Default::default()
    }]);
}

/// A streaming reply through grouped Bots: `rail`, or the full sidebar.
fn streams_in_a_group(rail: bool) {
    let mut app = start();
    app.press("seed");
    app.press("group");
    if rail {
        app.press("rail");
    }
    app.pump(Duration::from_millis(400));
    let start = app.live.clone();
    let made = built();
    for k in 0..30 {
        app.press("reply");
        app.pump(Duration::from_millis(60));
        if !rail {
            let shown = app.texts().iter().filter(|t| t.contains(&format!("streamed {k} words"))).count();
            assert_eq!(shown, BOTS, "step {k}: rows showing the reply");
        }
    }
    println!("grouped reply, rail {rail}: {} avatars built", built() - made);
    assert_eq!(built() - made, 0, "avatars built by a streaming reply");
    assert_eq!(start.difference(&app.live).count(), 0, "nodes rebuilt by a streaming reply");
}

#[test]
fn a_streaming_reply_rebuilds_no_avatar_in_a_group() {
    if in_child("a_streaming_reply_rebuilds_no_avatar_in_a_group") {
        streams_in_a_group(false);
    }
}

#[test]
fn a_streaming_reply_rebuilds_no_avatar_in_a_rail_group() {
    if in_child("a_streaming_reply_rebuilds_no_avatar_in_a_rail_group") {
        streams_in_a_group(true);
    }
}

/// A member leaving its group mid-reply: every Bot still has its row. Keyed
/// lists nested here once lost rows when records changed.
#[test]
fn no_row_is_lost_when_a_group_changes_mid_reply() {
    if !in_child("no_row_is_lost_when_a_group_changes_mid_reply") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.press("group");
    app.pump(Duration::from_millis(300));
    for _ in 0..3 {
        app.press("reply");
        app.pump(Duration::from_millis(60));
    }
    app.press("shrink");
    app.pump(Duration::from_millis(200));
    for _ in 0..3 {
        app.press("reply");
        app.pump(Duration::from_millis(60));
    }
    for i in 0..BOTS {
        let name = format!("Bot {i}");
        let rows = app.texts().iter().filter(|t| **t == name).count();
        assert_eq!(rows, 1, "rows for {name}");
    }
}

/// A status write reaches the avatar: not rebuilt, but not frozen either.
#[test]
fn an_avatar_follows_its_own_state() {
    if !in_child("an_avatar_follows_its_own_state") {
        return;
    }
    let painted = |bot: &str| crate::avatar::PAINTED.lock().unwrap().get(bot).copied();
    let mut app = start();
    app.press("seed");
    app.pump(Duration::from_millis(400));
    assert_eq!(painted("b3"), Some("idle"));
    let start = app.live.clone();
    for _ in 0..3 {
        app.press("work");
        app.pump(Duration::from_millis(200));
        assert_eq!(painted("b3"), Some("thinking"), "idle to working");
        assert_eq!(painted("b4"), Some("idle"), "another Bot stays as it was");
        app.press("rest");
        app.pump(Duration::from_millis(200));
        assert_eq!(painted("b3"), Some("idle"), "working to idle");
    }
    assert_eq!(start.difference(&app.live).count(), 0, "rebuilt on the way");
}

/// A streaming reply changes each Bot's `desc` on every refresh, and that's
/// no reason to rebuild its avatar.
#[test]
fn a_streaming_reply_rebuilds_no_avatar() {
    if !in_child("a_streaming_reply_rebuilds_no_avatar") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.pump(Duration::from_millis(400));
    let start = app.live.clone();
    let made = built();
    for k in 0..30 {
        app.press("reply");
        app.pump(Duration::from_millis(60));
        assert!(app.texts().iter().any(|t| t.contains(&format!("streamed {k} words"))), "step {k}: the reply never showed");
    }
    let gone = start.difference(&app.live).count();
    println!("reply: {gone} nodes gone, {} avatars built", built() - made);
    assert_eq!(built() - made, 0, "avatars built by a streaming reply");
    assert_eq!(gone, 0, "nodes rebuilt by a streaming reply");
}

/// A Bot's new face does build its avatar again, and only that Bot's.
#[test]
fn a_new_face_rebuilds_that_avatar_alone() {
    if !in_child("a_new_face_rebuilds_that_avatar_alone") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.pump(Duration::from_millis(400));
    let made = built();
    app.press("paint");
    app.pump(Duration::from_millis(200));
    assert_eq!(built() - made, 1, "avatars built by one Bot's new colour");
}

/// A sweep down the rail crosses a tile every few pixels, and each one puts
/// its hover card up and takes it down: the tiles themselves stay. This isn't
/// what stops the rebuilds above: gaze is a plain cell, and never caused one.
#[test]
fn a_hover_sweep_keeps_the_rail_tiles() {
    if !in_child("a_hover_sweep_keeps_the_rail_tiles") {
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
