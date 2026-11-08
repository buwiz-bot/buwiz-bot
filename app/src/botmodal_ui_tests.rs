//! The Create New Bot sheet's Default | Custom tabs on a headless engine: what a person sees
//! and what a press does. Nothing reaches a server: the backend is a port nothing listens on.

use std::sync::{Arc, Mutex};

use buwiz_office_kit::testing::{Headless, read, run, until};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvent, FetchEvents, FetchTask, HttpRequest, Network};
use serde_json::{Value, json};

use crate::live::store;

const CHILD: &str = "BUWIZ_BOTMODAL_UI_TEST";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("botmodal_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// A broker that answers `/state/buwiz-avatar-designs` with `designs`, `{}` to the rest, and
/// keeps every path asked for.
#[derive(Clone)]
struct Stub {
    designs: Value,
    asked: Arc<Mutex<Vec<String>>>,
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
            stub.asked.lock().unwrap().push(format!("{} {}", request.method, request.url));
            let reply = if request.url.ends_with("/state/buwiz-avatar-designs") && request.method == "GET" {
                json!({"state": {"designs": stub.designs}}).to_string()
            } else {
                "{}".to_owned()
            };
            let head = nsh_engine::HttpHead { status: 200, url: request.url, ..Default::default() };
            events(FetchEvent::Head(head));
            events(FetchEvent::Data(reply.into_bytes()));
            events(FetchEvent::End);
        });
        Box::new(Done)
    }
}

impl Stub {
    /// The POSTs, by path.
    fn posts(&self) -> Vec<String> {
        self.asked.lock().unwrap().iter().filter(|a| a.starts_with("POST ")).cloned().collect()
    }
}

#[component]
fn Sheet() -> impl IntoElement {
    let st = store();
    st.set_threads(json!({
        "list": [{"id": "t1", "title": "One", "bot": {"id": "b1", "name": "One"}}],
        "bots": [{"id": "b1", "name": "One", "threadId": "t1", "shape": "blob", "color": "#1884FC"}],
    }));
    st.active_id.set("t1".to_owned());
    st.bot_shape.set("blob".to_owned());
    Owner::detached().run(|| st.load_designs());
    crate::history::track();
    view()
        .size_full()
        .child(text(|| {
            let st = store();
            // spaces would split the probe
            let draft = st.draft.get().replace(' ', "_");
            format!(
                "probe shape={} modal={} tab={:?} draft={draft} loaded={}",
                st.bot_shape.get(),
                st.bot_modal.get(),
                st.bot_sheet_tab.get(),
                st.design_loaded.get()
            )
        }))
        .child(view().label("open the sheet").w(px(40.0)).h(px(40.0)).on_press(|| store().open_bot_modal()))
        .child(view().label("type a message").w(px(40.0)).h(px(40.0)).on_press(|| store().draft.set("half a thought".to_owned())))
        .child(view().label("step back").w(px(40.0)).h(px(40.0)).on_press(|| crate::history::step(-1)))
        .child(crate::botmodal::layer())
}

fn start(designs: Value) -> (Headless, Stub) {
    let broker = Stub { designs, asked: Arc::default() };
    let mut config = EngineConfig::new(App::Rust(Root::new::<Sheet>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1200.0), px(900.0));
    config.network = Some(Arc::new(broker.clone()));
    let mut app = run(config);
    until(&mut app, "the list read", |p| p.get("loaded").is_some_and(|l| l == "true"));
    app.press("open the sheet");
    app.wait_for("Default tab");
    (app, broker)
}

fn mine() -> Value {
    json!([{"name": "Mine", "shape": "cloud", "color": "#FFFFFF"}])
}

/// The sheet opens on Default, with the shapes and colours and no Design panel; it opens on
/// Default again after Custom was left picked.
#[test]
fn the_sheet_opens_on_default() {
    if !in_child("the_sheet_opens_on_default") {
        return;
    }
    let (mut app, _) = start(mine());
    assert_eq!(read(&app).get("tab").map(String::as_str), Some("Default"));
    assert!(app.has("Custom tab"), "the Custom tab");
    assert!(app.has("star character shape"), "the shape grid");
    assert!(app.has("#FFFFFF character color"), "the colour row");
    assert!(!app.has("Design avatars") && !app.has("Mark mode"), "the Design panel");
    assert!(!app.has("Use your Mine design"), "custom avatars on Default");
    app.press("Custom tab");
    app.wait_for("Use your Mine design");
    app.press("Close dialog");
    until(&mut app, "the sheet shut", |p| p.get("modal").is_some_and(|m| m == "false"));
    app.press("open the sheet");
    app.wait_for("star character shape");
    assert_eq!(read(&app).get("tab").map(String::as_str), Some("Default"), "opened on Custom");
}

