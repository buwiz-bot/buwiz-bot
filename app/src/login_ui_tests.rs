//! The broker's login card and the person-only commands, through the real card and composer on a
//! headless engine, against a stand-in broker that records what reaches it.
//!
//! Each test runs in a child of its own (the store and the vault are one per process), with its
//! own home, data folder and host token, and the vault's test store: `BUWIZ_NO_KEYCHAIN=1` and
//! `BUWIZ_KEYSTORE=file:…`, so nothing here can reach the Keychain. The password is a fake marker.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::element::Root;
use nsh_core::platform::{FetchEvent, HttpBody, HttpHead, HttpRequest};
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvents, FetchTask, Network};
use serde_json::{Value, json};

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;

const CHILD: &str = "BUWIZ_LOGIN_UI_TEST";
const TOKEN: &str = "login-ui-test-host-token";
/// Stands for a password in these tests; it must reach only the submit's body.
const MARK: &str = "fake-secret-marker-41c2";
const SUBMIT: &str = "/v1/host/computer/login/submit";
const DISMISS: &str = "/v1/host/computer/login/dismiss";
const SAVED: &str = "/v1/host/computer/login/saved";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let dir = std::env::temp_dir().join(format!("buwiz-login-ui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("host-token"), TOKEN).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("login_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .env("HOME", &dir)
        .env("BUWIZ_DATA_DIR", dir.join("data"))
        .env("BUWIZ_HOST_TOKEN_FILE", dir.join("host-token"))
        .env("BUWIZ_NO_KEYCHAIN", "1")
        .env("BUWIZ_KEYSTORE", format!("file:{}", dir.join("keys").display()))
        .env("BUWIZ_TEST_OWNER", "verified")
        .output()
        .expect("run the test's child");
    let _ = std::fs::remove_dir_all(&dir);
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    assert!(!log.contains(MARK), "the secret is in {name}'s output");
    false
}

/// A stand-in broker: every request's path and body; its answers to the login routes as set.
struct Broker {
    log: Mutex<Vec<(String, Value, bool)>>,
    submit: Mutex<(u16, Value)>,
    saved: Mutex<(u16, Value)>,
}

struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

impl Network for Broker {
    fn fetch(&self, request: HttpRequest, mut events: FetchEvents) -> Box<dyn FetchTask> {
        let path = request.url.strip_prefix(crate::client::backend()).unwrap_or(&request.url).to_owned();
        let body = match &request.body {
            HttpBody::Bytes(b) => serde_json::from_slice(b).unwrap_or(Value::Null),
            _ => Value::Null,
        };
        let token = request.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("x-buwiz-host") && v == TOKEN);
        self.log.lock().unwrap().push((path.clone(), body.clone(), token));
        let (status, answer) = match path.as_str() {
            "/v1/host/prove" => (200, json!({"ok": true, "proof": buwiz_protocol::proof::broker_proof(TOKEN, body["nonce"].as_str().unwrap_or_default())})),
            SUBMIT => self.submit.lock().unwrap().clone(),
            SAVED => self.saved.lock().unwrap().clone(),
            DISMISS => (200, json!({"ok": true, "notice": {"kind": "user_form", "id": body["id"], "state": if body["mode"] == "escalated" { "escalated" } else { "dismissed" }}})),
            "/threads" => (200, threads()),
            p if p.ends_with("/session") => (200, json!({"ok": true, "op": body["op"], "name": body["name"], "cookies": 2, "origins": 1, "pending": 0})),
            _ => (200, json!({"ok": true})),
        };
        events(FetchEvent::Head(HttpHead { status, ..Default::default() }));
        events(FetchEvent::Data(answer.to_string().into_bytes()));
        events(FetchEvent::End);
        Box::new(Done)
    }
}

fn threads() -> Value {
    json!({"list": [{"id": "t_nova", "title": "Nova", "bot": {"id": "nova", "name": "Nova", "shape": "blob", "color": "#1884FC"}}],
        "bots": [{"id": "nova", "name": "Nova", "shape": "blob", "color": "#1884FC"}]})
}

fn submitted(state: &str) -> (u16, Value) {
    (200, json!({"ok": true, "notice": {"kind": "user_form", "id": "uf_1", "state": state, "outcomes": [{"id": "email", "filled": true}, {"id": "password", "filled": true}]}}))
}

