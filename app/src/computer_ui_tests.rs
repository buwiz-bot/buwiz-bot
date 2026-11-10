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
    let takeover = takeover.then(|| Takeover { reason: "Sign in to the shop".to_owned(), seconds_left: 125, heard: std::time::Instant::now() });
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
        .child(view().label("heard now").w(px(40.0)).h(px(20.0)).on_press(|| {
            let takeover = Takeover { reason: "Sign in to the shop".to_owned(), seconds_left: 125, heard: std::time::Instant::now() };
            crate::computer::show_status("b1", Status { state: "awake".to_owned(), viewer: "drive".to_owned(), takeover: Some(takeover), ..Status::default() });
        }))
        .child(crate::computer::mode_line("computer:b1"))
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

/// A hand-over's time counts down between the 2 s polls, from what the last one said: here no
/// poll comes after the first, and the time still moves.
#[test]
fn a_hand_overs_time_counts_down_between_polls() {
    if !in_child("a_hand_overs_time_counts_down_between_polls") {
        return;
    }
    let mut app = start();
    app.press("heard now");
    assert!(shows(&mut app, "Sign in to the shop · 2:05 left"), "{:?}", app.texts());
    app.pump(Duration::from_millis(1300));
    let line = app.texts().iter().find(|t| t.starts_with("Sign in to the shop · ")).map(|t| (*t).to_owned()).unwrap_or_default();
    assert!(["2:04", "2:03", "2:02"].iter().any(|t| line == format!("Sign in to the shop · {t} left")), "a second on, with no poll: {line:?}");
}

/// The words the broker gave for a hand-over: longer than the bar has room for.
const LONG: &str = "Sign in to the shop with the account the owner uses for every order, then pass the code it sends";

/// A computer full screen, as the window shows it.
#[component]
fn Full() -> impl IntoElement {
    let _ = crate::live::store();
    view()
        .size_full()
        .child(view().label("open").w(px(40.0)).h(px(20.0)).on_press(|| {
            crate::live::store().open_mini_token(&crate::computer::key_of("b1"), "Ann's computer", "");
            let takeover = Takeover { reason: LONG.to_owned(), seconds_left: 125, heard: std::time::Instant::now() };
            crate::computer::show_status("b1", Status { state: "awake".to_owned(), viewer: "drive".to_owned(), takeover: Some(takeover), ..Status::default() });
        }))
        .child(text("probe ready=1").text_size(px(8.0)))
        .child(view().flex_1().child(crate::mini::full_layer()))
}

/// The frame of `id` in the window, by the frames of what holds it.
fn in_window(app: &Headless, mut id: nsh_core::NodeId) -> (f32, f32, f32, f32) {
    let f = app.frames[&id];
    let (mut x, mut y) = (0.0, 0.0);
    loop {
        let at = app.frames.get(&id).map_or((0.0, 0.0), |f| (f.origin.x.0, f.origin.y.0));
        x += at.0;
        y += at.1;
        match app.parents.get(&id) {
            Some(p) => id = *p,
            None => break,
        }
    }
    (x, y, f.size.width.0, f.size.height.0)
}

