//! A plugin card's update signal on a headless engine (`nsh-testing`): the
//! (i) and its dot, the panel that floats over the card, Later and Skip,
//! Review, the line while an install runs and "Made with 1.4.0 · Refresh".
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
use crate::mini_v1::{V1Key, connect::Listed, load::PluginEntry, records::Records, ui};
use crate::store::DriveLink;

const CHILD: &str = "BUWIZ_UPDATE_UI_TEST";
const GWS: &str = "app.buwiz.google-workspace";
/// The (i)'s name while 1.5.0 waits.
const SIGNAL: &str = "Google Workspace 1.5.0 update available";

fn data_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("buwiz-update-ui-{}-{name}", std::process::id()))
}

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let dir = data_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("update_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
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

fn key() -> String {
    V1Key { plugin: GWS.to_owned(), app: "mail".to_owned(), ticket: None, props: Default::default() }.to_key()
}

/// A saved chat's reply with Google Workspace's Mail card.
fn links() -> Vec<DriveLink> {
    vec![DriveLink {
        provider: "buwiz-app".to_owned(),
        id: "mail".to_owned(),
        card: Some(json!({"plugin": GWS, "app": "mail", "title": "Mail", "placement": "inline"})),
        ..Default::default()
    }]
}

/// The broker's list: Google Workspace at `installed`, with `update`.
fn list(installed: &str, update: Value) {
    let v = json!({"id": GWS, "title": "Google Workspace", "version": installed, "state": "running", "enabled": true, "update": update});
    batch(|| {
        store().v1.listed.set(vec![Listed::of(&v)]);
        store().v1.entries.set(vec![PluginEntry::from_json(&v).unwrap()]);
        store().v1.known.set(true);
    });
}

fn waiting(version: &str) -> Value {
    json!({
        "version": version,
        "notes": ["Search is faster", "Drafts keep their place", "Calendar shows working hours"],
        "url": "https://github.com/buwiz-app/google-workspace/releases/tag/v1.5.0",
        "access": ["Google Drive"],
    })
}

fn button(label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view().label(label).w(px(30.0)).h_full().on_press(on)
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
                .child(button("seed", || store().active_id.set("chat".to_owned())))
                .child(button("1.5.0 waits", || list("1.4.0", waiting("1.5.0"))))
                .child(button("plain 1.5.0 waits", || list("1.4.0", json!({"version": "1.5.0"}))))
                .child(button("1.6.0 waits", || list("1.4.0", waiting("1.6.0"))))
                .child(button("installed", || list("1.5.0", Value::Null)))
                .child(button("installed, old news", || list("1.5.0", waiting("1.5.0"))))
                .child(button("made with 1.4.0", || store().v1_running(7, &key(), "sha256:aa", "1.4.0")))
                .child(button("installing", || {
                    store().v1.installing.update(|i| {
                        i.insert(GWS.to_owned());
                    })
                }))
                .child(button("install ended", || store().v1.installing.set(Default::default())))
                .child(button("plugin restarts", || {
                    store().v1_reconnecting(&key(), "Mail");
                    store().v1_plugin_event(&json!({"id": GWS, "state": "running", "generation": 2, "enabled": true}));
                }))
                .child(button("next launch", || store().v1.later.set(Default::default())))
                .child(button("made again with 1.5.0", || {
                    store().v1_stopped(7);
                    store().v1_running(8, &key(), "sha256:bb", "1.5.0");
                })),
        )
        .child(text(|| {
            format!(
                "probe panel={} seen={} waiting={} skipped={} notices={} version={}",
                store().v1.update_panel.with(|p| p.as_ref().map_or("none".to_owned(), |(id, pinned)| format!("{id}/{pinned}"))),
                store().v1_update_seen(GWS, "1.5.0"),
                store().v1_updates_waiting().len(),
                store().v1.records.with(|r| r.updates.iter().map(|u| u.skipped.clone()).collect::<Vec<_>>().join("+")),
                store().v1.notice.with(|n| n.len()),
                store().v1.entries.with(|e| e.first().map(|p| p.version.clone()).unwrap_or_default()),
            )
        }))
        // as tall as its card: anything that grew the card would move `after`
        .child(view().label("cards").child(ui::answer_cards("turn", links(), || 0)))
        .child(view().label("after").h(px(10.0)))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    let mut app = run(config);
    app.press("seed");
    app
}

