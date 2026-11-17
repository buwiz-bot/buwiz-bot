//! A plugin's badge on its pins (spec 16.4, `buwiz/badge`) in the sidebar, on a headless
//! engine (`nsh-testing`): each dot colour with its text, a clear and an update
//! arriving while the app is open, and the row, which a badge never changes.
//!
//! The store is one per process, so each test runs in a child of its own.
//! Layout is asserted here; no real window draws it, so the colours as a
//! person sees them are not.

use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig};
use serde_json::{Value, json};

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;
use crate::mini_v1::{load::PluginEntry, records::Pin, ui};
use crate::theme;

const CHILD: &str = "BUWIZ_PIN_BADGE_UI_TEST";
const TAX: &str = "app.buwiz.ebirforms";
/// The pins' titles, and the app each is a pin of.
const PINS: [(&str, &str); 6] = [("Tax forms", "forms"), ("Calendar", "calendar"), ("Board", "board"), ("People", "people"), ("Plain", "plain"), ("Form A", "forms")];

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("pin_badge_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// What the broker lists for eBIRForms: a badge on four of the apps, and one
/// for the pin of `forms` that carries ticket `tkt_A`.
fn listed() -> Value {
    json!({
        "id": TAX, "title": "eBIRForms", "enabled": true,
        "badges": [
            {"app": "forms", "ticket": null, "dot": "red", "text": "2 due"},
            {"app": "forms", "ticket": "tkt_A", "dot": "green", "text": "ok"},
            {"app": "calendar", "ticket": null, "dot": "amber", "text": ""},
            {"app": "board", "ticket": null, "dot": "green", "text": "18h"},
            {"app": "people", "ticket": null, "dot": "none", "text": "3 new"},
        ],
    })
}

/// A badge as the broker's `badge` event says it.
fn event(plugin: &str, app: &str, ticket: Option<&str>, dot: &str, text: &str) {
    store().v1_badge_event(&json!({"plugin": plugin, "app": app, "ticket": ticket, "dot": dot, "text": text}));
}

fn button(label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view().label(label).w(px(30.0)).h_full().on_press(on)
}

fn seed() {
    let st = store();
    st.v1.entries.set(vec![PluginEntry::from_json(&listed()).unwrap()]);
    st.v1.records.update(|r| {
        for (title, app) in PINS {
            let ticket = (title == "Form A").then(|| "tkt_A".to_owned());
            r.pin(Pin { plugin: TAX.into(), app: app.into(), ticket, placement: "full".into(), title: title.into(), ..Default::default() });
        }
    });
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
                .child(button("clear forms", || event(TAX, "forms", None, "none", "")))
                .child(button("forms amber", || event(TAX, "forms", None, "amber", "18h")))
                .child(button("forms a red", || event(TAX, "forms", Some("tkt_A"), "red", "late")))
                .child(button("forms a clear", || event(TAX, "forms", Some("tkt_A"), "none", "")))
                .child(button("plain red", || event(TAX, "plain", None, "red", "")))
                .child(button("other plugin", || event("app.buwiz.other", "forms", None, "green", "x")))
                .child(button("broken", || store().v1_badge_event(&json!({"plugin": TAX, "app": "forms", "ticket": null, "dot": "blue", "text": "no"})))),
        )
        .child(text(|| "probe ready=1".to_owned()))
        .child(view().label("stage").flex_1().w(px(240.0)).child(ui::pins_section()))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    let mut app = run(config);
    app.press("seed");
    app
}

/// Whether a live view is now labelled `label`: a view keeps the labels it had before, in the
/// harness's list, so only its last one counts.
fn has(app: &Headless, label: &str) -> bool {
    let mut seen = std::collections::HashSet::new();
    app.labels.iter().rev().filter(|(id, _)| app.live.contains(id) && seen.insert(*id)).any(|(_, l)| l == label)
}

fn says(app: &Headless, text: &str) -> bool {
    app.texts().contains(&text)
}

/// A row's size and where its title starts and how wide it is.
fn shape(app: &Headless, row: &str, title: &str) -> [f32; 5] {
    let (r, t) = (app.frame(row), app.frames[&app.text_view(title).0]);
    [r.size.width.0, r.size.height.0, r.origin.y.0, t.origin.x.0, t.size.width.0]
}

