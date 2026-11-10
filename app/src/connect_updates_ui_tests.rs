//! Updates on the Connect apps pill and in Connect Apps, on a headless
//! engine (`nsh-testing`): the badge that counts them, the pill that keeps
//! its width under the pointer and says the rest in a tooltip, and the
//! modal's Updates section, whose buttons only ask for a review.
//!
//! The store is one per process, so each test runs in a child of its own,
//! with a data dir of its own. Layout is asserted here; no window draws it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};
use serde_json::{Value, json};

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;
use crate::mini_v1::connect::Listed;

const CHILD: &str = "BUWIZ_CONNECT_UPDATES_UI_TEST";

fn data_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("buwiz-connect-updates-ui-{}-{name}", std::process::id()))
}

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let dir = data_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("connect_updates_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        // nothing reaches a server or the owner's records: an origin nothing
        // listens on (or the test's own stand-in), and a folder of its own
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .env("BUWIZ_DATA_DIR", &dir)
        .output()
        .expect("run the test's child");
    let _ = std::fs::remove_dir_all(&dir);
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// `n` plugins listed, each with a newer version waiting; the first asks
/// for more access.
fn list(n: usize) {
    let listed: Vec<Listed> = (0..n)
        .map(|i| {
            let access: Vec<&str> = if i == 0 { vec!["Google Drive"] } else { Vec::new() };
            Listed::of(&json!({
                "id": format!("app.buwiz.fixture{i}"), "title": format!("Fixture {i}"), "version": "1.4.0", "state": "running", "enabled": true,
                "update": {"version": "1.5.0", "notes": ["Faster"], "access": access},
            }))
        })
        .collect();
    batch(|| {
        store().v1.listed.set(listed);
        store().v1.known.set(true);
    });
}

fn button(label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view().label(label).w(px(40.0)).h_full().on_press(on)
}

#[component]
fn Stage() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    let rail = signal(false);
    view()
        .size_full()
        .child(
            view()
                .flex_row()
                .h(px(24.0))
                .child(button("chat", || store().active_id.set("chat".to_owned())))
                .child(button("none wait", || list(0)))
                .child(button("1 waits", || list(1)))
                .child(button("3 wait", || list(3)))
                .child(button("12 wait", || list(12)))
                .child(button("skip the first", || store().v1_skip_update("app.buwiz.fixture0", "1.5.0")))
                .child(button("later the first", || store().v1_update_later("app.buwiz.fixture0", "1.5.0")))
                .child(button("a category", || store().plugin_cat.set("Productivity".to_owned())))
                .child(button("every category", || store().plugin_cat.set("All".to_owned())))
                .child(button("modal closed", || store().close_plugins()))
                .child(button("modal open", || store().plugins_open.set(true))),
        )
        .child(text(|| format!("probe open={} cat={}", store().plugins_open.get(), store().plugin_cat.get())))
        .child(button("collapse", move || rail.set(true)))
        // the sidebar's width, so the two pills share its row; collapsed, the rail's footer
        .child(dynamic(move || {
            if rail.get() {
                view().label("rail").w(px(60.0)).pt(px(60.0)).child(crate::sidebar::rail_footer()).into_element()
            } else {
                view().label("row").w(px(240.0)).pt(px(60.0)).child(crate::sidebar::footer()).into_element()
            }
        }))
        .child(view().label("modal").w(px(600.0)).child(crate::plugins::updates_section()))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    let mut app = run(config);
    app.press("chat");
    app
}

fn says(app: &Headless, text: &str) -> bool {
    app.texts().iter().any(|t| &**t == text)
}

/// A stand-in broker: every request's first line and body, answered `{"ok":true}`.
fn stub_broker() -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    // the child has one thread here, and nothing has read the variable yet
    unsafe { std::env::set_var("BUWIZ_BACKEND", format!("http://{}", listener.local_addr().unwrap())) };
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let mut s = s;
            let mut raw = Vec::new();
            let mut buf = [0; 4096];
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                let length = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length: ")?.trim().parse::<usize>().ok()).unwrap_or(0);
                if body.len() >= length {
                    log.lock().unwrap().push(format!("{} {body}", head.lines().next().unwrap_or_default()));
                    break;
                }
            }
            let body = json!({"ok": true, "posted": true}).to_string();
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    seen
}