/// A long reason has a row of its own under the bar, one line the window's width, and so never
/// runs under the centred "Ann's computer". The bar holds the title and its buttons, and no words of
/// the hand-over.
#[test]
fn a_long_reason_has_its_own_row_under_the_title() {
    if !in_child("a_long_reason_has_its_own_row_under_the_title") {
        return;
    }
    let mut config = EngineConfig::new(App::Rust(Root::new::<Full>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(700.0), px(500.0));
    let mut app = run(config);
    app.press("open");
    let words = format!("{LONG} · 2:05 left");
    app.wait_for(&words);
    assert!(shows(&mut app, "Ann's computer"), "{:?}", app.texts());
    let bar = app.parents[&app.view("Close")];
    let row = app.view(&words);
    let (bx, by, bw, bh) = in_window(&app, bar);
    let (rx, ry, rw, rh) = in_window(&app, row);
    println!("bar {bx},{by} {bw}x{bh}; row {rx},{ry} {rw}x{rh}");
    assert!(ry >= by + bh, "the row starts at {ry}, inside the bar that ends at {}", by + bh);
    assert!(rx >= bx && rx + rw <= bx + bw && rw <= 700.0, "the row runs past the window: {rx} + {rw}");
    assert_eq!(rh, 28.0, "the row is one line, cut short with an ellipsis, not wrapped");
    let mut up = row;
    while let Some(p) = app.parents.get(&up) {
        assert_ne!(*p, bar, "the reason is in the bar again");
        up = *p;
    }
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
        .child(view().label("close menu").w(px(40.0)).h(px(20.0)).on_press(|| crate::live::store().ctx.set(None)))
        .child(dynamic(move || crate::computer::body_of(on.get()).into_element()))
        .child(crate::ctx::layer())
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
    app.right_click("Open computer", 20.0, 20.0);
    assert!(shows(&mut app, "Sleep") && !shows(&mut app, "'s job"), "{:?}", app.texts());
    app.press("close menu");

    app.press("a job");
    assert!(shows(&mut app, "Ann's job \"the build\" is running · 3 h 59 min left"), "{:?}", app.texts());
    app.right_click("Open computer", 20.0, 20.0);
    assert!(shows(&mut app, "Sleep, and freeze its job"), "{:?}", app.texts());
    app.press("close menu");
    // pick 1B: a job's state and nothing of what the Bot can be counted on for
    for promise in ["always", "keeps working", "while this Mac", "isn't a server"] {
        assert!(!app.texts().iter().any(|t| t.to_lowercase().contains(promise)), "{promise}: {:?}", app.texts());
    }

    app.press("asleep");
    assert!(shows(&mut app, "frozen while this computer sleeps"), "{:?}", app.texts());

    app.press("over");
    app.right_click("Open computer", 20.0, 20.0);
    assert!(!shows(&mut app, "'s job") && !shows(&mut app, "freeze") && shows(&mut app, "Sleep"), "{:?}", app.texts());
}

/// A Bot, what a poll says of it, and the screen's picture.
type Said = (String, Status, Option<String>);
/// What the `say` button puts on the pane, and the row the `pick` button picks.
static NEXT: std::sync::Mutex<(Option<Said>, String)> = std::sync::Mutex::new((None, String::new()));

const PICTURE: &str = "data:image/png;base64,iVBORw0KGgo=";

#[component]
fn Live() -> impl IntoElement {
    let st = crate::live::store();
    view()
        .size_full()
        .child(view().label("on b1").w(px(40.0)).h(px(20.0)).on_press(|| crate::computer::pane_for("b1")))
        .child(view().label("on b2").w(px(40.0)).h(px(20.0)).on_press(|| crate::computer::pane_for("b2")))
        .child(view().label("say").w(px(40.0)).h(px(20.0)).on_press(|| {
            if let Some((bot, status, tile)) = NEXT.lock().unwrap().0.take() {
                crate::computer::pane_says(&bot, status, tile);
            }
        }))
        .child(view().label("pick").w(px(40.0)).h(px(20.0)).on_press(move || {
            let label = NEXT.lock().unwrap().1.clone();
            if let Some(i) = st.ctx.get_untracked().and_then(|m| m.items.into_iter().find(|i| i.label == label)) {
                crate::ctx::act(&i);
            }
        }))
        .child(view().label("close menu").w(px(40.0)).h(px(20.0)).on_press(move || st.ctx.set(None)))
        .child(view().label("back").w(px(40.0)).h(px(20.0)).on_press(move || st.mini_full.set(Vec::new())))
        .child(dynamic(|| crate::computer::pane_body().into_element()))
        .child(crate::ctx::layer())
        .child(text(move || {
            let rows = st.ctx.with(|m| m.as_ref().map(|m| m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>().join("+")));
            let open = st.mini_full.with(|v| v.iter().map(|m| format!("{} {}", m.key, m.title)).collect::<Vec<_>>().join("+"));
            format!("probe ready=1 menu=[{}] open=[{open}]", rows.unwrap_or_else(|| "shut".to_owned()))
        }).text_size(px(8.0)))
}

fn on_the_tab() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Live>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(600.0));
    let mut app = run(config);
    app.press("on b1");
    app
}

