//! Boot on a headless engine against a broker that is not up yet: the app starts the bundled
//! broker as the window opens, and boot's one-shot loads must wait for it, not come back empty.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buwiz_office_kit::testing::{Headless, read, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvent, FetchEvents, FetchTask, HttpHead, HttpRequest, Network};
use serde_json::json;

use crate::live::store;

const CHILD: &str = "BUWIZ_BOOT_UI_TEST";

/// Runs the test in a child with a fresh store, a dead backend for anything off the engine's
/// network, and a home, data dir and key store of its own.
fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let dir = std::env::temp_dir().join(format!("buwiz-boot-ui-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("keys")).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("boot_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .env("HOME", &dir)
        .env("BUWIZ_DATA_DIR", dir.join("data"))
        .env("BUWIZ_HOST_TOKEN_FILE", dir.join("host-token"))
        .env("BUWIZ_NO_KEYCHAIN", "1")
        .env("BUWIZ_KEYSTORE", format!("file:{}", dir.join("keys").display()))
        .output()
        .expect("run the test's child");
    let _ = std::fs::remove_dir_all(&dir);
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// A broker that refuses connections until `listens`, then answers `/health` 503 (its node
/// half starting) and everything else 502 until `ready`, then serves one chat. Keeps each
/// request's path and whether it got a real answer.
#[derive(Clone)]
struct Late {
    start: Instant,
    listens: Duration,
    ready: Duration,
    heard: Arc<Mutex<Vec<(String, bool)>>>,
}

struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

impl Network for Late {
    fn fetch(&self, request: HttpRequest, mut events: FetchEvents) -> Box<dyn FetchTask> {
        let late = self.clone();
        std::thread::spawn(move || {
            let path = request.url.split_once("://").and_then(|(_, r)| r.find('/').map(|i| r[i..].to_owned())).unwrap_or_default();
            let since = late.start.elapsed();
            if since < late.listens {
                late.heard.lock().unwrap().push((path, false));
                events(FetchEvent::Failed("Connection refused".to_owned()));
                return;
            }
            let up = since >= late.ready;
            let (status, body) = match path.as_str() {
                "/health" if up => (200, json!({"ok": true, "broker": "buwiz-broker"})),
                "/health" => (503, json!({"ok": false, "broker": "buwiz-broker", "node": "starting"})),
                _ if !up => (502, json!({"ok": false, "error": "the broker is starting"})),
                "/threads" => (
                    200,
                    json!({
                        "list": [{"id": "t1", "title": "One", "bot": {"id": "b1", "name": "One"}}],
                        "bots": [{"id": "b1", "name": "One", "threadId": "t1", "shape": "blob", "color": "#1884FC"}],
                    }),
                ),
                "/prefs" => (200, json!({"model": "late-model"})),
                _ => (200, json!({})),
            };
            late.heard.lock().unwrap().push((path, up));
            events(FetchEvent::Head(HttpHead { status, url: request.url, ..HttpHead::default() }));
            events(FetchEvent::Data(body.to_string().into_bytes()));
            events(FetchEvent::End);
        });
        Box::new(Done)
    }
}

#[component]
fn Booting() -> impl IntoElement {
    store().boot();
    view().size_full().child(text(|| format!("probe bots={} model={}", store().bots.get().len(), store().model.get())))
}

fn start(late: &Late) -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Booting>(|| ())));
    config.network = Some(Arc::new(late.clone()));
    run(config)
}

/// The broker the app just launched listens 0.6 s later and is ready 1.2 s in: boot still
/// loads the chats and the prefs, each asked once the broker answers.
#[test]
fn boot_waits_for_a_broker_that_starts_late() {
    if !in_child("boot_waits_for_a_broker_that_starts_late") {
        return;
    }
    let late = Late {
        start: Instant::now(),
        listens: Duration::from_millis(600),
        ready: Duration::from_millis(1200),
        heard: Arc::default(),
    };
    let mut app = start(&late);
    // `/prefs` is the last of boot's chain: once it was asked, the chain has run
    let end = Instant::now() + Duration::from_secs(20);
    while !late.heard.lock().unwrap().iter().any(|(p, _)| p == "/prefs") && Instant::now() < end {
        app.pump(Duration::from_millis(20));
    }
    app.pump(Duration::from_millis(300));
    let heard = late.heard.lock().unwrap().clone();
    let p = read(&app);
    assert_eq!(p.get("bots").map(String::as_str), Some("1"), "no chats loaded; heard {heard:?}");
    assert_eq!(p.get("model").map(String::as_str), Some("late-model"), "no prefs loaded; heard {heard:?}");
    // boot's one-shot loads waited for the broker: none was asked before it was ready
    let once = ["/plugins/catalog", "/apps/meta", "/state/rox-main-bot", "/groups", "/state/rox-reactions", "/state/rox-zoom", "/state/rox-drafts", "/threads", "/models", "/keymap", "/state/rox-mermaid", "/prefs"];
    for path in once {
        assert!(heard.iter().any(|(p, up)| p == path && *up), "{path} never reached the broker: {heard:?}");
        assert!(!heard.iter().any(|(p, up)| p == path && !*up), "{path} was asked before the broker was up: {heard:?}");
    }
}
