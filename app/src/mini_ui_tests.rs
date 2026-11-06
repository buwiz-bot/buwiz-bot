//! Cards, pins and pages of mini apps that can't run, on a headless engine
//! (`nsh-testing`): what a chat, the sidebar and the full-screen view show
//! for a plugin uninstalled, disabled or failed.
//!
//! The store is one per process, so each test runs in a child of its own.
//! Layout is asserted here; no real window draws it.

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
use crate::mini_v1::{V1Key, connect::Listed, records::Pin, ui};
use crate::store::{AccountRequiredNotice, AppAccountsNotice, AppSetupNotice, DriveLink, GroupItem, MiniOpen, PluginNav, RoxGroup, Turn};

const CHILD: &str = "BUWIZ_MINI_UI_TEST";
const TAX: &str = "app.buwiz.ebirforms";
const BOOKS: &str = "app.buwiz.books";
/// The inline card's id in [`links`] under turn `turn`.
const CARD: &str = "turn:tax";
/// The titles [`notices`] gives its rows.
const ROWS: [&str; 3] = ["Account Fixture", "Setup Fixture", "Accounts Fixture"];

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("mini_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        // nothing reaches a server: an origin nothing listens on, or the
        // test's own stand-in
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

fn key(plugin: &str, app: &str) -> String {
    V1Key { plugin: plugin.to_owned(), app: app.to_owned(), ticket: None, props: Default::default() }.to_key()
}

/// A saved chat's reply: eBIRForms' inline card and Books' full one.
fn links() -> Vec<DriveLink> {
    let link = |id: &str, plugin: &str, app: &str, title: &str, placement: &str| DriveLink {
        provider: "buwiz-app".to_owned(),
        id: id.to_owned(),
        card: Some(json!({"plugin": plugin, "app": app, "title": title, "placement": placement})),
        ..Default::default()
    };
    vec![link("tax", TAX, "card-people", "Tax profiles", "inline"), link("led", BOOKS, "books", "Ledger", "full")]
}

/// The broker's list: `plugins` as `(id, state, enabled)`.
fn list(plugins: &[(&str, &str, bool)]) {
    let of = |(id, state, enabled): &(&str, &str, bool)| -> Value { json!({"id": id, "title": if *id == TAX { "eBIRForms" } else { "Books" }, "state": state, "enabled": enabled}) };
    store().v1.listed.set(plugins.iter().map(|p| Listed::of(&of(p))).collect());
    store().v1.known.set(true);
}

/// eBIRForms failed, the broker's list saying `why`.
fn failed_with(why: Value) {
    let mut v = json!({"id": TAX, "title": "eBIRForms", "state": "failed", "enabled": true});
    v.as_object_mut().unwrap().extend(why.as_object().unwrap().clone());
    store().v1.listed.set(vec![Listed::of(&v)]);
    store().v1.known.set(true);
    store().mini_err(&key(TAX, "card-people")).set(String::new());
}

/// A saved chat's account and setup rows about eBIRForms.
fn notices() -> Vec<Turn> {
    let plugin = TAX.to_owned();
    vec![
        Turn {
            account_required: Some(AccountRequiredNotice { plugin: plugin.clone(), title: ROWS[0].into(), ..Default::default() }),
            ..Default::default()
        },
        Turn { app_setup: Some(AppSetupNotice { plugin: plugin.clone(), title: ROWS[1].into(), ..Default::default() }), ..Default::default() },
        Turn { app_accounts: Some(AppAccountsNotice { plugin, title: ROWS[2].into(), ..Default::default() }), ..Default::default() },
    ]
}

fn button(label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view().label(label).w(px(30.0)).h_full().on_press(on)
}

fn seed() {
    let st = store();
    st.active_id.set("chat".to_owned());
    st.v1.shown.update(|s| {
        s.insert(CARD.to_owned());
    });
    st.v1.records.update(|r| {
        r.pin(Pin { plugin: TAX.into(), app: "card-people".into(), placement: "inline".into(), title: "Pinned profiles".into(), ..Default::default() });
    });
    st.groups.set(vec![RoxGroup {
        id: "g".to_owned(),
        items: vec![GroupItem { kind: "app".to_owned(), id: key(BOOKS, "books"), title: "Grouped Books".to_owned(), ..Default::default() }],
        ..Default::default()
    }]);
}

#[component]
fn Stage() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view()
        .size_full()
        .child(
            view()
                .flex_row()
                .h(px(24.0))
                .child(button("seed", seed))
                .child(button("running", || list(&[(TAX, "running", true), (BOOKS, "running", true)])))
                .child(button("disabled", || list(&[(TAX, "stopped", false), (BOOKS, "stopped", false)])))
                .child(button("uninstalled", || list(&[])))
                .child(button("failed", || list(&[(TAX, "failed", true), (BOOKS, "failed", true)])))
                .child(button("key changed", || failed_with(json!({"code": "plugin_key_changed", "message": "Another key is held."}))))
                .child(button("code only", || failed_with(json!({"code": "plugin_key_invalid"}))))
                .child(button("no reason", || failed_with(json!({}))))
                .child(button("open err", || store().mini_err(&key(TAX, "card-people")).set("Could not open Tax profiles: eBIRForms failed its signature check.".to_owned())))
                .child(button("pin press", || store().open_v1_pin(&key(TAX, "card-people"), "Tax profiles")))
                .child(button("full page", || {
                    let key = key(BOOKS, "books");
                    store().mini_full.set(vec![MiniOpen { key: key.clone(), title: "Books".to_owned(), n: 1, token: String::new() }]);
                    store().mini_err(&key).set("Books is not running.".to_owned());
                }))
                .child(button("refreshed", || store().v1_close_gone())),
        )
        .child(text(|| {
            format!(
                "probe ready=1 rows={} full={} pill={}",
                store().group_rows("g").len(),
                store().mini_full.with(Vec::len),
                crate::sidebar::connect_apps().join("+")
            )
        }))
        .child(
            view()
                .label("stage")
                .flex_1()
                .child(ui::answer_cards("turn", links(), || 0))
                .children(notices().into_iter().map(|t| crate::chat::turn_row(t, "chat").into_element()))
                .child(ui::pins_section())
                .child(crate::mini::full_layer()),
        )
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    let mut app = run(config);
    app.press("seed");
    app
}

