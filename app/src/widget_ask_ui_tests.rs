//! The widget permission card (`widget_ask.rs`) on a headless engine,
//! against a stand-in broker that records what reaches it.
//!
//! The store is one per process, so each test runs in a child of its own.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::element::Root;
use nsh_core::platform::{FetchEvent, HttpBody, HttpHead, HttpRequest};
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvents, FetchTask, Network};
use serde_json::{Value, json};

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;

const CHILD: &str = "BUWIZ_WIDGET_ASK_UI_TEST";
const GRANT: &str = "/v1/host/widget/grant";
const TITLE: &str = "Time & Task Dashboard wants to use Reminders";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("widget_ask_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// A `widget_ask` notice turn, as the broker's JSON (the engine thread builds the Turn).
fn turn(extra: Value) -> Value {
    let mut n = json!({
        "kind": "widget_ask", "id": "w_1", "chat": "t_plain", "bot": "", "botName": "Plain chats",
        "widget": "R37", "widgetTitle": "Time & Task Dashboard",
        "source": "reminders", "sourceTitle": "Reminders",
        "reads": ["Open reminders, 50 at most"],
        "writes": ["Add a reminder", "Complete a reminder"],
        "state": "pending", "at": 1_763_300_000_000i64,
    });
    for (k, v) in extra.as_object().unwrap() {
        n[k] = v.clone();
    }
    json!({"role": "notice", "text": "x", "notice": n})
}

/// A stand-in broker: every request's path and JSON body; the grant route
/// answers `grant`.
struct Broker {
    log: Mutex<Vec<(String, Value)>>,
    grant: Value,
    /// `GET /v1/host/widget/grants`'s answer; null: the route is not there.
    grants: Value,
}

struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

impl Network for Broker {
    fn fetch(&self, request: HttpRequest, mut events: FetchEvents) -> Box<dyn FetchTask> {
        let path = request.url.strip_prefix(crate::client::backend()).unwrap_or(&request.url).to_owned();
        let body = match &request.body {
            HttpBody::Bytes(b) => serde_json::from_slice(b).unwrap_or(Value::Null),
            _ => Value::Null,
        };
        self.log.lock().unwrap().push((path.clone(), body));
        let grants = path.starts_with("/v1/host/widget/grants");
        let answer = if path == GRANT {
            self.grant.clone()
        } else if grants && !self.grants.is_null() {
            self.grants.clone()
        } else if grants {
            json!({"ok": false, "error": "no route"})
        } else {
            json!({"ok": true})
        };
        let status = if grants && self.grants.is_null() { 404 } else { 200 };
        events(FetchEvent::Head(HttpHead { status, ..Default::default() }));
        events(FetchEvent::Data(answer.to_string().into_bytes()));
        events(FetchEvent::End);
        Box::new(Done)
    }
}

fn start(grant: Value) -> (Headless, Arc<Broker>) {
    start_with(grant, Value::Null)
}

fn start_with(grant: Value, grants: Value) -> (Headless, Arc<Broker>) {
    let seen = Arc::new(Broker { log: Mutex::new(Vec::new()), grant, grants });
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    config.network = Some(seen.clone());
    let mut app = run(config);
    settle(&mut app);
    (app, seen)
}

fn heard(seen: &Broker, path: &str) -> Vec<Value> {
    seen.log.lock().unwrap().iter().filter(|(p, _)| p == path).map(|(_, b)| b.clone()).collect()
}

/// `app` run until `path` has been heard `n` times.
fn wait_for(app: &mut Headless, seen: &Broker, path: &str, n: usize) -> Vec<Value> {
    for _ in 0..200 {
        app.pump(Duration::from_millis(20));
        let got = heard(seen, path);
        if got.len() >= n {
            return got;
        }
    }
    panic!("{path} not heard {n} times: {:?}", seen.log.lock().unwrap());
}

fn settle(app: &mut Headless) {
    for _ in 0..25 {
        app.pump(Duration::from_millis(20));
    }
}

/// What the Stage's `job` press runs next.
static JOB: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// `f` on the engine thread, where the store's signals live.
fn on<R: Send + 'static>(app: &mut Headless, f: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = std::sync::mpsc::channel();
    *JOB.lock().unwrap() = Some(Box::new(move || {
        let _ = tx.send(f());
    }));
    app.press("job");
    for _ in 0..500 {
        if let Ok(r) = rx.try_recv() {
            return r;
        }
        app.pump(Duration::from_millis(5));
    }
    panic!("the job never ran");
}

fn show(app: &mut Headless, turn: Value) {
    on(app, move || store().turns.set(vec![crate::store::Turn::of(&turn)]));
    settle(app);
}