fn put(app: &mut Headless, bot: &str, status: Status, tile: Option<&str>) {
    NEXT.lock().unwrap().0 = Some((bot.to_owned(), status, tile.map(str::to_owned)));
    app.press("say");
    app.pump(Duration::from_millis(60));
}

fn pick(app: &mut Headless, row: &str) {
    NEXT.lock().unwrap().1 = row.to_owned();
    app.press("pick");
    app.pump(Duration::from_millis(60));
}

fn in_state(state: &str) -> Status {
    Status { state: state.to_owned(), ..Status::default() }
}

fn sent() -> Vec<String> {
    std::mem::take(&mut crate::computer::CANNED.lock().unwrap().1).into_iter().map(|(path, _)| path).collect()
}

#[test]
fn the_tab_has_no_open_or_sleep_button_and_a_right_click_has_them() {
    if !in_child("the_tab_has_no_open_or_sleep_button_and_a_right_click_has_them") {
        return;
    }
    let mut app = on_the_tab();
    put(&mut app, "b1", in_state("awake"), Some(PICTURE));
    assert!(shows(&mut app, "menu=[shut]"), "{:?}", app.texts());
    assert!(!app.has("Open") && !app.has("Sleep"), "the buttons are gone");
    assert!(!app.texts().iter().any(|t| *t == "Open" || t.starts_with("Sleep")), "{:?}", app.texts());

    app.right_click("Open computer", 20.0, 20.0);
    assert!(shows(&mut app, "menu=[Open+Sleep]"), "{:?}", app.texts());
    assert!(app.texts().contains(&"Open") && app.texts().contains(&"Sleep"), "the card draws them: {:?}", app.texts());
    pick(&mut app, "Open");
    assert!(shows(&mut app, "open=[computer:b1 Ann's computer]") && shows(&mut app, "menu=[shut]"), "{:?}", app.texts());

    app.right_click("Open computer", 20.0, 20.0);
    pick(&mut app, "Sleep");
    assert!(shows(&mut app, "menu=[shut]"), "{:?}", app.texts());
    wait_until_sent(&mut app);
    assert_eq!(sent(), ["/v1/host/computer/bot/b1/sleep"]);
}

fn wait_until_sent(app: &mut Headless) {
    for _ in 0..100 {
        if !crate::computer::CANNED.lock().unwrap().1.is_empty() {
            return;
        }
        app.pump(Duration::from_millis(20));
    }
    panic!("nothing reached the broker");
}

#[test]
fn the_menu_offers_what_the_state_allows() {
    if !in_child("the_menu_offers_what_the_state_allows") {
        return;
    }
    let mut app = on_the_tab();
    let remote = |screen, state: &str| Status { remote: Some(crate::computer::Remote { screen, place: "me@h".to_owned() }), ..in_state(state) };

    // an SSH host has no screen to open, and a remote's sleep is its disconnect
    put(&mut app, "b1", remote(false, "awake"), None);
    app.right_click("Wake computer", 20.0, 20.0);
    assert!(shows(&mut app, "menu=[Disconnect]"), "{:?}", app.texts());
    app.press("close menu");

    // what sits beside the pair stays
    let shared = Status { bots: vec![("b1".to_owned(), "Ann".to_owned()), ("b2".to_owned(), "Bea".to_owned())], ..remote(true, "awake") };
    put(&mut app, "b1", shared, None);
    assert!(shows(&mut app, "Leave this computer") && shows(&mut app, "Remove"), "{:?}", app.texts());

    for state in ["asleep", "absent", "error"] {
        put(&mut app, "b1", in_state(state), None);
        app.right_click("Wake computer", 20.0, 20.0);
        assert!(shows(&mut app, "menu=[Wake computer]"), "{state}: {:?}", app.texts());
        pick(&mut app, "Wake computer");
        wait_until_sent(&mut app);
        assert_eq!(sent(), ["/v1/host/computer/bot/b1/wake"], "{state}");
    }

    // nothing to ask of a computer that's coming up, or of one not heard from yet
    for state in ["waking", ""] {
        put(&mut app, "b1", in_state(state), None);
        app.right_click("Wake computer", 20.0, 20.0);
        assert!(shows(&mut app, "menu=[shut]"), "{state}: {:?}", app.texts());
    }
}