/// Custom lists the custom avatars, not the shape grid, and a tap puts one on the draft.
#[test]
fn custom_lists_them_and_a_tap_sets_the_draft() {
    if !in_child("custom_lists_them_and_a_tap_sets_the_draft") {
        return;
    }
    let (mut app, _) = start(mine());
    app.press("Custom tab");
    app.wait_for("Use your Mine design");
    assert!(!app.has("star character shape"), "the shape grid on Custom");
    assert!(!app.has("Insert in chat"), "the empty state with avatars there");
    app.press("Use your Mine design");
    until(&mut app, "the draft", |p| p.get("shape").is_some_and(|s| s == "cloud"));
    assert_eq!(read(&app).get("modal").map(String::as_str), Some("true"), "the sheet shut on a pick");
}

/// With no custom avatars, Custom says how to get some.
#[test]
fn custom_with_none_shows_the_empty_state() {
    if !in_child("custom_with_none_shows_the_empty_state") {
        return;
    }
    let (mut app, _) = start(json!([]));
    app.press("Custom tab");
    app.wait_for("Insert in chat");
    let texts = app.texts();
    for line in [
        "No custom avatars yet",
        "Install a pack in chat. They show up here, and in Edit Avatar.",
        crate::design::INSTALL_ALL,
    ] {
        assert!(texts.contains(&line), "{line:?} missing: {texts:?}");
    }
    assert!(app.has("Copy"), "Copy");
}

/// Insert in chat shuts the sheet and puts the command in the message box, unsent.
#[test]
fn insert_in_chat_fills_the_composer_and_sends_nothing() {
    if !in_child("insert_in_chat_fills_the_composer_and_sends_nothing") {
        return;
    }
    let (mut app, broker) = start(json!([]));
    app.press("Custom tab");
    app.wait_for("Insert in chat");
    app.press("Insert in chat");
    until(&mut app, "the sheet shut", |p| p.get("modal").is_some_and(|m| m == "false"));
    assert_eq!(read(&app).get("draft").map(String::as_str), Some("/avatars_install_all"));
    app.pump(std::time::Duration::from_millis(300));
    assert!(broker.posts().is_empty(), "posted: {:?}", broker.posts());
}

/// Copy puts the command on the clipboard (the broker's `/copy`) and leaves the sheet open.
#[test]
fn copy_copies_the_command() {
    if !in_child("copy_copies_the_command") {
        return;
    }
    let (mut app, broker) = start(json!([]));
    app.press("Custom tab");
    app.wait_for("Copy");
    app.press("Copy");
    for _ in 0..100 {
        if broker.posts().iter().any(|p| p.ends_with("/copy")) {
            break;
        }
        app.pump(std::time::Duration::from_millis(50));
    }
    assert!(broker.posts().iter().any(|p| p.ends_with("/copy")), "no copy: {:?}", broker.posts());
    assert_eq!(read(&app).get("modal").map(String::as_str), Some("true"));
    assert!(app.texts().contains(&"Copied"), "Copied");
}

/// Words in the message box are never replaced: Insert in chat isn't offered, Copy is.
#[test]
fn a_written_message_hides_insert_in_chat() {
    if !in_child("a_written_message_hides_insert_in_chat") {
        return;
    }
    let (mut app, _) = start(json!([]));
    app.press("Custom tab");
    app.wait_for("Insert in chat");
    app.press("type a message");
    until(&mut app, "the draft", |p| p.get("draft").is_some_and(|d| d == "half_a_thought"));
    app.pump(std::time::Duration::from_millis(100));
    assert!(!app.has("Insert in chat"), "Insert in chat over a written message");
    assert!(app.has("Copy"), "Copy");
    assert_eq!(read(&app).get("draft").map(String::as_str), Some("half_a_thought"));
}

/// "Copied" goes back to "Copy" after a moment.
#[test]
fn copied_goes_back_to_copy() {
    if !in_child("copied_goes_back_to_copy") {
        return;
    }
    let (mut app, _) = start(json!([]));
    app.press("Custom tab");
    app.wait_for("Copy");
    app.press("Copy");
    app.pump(std::time::Duration::from_millis(200));
    assert!(app.texts().contains(&"Copied"), "Copied");
    for _ in 0..60 {
        if app.texts().contains(&"Copy") {
            break;
        }
        app.pump(std::time::Duration::from_millis(50));
    }
    assert!(app.texts().contains(&"Copy") && !app.texts().contains(&"Copied"), "still Copied");
}

/// A step back to the open sheet, left on Custom, brings it back on Default.
#[test]
fn a_restored_sheet_opens_on_default() {
    if !in_child("a_restored_sheet_opens_on_default") {
        return;
    }
    let (mut app, _) = start(mine());
    app.press("Custom tab");
    app.wait_for("Use your Mine design");
    app.press("Close dialog");
    until(&mut app, "the sheet shut", |p| p.get("modal").is_some_and(|m| m == "false"));
    app.press("step back");
    until(&mut app, "the sheet back", |p| p.get("modal").is_some_and(|m| m == "true"));
    app.wait_for("star character shape");
    assert_eq!(read(&app).get("tab").map(String::as_str), Some("Default"), "restored on Custom");
}