#[component]
fn Stage() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view()
        .size_full()
        .child(text("probe ready=1"))
        .child(view().label("job").w(px(40.0)).h(px(24.0)).on_press(|| {
            if let Some(job) = JOB.lock().unwrap().take() {
                job();
            }
        }))
        .child(crate::appdetails::layer())
        .child(dynamic(|| {
            let turns: Vec<_> = store().turns.get().into_iter().filter(|t| t.widget_ask.is_some()).collect();
            view().children(turns.into_iter().map(|t| crate::chat::turn_row(t, "t_plain").into_element())).into_element()
        }))
}

/// A pending card: its words, three buttons; pressing `label` sends
/// `answer` and leaves `said` where the buttons were, with the card the
/// same height.
fn answering(name: &str, label: &str, answer: &str, said: &str) {
    if !in_child(name) {
        return;
    }
    let (mut app, seen) = start(json!({"ok": true}));
    show(&mut app, turn(json!({})));
    assert!(app.has(TITLE), "the title");
    assert!(app.texts().contains(&"Open reminders, 50 at most"));
    assert!(app.texts().contains(&"It can also change things:"));
    assert!(app.texts().contains(&"Add a reminder"));
    for button in ["Not now", "Read only", "Allow"] {
        assert!(app.has(button), "{button}");
    }
    let before = app.frame(TITLE).size.height;
    app.press(label);
    let sent = wait_for(&mut app, &seen, GRANT, 1);
    assert_eq!(sent, [json!({"id": "w_1", "answer": answer})]);
    settle(&mut app);
    assert!(app.texts().contains(&said), "{said}: {:?}", app.texts());
    for button in ["Not now", "Read only", "Allow"] {
        assert!(!app.has(button), "{button} is gone");
    }
    assert_eq!(app.frame(TITLE).size.height, before, "the card does not move");
}

#[test]
fn allow_sends_write_and_shows_allowed() {
    answering("allow_sends_write_and_shows_allowed", "Allow", "write", "Allowed");
}

#[test]
fn read_only_sends_read_and_shows_read_only() {
    answering("read_only_sends_read_and_shows_read_only", "Read only", "read", "Read only");
}

#[test]
fn not_now_sends_no_and_shows_not_allowed() {
    answering("not_now_sends_no_and_shows_not_allowed", "Not now", "no", "Not allowed");
}

#[test]
fn an_answered_notice_has_its_state_and_no_buttons() {
    if !in_child("an_answered_notice_has_its_state_and_no_buttons") {
        return;
    }
    let (mut app, seen) = start(json!({"ok": true}));
    for (state, said) in [("write", "Allowed"), ("read", "Read only"), ("no", "Not allowed")] {
        show(&mut app, turn(json!({"state": state})));
        assert!(app.texts().contains(&said), "{state}: {:?}", app.texts());
        assert!(app.has(TITLE));
        for button in ["Not now", "Read only", "Allow"] {
            assert!(!app.has(button), "{state}: {button}");
        }
    }
    assert!(heard(&seen, GRANT).is_empty(), "drawing an answered card asks nothing");
}

#[test]
fn a_source_with_no_writes_offers_no_read_only() {
    if !in_child("a_source_with_no_writes_offers_no_read_only") {
        return;
    }
    let (mut app, seen) = start(json!({"ok": true}));
    show(&mut app, turn(json!({"writes": []})));
    assert!(app.has("Not now") && app.has("Allow"));
    assert!(!app.has("Read only"));
    assert!(!app.texts().contains(&"It can also change things:"));
    app.press("Allow");
    assert_eq!(wait_for(&mut app, &seen, GRANT, 1), [json!({"id": "w_1", "answer": "write"})]);
    settle(&mut app);
    assert!(app.texts().contains(&"Allowed"));
}

#[test]
fn a_refused_answer_brings_the_buttons_back_with_the_reason() {
    if !in_child("a_refused_answer_brings_the_buttons_back_with_the_reason") {
        return;
    }
    let (mut app, seen) = start(json!({"ok": false, "error": "That card was already answered."}));
    show(&mut app, turn(json!({})));
    app.press("Allow");
    wait_for(&mut app, &seen, GRANT, 1);
    settle(&mut app);
    assert!(app.texts().contains(&"That card was already answered."), "{:?}", app.texts());
    assert!(app.has("Allow") && app.has("Not now"), "it can be answered again");
}

