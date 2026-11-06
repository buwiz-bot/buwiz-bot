//! Edit Avatar's tabs on a headless engine: what a person sees and what a press does. Nothing
//! reaches a server: the backend is a port nothing listens on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use buwiz_office_kit::testing::{Headless, read, run, until};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvent, FetchEvents, FetchTask, HttpBody, HttpHead, HttpRequest, Network};
use serde_json::{Value, json};

use crate::live::store;
use crate::store::AvatarCand;

const CHILD: &str = "BUWIZ_EDITOR_UI_TEST";
const EMPTY: &str = "Avatars you design show up here.";
const LOADING: &str = "Loading your avatars…";

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

/// A broker's `/state/buwiz-avatar-designs`: answers GETs with what `state` holds after
/// failing the first `fails`, takes `slow` over each POST, and keeps what's posted.
#[derive(Clone)]
struct Stub {
    state: Arc<Mutex<Value>>,
    fails: Arc<AtomicUsize>,
    save_fails: Arc<AtomicUsize>,
    /// What `POST /avatar/packs` answers, and what it was asked.
    packs: Arc<Mutex<(u16, Value)>>,
    pack_asks: Arc<Mutex<Vec<Value>>>,
    slow: Duration,
    gets: Arc<AtomicUsize>,
    posts: Arc<Mutex<Vec<Value>>>,
    open: Arc<AtomicUsize>,
    most_open: Arc<AtomicUsize>,
}

fn stub(state: Value, fails: usize, slow: Duration) -> Stub {
    Stub {
        state: Arc::new(Mutex::new(state)),
        fails: Arc::new(AtomicUsize::new(fails)),
        save_fails: Arc::default(),
        packs: Arc::new(Mutex::new((200, json!({"ok": true, "designs": [], "refused": []})))),
        pack_asks: Arc::default(),
        slow,
        gets: Arc::default(),
        posts: Arc::default(),
        open: Arc::default(),
        most_open: Arc::default(),
    }
}

struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

impl Network for Stub {
    fn fetch(&self, request: HttpRequest, mut events: FetchEvents) -> Box<dyn FetchTask> {
        let stub = self.clone();
        std::thread::spawn(move || {
            let (status, reply) = stub.answer(&request);
            let head = HttpHead { status, url: request.url, ..HttpHead::default() };
            events(FetchEvent::Head(head));
            events(FetchEvent::Data(reply.into_bytes()));
            events(FetchEvent::End);
        });
        Box::new(Done)
    }
}