/// The review requests the stand-in broker heard, as `(id, chat)`.
fn reviews(seen: &Arc<Mutex<Vec<String>>>) -> Vec<(String, String)> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|l| l.starts_with("POST /v1/host/plugins/updates/review "))
        .filter_map(|l| serde_json::from_str::<Value>(l.split_once("HTTP/1.1 ")?.1).ok())
        .map(|v| (v["id"].as_str().unwrap_or_default().to_owned(), v["chat"].as_str().unwrap_or_default().to_owned()))
        .collect()
}

fn wait_for(app: &mut Headless, what: &str, done: impl Fn() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        app.pump(Duration::from_millis(25));
    }
    panic!("{what} never happened");
}

/// F1-1, F2-2: the badge counts the updates waiting, 1 to 9 then "9+",
/// and there is none at zero. Skip takes one off; Later doesn't.
#[test]
fn the_badge_counts_updates_to_nine_then_says_nine_plus() {
    if !in_child("the_badge_counts_updates_to_nine_then_says_nine_plus") {
        return;
    }
    let mut app = start();
    app.press("none wait");
    assert!(!app.has("0 updates waiting") && !says(&app, "0"), "no badge at none: {:?}", app.texts());
    app.press("1 waits");
    assert!(app.has("1 update waiting") && says(&app, "1"), "{:?}", app.texts());
    app.press("3 wait");
    assert!(app.has("3 updates waiting") && says(&app, "3"), "{:?}", app.texts());
    app.press("later the first");
    assert!(app.has("3 updates waiting"), "Later hides a card's signal, not the count");
    app.press("skip the first");
    assert!(app.has("2 updates waiting") && says(&app, "2"), "a skipped version is not waiting: {:?}", app.texts());
    app.press("12 wait");
    assert!(app.has("11 updates waiting") && says(&app, "9+"), "{:?}", app.texts());
}

/// F4-1: the pointer on the Connect apps pill moves nothing (the account
/// keeps its name and its width), and after a moment a tooltip says how
/// many updates wait.
#[test]
fn the_pill_keeps_its_width_under_the_pointer_and_a_tooltip_says_the_rest() {
    if !in_child("the_pill_keeps_its_width_under_the_pointer_and_a_tooltip_says_the_rest") {
        return;
    }
    let mut app = start();
    app.press("3 wait");
    let rest = (app.frame("Account"), app.frame("Account name"), app.frame("Connect apps"), app.frame("row"));
    assert!(rest.1.size.width.0 > 40.0, "the name has room: {:?}", rest.1);
    app.hover("Connect apps", true);
    assert!(!app.has("Connect apps tooltip"), "not at once");
    assert_eq!((app.frame("Account"), app.frame("Account name"), app.frame("Connect apps"), app.frame("row")), rest, "hovered, nothing moved");
    app.pump(Duration::from_millis(700));
    assert!(app.has("Connect apps tooltip") && says(&app, "Connect apps · 3 updates"), "{:?}", app.texts());
    assert_eq!((app.frame("Account"), app.frame("Account name"), app.frame("Connect apps"), app.frame("row")), rest, "the tooltip floats: nothing moved");
    app.hover("Connect apps", false);
    assert!(!app.has("Connect apps tooltip"));

    app.press("1 waits");
    app.hover("Connect apps", true);
    app.pump(Duration::from_millis(700));
    assert!(says(&app, "Connect apps · 1 update"), "{:?}", app.texts());
    app.hover("Connect apps", false);
    app.press("none wait");
    app.hover("Connect apps", true);
    app.pump(Duration::from_millis(700));
    assert!(says(&app, "Connect apps"), "{:?}", app.texts());
}

