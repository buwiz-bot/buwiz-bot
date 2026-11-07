//! The viewer's bar on a headless engine: what a person sees and what a press asks for. Nothing
//! reaches a server: the backend is a port nothing listens on.

use std::time::Duration;

use buwiz_office_kit::testing::{Headless, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};
use serde_json::json;

use crate::computer::{Job, Status, Takeover};

const CHILD: &str = "BUWIZ_COMPUTER_UI_TEST";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("computer_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

fn say(viewer: &str, takeover: bool) {
    let takeover = takeover.then(|| Takeover { reason: "Sign in to the shop".to_owned(), seconds_left: 125 });
    crate::computer::show_status("b1", Status { state: "awake".to_owned(), viewer: viewer.to_owned(), takeover, ..Status::default() });
}

#[component]
fn Bar() -> impl IntoElement {
    view()
        .size_full()
        .child(view().label("watching").w(px(40.0)).h(px(20.0)).on_press(|| say("watch", false)))
        .child(view().label("driving").w(px(40.0)).h(px(20.0)).on_press(|| say("drive", false)))
        .child(view().label("handed").w(px(40.0)).h(px(20.0)).on_press(|| say("drive", true)))
        .child(view().label("another bot").w(px(40.0)).h(px(20.0)).on_press(|| crate::computer::show_status("b2", Status { viewer: "drive".to_owned(), ..Status::default() })))
        .child(crate::computer::drive_button("computer:b1"))
        .child(text(|| format!("probe ready=1 wants={}", crate::computer::wants_to_drive("b1"))).text_size(px(8.0)))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Bar>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(300.0));
    run(config)
}

fn shows(app: &mut Headless, word: &str) -> bool {
    app.pump(Duration::from_millis(60));
    app.texts().iter().any(|t| t.contains(word))
}

#[test]
fn the_bar_takes_control_gives_it_up_and_hands_back() {
    if !in_child("the_bar_takes_control_gives_it_up_and_hands_back") {
        return;
    }
    let mut app = start();
    app.press("watching");
    assert!(shows(&mut app, "Take control") && shows(&mut app, "Your clicks don't reach"), "{:?}", app.texts());
    assert!(shows(&mut app, "wants=false"));

    app.press("Take control");
    assert!(shows(&mut app, "wants=true"), "{:?}", app.texts());
    app.press("driving");
    assert!(shows(&mut app, "Stop driving") && shows(&mut app, "The agent waits"), "{:?}", app.texts());
    app.press("Take control");
    assert!(shows(&mut app, "wants=false"), "pressing it again gives control up");

    app.press("handed");
    assert!(shows(&mut app, "Hand back") && shows(&mut app, "Sign in to the shop · 2:05 left"), "{:?}", app.texts());
    app.press("Take control");
    assert!(shows(&mut app, "wants=false"), "Hand back asks for no ticket of its own");

    app.press("another bot");
    assert!(shows(&mut app, "Take control"), "another Bot's viewer says nothing of this one: {:?}", app.texts());
}

#[component]
fn Tab() -> impl IntoElement {
    // first read here: the store's owner is the root's, and it lives as long as the app
    let _ = crate::live::store();
    let on = signal(Status { state: "awake".to_owned(), ..Status::default() });
    let job = |frozen| Job { name: "the build".to_owned(), bot: "Ann".to_owned(), frozen, seconds_left: 14_340 };
    view()
        .size_full()
        .child(view().label("a job").w(px(40.0)).h(px(20.0)).on_press(move || on.update(|s| s.jobs = vec![job(false)])))
        .child(view().label("asleep").w(px(40.0)).h(px(20.0)).on_press(move || on.set(Status { state: "asleep".to_owned(), jobs: vec![job(true)], ..Status::default() })))
        .child(view().label("over").w(px(40.0)).h(px(20.0)).on_press(move || on.set(Status { state: "awake".to_owned(), ..Status::default() })))
        .child(dynamic(move || crate::computer::body_of(on.get()).into_element()))
        .child(text("probe ready=1").text_size(px(8.0)))
}