fn start(submit: (u16, Value)) -> (Headless, Arc<Broker>) {
    let seen = Arc::new(Broker { log: Mutex::new(Vec::new()), submit: Mutex::new(submit), saved: Mutex::new((200, json!({"ok": true, "usable": true}))) });
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(900.0));
    config.network = Some(seen.clone());
    let mut app = run(config);
    settle(&mut app);
    (app, seen)
}

fn heard(seen: &Broker, path: &str) -> Vec<(Value, bool)> {
    seen.log.lock().unwrap().iter().filter(|(p, _, _)| p == path).map(|(_, b, t)| (b.clone(), *t)).collect()
}

fn wait_for(app: &mut Headless, seen: &Broker, path: &str, n: usize) -> Vec<(Value, bool)> {
    for _ in 0..200 {
        app.pump(Duration::from_millis(20));
        let got = heard(seen, path);
        if got.len() >= n {
            return got;
        }
    }
    panic!("{path} not heard {n} times: {:?}", seen.log.lock().unwrap().iter().map(|(p, _, _)| p.clone()).collect::<Vec<_>>());
}

fn settle(app: &mut Headless) {
    for _ in 0..25 {
        app.pump(Duration::from_millis(20));
    }
}

static JOB: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

fn on<R: Send + 'static>(app: &mut Headless, f: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = std::sync::mpsc::channel();
    *JOB.lock().unwrap() = Some(Box::new(move || {
        let _ = tx.send(f());
    }));
    app.press("job");
    for _ in 0..500 {
        if let Ok(r) = rx.try_recv() {
            return r;
        }
        app.pump(Duration::from_millis(5));
    }
    panic!("the job never ran");
}

/// Nova's chat open, with the broker's login card in it.
fn with_card(app: &mut Headless) {
    on(app, || {
        let st = store();
        st.mcp_loaded.set(true);
        st.set_threads(threads());
        st.active_id.set("t_nova".to_owned());
        st.turns_of.set("t_nova".to_owned());
        st.add_local_wire_turn(&json!({"role": "notice", "text": "Sign in to Shop", "notice": {
            "kind": "user_form", "id": "uf_1", "bot": "nova", "botName": "Nova", "chat": "t_nova", "title": "Sign in to Shop", "state": "pending",
            "takeoverNote": "You'd use its own browser on its own screen.",
            "form": {"title": "Sign in to Shop", "domain": "shop.example", "samePage": true, "submit": true, "fields": [
                {"id": "email", "label": "Email", "type": "email", "required": true},
                {"id": "password", "label": "Password", "type": "password", "required": true, "secret": true}]}}}));
    });
    settle(app);
}

#[component]
fn Stage() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view()
        .size_full()
        .child(text("probe ready=1"))
        .child(crate::toast::layer())
        .child(view().label("job").w(px(40.0)).h(px(24.0)).on_press(|| {
            if let Some(job) = JOB.lock().unwrap().take() {
                job();
            }
        }))
        .child(crate::mini_v1::ui::suggest_chips(vec![
            ("Chip computer".to_owned(), "/computer session save shop".to_owned()),
            ("Chip takeover".to_owned(), "/computer takeover".to_owned()),
            ("Chip messages".to_owned(), "/messages none".to_owned()),
            ("Chip vault".to_owned(), "/vault add shop.example".to_owned()),
            ("Chip network".to_owned(), "/network allow shop.example".to_owned()),
        ]))
        .child(dynamic(|| {
            let turns: Vec<_> = store().turns.get().into_iter().filter(|t| t.user_form.is_some()).collect();
            view().children(turns.into_iter().map(|t| crate::chat::turn_row(t, "t_nova").into_element())).into_element()
        }))
}

fn typed_login(app: &mut Headless) {
    app.type_text("Email", "kid@example.com");
    app.type_text("Password", MARK);
}

fn has_text(app: &Headless, words: &str) -> bool {
    app.texts().iter().any(|t| t.contains(words))
}

fn pending(app: &mut Headless) -> bool {
    on(app, || crate::vault::vault().has_pending("uf_1"))
}