impl Stub {
    /// A refused save answers with JSON that looks fine and a saved one with none: it's the
    /// status that says which.
    fn answer(&self, request: &HttpRequest) -> (u16, String) {
        if request.url.ends_with("/avatar/packs") {
            let HttpBody::Bytes(body) = &request.body else { panic!("an ask with no body") };
            self.pack_asks.lock().unwrap().push(serde_json::from_slice(body).unwrap());
            let (status, reply) = self.packs.lock().unwrap().clone();
            return (status, reply.to_string());
        }
        if !request.url.ends_with("/state/buwiz-avatar-designs") {
            return (200, "{}".to_owned());
        }
        if request.method == "POST" {
            let fails = self.save_fails.load(Ordering::SeqCst);
            if fails > 0 {
                self.save_fails.store(fails - 1, Ordering::SeqCst);
                return (500, r#"{"ok":true}"#.to_owned());
            }
            let open = self.open.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_open.fetch_max(open, Ordering::SeqCst);
            std::thread::sleep(self.slow);
            let HttpBody::Bytes(body) = &request.body else { panic!("a save with no body") };
            let list: Value = serde_json::from_slice(body).unwrap();
            *self.state.lock().unwrap() = json!({"state": list.clone()});
            self.posts.lock().unwrap().push(list);
            self.open.fetch_sub(1, Ordering::SeqCst);
            return (200, "saved".to_owned());
        }
        self.gets.fetch_add(1, Ordering::SeqCst);
        // one read at a time: the app waits for each answer
        let fails = self.fails.load(Ordering::SeqCst);
        if fails > 0 {
            self.fails.store(fails - 1, Ordering::SeqCst);
            return (500, "{}".to_owned());
        }
        (200, self.state.lock().unwrap().to_string())
    }

    fn posts(&self) -> Vec<Value> {
        self.posts.lock().unwrap().clone()
    }

    /// The names in the list the broker holds now.
    fn names(&self) -> Vec<String> {
        names(&self.state.lock().unwrap()["state"])
    }
}

fn names(list: &Value) -> Vec<String> {
    list["designs"].as_array().map(|d| d.iter().map(|e| e["name"].as_str().unwrap_or("").to_owned()).collect()).unwrap_or_default()
}

fn saved(designs: Value) -> Value {
    json!({"state": {"designs": designs}})
}

fn keep(name: &'static str, shape: &'static str) {
    store().design_keep(&AvatarCand {
        name: name.to_owned(),
        shape: shape.to_owned(),
        color: "#FFFFFF".to_owned(),
        ..Default::default()
    });
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
    Owner::detached().run(|| st.load_designs());
    view()
        .size_full()
        .child(text(|| {
            let shape = store().active_bot().map(|b| b.shape).unwrap_or_default();
            let n = store().design_saved.get().len();
            format!("probe shape={shape} shown={n} loaded={}", store().design_loaded.get())
        }))
        .child(view().label("add a design").w(px(40.0)).h(px(40.0)).on_press(|| keep("Mine", "cloud")))
        .child(view().label("keep heart").w(px(40.0)).h(px(40.0)).on_press(|| keep("Heart", "heart")))
        .child(view().label("keep star").w(px(40.0)).h(px(40.0)).on_press(|| keep("Star", "star")))
        .child(view().label("forget first").w(px(40.0)).h(px(40.0)).on_press(|| store().design_forget(0)))
        .child(view().label("install fox").w(px(40.0)).h(px(40.0)).on_press(|| crate::avatars::run(" install fox https://github.com/me/mine")))
        .child(view().label("read again").w(px(40.0)).h(px(40.0)).on_press(|| store().load_designs()))
        .child(view().label("set a photo").w(px(40.0)).h(px(40.0)).on_press(|| {
            store().avatar_draft.set(Some(crate::store::AvatarDraft { uri: "/tmp/p.png".to_owned(), ..Default::default() }));
            store().set_avatar();
            store().open_avatar_editor();
        }))
        .child(crate::pane::editor_overlay())
        .child(crate::toast::layer())
}

fn start(broker: &Stub) -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Editor>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1200.0), px(900.0));
    config.network = Some(Arc::new(broker.clone()));
    let mut app = run(config);
    app.wait_for("Own tab");
    app
}

/// [`start`], with the list read.
fn start_loaded(broker: &Stub) -> Headless {
    let mut app = start(broker);
    until(&mut app, "the list read", |p| p.get("loaded").is_some_and(|l| l == "true"));
    app
}

fn shows(app: &Headless, line: &str) -> bool {
    app.texts().contains(&line)
}

fn pump_until(app: &mut Headless, what: &str, done: impl Fn(&Headless) -> bool) {
    for _ in 0..1200 {
        if done(app) {
            return;
        }
        app.pump(Duration::from_millis(50));
    }
    panic!("{what}: not within a minute: {:?}", app.texts());
}