#[test]
fn the_tab_says_how_a_job_stands_and_sleep_says_what_it_does() {
    if !in_child("the_tab_says_how_a_job_stands_and_sleep_says_what_it_does") {
        return;
    }
    let mut config = EngineConfig::new(App::Rust(Root::new::<Tab>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(600.0));
    let mut app = run(config);
    assert!(shows(&mut app, "Sleep") && !shows(&mut app, "'s job"), "{:?}", app.texts());

    app.press("a job");
    assert!(shows(&mut app, "Ann's job \"the build\" is running · 3 h 59 min left"), "{:?}", app.texts());
    assert!(shows(&mut app, "Sleep, and freeze its job"), "{:?}", app.texts());
    // pick 1B: a job's state and nothing of what the Bot can be counted on for
    for promise in ["always", "keeps working", "while this Mac", "isn't a server"] {
        assert!(!app.texts().iter().any(|t| t.to_lowercase().contains(promise)), "{promise}: {:?}", app.texts());
    }

    app.press("asleep");
    assert!(shows(&mut app, "frozen while this computer sleeps"), "{:?}", app.texts());

    app.press("over");
    assert!(!shows(&mut app, "'s job") && !shows(&mut app, "freeze"), "{:?}", app.texts());
}

#[component]
fn RemoteForm() -> impl IntoElement {
    view()
        .size_full()
        .child(view().label("on b1").w(px(40.0)).h(px(20.0)).on_press(|| crate::computer::pane_for("b1")))
        .child(crate::computer::remote_form())
        .child(text(|| format!("probe ready=1 empty={}", crate::computer::form_is_empty())).text_size(px(8.0)))
}

fn wait_for(app: &mut Headless, word: &str) {
    for _ in 0..100 {
        if shows(app, word) {
            return;
        }
    }
    panic!("{word} never showed: {:?}", app.texts());
}

#[test]
fn the_form_trusts_a_key_only_on_a_press_and_adds_only_on_a_yes_and_keeps_no_secret() {
    if !in_child("the_form_trusts_a_key_only_on_a_press_and_adds_only_on_a_yes_and_keeps_no_secret") {
        return;
    }
    crate::computer::CANNED.lock().unwrap().0 = vec![
        json!({"ok": false, "code": "trust", "fingerprint": "SHA256:abc", "pin": "ssh-ed25519 AAAA"}),
        json!({"ok": true, "screen": false, "system": "Linux", "consent": "The AI will act on this machine."}),
        json!({"state": "asleep", "computer": "c", "remote": {"screen": false, "where": "me@h"}}),
    ];
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("on b1");
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    app.type_text("ssh://you@host or https://box.example.com", "ssh://me@h");
    app.type_text("SSH key file, like ~/.ssh/id_ed25519", "~/.ssh/id");
    app.type_text("The box's token (none for a plain SSH host)", "sesame");
    app.press("Test");
    wait_for(&mut app, "SHA256:abc");
    assert!(shows(&mut app, "Nothing has been sent yet"), "{:?}", app.texts());
    assert!(!shows(&mut app, "I understand"), "no yes before the key is trusted");
    app.press("I checked, trust it");
    wait_for(&mut app, "The AI will act on this machine.");
    assert!(shows(&mut app, "Linux answered") && shows(&mut app, "no screen"), "{:?}", app.texts());
    app.press("I understand, add it");
    wait_for(&mut app, "empty=true");
    let sent = std::mem::take(&mut crate::computer::CANNED.lock().unwrap().1);
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent[0].0.ends_with("/computer/bot/b1/remote/test") && sent[0].1["token"] == "sesame" && sent[0].1["pin"] == "", "{sent:?}");
    assert_eq!(sent[1].1["pin"], "ssh-ed25519 AAAA", "the pin follows the press");
    assert!(sent[2].0.ends_with("/computer/bot/b1/remote") && sent[2].1["consent"] == true, "{sent:?}");
}

#[test]
fn an_https_address_has_no_ssh_key_field_and_a_certificate_to_check() {
    if !in_child("an_https_address_has_no_ssh_key_field_and_a_certificate_to_check") {
        return;
    }
    crate::computer::CANNED.lock().unwrap().0 = vec![json!({"ok": false, "code": "trust", "fingerprint": "ab12", "pin": "ab12"})];
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("on b1");
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    let key = "SSH key file, like ~/.ssh/id_ed25519";
    assert!(app.has(key), "an ssh:// form needs its key field");
    assert!(app.has("The box's viewer password (none for a plain SSH host)"), "a plain SSH host needs no viewer password");
    app.type_text("ssh://you@host or https://box.example.com", "https://box.example.com");
    app.pump(Duration::from_millis(60));
    assert!(!app.has(key), "https asks for no SSH key");
    app.press("Test");
    wait_for(&mut app, "ab12");
    assert!(shows(&mut app, "the certificate on the server") && !shows(&mut app, "on the machine itself"), "{:?}", app.texts());
}

fn reach_ready(app: &mut Headless) {
    app.press("Test");
    wait_for(app, "SHA256:abc");
    app.press("I checked, trust it");
    wait_for(app, "The AI will act");
}