#[test]
fn a_left_click_on_the_computer_opens_it() {
    if !in_child("a_left_click_on_the_computer_opens_it") {
        return;
    }
    let mut app = on_the_tab();
    put(&mut app, "b1", in_state("awake"), Some(PICTURE));
    app.press("Open computer");
    assert!(shows(&mut app, "open=[computer:b1 Ann's computer]"), "{:?}", app.texts());

    // with Open gone, the tile is what opens a computer whose first picture hasn't come
    app.press("back");
    put(&mut app, "b1", in_state("awake"), None);
    assert!(shows(&mut app, "open=[]"), "{:?}", app.texts());
    app.press("Open computer");
    assert!(shows(&mut app, "open=[computer:b1 Ann's computer]"), "{:?}", app.texts());
}

#[test]
fn woke_in_goes_ten_seconds_after_it_shows_and_a_new_wake_shows_it_again() {
    if !in_child("woke_in_goes_ten_seconds_after_it_shows_and_a_new_wake_shows_it_again") {
        return;
    }
    let woke = |ms| Status { wake_ms: Some(ms), ..in_state("awake") };
    let mut app = on_the_tab();
    put(&mut app, "b1", woke(1200), None);
    assert!(shows(&mut app, "Woke in 1.2 s"), "{:?}", app.texts());

    // a poll says the same thing again, and that's no new wake
    app.pump(Duration::from_secs(5));
    put(&mut app, "b1", woke(1200), None);
    app.pump(Duration::from_millis(4000));
    assert!(shows(&mut app, "Woke in 1.2 s"), "still there at 9 seconds: {:?}", app.texts());
    app.pump(Duration::from_millis(1200));
    assert!(!shows(&mut app, "Woke in"), "gone at 10: {:?}", app.texts());
    put(&mut app, "b1", woke(1200), None);
    assert!(!shows(&mut app, "Woke in"), "the broker still says it, and it stays gone: {:?}", app.texts());

    // moving to another Bot and back is no new wake either
    app.press("on b2");
    app.pump(Duration::from_millis(60));
    app.press("on b1");
    put(&mut app, "b1", woke(1200), None);
    assert!(!shows(&mut app, "Woke in"), "{:?}", app.texts());

    // a sleep and a wake that took just as long is
    put(&mut app, "b1", in_state("asleep"), None);
    put(&mut app, "b1", woke(1200), None);
    assert!(shows(&mut app, "Woke in 1.2 s"), "{:?}", app.texts());
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

#[test]
fn with_no_computer_the_tab_says_where_the_next_one_is_made() {
    if !in_child("with_no_computer_the_tab_says_where_the_next_one_is_made") {
        return;
    }
    let home = Some(crate::homes::Next { kind: "e2b".to_owned(), label: "E2B".to_owned(), source: "bot".to_owned() });
    let parsed = Status::parse(&json!({"state": "absent", "home": {"kind": "e2b", "label": "E2B", "source": "bot"}}));
    assert_eq!(parsed.home, home);
    let mut app = on_the_tab();
    put(&mut app, "b1", Status { home, ..in_state("absent") }, None);
    assert!(shows(&mut app, "New computers for this Bot: E2B"), "{:?}", app.texts());
}

#[test]
fn set_up_a_computer_opens_the_pick_a_home_card_in_the_tab() {
    if !in_child("set_up_a_computer_opens_the_pick_a_home_card_in_the_tab") {
        return;
    }
    let mut config = EngineConfig::new(App::Rust(Root::new::<RemoteForm>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(900.0), px(700.0));
    let mut form = run(config);
    form.press("on b1");
    assert!(form.has("Connect a remote computer"));
    form.press("Set up a computer");
    wait_for(&mut form, "Where should a new computer be made?");
    assert!(form.has("Docker on this Mac") && form.has("A remote computer"), "{:?}", form.texts());
}