fn says(app: &Headless, text: &str) -> bool {
    app.texts().iter().any(|t| t.contains(text))
}

/// Where the card and what follows it sit.
fn layout(app: &Headless) -> (Bounds<Points>, Bounds<Points>) {
    (app.frame("cards"), app.frame("after"))
}

/// A stand-in broker: every request's first line and body, answered `{"ok":true}`.
fn stub_broker() -> Arc<Mutex<Vec<String>>> {
    stub_broker_listing("1.4.0")
}

/// [`stub_broker`], whose plugin list is Google Workspace at `version`.
fn stub_broker_listing(version: &'static str) -> Arc<Mutex<Vec<String>>> {
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
            let plugin = json!({"id": GWS, "title": "Google Workspace", "version": version, "state": "running", "enabled": true, "generation": 2});
            let body = json!({"ok": true, "plugins": [plugin]}).to_string();
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    seen
}

/// B1-1, C1-1, C2-2: with a newer version the card keeps its place and
/// shows the (i) with a dot; a press opens the panel over the card, which
/// grows nothing, and the dot goes for good.
#[test]
fn a_waiting_update_shows_the_signal_and_its_panel_floats_over_the_card() {
    if !in_child("a_waiting_update_shows_the_signal_and_its_panel_floats_over_the_card") {
        return;
    }
    let mut app = start();
    app.press("installed");
    assert!(says(&app, "Mail") && !app.has(SIGNAL), "no signal without an update");
    let before = layout(&app);
    app.press("1.5.0 waits");
    assert!(app.has(SIGNAL) && app.has("New update"), "the (i) and its dot");
    assert!(says(&app, "waiting=1") && says(&app, "seen=false"), "{:?}", app.texts());
    assert_eq!(layout(&app), before, "the signal takes no room in the chat");
    assert!(!app.has("Update panel"));

    app.press(SIGNAL);
    assert!(app.has("Update panel") && says(&app, "panel=turn:mail/true"), "{:?}", app.texts());
    for line in ["Google Workspace 1.4.0 → 1.5.0", "Search is faster", "Drafts keep their place", "Calendar shows working hours", "Needs new access: Google Drive", "Read more"] {
        assert!(says(&app, line), "{line}: {:?}", app.texts());
    }
    for b in ["Review access", "Later", "Skip 1.5.0"] {
        assert!(app.has(b), "{b}");
    }
    assert_eq!(layout(&app), before, "the panel floats: the card and what follows stay where they were");
    // looked at: no dot, here and after a relaunch
    assert!(!app.has("New update") && says(&app, "seen=true"));
    let dir = std::path::PathBuf::from(std::env::var_os("BUWIZ_DATA_DIR").unwrap());
    for _ in 0..100 {
        if Records::load(&dir).update_seen(GWS, "1.5.0") {
            break;
        }
        app.pump(Duration::from_millis(20));
    }
    assert!(Records::load(&dir).update_seen(GWS, "1.5.0"), "the dot's end is saved");

    // Escape closes it; a press on the icon again opens and closes it
    app.cancel("Update panel");
    assert!(!app.has("Update panel") && app.has(SIGNAL) && says(&app, "panel=none"));
    app.press(SIGNAL);
    assert!(app.has("Update panel"));
    app.press(SIGNAL);
    assert!(!app.has("Update panel"));
    assert_eq!(layout(&app), before);

    // an update that asks for nothing new says Review update
    app.press("plain 1.5.0 waits");
    app.press(SIGNAL);
    assert!(app.has("Review update") && !app.has("Review access") && !says(&app, "Needs new access") && !says(&app, "Read more"), "{:?}", app.texts());
}

