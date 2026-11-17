//! The composer's `/` popup through the real composer path on a headless engine: a command
//! typed mid-name (`/uptime` for a plugin's `bir-uptime`) is offered after the commands that
//! start with it, and picking it completes the whole command.
//!
//! The store is one per process, so each test runs in a child of its own.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::sync::Mutex;

use nsh_core::element::Root;
use nsh_core::platform::{FetchEvent, HttpHead, HttpRequest};
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvents, FetchTask, Network};
use serde_json::{Value, json};

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;

const CHILD: &str = "BUWIZ_SLASH_MENU_UI_TEST";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("slash_menu_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

/// A stand-in broker: the servers' manifest lists none; everything else is `ok`.
struct Broker;

impl Network for Broker {
    fn fetch(&self, request: HttpRequest, events: FetchEvents) -> Box<dyn FetchTask> {
        let path = request.url.strip_prefix(crate::client::backend()).unwrap_or(&request.url).to_owned();
        let answer = match path.as_str() {
            "/mcp/manifest" => json!({"servers": []}),
            _ => json!({"ok": true}),
        };
        finish(events, answer)
    }
}

fn finish(mut events: FetchEvents, answer: Value) -> Box<dyn FetchTask> {
    events(FetchEvent::Head(HttpHead { status: 200, ..Default::default() }));
    events(FetchEvent::Data(answer.to_string().into_bytes()));
    events(FetchEvent::End);
    Box::new(Done)
}

/// How long a test waits for what it expects before it fails: generous, for a loaded runner.
const PATIENCE: Duration = Duration::from_secs(60);

/// `app` pumped until `done()` (run on the engine thread) says so.
fn wait_until(app: &mut Headless, what: &str, done: impl Fn() -> bool + Send + Clone + 'static) {
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if on(app, done.clone()) {
            return;
        }
        app.pump(Duration::from_millis(10));
    }
    panic!("waited for {what}");
}

fn start() -> (Headless, Arc<Broker>) {
    let seen = Arc::new(Broker);
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    config.network = Some(seen.clone());
    let mut app = run(config);
    on(&mut app, crate::slash::boot);
    wait_until(&mut app, "the manifest", || store().mcp_loaded.get());
    (app, seen)
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
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if let Ok(r) = rx.try_recv() {
            return r;
        }
        app.pump(Duration::from_millis(5));
    }
    panic!("the job never ran");
}

#[component]
fn Stage() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view().size_full().child(text("probe ready=1")).child(crate::toast::layer()).child(view().label("job").w(px(40.0)).h(px(24.0)).on_press(|| {
        if let Some(job) = JOB.lock().unwrap().take() {
            job();
        }
    }))
}

/// A plugin whose commands are `bir-uptime` and `home-menu`.
fn install_plugin(app: &mut Headless) {
    on(app, || {
        let p = crate::mini_v1::load::PluginEntry {
            id: "app.buwiz.ebir".to_owned(),
            short: "ebir".to_owned(),
            title: "eBIR".to_owned(),
            enabled: true,
            commands: vec![json!({"name": "bir-uptime", "description": "Is the BIR up"}), json!({"name": "home-menu"})],
            ..Default::default()
        };
        store().v1.entries.set(vec![p]);
    });
}

/// The popup's rows once `typed` is in the box.
fn menu(app: &mut Headless, typed: &'static str) -> Vec<String> {
    on(app, move || crate::composer::draft_changed(typed));
    on(app, || store().pop_rows.get_untracked().into_iter().map(|r| r.label).collect())
}

/// `/uptime` offers the plugin's `bir-uptime`; `/bir` offers it as a start match; one character
/// offers what it always did; start matches come before mid-name ones; the built-ins stay first
/// among start matches.
#[test]
fn the_slash_popup_offers_a_command_mid_name_after_the_ones_that_start_with_it() {
    if !in_child("the_slash_popup_offers_a_command_mid_name_after_the_ones_that_start_with_it") {
        return;
    }
    let (mut app, _) = start();
    install_plugin(&mut app);
    assert_eq!(menu(&mut app, "/uptime"), ["/bir-uptime"]);
    assert_eq!(menu(&mut app, "/UPTIME"), ["/bir-uptime"], "case is ignored");
    assert_eq!(menu(&mut app, "/bir"), ["/bir-uptime"], "a start match");
    assert_eq!(menu(&mut app, "/me"), ["/messages", "/bir-uptime", "/home-menu"], "the start match first, the rest in their order");
    assert!(menu(&mut app, "/u").is_empty(), "one character: starts only, as before");
    assert_eq!(menu(&mut app, "/s"), ["/status"]);
    assert!(menu(&mut app, "/zz").is_empty());
    assert_eq!(menu(&mut app, "/").len(), 9, "nothing typed: every command");
}

/// Picking the mid-name row completes the whole command; sending `/uptime` typed by hand is no
/// command here, as before.
#[test]
fn picking_the_mid_name_row_completes_the_full_command_and_a_partial_name_still_is_no_command() {
    if !in_child("picking_the_mid_name_row_completes_the_full_command_and_a_partial_name_still_is_no_command") {
        return;
    }
    let (mut app, _) = start();
    install_plugin(&mut app);
    assert_eq!(menu(&mut app, "/uptime"), ["/bir-uptime"]);
    let draft = on(&mut app, || {
        crate::slash::enter();
        store().draft.get_untracked()
    });
    assert_eq!(draft, "/bir-uptime ");
    // typed in full and sent by hand, `/uptime` is not taken as a command: left to the chat as before
    assert!(!on(&mut app, || crate::slash::dispatch("/uptime", true)));
    assert!(on(&mut app, || crate::slash::names_a_builtin("/status").is_some()));
}