/// Every dot colour shows on its pin with its text; the pin's name says the
/// state in words, and the ticket's own badge is for the pins with that ticket.
#[test]
fn each_dot_and_its_text_show_on_the_pin() {
    if !in_child("each_dot_and_its_text_show_on_the_pin") {
        return;
    }
    let app = start();
    for name in [
        "Tax forms, status red, 2 due",
        "Calendar, status amber",
        "Board, status green, 18h",
        "People, 3 new",
        "Plain",
        "Form A, status green, ok",
    ] {
        assert!(has(&app, name), "{name}: {:?}", app.labels);
    }
    for text in ["2 due", "18h", "3 new", "ok"] {
        assert!(says(&app, text), "{text}: {:?}", app.texts());
    }
    // one dot a colour, the theme's status colours; the pin with no dot has none
    let ink = |label: &str| app.style(label).background_color;
    assert_eq!(ink("Status red"), nsh_core::Color::from(theme::DOWN.light));
    assert_eq!(ink("Status amber"), nsh_core::Color::from(theme::WARN.light));
    assert_ne!(ink("Status red"), ink("Status amber"));
    assert!(has(&app, "Status green") && !has(&app, "Status none"));
    assert_eq!(app.style("Status green").background_color, nsh_core::Color::from(theme::UP.light));
}

/// A clear and an update arrive while the app is open: the pin changes with
/// no list asked for, and another plugin's badge or a broken one changes nothing.
#[test]
fn a_clear_and_an_update_arrive_while_the_app_is_open() {
    if !in_child("a_clear_and_an_update_arrive_while_the_app_is_open") {
        return;
    }
    let mut app = start();
    app.press("forms amber");
    assert!(has(&app, "Tax forms, status amber, 18h") && !has(&app, "Tax forms, status red, 2 due"), "{:?}", app.labels);
    assert!(says(&app, "18h") && !says(&app, "2 due"), "{:?}", app.texts());
    assert!(has(&app, "Form A, status green, ok"), "the ticket's pin keeps its own");
    app.press("forms a red");
    assert!(has(&app, "Form A, status red, late"), "{:?}", app.labels);
    app.press("forms a clear");
    assert!(has(&app, "Form A, status amber, 18h"), "a cleared ticket badge falls back to the app's: {:?}", app.labels);
    app.press("clear forms");
    assert!(has(&app, "Tax forms") && has(&app, "Form A"), "a clear: no state in the name: {:?}", app.labels);
    assert!(!has(&app, "Status red"), "no red dot is left: {:?}", app.labels);
    assert_eq!(app.texts().iter().filter(|t| **t == "18h").count(), 1, "only Board's 18h is left: {:?}", app.texts());
    app.press("plain red");
    assert!(has(&app, "Plain, status red") && has(&app, "Status red"), "{:?}", app.labels);
    app.press("other plugin");
    app.press("broken");
    assert!(has(&app, "Tax forms") && has(&app, "Plain, status red"), "nothing else moved: {:?}", app.labels);
    assert!(!has(&app, "Tax forms, status green, x") && !has(&app, "Tax forms, status blue, no"));
}

/// A badge never changes a pin's size or moves its title: the same row, the
/// same title, whether it has a badge, another or none.
#[test]
fn a_badge_never_changes_the_row_or_its_title() {
    if !in_child("a_badge_never_changes_the_row_or_its_title") {
        return;
    }
    let mut app = start();
    let plain = shape(&app, "Plain", "Plain");
    let red = shape(&app, "Tax forms, status red, 2 due", "Tax forms");
    let none = shape(&app, "People, 3 new", "People");
    assert_eq!([plain[0], plain[1], plain[3], plain[4]], [red[0], red[1], red[3], red[4]], "a badged pin is the size of a plain one");
    assert_eq!([plain[0], plain[1], plain[3], plain[4]], [none[0], none[1], none[3], none[4]]);
    // arriving, changing, clearing: the row stays where it was, its title too
    app.press("plain red");
    assert_eq!(shape(&app, "Plain, status red", "Plain"), plain);
    app.press("forms amber");
    assert_eq!(shape(&app, "Tax forms, status amber, 18h", "Tax forms"), red);
    app.press("clear forms");
    assert_eq!(shape(&app, "Tax forms", "Tax forms"), red);
}