/// C1-1: resting the pointer on the (i) peeks the panel after a moment;
/// moving away closes a peek, after a grace that lets the pointer travel
/// into the panel; a pressed one stays.
#[test]
fn the_panel_peeks_on_hover_and_a_peek_closes_when_the_pointer_leaves() {
    if !in_child("the_panel_peeks_on_hover_and_a_peek_closes_when_the_pointer_leaves") {
        return;
    }
    let mut app = start();
    app.press("1.5.0 waits");
    app.hover(SIGNAL, true);
    assert!(!app.has("Update panel"), "not before the pointer rested");
    app.pump(Duration::from_millis(400));
    assert!(app.has("Update panel") && says(&app, "panel=turn:mail/false"), "{:?}", app.texts());
    assert!(says(&app, "seen=true"), "a peek is a look");
    // off the icon and onto the panel within the grace: it stays
    app.hover(SIGNAL, false);
    app.hover("Update details", true);
    app.pump(Duration::from_millis(400));
    assert!(app.has("Update panel"), "the pointer travelled into it");
    // away from both: it closes
    app.hover("Update details", false);
    app.pump(Duration::from_millis(400));
    assert!(!app.has("Update panel") && app.has(SIGNAL));
    // a pass over the icon shorter than the rest opens nothing
    app.hover(SIGNAL, true);
    app.hover(SIGNAL, false);
    app.pump(Duration::from_millis(400));
    assert!(!app.has("Update panel"));
    // a pressed panel is pinned: leaving doesn't close it
    app.press(SIGNAL);
    app.hover(SIGNAL, false);
    app.pump(Duration::from_millis(400));
    assert!(app.has("Update panel") && says(&app, "panel=turn:mail/true"));
}

/// H1-3, E2-1: Later hides the signal for this run, Skip hides that
/// version for good and a newer one shows again; an installed update
/// leaves no signal, whatever an old list still says.
#[test]
fn later_skip_and_an_installed_update_clear_the_signal() {
    if !in_child("later_skip_and_an_installed_update_clear_the_signal") {
        return;
    }
    let mut app = start();
    app.press("1.5.0 waits");
    app.press(SIGNAL);
    app.press("Later");
    assert!(!app.has("Update panel") && !app.has(SIGNAL), "put off until the next launch");
    assert!(says(&app, "waiting=1"), "Later is the card's: the update still waits");
    assert!(says(&app, "skipped= notices"), "nothing is saved for Later: {:?}", app.texts());
    // a newer version is news again
    app.press("1.6.0 waits");
    let newer = "Google Workspace 1.6.0 update available";
    assert!(app.has(newer));
    app.press(newer);
    assert!(says(&app, "Google Workspace 1.4.0 → 1.6.0"));
    app.press("Skip 1.6.0");
    assert!(!app.has("Update panel") && !app.has(newer) && says(&app, "waiting=0"), "{:?}", app.texts());
    let dir = std::path::PathBuf::from(std::env::var_os("BUWIZ_DATA_DIR").unwrap());
    for _ in 0..100 {
        if Records::load(&dir).update_skipped(GWS, "1.6.0") {
            break;
        }
        app.pump(Duration::from_millis(20));
    }
    assert!(Records::load(&dir).update_skipped(GWS, "1.6.0"), "the skip is saved");

    // E2-1: installed means no signal, in this run and the next
    app.press("next launch");
    app.press("1.5.0 waits");
    assert!(app.has(SIGNAL));
    app.press("installed");
    assert!(!app.has(SIGNAL) && says(&app, "waiting=0"));
    app.press("installed, old news");
    assert!(!app.has(SIGNAL) && says(&app, "waiting=0"), "an update at the installed version is no news");
}