/// The Connect Apps page of failed eBIRForms.
#[component]
fn Page() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view()
        .size_full()
        .child(view().h(px(24.0)).child(button("seed page", || {
            store().plugin_nav.set(vec![PluginNav { page: "v1".to_owned(), name: TAX.to_owned(), id: String::new() }]);
            failed_with(json!({"code": "plugin_key_changed", "message": "Another key is held."}));
        })))
        .child(text("probe ready=1"))
        .child(view().flex_1().child(crate::plugins::v1_plugin_page()))
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
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                let length = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length: ")?.trim().parse::<usize>().ok()).unwrap_or(0);
                if body.len() >= length {
                    log.lock().unwrap().push(format!("{} {body}", head.lines().next().unwrap_or_default()));
                    break;
                }
            }
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}\n");
        }
    });
    seen
}

/// What the broker heard of enabling eBIRForms, after `app` pressed Retry.
fn heard_enable(app: &mut Headless, seen: &Arc<Mutex<Vec<String>>>) -> String {
    for _ in 0..200 {
        app.pump(Duration::from_millis(20));
        if let Some(r) = seen.lock().unwrap().iter().find(|r| r.contains(&format!("{TAX}/enable"))) {
            return r.clone();
        }
    }
    panic!("no enable reached the broker: {:?}", seen.lock().unwrap());
}

