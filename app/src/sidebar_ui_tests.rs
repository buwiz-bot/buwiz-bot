//! The sidebar on a headless engine (`nsh-testing`), to see what survives:
//! a row's node id gone from `live` means the row, and the avatar in it,
//! was built again, so its animation started over.
//!
//! The store is one per process, so each test runs in a child of its own.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use buwiz_office_kit::testing::{Headless, num, read, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};

use crate::live::store;

const CHILD: &str = "BUWIZ_SIDEBAR_UI_TEST";

fn built() -> usize {
    crate::avatar::BUILT.load(Ordering::Relaxed)
}
const BOTS: usize = 27;
const MANY: usize = 120;
/// Seconds rich avatars get to come to rest, and the calm ones that prove it.
const RESTS_WITHIN: usize = 180;
const CALM_SECONDS: usize = 4;
/// 50 ms steps a wait gets, and the steps without a build that mean done.
const WAIT_STEPS: usize = 1200;
const STILL_STEPS: usize = 10;
static STEP: AtomicUsize = AtomicUsize::new(0);
static REPLY: AtomicUsize = AtomicUsize::new(0);
static RECORD: AtomicBool = AtomicBool::new(true);

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

/// The Bots as rich avatars: these rest live, on no baked frames.
fn seed_rich() {
    let st = store();
    st.side_shown.set(true);
    st.side_mode.set("full".to_owned());
    let mut v = threads("first words");
    for b in v["bots"].as_array_mut().unwrap() {
        b["shape"] = "custom".into();
        b["color"] = "#E0A070".into();
        b["outline"] = "M50 10 C74 10 88 30 88 52 C88 76 72 92 50 92 C28 92 12 76 12 52 C12 30 26 10 50 10 Z".into();
        b["layers"] = serde_json::json!([{"path": "M30 30 L70 30 L70 44 L30 44 Z", "fill": "#335577"}]);
        b["eyes"] = "#111111".into();
    }
    st.set_threads(v);
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
                .child(view().label("seed later").w(px(40.0)).h_full().on_press(|| {
                    spawn(async {
                        timer(Duration::from_millis(1500)).await;
                        seed();
                    });
                }))
                .child(view().label("rail").w(px(40.0)).h_full().on_press(|| store().side_mode.set("rail".to_owned())))
                .child(view().label("reply").w(px(40.0)).h_full().on_press(reply))
                .child(view().label("paint").w(px(40.0)).h_full().on_press(paint))
                .child(view().label("group").w(px(40.0)).h_full().on_press(|| group(5)))
                .child(view().label("pin").w(px(40.0)).h_full().on_press(|| {
                    let mut v = threads("first words");
                    v["bots"][20]["pinned"] = true.into();
                    store().set_threads(v);
                }))
                .child(view().label("rename").w(px(40.0)).h_full().on_press(rename))
                .child(view().label("chats").w(px(40.0)).h_full().on_press(chats))
                .child(view().label("status 3").w(px(40.0)).h_full().on_press(|| status_on(3)))
                .child(view().label("real run 3").w(px(40.0)).h_full().on_press(|| {
                    store().agent_set("t3".to_owned(), Some("b3".to_owned()), Some(("writing".to_owned(), "A real run".to_owned())));
                }))
                .child(view().label("short demo 3").w(px(40.0)).h_full().on_press(|| {
                    store().active_id.set("t3".to_owned());
                    store().mock_status_stepping("demo", vec!["thinking", "writing"], Duration::from_millis(1000));
                }))
                .child(view().label("check record").w(px(40.0)).h_full().on_press(|| {
                    RECORD.store(store().mock_run.borrow().is_some(), Ordering::Relaxed);
                }))
                .child(view().label("status 4").w(px(40.0)).h_full().on_press(|| status_on(4)))
                .child(view().label("run all").w(px(40.0)).h_full().on_press(|| {
                    for i in 0..BOTS {
                        store().agent_set(format!("t{i}"), Some(format!("b{i}")), Some(("thinking".to_owned(), "Thinking".to_owned())));
                    }
                }))
                .child(view().label("seed many").w(px(40.0)).h_full().on_press(|| {
                    let bots: Vec<_> = (0..MANY)
                        .map(|i| serde_json::json!({"id": format!("b{i}"), "name": format!("Bot {i}"), "threadId": format!("t{i}"), "shape": "blob", "color": "#1884FC"}))
                        .collect();
                    store().set_threads(serde_json::json!({"list": [], "bots": bots}));
                }))
                .child(view().label("run many").w(px(40.0)).h_full().on_press(|| {
                    for i in 0..MANY {
                        store().agent_set(format!("t{i}"), Some(format!("b{i}")), Some(("thinking".to_owned(), "Thinking".to_owned())));
                    }
                }))
                .child(view().label("reverse").w(px(40.0)).h_full().on_press(|| {
                    let mut bots = store().bots.get_untracked();
                    bots.reverse();
                    store().bots.set(bots);
                }))
                .child(view().label("place").w(px(40.0)).h_full().on_press(|| crate::sidebar::place_rows(1)))
                .child(view().label("place often").w(px(40.0)).h_full().on_press(|| {
                    for _ in 0..50 {
                        crate::sidebar::place_rows(1);
                    }
                }))
                .child(view().label("place and go").w(px(40.0)).h_full().on_press(|| {
                    let (owner, _) = nsh_core::element::create_root(|| crate::sidebar::place_rows(1));
                    owner.dispose();
                }))
                .child(view().label("lose the scroll").w(px(40.0)).h_full().on_press(|| {
                    store().side_window[1].update(|w| w.top = 0.0);
                }))
                .child(view().label("stale answer").w(px(40.0)).h_full().on_press(|| {
                    let asked = store().side_asked[1].get();
                    crate::sidebar::place_rows(1);
                    crate::sidebar::settle(1, asked);
                    RECORD.store(store().side_settled[1].get_untracked(), Ordering::Relaxed);
                }))
                .child(view().label("check settled").w(px(40.0)).h_full().on_press(|| {
                    RECORD.store(store().side_settled[1].get_untracked(), Ordering::Relaxed);
                }))
                .child(view().label("same height").w(px(40.0)).h_full().on_press(|| {
                    let h = store().side_window[1].get_untracked().height;
                    crate::sidebar::list_laid_out(true, h);
                }))
                .child(view().label("lose the list").w(px(40.0)).h_full().on_press(|| {
                    store().side_content[1].set(None);
                    crate::sidebar::place_rows(1);
                }))
                .child(view().label("seed photos").w(px(40.0)).h_full().on_press(|| {
                    let bots: Vec<_> = (0..BOTS)
                        .map(|i| serde_json::json!({"id": format!("b{i}"), "name": format!("Bot {i}"), "threadId": format!("t{i}"),
                            "shape": "blob", "color": "#1884FC", "image": "photo.png", "desc": "first words"}))
                        .collect();
                    store().set_threads(serde_json::json!({"list": [], "bots": bots}));
                }))
                .child(view().label("run last").w(px(40.0)).h_full().on_press(|| {
                    let i = BOTS - 1;
                    store().agent_set(format!("t{i}"), Some(format!("b{i}")), Some(("thinking".to_owned(), "Thinking".to_owned())));
                }))
                .child(view().label("hide").w(px(40.0)).h_full().on_press(|| store().side_shown.set(false)))
                .child(view().label("show").w(px(40.0)).h_full().on_press(|| store().side_shown.set(true)))
                .child(view().label("run 6").w(px(40.0)).h_full().on_press(|| bots_running(6)))
                .child(view().label("run all").w(px(40.0)).h_full().on_press(|| bots_running(BOTS)))
                .child(view().label("unpin and open").w(px(40.0)).h_full().on_press(|| {
                    let n = REPLY.fetch_add(1, Ordering::Relaxed);
                    store().set_threads(threads("first words"));
                    store().active_id.set(format!("t{}", n % BOTS));
                }))
                .child(view().label("seed rich").w(px(40.0)).h_full().on_press(seed_rich))
                .child(view().label("work rows").w(px(40.0)).h_full().on_press(|| {
                    let mut v = threads("first words");
                    let row = |i: usize, mark: &str| serde_json::json!({"id": format!("t{i}"), "title": format!("Bot {i}"), "bot": {"id": format!("b{i}"), "name": format!("Bot {i}")}, mark: true});
                    v["list"] = serde_json::json!([row(1, "working"), row(2, "waiting")]);
                    store().set_threads(v);
                }))
                .child(view().label("rows clear").w(px(40.0)).h_full().on_press(|| store().set_threads(threads("first words"))))
                .child(view().label("stop").w(px(40.0)).h_full().on_press(|| {
                    for i in 0..BOTS {
                        store().agent_set(format!("t{i}"), Some(format!("b{i}")), None);
                    }
                }))
                .child(view().label("open 1").w(px(40.0)).h_full().on_press(|| store().active_id.set("new1".to_owned())))
                .child(view().label("open 10").w(px(40.0)).h_full().on_press(|| store().active_id.set("new10".to_owned())))
                .child(view().label("open 20").w(px(40.0)).h_full().on_press(|| store().active_id.set("new20".to_owned())))
                .child(view().label("shrink").w(px(40.0)).h_full().on_press(|| group(3)))
                .child(view().label("work").w(px(40.0)).h_full().on_press(|| {
                    store().agent_set("t3".to_owned(), Some("b3".to_owned()), Some(("thinking".to_owned(), "Thinking".to_owned())));
                }))
                .child(view().label("rest").w(px(40.0)).h_full().on_press(|| store().agent_set("t3".to_owned(), Some("b3".to_owned()), None)))
                .child(text(|| {
                    let grouped = store().groups.get().iter().any(|g| g.items.iter().any(|it| it.id == "b1"));
                    format!("probe ready=1 grouped={} work={:?},{:?},{:?}", grouped as u8, store().work_of_bot("b1"), store().work_of_bot("b2"), store().work_of_bot("b3"))
                }).text_size(px(8.0))),
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
    pump_until_built(&mut app);
    assert!(app.live.len() > BOTS * 3, "only {} nodes: the rows never showed", app.live.len());
    // the first write puts the status rows up, which is new, not rebuilt
    app.press("step");
    app.pump(Duration::from_millis(100));
    pump_until_built(&mut app);
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

/// New names and threads for a grouped, an ungrouped and a pinned Bot.
fn rename() {
    let mut v = threads("first words");
    v["bots"][20]["pinned"] = true.into();
    for i in [1, 10, 20] {
        v["bots"][i]["name"] = format!("Renamed {i}").into();
        v["bots"][i]["threadId"] = format!("new{i}").into();
    }
    store().set_threads(v);
}

/// Chat rows for Bots 3 and 4: `/status` finds its Bot through them.
fn chats() {
    let mut v = threads("first words");
    v["list"] = serde_json::json!([
        {"id": "t3", "title": "Three", "bot": {"id": "b3", "name": "Bot 3"}},
        {"id": "t4", "title": "Four", "bot": {"id": "b4", "name": "Bot 4"}},
    ]);
    store().set_threads(v);
}

/// `/status:thinking` in Bot `i`'s chat.
fn status_on(i: usize) {
    store().active_id.set(format!("t{i}"));
    store().mock_status("thinking");
}

/// The first `n` Bots running a state that keeps them moving.
fn bots_running(n: usize) {
    for i in 0..n {
        store().agent_set(format!("t{i}"), Some(format!("b{i}")), Some(("thinking".to_owned(), "Thinking".to_owned())));
    }
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
    pump_until_built(&mut app);
    let start = app.live.clone();
    let made = built();
    for k in 0..30 {
        app.press("reply");
        app.pump(Duration::from_millis(60));
        if !rail {
            let words = format!("streamed {k} words");
            pump_until(&mut app, "every row showing the reply", |app| {
                app.texts().iter().filter(|t| t.contains(&words)).count() == BOTS
            });
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

/// A tile isn't rebuilt when its Bot changes, so it reads the record itself:
/// a new name labels it, and a new thread lights it when that chat opens. In
/// a group, loose, and pinned.
#[test]
fn a_tile_follows_its_bots_name_and_thread() {
    if !in_child("a_tile_follows_its_bots_name_and_thread") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.press("group");
    app.press("pin");
    app.press("rail");
    pump_until(&mut app, "Bot 1 in the group", |app| num(&read(app), "grouped") == 1.0);
    app.press("rename");
    for i in [1, 10, 20] {
        let name = format!("Renamed {i}");
        pump_until(&mut app, &format!("{name}: the tile's label"), |app| app.has(&name));
        let tile = app.view(&name);
        let fill = |app: &Headless| app.styles.get(&tile).map(|s| s.background_color);
        let dark = fill(&app);
        app.press(&format!("open {i}"));
        pump_until(&mut app, &format!("{name}: lit when its chat opens"), |app| fill(app) != dark);
        let lit = fill(&app).expect("a tile's style");
        assert!(lit.0 & 0xff > 0, "{name}: an active fill, not a transparent one");
    }
}

/// A tile leaving while what it reads changes, in one update: unpinned as a
/// chat opens, over and over. Nothing may read the row after it's gone.
#[test]
fn a_tile_can_go_while_it_is_changing() {
    if !in_child("a_tile_can_go_while_it_is_changing") {
        return;
    }
    let mut app = start();
    app.press("seed");
    for mode in ["full", "rail"] {
        if mode == "rail" {
            app.press("rail");
        }
        for _ in 0..12 {
            app.press("pin");
            app.pump(Duration::from_millis(40));
            app.press("unpin and open");
            app.pump(Duration::from_millis(40));
        }
    }
    app.press("reply");
    app.pump(Duration::from_millis(100));
    assert!(app.has("Bot 20"), "the unpinned Bot keeps its tile");
}

/// Once every run ends the avatars come to rest and stop painting: an idle
/// Bot costs nothing. Without a rebuild to reset them, they settle themselves.
#[test]
fn avatars_stop_painting_when_every_run_ends() {
    if in_child("avatars_stop_painting_when_every_run_ends") {
        rests_after_runs("seed");
    }
}

#[test]
fn rich_avatars_stop_painting_when_every_run_ends() {
    if in_child("rich_avatars_stop_painting_when_every_run_ends") {
        rests_after_runs("seed rich");
    }
}

fn rests_after_runs(seed: &str) {
    let paints = || crate::avatar::PAINTS.load(Ordering::Relaxed);
    let mut app = start();
    app.press(seed);
    app.pump(Duration::from_millis(400));
    pump_until_built(&mut app);
    let before = paints();
    app.pump(Duration::from_millis(1000));
    println!("idle at launch: {} paints in a second", paints() - before);
    for _ in 0..40 {
        app.press("step");
        app.pump(Duration::from_millis(50));
    }
    app.press("stop");
    // how long clips take to finish depends on the machine, and a loaded one
    // takes minutes: what matters is that painting stops, and stays stopped
    let mut calm = 0;
    for s in 0..RESTS_WITHIN {
        let at = paints();
        app.pump(Duration::from_millis(1000));
        println!("second {s} after the runs end: {} paints", paints() - at);
        calm = if paints() == at { calm + 1 } else { 0 };
        if calm == CALM_SECONDS {
            let at = paints();
            app.pump(Duration::from_millis(2000));
            assert_eq!(paints() - at, 0, "painting started again with nothing running");
            return;
        }
    }
    panic!("still painting {RESTS_WITHIN} seconds after every run ended");
}

/// Pumps until `done`, for machines too loaded to keep to a fixed wait.
fn pump_until(app: &mut Headless, what: &str, mut done: impl FnMut(&mut Headless) -> bool) {
    for _ in 0..WAIT_STEPS {
        if done(app) {
            return;
        }
        app.pump(Duration::from_millis(50));
    }
    panic!("{what}: not within {} seconds", WAIT_STEPS / 20);
}

/// Pumps until `bot`'s avatar has painted `state`.
fn pump_until_painted(app: &mut Headless, bot: &str, state: &str) {
    let painted = || crate::avatar::PAINTED.lock().unwrap().get(bot).copied();
    pump_until(app, &format!("{bot} painting {state}"), |_| painted() == Some(state));
}

/// Pumps until `count` has stopped moving. Waiting for that, not for a set
/// time, is what holds on a slow machine.
fn pump_until_still(app: &mut Headless, what: &str, count: impl Fn() -> usize) {
    let mut last = count();
    let mut still = 0;
    pump_until(app, what, |_| {
        let now = count();
        still = if now == last { still + 1 } else { 0 };
        last = now;
        still == STILL_STEPS
    });
}

/// Pumps until passes over the rows have stopped: the first layouts ask for
/// them too, and a slow machine is still doing those after any fixed wait.
fn pump_until_placed(app: &mut Headless) {
    pump_until_still(app, "rows still being placed", || {
        crate::sidebar::ROW_PASSES.load(Ordering::Relaxed)
    });
}

/// Whether `name` has its row: a rail tile by its label, a sidebar row by its text.
fn row_live(app: &mut Headless, name: &str) -> bool {
    app.has(name) || app.texts().contains(&name)
}

/// Pumps until every Bot's row is live and no avatar has been built for a
/// while: the list is up, so what a test builds after this is its own.
fn pump_until_built(app: &mut Headless) {
    let mut last = built();
    let mut still = 0;
    pump_until(app, "the list still being built", |app| {
        let rows = (0..BOTS).all(|i| row_live(app, &format!("Bot {i}")));
        let now = built();
        still = if rows && now == last { still + 1 } else { 0 };
        last = now;
        still == STILL_STEPS
    });
}

/// The wait for the list holds out for its rows: quiet before they arrive
/// isn't the list being up.
#[test]
fn the_wait_for_the_list_waits_for_its_rows() {
    if !in_child("the_wait_for_the_list_waits_for_its_rows") {
        return;
    }
    let mut app = start();
    app.press("seed later");
    pump_until_built(&mut app);
    assert!(row_live(&mut app, &format!("Bot {}", BOTS - 1)), "the last row is there");
}

/// An avatar the pointer has passed over goes back to sleep: it doesn't stay
/// awake, repainting an idle Bot, for each one touched.
#[test]
fn a_hovered_avatar_goes_back_to_sleep() {
    if !in_child("a_hovered_avatar_goes_back_to_sleep") {
        return;
    }
    let paints = || crate::avatar::PAINTS.load(Ordering::Relaxed);
    let mut app = start();
    app.press("seed rich");
    app.press("rail");
    pump_until(&mut app, "the rows to hover", |app| (0..12).all(|i| app.has(&format!("Bot {i}"))));
    app.pump(Duration::from_millis(600));
    let at = paints();
    app.pump(Duration::from_millis(1000));
    println!("before any hover: {} paints in a second", paints() - at);
    let quiet = paints();
    for i in 0..12 {
        let row = format!("Bot {i}");
        app.hover(&row, true);
        for x in [8.0, 18.0, 28.0, 38.0] {
            app.hover_at(&row, x, 20.0);
        }
        app.hover(&row, false);
    }
    assert!(paints() > quiet, "the hover woke no avatar, so this proves nothing");
    for s in 0..12 {
        let at = paints();
        app.pump(Duration::from_millis(1000));
        println!("second {s} after the hover: {} paints", paints() - at);
    }
    let at = paints();
    app.pump(Duration::from_millis(1000));
    assert_eq!(paints() - at, 0, "paints a second, long after the pointer left");
}

/// A new `/status` clears what the last one left playing in another chat,
/// or every Bot it was ever run on keeps animating.
#[test]
fn a_new_status_clears_the_last_one() {
    if !in_child("a_new_status_clears_the_last_one") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.press("chats");
    app.pump(Duration::from_millis(400));
    app.press("status 3");
    pump_until_painted(&mut app, "b3", "thinking");
    app.press("status 4");
    pump_until_painted(&mut app, "b4", "thinking");
    pump_until_painted(&mut app, "b3", "idle");
}

/// `/status` clears only what it set: a real run that has started in that
/// chat since is left alone.
#[test]
fn a_new_status_leaves_a_real_run_alone() {
    if !in_child("a_new_status_leaves_a_real_run_alone") {
        return;
    }
    let painted = |bot: &str| crate::avatar::PAINTED.lock().unwrap().get(bot).copied();
    let mut app = start();
    app.press("seed");
    app.press("chats");
    app.pump(Duration::from_millis(400));
    app.press("status 3");
    pump_until_painted(&mut app, "b3", "thinking");
    app.press("real run 3");
    pump_until_painted(&mut app, "b3", "writing");
    app.press("status 4");
    pump_until_painted(&mut app, "b4", "thinking");
    app.pump(Duration::from_millis(300));
    assert_eq!(painted("b3"), Some("writing"), "the real run is still there");
}

/// A demo that runs to its end leaves nothing for the next one to clear.
#[test]
fn a_finished_demo_leaves_no_record() {
    if !in_child("a_finished_demo_leaves_no_record") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.press("chats");
    app.pump(Duration::from_millis(400));
    app.press("short demo 3");
    app.press("check record");
    assert!(RECORD.load(Ordering::Relaxed), "a running demo has a record");
    pump_until(&mut app, "a finished demo still has a record", |app| {
        app.press("check record");
        !RECORD.load(Ordering::Relaxed)
    });
    pump_until_painted(&mut app, "b3", "idle");
}

/// Avatars in a hidden sidebar paint nothing, however many are running, and
/// pick up again when it comes back.
#[test]
fn a_hidden_sidebar_paints_nothing() {
    if !in_child("a_hidden_sidebar_paints_nothing") {
        return;
    }
    let paints = || crate::avatar::PAINTS.load(Ordering::Relaxed);
    let mut app = start_placed();
    app.press("run all");
    let at = paints();
    pump_until(&mut app, "the Bots running", |_| paints() - at > 100);
    app.press("hide");
    pump_until_still(&mut app, "paints with the sidebar hidden", paints);
    let at = paints();
    app.pump(Duration::from_millis(1000));
    let hidden = paints() - at;
    println!("sidebar hidden: {hidden} paints in a second");
    assert_eq!(hidden, 0, "paints a second with the sidebar hidden");
    app.press("show");
    pump_until(&mut app, "the Bots back when it's shown", |_| paints() - at > 100);
}

/// Pumps until `bot` has drawn a few more frames, and gives what every Bot
/// drew meanwhile.
fn paints_until(app: &mut Headless, bot: usize) -> Vec<usize> {
    let of = |i: usize| crate::avatar::PAINTED_BY.lock().unwrap().get(&format!("b{i}")).copied().unwrap_or(0);
    let at: Vec<usize> = (0..BOTS).map(of).collect();
    pump_until(app, &format!("Bot {bot} painting"), |_| of(bot) - at[bot] > 5);
    (0..BOTS).zip(at).map(|(i, n)| of(i) - n).collect()
}

/// How many Bots draw while the first draws a few frames: what a frame costs,
/// however fast the machine is.
fn rows_painting(app: &mut Headless) -> usize {
    let at = crate::avatar::PAINTED_BY.lock().unwrap().clone();
    let first = || crate::avatar::PAINTED_BY.lock().unwrap().get("b0").copied().unwrap_or(0);
    let from = first();
    pump_until(app, "the first Bot painting", |_| first() - from > 5);
    let now = crate::avatar::PAINTED_BY.lock().unwrap().clone();
    now.iter().filter(|(bot, n)| at.get(*bot).copied().unwrap_or(0) < **n).count()
}

/// The seeded list, built, with nothing left placing it.
fn start_placed() -> Headless {
    let mut app = start();
    app.press("seed");
    pump_until_built(&mut app);
    pump_until_placed(&mut app);
    app
}

/// A window too short for the list, with every Bot running.
fn start_running_short() -> Headless {
    let mut app = start_placed();
    app.resize(1200.0, 400.0);
    pump_until_placed(&mut app);
    app.press("run all");
    pump_until_placed(&mut app);
    app
}

fn scroll_to(app: &mut Headless, list: &str, y: f32) {
    let id = app.view(list);
    app.scroll(id, y);
    pump_until_placed(app);
}

/// Only rows in the window paint, and scrolling moves the window: a row at
/// the top stops as it leaves, one further down starts as it arrives.
#[test]
fn rows_out_of_view_paint_nothing() {
    if !in_child("rows_out_of_view_paint_nothing") {
        return;
    }
    let mut app = start_running_short();
    let top = paints_until(&mut app, 0);
    println!("at the top: {top:?}");
    assert_eq!(top[BOTS - 1], 0, "the last row is out of view: {top:?}");
    scroll_to(&mut app, "Bot list", 1000.0);
    let down = paints_until(&mut app, BOTS - 1);
    println!("scrolled down: {down:?}");
    assert_eq!(down[0], 0, "the first row has left: {down:?}");
}

/// A new order leaves every size alone, but the rows have moved: the ones now
/// in view move, and the ones that left stop.
#[test]
fn a_reorder_keeps_the_rows_in_view_moving() {
    if !in_child("a_reorder_keeps_the_rows_in_view_moving") {
        return;
    }
    let mut app = start_running_short();
    app.press("reverse");
    pump_until_placed(&mut app);
    let now = paints_until(&mut app, BOTS - 1);
    println!("reversed: {now:?}");
    assert_eq!(now[0], 0, "the row now at the bottom stops: {now:?}");
}

/// A scroll landing while rows are being placed doesn't misplace them: their
/// place is in the content, whatever the offset.
#[test]
fn a_scroll_while_placing_leaves_rows_right() {
    if !in_child("a_scroll_while_placing_leaves_rows_right") {
        return;
    }
    let mut app = start_running_short();
    app.press("place");
    scroll_to(&mut app, "Bot list", 1000.0);
    // a pass that measured before the scroll is put right by the watch
    app.pump(Duration::from_millis(1200));
    pump_until_placed(&mut app);
    let down = paints_until(&mut app, BOTS - 1);
    println!("scrolled while placing: {down:?}");
    assert_eq!(down[0], 0, "the first row has left: {down:?}");
}

/// However often a tick asks, the rows are placed once.
#[test]
fn rows_are_placed_once_a_tick() {
    if !in_child("rows_are_placed_once_a_tick") {
        return;
    }
    let passes = || crate::sidebar::ROW_PASSES.load(Ordering::Relaxed);
    let mut app = start_placed();
    let at = passes();
    app.press("place often");
    pump_until_placed(&mut app);
    assert_eq!(passes() - at, 1, "passes for 50 asks in a tick");
}

/// Scrolling moves the window and asks for no pass, so rows out of view stay
/// asleep the whole way.
#[test]
fn a_scroll_wakes_no_row_out_of_view() {
    if !in_child("a_scroll_wakes_no_row_out_of_view") {
        return;
    }
    let of = || crate::avatar::PAINTED_BY.lock().unwrap().get(&format!("b{}", BOTS - 1)).copied().unwrap_or(0);
    let mut app = start_running_short();
    let id = app.view("Bot list");
    let at = of();
    for step in 1..=20 {
        app.scroll(id, step as f32 * 5.0);
        app.pump(Duration::from_millis(50));
    }
    assert_eq!(of() - at, 0, "frames a row out of view drew over 20 scrolls");
}

/// A list nobody touches isn't placed again: the watch for scrolls nobody
/// reported only asks when the offset has moved.
#[test]
fn a_settled_list_stays_quiet() {
    if !in_child("a_settled_list_stays_quiet") {
        return;
    }
    let passes = || crate::sidebar::ROW_PASSES.load(Ordering::Relaxed);
    let mut app = start_placed();
    let at = passes();
    app.pump(Duration::from_millis(3500));
    assert_eq!(passes() - at, 0, "passes over three watches of a still list");
}

/// A row going away with a pass on its way doesn't stop the list being
/// placed from then on.
#[test]
fn a_row_going_away_leaves_the_list_placed() {
    if !in_child("a_row_going_away_leaves_the_list_placed") {
        return;
    }
    let passes = || crate::sidebar::ROW_PASSES.load(Ordering::Relaxed);
    let mut app = start_placed();
    app.press("place and go");
    pump_until_placed(&mut app);
    let at = passes();
    app.press("place");
    pump_until_placed(&mut app);
    assert_eq!(passes() - at, 1, "a pass after the one whose owner went");
}

/// A scroll that never reported is found by the next pass.
#[test]
fn a_scroll_nobody_reported_is_found() {
    if !in_child("a_scroll_nobody_reported_is_found") {
        return;
    }
    let mut app = start_running_short();
    scroll_to(&mut app, "Bot list", 1000.0);
    app.press("lose the scroll");
    // nothing asks: the watch on the offset finds it, and the last row is back
    paints_until(&mut app, BOTS - 1);
    pump_until_placed(&mut app);
    let now = paints_until(&mut app, BOTS - 1);
    println!("after a lost scroll: {now:?}");
    assert_eq!(now[0], 0, "the first row isn't in view: {now:?}");
}

/// A pass that can't see the list leaves its rows painting: unknown is seen.
#[test]
fn rows_nobody_can_place_paint() {
    if !in_child("rows_nobody_can_place_paint") {
        return;
    }
    let mut app = start_running_short();
    let before = paints_until(&mut app, 0);
    assert_eq!(before[BOTS - 1], 0, "placed, the last row is out of view: {before:?}");
    app.press("lose the list");
    let now = paints_until(&mut app, BOTS - 1);
    println!("unplaced: {now:?}");
}

/// An answer to an older ask doesn't settle a list with a newer one waiting.
#[test]
fn an_older_pass_doesnt_settle_a_newer_ask() {
    if !in_child("an_older_pass_doesnt_settle_a_newer_ask") {
        return;
    }
    let mut app = start_placed();
    app.press("stale answer");
    assert!(!RECORD.load(Ordering::Relaxed), "settled by the older answer");
    pump_until_placed(&mut app);
    app.press("check settled");
    assert!(RECORD.load(Ordering::Relaxed), "the newer pass settles it");
}

/// Each ask gets its own tries, not what the last one left.
#[test]
fn each_ask_gets_its_own_tries() {
    if !in_child("each_ask_gets_its_own_tries") {
        return;
    }
    let passes = || crate::sidebar::ROW_PASSES.load(Ordering::Relaxed);
    let mut app = start_placed();
    app.press("lose the list");
    pump_until_placed(&mut app);
    let at = passes();
    app.press("place");
    pump_until_placed(&mut app);
    println!("passes for a second ask: {}", passes() - at);
    assert!(passes() - at > 3, "tries for the second ask: {}", passes() - at);
}

/// A layout that changes nothing asks for nothing.
#[test]
fn a_layout_at_the_same_height_places_nothing() {
    if !in_child("a_layout_at_the_same_height_places_nothing") {
        return;
    }
    let passes = || crate::sidebar::ROW_PASSES.load(Ordering::Relaxed);
    let mut app = start_placed();
    let at = passes();
    app.press("same height");
    pump_until_placed(&mut app);
    assert_eq!(passes() - at, 0, "passes for a layout that moved nothing");
}

/// What shows can change with the ids the same: a pin, a hide, a new section.
#[test]
fn what_shows_follows_pins_and_hiding() {
    let bots = || vec![crate::store::RoxBot { id: "a".to_owned(), ..Default::default() }];
    let before = crate::sidebar::what_shows(&bots());
    for change in [
        (|b: &mut crate::store::RoxBot| b.pinned = true) as fn(&mut crate::store::RoxBot),
        |b| b.hidden = true,
        |b| b.section = "team".to_owned(),
    ] {
        let mut now = bots();
        change(&mut now[0]);
        assert_ne!(crate::sidebar::what_shows(&now), before);
    }
}

/// A state change on a hidden Bot draws nothing, and it shows on return.
#[test]
fn a_state_change_out_of_sight_paints_nothing() {
    if !in_child("a_state_change_out_of_sight_paints_nothing") {
        return;
    }
    let paints = || crate::avatar::PAINTS.load(Ordering::Relaxed);
    let mut app = start_placed();
    app.press("hide");
    pump_until_still(&mut app, "paints with the sidebar hidden", paints);
    let at = paints();
    app.press("run all");
    app.pump(Duration::from_millis(500));
    assert_eq!(paints() - at, 0, "paints for a state change with the sidebar hidden");
    app.press("show");
    pump_until(&mut app, "the change painting on return", |_| paints() > at);
}

/// A photo Bot's overlay runs the clock, so out of sight it mustn't be up.
#[test]
fn a_photo_out_of_sight_asks_for_no_frames() {
    if !in_child("a_photo_out_of_sight_asks_for_no_frames") {
        return;
    }
    let ticks = || crate::store::TICKS.load(Ordering::Relaxed);
    let mut app = start();
    app.press("seed");
    app.press("seed photos");
    pump_until_built(&mut app);
    app.resize(1200.0, 400.0);
    pump_until_placed(&mut app);
    // the last Bot is below the window
    app.press("run last");
    pump_until_still(&mut app, "frames for a photo out of view", ticks);
    let at = ticks();
    app.pump(Duration::from_millis(1000));
    assert_eq!(ticks() - at, 0, "frames a second for a photo scrolled out of view");
    scroll_to(&mut app, "Bot list", 1000.0);
    let at = ticks();
    pump_until(&mut app, "frames for a photo in view", |_| ticks() - at > 5);
    app.press("hide");
    pump_until_still(&mut app, "frames with the sidebar hidden", ticks);
    let at = ticks();
    app.pump(Duration::from_millis(1000));
    assert_eq!(ticks() - at, 0, "frames a second for a photo with the sidebar hidden");
}

/// The cost of avatars is what's on screen: many more Bots, no more painting.
#[test]
fn many_bots_cost_what_fits_on_screen() {
    if !in_child("many_bots_cost_what_fits_on_screen") {
        return;
    }
    let mut app = start_placed();
    app.press("run all");
    pump_until_placed(&mut app);
    let few = rows_painting(&mut app);
    app.press("seed many");
    pump_until_still(&mut app, "the new rows being built", built);
    pump_until_placed(&mut app);
    app.press("run many");
    pump_until_placed(&mut app);
    let many = rows_painting(&mut app);
    println!("rows painting: {few} with {BOTS} Bots, {many} with {MANY}");
    assert!(few > 0, "the Bots are running");
    assert!(many <= few + 2, "{MANY} Bots paint {many} rows against {few} for {BOTS}");
}

/// How many avatars usually paint on a tick with `button` pressed. Ticks, not
/// seconds: a slow machine paints less often, but the same avatars together.
fn usual_tick(app: &mut Headless, button: &str) -> usize {
    let ticked = || crate::avatar::TICKED.lock().unwrap();
    app.press(button);
    app.pump(Duration::from_millis(1500));
    ticked().clear();
    pump_until(app, "enough ticks", |_| ticked().len() >= 40);
    let mut sizes: Vec<usize> = ticked().values().copied().collect();
    sizes.sort_unstable();
    sizes[sizes.len() / 2]
}

/// In a crowd sidebar avatars take turns, so a tick repaints a quarter of them;
/// a few all move on every tick.
#[test]
fn sidebar_avatars_take_turns_in_a_crowd() {
    if !in_child("sidebar_avatars_take_turns_in_a_crowd") {
        return;
    }
    let mut app = start();
    app.press("seed");
    app.pump(Duration::from_millis(400));
    let few = usual_tick(&mut app, "run 6");
    let crowd = usual_tick(&mut app, "run all");
    println!("a tick usually paints {few} of 6, {crowd} of {BOTS}");
    assert_eq!(few, 6, "a few all move on a tick");
    assert!((2..=BOTS.div_ceil(4) + 1).contains(&crowd), "a tick carries a quarter of the crowd, not {crowd}");
}

/// A crowd of avatars with others that can't give way built between them.
#[allow(non_snake_case)]
#[component]
fn Mixed() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    let blob = |give_way: bool| {
        crate::avatar::bot_blob(crate::avatar::BotBlobProps {
            shape: "blob".to_owned(),
            color: "#1884FC".to_owned(),
            size: 10.0,
            state: "thinking",
            give_way,
            ..Default::default()
        })
        .into_element()
    };
    view().size_full().child(text("probe ready=1")).children((0..48).map(move |i| blob(i % 4 == 0)))
}

/// The lanes fill with whoever's live, however the avatars around them were
/// built: in the order they're made, every one of these would share a lane.
#[test]
fn a_crowd_shares_the_lanes_however_it_was_built() {
    if !in_child("a_crowd_shares_the_lanes_however_it_was_built") {
        return;
    }
    let mut config = EngineConfig::new(App::Rust(Root::new::<Mixed>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(400.0), px(800.0));
    let mut app = run(config);
    let ticked = || crate::avatar::TICKED.lock().unwrap();
    app.pump(Duration::from_millis(1500));
    ticked().clear();
    pump_until(&mut app, "enough ticks", |_| ticked().len() >= 40);
    let mut sizes: Vec<usize> = ticked().values().copied().collect();
    sizes.sort_unstable();
    let usual = sizes[sizes.len() / 2];
    println!("a tick usually paints {usual} of 12");
    assert!((2..=4).contains(&usual), "a tick carries a quarter of the crowd, not {usual}");
}

static LANE_PAINTS: AtomicUsize = AtomicUsize::new(0);

/// More than a crowd of live avatars, every one in the first lane.
#[allow(non_snake_case)]
#[component]
fn OneLane() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view()
        .size_full()
        .child(text("probe ready=1"))
        .children((0..12).map(|_| {
            let lane = std::cell::Cell::new((0, 0));
            canvas(move || {
                // every one in the first lane of whatever crowd this is
                lane.set((store().frame_crowds.get(), 0));
                store().tick_giving_way(&lane);
                LANE_PAINTS.fetch_add(1, Ordering::Relaxed);
                Painter::new()
            })
            .size(px(10.0))
            .into_element()
        }))
}

/// Avatars that come to rest can leave a crowd's lanes empty. The clock has to
/// keep going through them, or everything stops where it is.
#[test]
fn a_crowd_in_one_lane_keeps_moving() {
    if !in_child("a_crowd_in_one_lane_keeps_moving") {
        return;
    }
    let mut config = EngineConfig::new(App::Rust(Root::new::<OneLane>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(400.0), px(400.0));
    let mut app = run(config);
    app.pump(Duration::from_millis(1500));
    let at = LANE_PAINTS.load(Ordering::Relaxed);
    app.pump(Duration::from_millis(1000));
    let painted = LANE_PAINTS.load(Ordering::Relaxed) - at;
    assert!(painted > 12 * 5, "{painted} paints in a second from 12 avatars");
}

/// Back from away, a row says what goes on there and what waits for a person, and the dot goes
/// with the work. Neither says anything of what the app can be counted on for.
#[test]
fn a_row_shows_work_going_on_and_a_card_waiting() {
    if !in_child("a_row_shows_work_going_on_and_a_card_waiting") {
        return;
    }
    let mut app = start();
    app.press("seed");
    pump_until_built(&mut app);
    assert!(!app.has("Working") && !app.has("Waiting for you"));
    app.press("work rows");
    pump_until(&mut app, "no dot came", |app| app.has("Working") && app.has("Waiting for you"));
    assert!(app.texts().iter().any(|t| t.contains("work=Working,Waiting,None")), "{:?}", app.texts());
    app.press("rows clear");
    pump_until(&mut app, "the dots stayed", |app| !app.has("Working") && !app.has("Waiting for you"));
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
    pump_until_built(&mut app);
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
    pump_until_built(&mut app);
    pump_until_built(&mut app);
    let start = app.live.clone();
    let made = built();
    for k in 0..30 {
        app.press("reply");
        app.pump(Duration::from_millis(60));
        let words = format!("streamed {k} words");
        pump_until(&mut app, "the reply showing", |app| app.texts().iter().any(|t| t.contains(&words)));
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
    pump_until_built(&mut app);
    pump_until_built(&mut app);
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
    pump_until_built(&mut app);
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
