//! A run that started before this app did, on a headless engine and a stand-in broker: it shows,
//! and its Stop reaches the broker.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use buwiz_office_kit::testing::{Headless, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};
use serde_json::{Value, json};

use crate::live::store;

const CHILD: &str = "BUWIZ_RUN_UI_TEST";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("run_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

static HEARD: Mutex<Vec<String>> = Mutex::new(Vec::new());
static STOPPED: AtomicBool = AtomicBool::new(false);

/// A broker with one run under way on `t7`, ended by `POST /job/stop`: a headless engine has no
/// network, so `store.rs` asks this.
pub(crate) fn broker(line: String) -> Option<Value> {
    HEARD.lock().unwrap().push(line.clone());
    if line.contains("/jobs/running") {
        return Some(json!({"jobs": [{"id": 7, "thread": "t7", "bot": "b1", "phase": "thinking", "detail": "Reading the mail"}]}));
    }
    if line.contains("/job/stop") {
        STOPPED.store(true, Ordering::SeqCst);
        return Some(json!({"ok": true}));
    }
    Some(if STOPPED.load(Ordering::SeqCst) {
        json!({"id": 7, "seq": 2, "phase": "done", "detail": "stopped", "done": true, "events": []})
    } else {
        json!({"id": 7, "seq": 1, "phase": "thinking", "detail": "Reading the mail", "done": false, "events": [{"seq": 1, "phase": "thinking", "detail": "Reading the mail"}]})
    })
}

#[component]
fn Row() -> impl IntoElement {
    // first read here: the store's owner is the root's, and it lives as long as the app
    let _ = store();
    view()
        .size_full()
        .child(view().label("relaunch").w(px(40.0)).h(px(20.0)).on_press(|| {
            store().active_id.set("t7".to_owned());
            store().resume_runs();
        }))
        .child(crate::composer::stop_run())
        .child(text(|| {
            let run = store().agent().map(|r| r.label).unwrap_or_else(|| "none".to_owned());
            format!("probe ready=1 run={run} busy={}", store().sending.get().contains("t7"))
        }))
}

fn shows(app: &mut Headless, word: &str) -> bool {
    for _ in 0..200 {
        app.pump(Duration::from_millis(20));
        if app.texts().iter().any(|t| t.contains(word)) {
            return true;
        }
    }
    false
}

#[test]
fn a_relaunched_app_follows_a_run_under_way() {
    if !in_child("a_relaunched_app_follows_a_run_under_way") {
        return;
    }
    assert!(crate::client::is_host_route("/job/stop"), "the app must send the host token with a Stop");
    let mut config = EngineConfig::new(App::Rust(Root::new::<Row>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(300.0));
    let mut app = run(config);
    assert!(shows(&mut app, "run=none busy=false"), "{:?}", app.texts());

    app.press("relaunch");
    assert!(shows(&mut app, "run=Reading the mail busy=true"), "nothing shows the run: {:?} {:?}", app.texts(), HEARD.lock().unwrap());
    assert!(shows(&mut app, "Stop"), "it can't be stopped: {:?}", app.texts());
    assert!(HEARD.lock().unwrap().iter().any(|l| l.contains("/status?id=7")), "it isn't polling: {:?}", HEARD.lock().unwrap());

    app.press("Stop this run");
    assert!(shows(&mut app, "run=none busy=false"), "the row stayed: {:?}", app.texts());
    assert!(HEARD.lock().unwrap().iter().any(|l| l.starts_with("POST /job/stop") && l.ends_with(r#"{"id":7}"#)), "{:?}", HEARD.lock().unwrap());
    assert!(!app.texts().contains(&"Stop"), "the Stop went with the run: {:?}", app.texts());
}