/// The card's Retry, pressed: it asks the broker to take the stored key.
#[test]
fn the_cards_retry_asks_for_the_stored_key() {
    if !in_child("the_cards_retry_asks_for_the_stored_key") {
        return;
    }
    let seen = stub_broker();
    let mut app = start();
    app.press("key changed");
    app.press("Retry");
    let heard = heard_enable(&mut app, &seen);
    assert!(heard.contains(r#"{"adopt_key":true}"#), "{heard}");
}

/// So does Connect Apps' Retry on a failed plugin's page.
#[test]
fn the_plugin_pages_retry_asks_for_the_stored_key() {
    if !in_child("the_plugin_pages_retry_asks_for_the_stored_key") {
        return;
    }
    let seen = stub_broker();
    let mut config = EngineConfig::new(App::Rust(Root::new::<Page>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    let mut app = run(config);
    app.press("seed page");
    app.pump(Duration::from_millis(50));
    assert!(says(&app, "Another key is held."), "{:?}", app.texts());
    app.press("Retry");
    let heard = heard_enable(&mut app, &seen);
    assert!(heard.contains(r#"{"adopt_key":true}"#), "{heard}");
}

fn says(app: &Headless, text: &str) -> bool {
    app.texts().iter().any(|t| t.contains(text))
}

/// An app that is gone leaves nothing behind in a chat that already holds
/// its card, in the pins or in the groups, and comes back with its plugin.
#[test]
fn nothing_shows_for_an_uninstalled_or_disabled_app() {
    if !in_child("nothing_shows_for_an_uninstalled_or_disabled_app") {
        return;
    }
    let mut app = start();
    // no list yet (a broker that is down): nothing is hidden on a guess
    assert!(says(&app, "Tax profiles") && says(&app, "Ledger") && says(&app, "Pinned profiles"));
    assert!(says(&app, "rows=1"), "{:?}", app.texts());
    for gone in ["uninstalled", "disabled"] {
        app.press("running");
        assert!(says(&app, "Tax profiles") && says(&app, "Ledger") && says(&app, "Pinned profiles"), "back after {gone}");
        assert!(says(&app, "rows=1"), "{:?}", app.texts());
        app.press(gone);
        for title in ["Tax profiles", "Ledger", "Pinned profiles"] {
            assert!(!says(&app, title), "{gone}: {title} shows: {:?}", app.texts());
        }
        assert!(says(&app, "rows=0"), "{gone}: {:?}", app.texts());
    }
}

/// A chat's account and setup rows go with their plugin too.
#[test]
fn account_and_setup_rows_go_with_their_app() {
    if !in_child("account_and_setup_rows_go_with_their_app") {
        return;
    }
    let mut app = start();
    app.press("running");
    for row in ROWS {
        assert!(says(&app, row), "{row}: {:?}", app.texts());
    }
    for gone in ["disabled", "uninstalled"] {
        app.press(gone);
        for row in ROWS {
            assert!(!says(&app, row), "{gone}: {row}: {:?}", app.texts());
        }
        app.press("running");
        assert!(says(&app, ROWS[1]), "back after {gone}");
    }
}

/// The Connect apps pill names no disabled app.
#[test]
fn the_connect_pill_names_no_disabled_app() {
    if !in_child("the_connect_pill_names_no_disabled_app") {
        return;
    }
    let mut app = start();
    app.press("running");
    assert!(says(&app, "pill=eBIRForms+Books"), "{:?}", app.texts());
    app.press("disabled");
    assert!(!says(&app, "eBIRForms+") && !says(&app, "pill=Books"), "{:?}", app.texts());
}

/// A failed app's card says what the broker lists: the message, else the
/// code, so a person can see why before pressing Retry.
#[test]
fn a_failed_card_shows_the_listed_reason() {
    if !in_child("a_failed_card_shows_the_listed_reason") {
        return;
    }
    let mut app = start();
    app.press("key changed");
    assert!(says(&app, "Another key is held."), "{:?}", app.texts());
    app.press("code only");
    assert!(says(&app, "plugin_key_invalid"), "{:?}", app.texts());
    app.press("no reason");
    assert!(says(&app, "is not running."), "nothing listed: {:?}", app.texts());
}

/// A pin or a sidebar item of a disabled app opens nothing.
#[test]
fn opening_a_gone_app_opens_nothing() {
    if !in_child("opening_a_gone_app_opens_nothing") {
        return;
    }
    let mut app = start();
    app.press("disabled");
    app.press("pin press");
    assert!(says(&app, "full=0"), "{:?}", app.texts());
    app.press("running");
    app.press("pin press");
    assert!(says(&app, "full=1"), "it opens once the plugin is back: {:?}", app.texts());
}

/// A page left open closes when its plugin goes behind it.
#[test]
fn a_page_of_a_gone_app_closes() {
    if !in_child("a_page_of_a_gone_app_closes") {
        return;
    }
    let mut app = start();
    app.press("running");
    app.press("full page");
    app.press("refreshed");
    assert!(says(&app, "full=1"), "{:?}", app.texts());
    app.press("uninstalled");
    app.press("refreshed");
    assert!(says(&app, "full=0"), "{:?}", app.texts());
}

/// A failed app may recover: its card and pin stay, and the card says so
/// and offers Retry, not an empty box.
#[test]
fn a_failed_apps_card_says_so() {
    if !in_child("a_failed_apps_card_says_so") {
        return;
    }
    let mut app = start();
    app.press("failed");
    assert!(says(&app, "Tax profiles") && says(&app, "Pinned profiles") && says(&app, "rows=1"), "{:?}", app.texts());
    assert!(app.has("Card can't open"), "{:?}", app.texts());
    assert!(says(&app, "Retry"), "{:?}", app.texts());
    assert!(app.frame("Card can't open").size.height.0 < 120.0, "a line, not the app's height");
}

/// An open that failed on a plugin the list still calls running says why,
/// in the card: the error used to reach only the full-screen view.
#[test]
fn a_failed_open_shows_its_reason_in_the_card() {
    if !in_child("a_failed_open_shows_its_reason_in_the_card") {
        return;
    }
    let mut app = start();
    app.press("running");
    app.press("open err");
    assert!(says(&app, "failed its signature check"), "{:?}", app.texts());
    assert!(says(&app, "Retry"), "{:?}", app.texts());
}

/// The full-screen error fills everything under the title bar, over the
/// app: its bottom half was the app's own darker view.
#[test]
fn the_full_screen_error_fills_the_page() {
    if !in_child("the_full_screen_error_fills_the_page") {
        return;
    }
    let mut app = start();
    app.press("running");
    app.press("full page");
    assert!(says(&app, "is not running"), "{:?}", app.texts());
    assert!(says(&app, "Retry"), "{:?}", app.texts());
    let (page, error) = (app.frame("stage"), app.frame("Mini app error"));
    assert_eq!(error.size.height.0, page.size.height.0 - 52.0, "all of it under the 52 pt title bar");
    assert_eq!(error.size.width.0, page.size.width.0);
    assert_eq!(app.style("Mini app error").background_color, nsh_core::Color::from(crate::theme::tc("#161616").light), "the window's colour");
}