/// The card's Log in sends the typed login to the broker's own route, after the broker proves
/// itself, with the host token; the card says it was typed in, and the offer to save it shows.
#[test]
fn a_typed_login_goes_to_the_brokers_route_and_the_save_offer_follows() {
    if !in_child("a_typed_login_goes_to_the_brokers_route_and_the_save_offer_follows") {
        return;
    }
    assert_eq!(crate::vault::keychain::kind(), crate::vault::keychain::Kind::Memory, "the test store, never the Keychain");
    let mut reply = submitted("submitted");
    reply.1["offerSave"] = json!({"origin": "shop.example", "username": "kid@example.com"});
    let (mut app, seen) = start(reply);
    with_card(&mut app);
    assert!(has_text(&app, "Sign in to Shop") && app.has("Open the screen"), "{:?}", app.texts());
    typed_login(&mut app);
    app.press("Log in");
    let sent = wait_for(&mut app, &seen, SUBMIT, 1);
    let (body, token) = &sent[0];
    assert!(token, "with the host token");
    assert_eq!(body, &json!({"chat": "t_nova", "id": "uf_1", "values": {"email": "kid@example.com", "password": MARK}}));
    let order: Vec<String> = seen.log.lock().unwrap().iter().map(|(p, _, _)| p.clone()).filter(|p| p.starts_with("/v1/host/prove") || p == SUBMIT).collect();
    assert_eq!(order, ["/v1/host/prove", SUBMIT], "the broker proves itself first");
    settle(&mut app);
    assert!(has_text(&app, "Typed into the page"), "{:?}", app.texts());
    assert!(has_text(&app, "Save login for shop.example as kid@example.com?"), "{:?}", app.texts());
    assert!(pending(&mut app), "held for the offer");
    app.press("Not now");
    settle(&mut app);
    assert!(!pending(&mut app), "dropped with Not now");
    assert!(!app.texts().iter().any(|t| t.contains(MARK)));
}

/// With no offer from the broker, the typed login isn't kept.
#[test]
fn a_typed_login_is_not_kept_when_no_offer_comes() {
    if !in_child("a_typed_login_is_not_kept_when_no_offer_comes") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    with_card(&mut app);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, "Submitted"), "{:?}", app.texts());
    assert!(!pending(&mut app), "nothing held past a card that settled with no offer");
    assert!(!has_text(&app, "Save login for"));
}

/// A refusal and a missing route show as failures: the card stays open, never "Submitted", and
/// the typed login isn't kept.
#[test]
fn a_refused_or_missing_route_is_a_failure_and_not_submitted() {
    if !in_child("a_refused_or_missing_route_is_a_failure_and_not_submitted") {
        return;
    }
    let wrong = "The page in front is evil.test, not shop.example, so nothing was typed.";
    let (mut app, seen) = start((403, json!({"ok": false, "code": "wrong-page", "message": wrong})));
    with_card(&mut app);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, wrong), "{:?}", app.texts());
    assert!(!has_text(&app, "Submitted") && app.has("Log in"), "still open: {:?}", app.texts());
    assert!(!pending(&mut app));

    // an older broker: its /v1/host/ has no such route
    *seen.submit.lock().unwrap() = (404, json!({"ok": false, "code": "not_found", "message": "no route /v1/host/computer/login/submit"}));
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 2);
    settle(&mut app);
    assert!(has_text(&app, "nothing was typed"), "{:?}", app.texts());
    assert!(!has_text(&app, "Submitted") && app.has("Log in"), "{:?}", app.texts());
    assert!(!pending(&mut app));
}

/// Dismiss and Open the screen answer the broker's dismiss route, and carry no value.
#[test]
fn dismiss_and_open_the_screen_go_to_the_brokers_route() {
    if !in_child("dismiss_and_open_the_screen_go_to_the_brokers_route") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    with_card(&mut app);
    app.type_text("Password", MARK);
    app.press("Open the screen");
    let sent = wait_for(&mut app, &seen, DISMISS, 1);
    assert_eq!(sent[0].0, json!({"chat": "t_nova", "id": "uf_1", "mode": "escalated"}));
    assert!(sent[0].1, "with the host token");
    settle(&mut app);
    assert!(has_text(&app, "On the computer"), "{:?}", app.texts());
    assert!(heard(&seen, SUBMIT).is_empty());
    assert!(!seen.log.lock().unwrap().iter().any(|(_, b, _)| b.to_string().contains(MARK)), "nothing typed left the app");
}