/// An answer mounts the widget's card again (its source is fetched once
/// more), so the widget asks for its data afresh; another widget's card
/// stays as it is.
#[test]
fn an_answer_mounts_the_widgets_card_again() {
    if !in_child("an_answer_mounts_the_widgets_card_again") {
        return;
    }
    let (mut app, seen) = start(json!({"ok": true}));
    on(&mut app, || {
        let _ = store().mini_source("R37@v2");
        let _ = store().mini_source("R38");
    });
    wait_for(&mut app, &seen, "/module?name=R37%40v2", 1);
    wait_for(&mut app, &seen, "/module?name=R38", 1);
    show(&mut app, turn(json!({})));
    app.press("Allow");
    wait_for(&mut app, &seen, "/module?name=R37%40v2", 2);
    settle(&mut app);
    assert_eq!(heard(&seen, "/module?name=R38").len(), 1, "R38 was not remounted");
    assert_eq!(heard(&seen, "/module?name=R37%40v2").len(), 2);
}

/// A card answered somewhere else (a reload brings its new state) mounts
/// the widget again too, once.
#[test]
fn a_card_answered_elsewhere_mounts_the_widget_again_once() {
    if !in_child("a_card_answered_elsewhere_mounts_the_widget_again_once") {
        return;
    }
    let (mut app, seen) = start(json!({"ok": true}));
    on(&mut app, || {
        let _ = store().mini_source("R37");
    });
    wait_for(&mut app, &seen, "/module?name=R37", 1);
    show(&mut app, turn(json!({})));
    settle(&mut app);
    assert_eq!(heard(&seen, "/module?name=R37").len(), 1, "a pending card remounts nothing");
    show(&mut app, turn(json!({"state": "read"})));
    wait_for(&mut app, &seen, "/module?name=R37", 2);
    show(&mut app, turn(json!({"state": "read"})));
    settle(&mut app);
    assert_eq!(heard(&seen, "/module?name=R37").len(), 2, "once");
}

fn details_of_widget(app: &mut Headless) {
    on(app, || {
        store().app_details.set(Some(crate::store::AppInfo {
            name: "R37".to_owned(),
            kind: "app".to_owned(),
            title: "Time & Task Dashboard".to_owned(),
            ..Default::default()
        }));
    });
    settle(app);
}

/// App Details of a widget lists the sources it may use, each with Remove;
/// Remove sends the widget and source and the row goes. A `no` is no grant.
#[test]
fn app_details_lists_what_a_widget_may_use_and_removes_it() {
    if !in_child("app_details_lists_what_a_widget_may_use_and_removes_it") {
        return;
    }
    let grants = json!({"ok": true, "grants": [
        {"source": "reminders", "sourceTitle": "Reminders", "mode": "write"},
        {"source": "timeclock", "mode": "read"},
        {"source": "linear", "mode": "no"},
    ]});
    let (mut app, seen) = start_with(json!({"ok": true}), grants);
    details_of_widget(&mut app);
    wait_for(&mut app, &seen, "/v1/host/widget/grants?name=R37", 1);
    settle(&mut app);
    let texts = app.texts();
    assert!(texts.contains(&"Data this widget can use"), "{texts:?}");
    assert!(texts.contains(&"Reminders") && texts.contains(&"timeclock"), "{texts:?}");
    assert!(texts.contains(&"Can read and change") && texts.contains(&"Can read"), "{texts:?}");
    assert!(!texts.contains(&"linear"), "a no is no grant");
    app.press("Remove Reminders");
    assert_eq!(wait_for(&mut app, &seen, "/v1/host/widget/revoke", 1), [json!({"name": "R37", "source": "reminders"})]);
    settle(&mut app);
    settle(&mut app);
    // (the harness keeps old labels, so the row's title is what shows it went)
    assert!(!app.texts().contains(&"Reminders"), "Reminders went: {:?}", app.texts());
    assert!(app.has("Remove timeclock"), "timeclock stayed: {:?}", app.texts());
}

/// An older broker has no grants route: the sheet shows no such section.
#[test]
fn app_details_without_a_grants_route_shows_no_section() {
    if !in_child("app_details_without_a_grants_route_shows_no_section") {
        return;
    }
    let (mut app, seen) = start_with(json!({"ok": true}), Value::Null);
    details_of_widget(&mut app);
    wait_for(&mut app, &seen, "/v1/host/widget/grants?name=R37", 1);
    settle(&mut app);
    assert!(!app.texts().contains(&"Data this widget can use"));
}

/// Not a generated widget: no section, and nothing asked.
#[test]
fn app_details_of_a_server_app_asks_nothing() {
    if !in_child("app_details_of_a_server_app_asks_nothing") {
        return;
    }
    let (mut app, seen) = start_with(json!({"ok": true}), json!({"ok": true, "grants": []}));
    on(&mut app, || {
        store().app_details.set(Some(crate::store::AppInfo { name: "mcp:bir/forms".to_owned(), kind: "app".to_owned(), ..Default::default() }));
    });
    settle(&mut app);
    assert!(!app.texts().contains(&"Data this widget can use"));
    assert!(seen.log.lock().unwrap().iter().all(|(p, _)| !p.starts_with("/v1/host/widget/")));
}