#[test]
fn an_edit_after_a_test_takes_back_the_yes_and_a_new_address_the_trust() {
    if !in_child("an_edit_after_a_test_takes_back_the_yes_and_a_new_address_the_trust") {
        return;
    }
    let trust = json!({"ok": false, "code": "trust", "fingerprint": "SHA256:abc", "pin": "ssh-ed25519 AAAA"});
    let ready = json!({"ok": true, "screen": false, "system": "Linux", "consent": "The AI will act on this machine."});
    crate::computer::CANNED.lock().unwrap().0 = vec![trust.clone(), ready.clone(), ready, trust];
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("on b1");
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    let (url, key) = ("ssh://you@host or https://box.example.com", "SSH key file, like ~/.ssh/id_ed25519");
    app.type_text(url, "ssh://me@h");
    app.type_text(key, "~/.ssh/a");
    reach_ready(&mut app);

    // another key: the yes is gone, and the host's key is still the one trusted
    app.type_text(key, "~/.ssh/b");
    wait_for(&mut app, "Test");
    assert!(!shows(&mut app, "I understand") && !shows(&mut app, "The AI will act"), "{:?}", app.texts());
    app.press("Test");
    wait_for(&mut app, "The AI will act");

    // another login: the trust goes too, so the next Test asks again
    app.type_text(url, "ssh://root@h");
    wait_for(&mut app, "Test");
    assert!(!shows(&mut app, "I understand"), "{:?}", app.texts());
    app.press("Test");
    wait_for(&mut app, "SHA256:abc");

    // and from a Trust that's showing
    app.type_text(key, "~/.ssh/c");
    wait_for(&mut app, "Test");
    assert!(!shows(&mut app, "I checked"), "{:?}", app.texts());

    let sent = std::mem::take(&mut crate::computer::CANNED.lock().unwrap().1);
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert_eq!(sent[2].1["pin"], "ssh-ed25519 AAAA", "a key change keeps what was trusted of the host");
    assert_eq!(sent[3].1["pin"], "", "an address change doesn't");
}

#[test]
fn an_answer_for_what_was_typed_before_an_edit_is_no_answer() {
    if !in_child("an_answer_for_what_was_typed_before_an_edit_is_no_answer") {
        return;
    }
    use std::sync::atomic::Ordering;
    crate::computer::CANNED.lock().unwrap().0 = vec![json!({"ok": true, "screen": false, "system": "Linux", "consent": "The AI will act on this machine."})];
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    let key = "SSH key file, like ~/.ssh/id_ed25519";
    app.type_text(key, "~/.ssh/a");
    crate::computer::SLOW.store(true, Ordering::SeqCst);
    app.press("Test");
    wait_for(&mut app, "Asking that machine");
    app.type_text(key, "~/.ssh/b");
    wait_for(&mut app, "Test");
    crate::computer::SLOW.store(false, Ordering::SeqCst);
    for _ in 0..10 {
        assert!(!shows(&mut app, "The AI will act"), "the yes came back for the old key: {:?}", app.texts());
    }
    assert_eq!(crate::computer::CANNED.lock().unwrap().1.len(), 1, "the answer did arrive");
}

#[test]
fn an_answer_after_cancel_doesnt_open_the_form_again() {
    if !in_child("an_answer_after_cancel_doesnt_open_the_form_again") {
        return;
    }
    use std::sync::atomic::Ordering;
    crate::computer::CANNED.lock().unwrap().0 = vec![json!({"ok": true, "screen": false, "system": "Linux", "consent": "The AI will act on this machine."})];
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    crate::computer::SLOW.store(true, Ordering::SeqCst);
    app.press("Test");
    wait_for(&mut app, "Asking that machine");
    app.press("Cancel");
    wait_for(&mut app, "Connect a remote computer");
    crate::computer::SLOW.store(false, Ordering::SeqCst);
    while crate::computer::CANNED.lock().unwrap().1.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..10 {
        assert!(!shows(&mut app, "The AI will act"), "the yes came back for an empty form: {:?}", app.texts());
    }
}

#[test]
fn cancelling_the_form_drops_what_was_typed_and_a_no_stays_on_it() {
    if !in_child("cancelling_the_form_drops_what_was_typed_and_a_no_stays_on_it") {
        return;
    }
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut app = run(config);
    app.press("Connect a remote computer");
    wait_for(&mut app, "Test");
    app.type_text("The box's token (none for a plain SSH host)", "sesame");
    wait_for(&mut app, "empty=false");
    app.press("Test");
    // the backend here is a port nothing listens on
    wait_for(&mut app, "The broker didn't answer.");
    assert!(shows(&mut app, "Test"), "a person can try again: {:?}", app.texts());
    app.press("Cancel");
    wait_for(&mut app, "empty=true");
    assert!(shows(&mut app, "Connect a remote computer"));
}