/// The saved-login picker: the broker is asked first, then Touch ID (the test store's gate), and
/// the saved password goes only into the submit. On a computer several Bots share it's refused
/// before any fingerprint, and nothing is sent.
#[test]
fn a_saved_login_is_asked_about_before_touch_id_and_refused_on_a_shared_computer() {
    if !in_child("a_saved_login_is_asked_about_before_touch_id_and_refused_on_a_shared_computer") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    on(&mut app, || crate::vault::vault().save("shop.example", "kid@example.com", crate::vault::KIND_PASSWORD, &crate::vault::Secret::new(MARK)).unwrap());
    *seen.saved.lock().unwrap() = (200, json!({"ok": true, "usable": false, "code": "shared-computer", "message": "This computer is shared with other bots or people, so a saved login is not used on it."}));
    with_card(&mut app);
    app.press("Use");
    wait_for(&mut app, &seen, SAVED, 1);
    settle(&mut app);
    assert!(has_text(&app, "This computer is shared"), "{:?}", app.texts());
    assert!(heard(&seen, SUBMIT).is_empty());

    *seen.saved.lock().unwrap() = (200, json!({"ok": true, "usable": true}));
    app.press("Type it by hand");
    settle(&mut app);
    app.press("Use a saved login");
    settle(&mut app);
    app.press("Use");
    wait_for(&mut app, &seen, SAVED, 2);
    settle(&mut app);
    app.press("Log in");
    let sent = wait_for(&mut app, &seen, SUBMIT, 1);
    assert_eq!(sent[0].0["values"], json!({"email": "kid@example.com", "password": MARK}));
    assert_eq!(sent[0].0["savedLogin"], true);
    settle(&mut app);
    assert!(!app.texts().iter().any(|t| t.contains(MARK)), "never painted");
    assert!(!has_text(&app, "Save login for"), "a saved login isn't offered again");
}

/// A chip's or a card's words that start with a built-in (or a name kept for one) run nothing:
/// they go to the AI as words. The same line typed by the person runs it.
#[test]
fn a_chip_or_a_card_never_runs_a_builtin_and_typed_it_does() {
    if !in_child("a_chip_or_a_card_never_runs_a_builtin_and_typed_it_does") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    with_card(&mut app);
    for (n, (chip, words)) in [("Chip computer", "/computer session save shop"), ("Chip takeover", "/computer takeover"), ("Chip messages", "/messages none"), ("Chip vault", "/vault add shop.example"), ("Chip network", "/network allow shop.example")].into_iter().enumerate() {
        app.press(chip);
        let chat = wait_for(&mut app, &seen, "/chat", n + 1);
        assert_eq!(chat[n].0["prompt"], words, "{chip}");
    }
    on(&mut app, || {
        let spec = crate::cards::choice::FormSpec::from_value(&json!({"title": "/computer session load shop", "fields": [{"id": "q", "label": "Pick", "options": ["Yes", "No"]}]})).expect("a form");
        crate::cards::choice::choose("k", &spec, 0, 0);
    });
    let chat = wait_for(&mut app, &seen, "/chat", 6);
    assert_eq!(chat[5].0["prompt"], "/computer session load shop\nPick: Yes");
    settle(&mut app);
    let ran: Vec<String> = seen.log.lock().unwrap().iter().map(|(p, _, _)| p.clone()).filter(|p| p.contains("/session") || p.contains("/accepts")).collect();
    assert!(ran.is_empty(), "a chip or a card ran a command: {ran:?}");
    assert!(on(&mut app, || !crate::computer::wants_to_drive("nova")), "no take-over from a chip");

    // the person's own Enter
    on(&mut app, || {
        store().draft.set("/computer session save shop".to_owned());
        store().send();
    });
    let sent = wait_for(&mut app, &seen, "/v1/host/computer/bot/nova/session", 1);
    assert_eq!(sent[0].0, json!({"op": "save", "name": "shop"}));
    settle(&mut app);
    assert!(has_text(&app, "Saved Nova's browser session as shop"), "{:?}", app.texts());
    on(&mut app, || {
        store().draft.set("/computer takeover".to_owned());
        store().send();
    });
    settle(&mut app);
    assert!(on(&mut app, || crate::computer::wants_to_drive("nova")), "typed, it takes over");
    assert_eq!(heard(&seen, "/chat").len(), 6, "neither went to the AI");
}