/// Your avatars are on Own, not on Bot, and there's no Design tab to find.
#[test]
fn own_lists_your_avatars_and_bot_doesnt() {
    if !in_child("own_lists_your_avatars_and_bot_doesnt") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
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

/// With nothing of your own, Own says so instead of being blank, and a first
/// design on a fresh app is saved.
#[test]
fn own_with_nothing_says_so() {
    if !in_child("own_with_nothing_says_so") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
    app.press("Own tab");
    pump_until(&mut app, "the empty line", |app| shows(app, EMPTY));
    app.press("add a design");
    app.wait_for("Use your Mine design");
    assert!(!shows(&app, EMPTY), "the line stays with a design there");
    pump_until(&mut app, "the save", |_| broker.names() == ["Mine"]);
}

/// A tap on Own puts the avatar on the Bot, and Revert brings the old one back.
#[test]
fn a_tap_on_own_uses_it_and_revert_undoes_it() {
    if !in_child("a_tap_on_own_uses_it_and_revert_undoes_it") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
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

/// A Bot-tab pick after an Own pick is the avatar now: Revert has nothing to undo.
#[test]
fn a_bot_tab_pick_ends_revert() {
    if !in_child("a_bot_tab_pick_ends_revert") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
    app.press("add a design");
    app.press("Own tab");
    app.wait_for("Use your Mine design");
    app.press("Use your Mine design");
    app.wait_for("Revert avatar");
    app.press("Bot tab");
    app.wait_for("star character shape");
    app.press("star character shape");
    until(&mut app, "the pick on the Bot", |p| p.get("shape").is_some_and(|s| s == "star"));
    app.press("Own tab");
    app.wait_for("Use your Mine design");
    app.pump(Duration::from_millis(100));
    assert!(!app.has("Revert avatar"), "Revert would undo the Bot tab's pick");
}

/// An entry this build can't draw isn't shown, and comes through a save whole.
#[test]
fn a_design_it_cant_draw_survives_a_save() {
    if !in_child("a_design_it_cant_draw_survives_a_save") {
        return;
    }
    let future = json!({"name": "Future", "shape": "hologram", "color": "#FFFFFF", "beam": 3});
    let known = json!({"name": "Old", "shape": "cloud", "color": "#FFFFFF", "extra": "kept"});
    let broker = stub(saved(json!([known, future])), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
    until(&mut app, "one shown", |p| p.get("shown").is_some_and(|n| n == "1"));
    app.press("keep heart");
    pump_until(&mut app, "the save", |_| broker.names() == ["Heart", "Old", "Future"]);
    let list = broker.posts().pop().unwrap();
    assert_eq!(list["designs"][2], future, "the hidden entry as it was");
    assert_eq!(list["designs"][1], known, "a known entry keeps a field this build doesn't use");
    app.press("forget first");
    pump_until(&mut app, "the second save", |_| broker.names() == ["Old", "Future"]);
}

/// A list that failed to load is no empty list: nothing is saved, Own doesn't say there's
/// nothing, the load tries again, and what was kept meanwhile joins the real list.
#[test]
fn a_failed_load_saves_nothing_and_loses_nothing() {
    if !in_child("a_failed_load_saves_nothing_and_loses_nothing") {
        return;
    }
    let old = json!({"name": "Old", "shape": "cloud", "color": "#FFFFFF"});
    let broker = stub(saved(json!([old])), 2, Duration::ZERO);
    let mut app = start(&broker);
    app.press("Own tab");
    pump_until(&mut app, "the loading line", |app| shows(app, LOADING));
    assert!(!shows(&app, EMPTY), "says there's nothing, with the list unread");
    app.press("keep heart");
    until(&mut app, "the pick shown", |p| p.get("shown").is_some_and(|n| n == "1"));
    assert!(broker.posts().is_empty(), "saved before the list was read");
    until(&mut app, "the list read", |p| p.get("loaded").is_some_and(|l| l == "true"));
    pump_until(&mut app, "the merged save", |_| broker.names() == ["Heart", "Old"]);
    assert!(broker.gets.load(Ordering::SeqCst) >= 3, "the load tried again");
}

/// An answer that isn't a list of designs isn't an empty list either.
#[test]
fn a_strange_answer_is_no_empty_list() {
    if !in_child("a_strange_answer_is_no_empty_list") {
        return;
    }
    let broker = stub(json!({"state": {"designs": "many"}}), 0, Duration::ZERO);
    let mut app = start(&broker);
    pump_until(&mut app, "a second look", |_| broker.gets.load(Ordering::SeqCst) >= 2);
    app.press("keep heart");
    app.pump(Duration::from_millis(300));
    assert!(broker.posts().is_empty(), "saved over a list it couldn't read");
    assert_eq!(read(&app).get("loaded").map(String::as_str), Some("false"));
}

/// However long the list is, a new design adds to it and takes nothing away.
#[test]
fn a_long_list_keeps_all_of_it() {
    if !in_child("a_long_list_keeps_all_of_it") {
        return;
    }
    let list: Vec<Value> = (0..60).map(|i| json!({"name": format!("D{i}"), "shape": "cloud", "color": "#FFFFFF"})).collect();
    let broker = stub(saved(json!(list)), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
    app.press("keep heart");
    pump_until(&mut app, "the save", |_| broker.names().len() == 61);
    assert_eq!(broker.names().last().map(String::as_str), Some("D59"), "the oldest is still there");
}

/// Saves go one at a time and the last holds the newest list, whatever the wire does.
#[test]
fn quick_changes_save_in_order() {
    if !in_child("quick_changes_save_in_order") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::from_millis(150));
    let mut app = start_loaded(&broker);
    app.press("keep heart");
    app.press("keep star");
    app.press("forget first");
    app.press("add a design");
    pump_until(&mut app, "the last list", |_| broker.names() == ["Mine", "Heart"]);
    app.pump(Duration::from_millis(400));
    assert_eq!(broker.names(), ["Mine", "Heart"], "a late save changed it");
    assert_eq!(broker.most_open.load(Ordering::SeqCst), 1, "two saves on the wire at once");
}

/// An answer with no `state` is a list never written, and a first design saves.
#[test]
fn no_state_at_all_is_an_empty_list() {
    if !in_child("no_state_at_all_is_an_empty_list") {
        return;
    }
    let broker = stub(json!({}), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
    app.press("keep heart");
    pump_until(&mut app, "the save", |_| broker.names() == ["Heart"]);
}

/// A save goes on trying for as long as it takes, and the status says when it worked: the
/// refusals here carry `{"ok":true}`, and the one that lands carries no JSON at all.
#[test]
fn a_save_keeps_trying_and_trusts_the_status() {
    if !in_child("a_save_keeps_trying_and_trusts_the_status") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    broker.save_fails.store(7, Ordering::SeqCst);
    let mut app = start_loaded(&broker);
    app.press("keep heart");
    pump_until(&mut app, "the save", |_| broker.names() == ["Heart"]);
    app.pump(Duration::from_millis(500));
    assert_eq!(broker.posts().len(), 1, "saved again after it worked");
}

/// The broker's entry and one kept before it came share a shape: both stay, and keeping that
/// shape again moves it to the front whole.
#[test]
fn entries_with_one_shape_all_stay() {
    if !in_child("entries_with_one_shape_all_stay") {
        return;
    }
    let old = json!({"name": "Old heart", "shape": "heart", "color": "#FFFFFF", "extra": "kept"});
    let broker = stub(saved(json!([old])), 1, Duration::ZERO);
    let mut app = start(&broker);
    app.press("keep heart");
    pump_until(&mut app, "the merged save", |_| broker.names() == ["Heart", "Old heart"]);
    app.press("forget first");
    pump_until(&mut app, "the forget", |_| broker.names() == ["Old heart"]);
    app.press("keep heart");
    app.press("keep star");
    pump_until(&mut app, "the star", |_| broker.names() == ["Star", "Old heart"]);
    app.press("keep heart");
    pump_until(&mut app, "the move", |_| broker.names() == ["Old heart", "Star"]);
    assert_eq!(broker.posts().pop().unwrap()["designs"][0], old, "kept whole");
}

/// There's one read of the list, however often it's asked for.
#[test]
fn the_list_is_read_once() {
    if !in_child("the_list_is_read_once") {
        return;
    }
    let old = json!({"name": "Old", "shape": "cloud", "color": "#FFFFFF"});
    let broker = stub(saved(json!([old])), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
    app.press("keep heart");
    pump_until(&mut app, "the save", |_| broker.names() == ["Heart", "Old"]);
    app.press("read again");
    app.pump(Duration::from_millis(300));
    assert_eq!(broker.gets.load(Ordering::SeqCst), 1, "read again");
    until(&mut app, "the list as it was", |p| p.get("shown").is_some_and(|n| n == "2"));
}

/// Setting a photo is the avatar now: Revert has nothing to undo.
#[test]
fn a_photo_ends_revert() {
    if !in_child("a_photo_ends_revert") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    let mut app = start_loaded(&broker);
    app.press("add a design");
    app.press("Own tab");
    app.wait_for("Use your Mine design");
    app.press("Use your Mine design");
    app.wait_for("Revert avatar");
    app.press("set a photo");
    app.wait_for("Use your Mine design");
    app.pump(Duration::from_millis(100));
    assert!(!app.has("Revert avatar"), "Revert would undo the photo");
}

fn pack_answer(designs: Value, refused: Value) -> (u16, Value) {
    (200, json!({"ok": true, "designs": designs, "refused": refused}))
}

/// An install adds to the front, in the pack's order, each entry whole, with what the list
/// had left as it was, and asks the broker for just what was typed.
#[test]
fn an_install_adds_and_replaces_nothing() {
    if !in_child("an_install_adds_and_replaces_nothing") {
        return;
    }
    let mine = json!({"name": "Mine", "shape": "cloud", "color": "#FFFFFF", "extra": "kept"});
    let broker = stub(saved(json!([mine])), 0, Duration::ZERO);
    let same = json!({"name": "Their cloud", "shape": "cloud", "color": "#000000"});
    let a = json!({"name": "A", "shape": "heart", "color": "#FFFFFF", "why": "kept too"});
    let b = json!({"name": "B", "shape": "star", "color": "#FFFFFF"});
    *broker.packs.lock().unwrap() = pack_answer(json!([a, same, b, a]), json!([]));
    let mut app = start_loaded(&broker);
    app.press("install fox");
    pump_until(&mut app, "the save", |_| broker.names() == ["A", "B", "Mine"]);
    let said = "Added 2 avatars to Your designs, 1 avatar already there, 1 avatar listed twice.";
    pump_until(&mut app, "the outcome", |app| shows(app, said));
    assert_eq!(broker.pack_asks.lock().unwrap()[0], json!({"link": "https://github.com/me/mine", "pick": "fox"}));
    let list = broker.posts().pop().unwrap();
    assert_eq!(list["designs"][0], a, "the pack's entry whole");
    assert_eq!(list["designs"][2], mine, "the list's own entry as it was");
    assert_eq!(broker.posts().len(), 1, "one save for all of it");
}

/// The Bot's own engine has the last word on what it can draw.
#[test]
fn an_install_skips_what_this_build_cant_draw() {
    if !in_child("an_install_skips_what_this_build_cant_draw") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    let good = json!({"name": "Good", "shape": "heart", "color": "#FFFFFF"});
    let odd = json!({"name": "Odd", "shape": "hologram", "color": "#FFFFFF"});
    *broker.packs.lock().unwrap() = pack_answer(json!([odd, good]), json!([]));
    let mut app = start_loaded(&broker);
    app.press("install fox");
    pump_until(&mut app, "the save", |_| broker.names() == ["Good"]);
}

/// Until the list has been read there's no telling what's in it: nothing is asked or added.
#[test]
fn an_install_waits_for_the_list() {
    if !in_child("an_install_waits_for_the_list") {
        return;
    }
    let broker = stub(json!({"state": null}), 1000, Duration::ZERO);
    *broker.packs.lock().unwrap() = pack_answer(json!([{"name": "A", "shape": "heart", "color": "#FFFFFF"}]), json!([]));
    let mut app = start(&broker);
    app.press("install fox");
    pump_until(&mut app, "the answer", |app| shows(app, "Your avatars are still loading. Try again in a moment."));
    assert!(broker.pack_asks.lock().unwrap().is_empty(), "asked with the list unread");
    assert_eq!(read(&app).get("shown").map(String::as_str), Some("0"), "added with the list unread");
}

/// A pack that couldn't be read is said as a pack, and the rest still come.
#[test]
fn a_failed_pack_is_said_as_one() {
    if !in_child("a_failed_pack_is_said_as_one") {
        return;
    }
    let broker = stub(json!({"state": null}), 0, Duration::ZERO);
    let design = json!({"name": "A", "shape": "heart", "color": "#FFFFFF"});
    let mut answer = pack_answer(json!([design]), json!([]));
    answer.1["failed"] = json!([{"name": "Pets", "why": "a pack isn't JSON"}]);
    *broker.packs.lock().unwrap() = answer;
    let mut app = start_loaded(&broker);
    app.press("install fox");
    let said = "Added 1 avatar to Your designs, 1 pack couldn't be read.";
    pump_until(&mut app, "the outcome", |app| shows(app, said));
    pump_until(&mut app, "the save", |_| broker.names() == ["A"]);
}

/// A link the broker refuses changes nothing.
#[test]
fn a_refused_link_changes_nothing() {
    if !in_child("a_refused_link_changes_nothing") {
        return;
    }
    let old = json!({"name": "Old", "shape": "cloud", "color": "#FFFFFF"});
    let broker = stub(saved(json!([old])), 0, Duration::ZERO);
    let design = json!({"name": "A", "shape": "heart", "color": "#FFFFFF"});
    *broker.packs.lock().unwrap() = (400, json!({"ok": false, "error": "no packs at that link", "designs": [design]}));
    let mut app = start_loaded(&broker);
    app.press("install fox");
    pump_until(&mut app, "the refusal", |app| shows(app, "No packs at that link."));
    app.pump(Duration::from_millis(300));
    assert!(broker.posts().is_empty(), "saved after a refusal");
    assert_eq!(broker.names(), ["Old"]);
}