/// D1-1: Review asks the broker for the install-review card in the open
/// chat, and installs nothing.
#[test]
fn review_asks_for_the_review_card_in_the_open_chat() {
    if !in_child("review_asks_for_the_review_card_in_the_open_chat") {
        return;
    }
    let seen = stub_broker();
    let mut app = start();
    app.press("1.5.0 waits");
    app.press(SIGNAL);
    app.press("Review access");
    assert!(!app.has("Update panel"), "the panel makes way for the chat");
    let mut heard = None;
    for _ in 0..200 {
        app.pump(Duration::from_millis(20));
        if let Some(r) = seen.lock().unwrap().iter().find(|r| r.contains("/v1/host/plugins/updates/review")) {
            heard = Some(r.clone());
            break;
        }
    }
    let heard = heard.unwrap_or_else(|| panic!("no review reached the broker: {:?}", seen.lock().unwrap()));
    assert!(heard.starts_with("POST ") && heard.contains(&format!(r#""id":"{GWS}""#)) && heard.contains(r#""chat":"chat""#), "{heard}");
    assert!(!seen.lock().unwrap().iter().any(|r| r.contains("/plugins/install")), "nothing installs without Install");
    assert!(app.has(SIGNAL), "the signal stays until the update is installed");
}

/// D3-1, E1-2: a line along the card's top edge while its plugin
/// installs, which grows nothing; then the old card says what it was made
/// with and offers Refresh.
#[test]
fn an_install_draws_a_line_and_the_old_card_says_what_it_was_made_with() {
    if !in_child("an_install_draws_a_line_and_the_old_card_says_what_it_was_made_with") {
        return;
    }
    let mut app = start();
    app.press("1.5.0 waits");
    let before = layout(&app);
    assert!(!app.has("Installing"));
    app.press("installing");
    assert!(app.has("Installing"));
    assert_eq!(app.frame("Installing").size.height, px(2.0));
    assert_eq!(layout(&app), before, "the line lies over the card");
    app.press("install ended");
    assert!(!app.has("Installing"));

    // made with the version still installed: nothing to say
    app.press("made with 1.4.0");
    assert!(!says(&app, "Made with"));
    // the update lands: the card keeps its place and says so
    app.press("installed");
    assert!(says(&app, "Made with 1.4.0 ·") && app.has("Refresh this card"), "{:?}", app.texts());
    assert!(!app.has(SIGNAL));
    // Refresh loads the card again; an open made with 1.5.0 says nothing
    app.press("Refresh this card");
    app.press("made again with 1.5.0");
    assert!(!says(&app, "Made with"), "{:?}", app.texts());
}

/// The list after `app` pressed "plugin restarts" says `version`.
fn listed_again(app: &mut Headless, version: &str) {
    for _ in 0..200 {
        app.pump(Duration::from_millis(20));
        if says(app, &format!("version={version}")) && says(app, "notices=0") {
            return;
        }
    }
    panic!("the list never came: {:?}", app.texts());
}

/// A1-1, E1-2, K1-1: when the plugin restarts into a new version the
/// card is not reloaded: its "reconnecting" goes and it says what it was
/// made with. A restart into the same version leaves nothing to say (the
/// card reopens as if nothing happened).
#[test]
fn a_restart_into_a_new_version_leaves_the_card_as_it_is() {
    if !in_child("a_restart_into_a_new_version_leaves_the_card_as_it_is") {
        return;
    }
    let _seen = stub_broker_listing("1.5.0");
    let mut app = start();
    app.press("1.5.0 waits");
    app.press("made with 1.4.0");
    app.press("plugin restarts");
    listed_again(&mut app, "1.5.0");
    assert!(says(&app, "Made with 1.4.0 ·") && app.has("Refresh this card"), "{:?}", app.texts());
    assert!(!app.has(SIGNAL), "installed: the signal is gone");
}

#[test]
fn a_restart_into_the_same_version_says_nothing() {
    if !in_child("a_restart_into_the_same_version_says_nothing") {
        return;
    }
    let _seen = stub_broker_listing("1.4.0");
    let mut app = start();
    app.press("installed");
    app.press("made with 1.4.0");
    app.press("plugin restarts");
    listed_again(&mut app, "1.4.0");
    assert!(!says(&app, "Made with"), "{:?}", app.texts());
}