/// F4-1: a press opens Connect Apps on the whole catalog, where the
/// Updates section leads, when an update waits; as it was otherwise.
#[test]
fn a_press_on_the_pill_opens_connect_apps_on_its_updates() {
    if !in_child("a_press_on_the_pill_opens_connect_apps_on_its_updates") {
        return;
    }
    let mut app = start();
    app.press("a category");
    app.press("none wait");
    app.press("Connect apps");
    assert!(says(&app, "probe open=true cat=Productivity"), "nothing waits: opened as it was left: {:?}", app.texts());
    app.press("modal closed");
    app.press("a category");
    app.press("3 wait");
    app.press("Connect apps");
    assert!(says(&app, "probe open=true cat=All"), "{:?}", app.texts());
}

/// F4-1, collapsed: the rail's Connect apps counts what waits as the
/// pill does, and its press lands on the Updates section too.
#[test]
fn the_collapsed_rails_connect_apps_counts_and_opens_the_updates() {
    if !in_child("the_collapsed_rails_connect_apps_counts_and_opens_the_updates") {
        return;
    }
    let mut app = start();
    app.press("collapse");
    app.press("a category");
    app.press("3 wait");
    assert!(app.has("3 updates waiting") && says(&app, "3"), "{:?}", app.texts());
    app.press("Connect apps");
    assert!(says(&app, "probe open=true cat=All"), "{:?}", app.texts());
    app.press("modal closed");
    app.press("none wait");
    assert!(!app.has("0 updates waiting"), "no badge at none: {:?}", app.texts());
}

/// F3-1: "Updates (n)" with a row for each update waiting; Update asks
/// the broker for that plugin's review card in the chat on screen and
/// closes Connect Apps. Nothing is installed.
#[test]
fn the_updates_section_lists_what_waits_and_update_asks_for_its_review() {
    if !in_child("the_updates_section_lists_what_waits_and_update_asks_for_its_review") {
        return;
    }
    let seen = stub_broker();
    let mut app = start();
    app.press("none wait");
    assert!(!app.has("Updates") && !app.has("Update all"), "no section when nothing waits");
    app.press("3 wait");
    assert!(app.has("Updates") && says(&app, "Updates (3)"), "{:?}", app.texts());
    assert!(app.has("Fixture 0 1.4.0 → 1.5.0 update") && says(&app, "1.4.0 → 1.5.0"), "{:?}", app.texts());
    assert!(says(&app, "Needs new access: Google Drive"), "{:?}", app.texts());
    assert_eq!(app.texts().iter().filter(|t| t.starts_with("Needs new access")).count(), 1, "only the one that asks for more");
    app.press("a category");
    assert!(!app.has("Updates"), "not inside a category");
    app.press("every category");
    app.press("modal open");
    assert!(says(&app, "probe open=true cat=All"));

    app.press("Update Fixture 1");
    assert!(says(&app, "probe open=false cat=All"), "Connect Apps closes to show the chat: {:?}", app.texts());
    wait_for(&mut app, "the review request", || !reviews(&seen).is_empty());
    assert_eq!(reviews(&seen), [("app.buwiz.fixture1".to_owned(), "chat".to_owned())]);
    let heard = seen.lock().unwrap().join("\n");
    assert!(!heard.contains("/v1/host/plugins/install"), "Update installs nothing: {heard}");
}

/// F3-1: Update all asks for every waiting plugin's review card, in the
/// order shown, and installs none.
#[test]
fn update_all_asks_for_every_review_in_order_and_installs_none() {
    if !in_child("update_all_asks_for_every_review_in_order_and_installs_none") {
        return;
    }
    let seen = stub_broker();
    let mut app = start();
    app.press("3 wait");
    app.press("skip the first");
    app.press("modal open");
    assert!(says(&app, "Updates (2)"), "{:?}", app.texts());
    app.press("Update all");
    assert!(says(&app, "probe open=false cat=All"), "{:?}", app.texts());
    wait_for(&mut app, "both review requests", || reviews(&seen).len() == 2);
    assert_eq!(reviews(&seen), [("app.buwiz.fixture1".to_owned(), "chat".to_owned()), ("app.buwiz.fixture2".to_owned(), "chat".to_owned())]);
    let heard = seen.lock().unwrap().join("\n");
    assert!(!heard.contains("/v1/host/plugins/install"), "Update all installs nothing: {heard}");
}
