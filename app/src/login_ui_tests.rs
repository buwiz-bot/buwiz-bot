//! The broker's login card and the person-only commands, through the real card and composer on a
//! headless engine, against a stand-in broker that records what reaches it.
//!
//! Each test runs in a child of its own (the store and the vault are one per process), with its
//! own home, data folder and host token, and the vault's test store: `BUWIZ_NO_KEYCHAIN=1` and
//! `BUWIZ_KEYSTORE=file:…`, so nothing here can reach the Keychain (each test that touches the
//! vault asserts its store is the test store). The password is a fake marker, and every test's
//! output is scanned for it.

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
    /// it answers the proof as a stranger on the broker's port would: with another token's
    impostor: std::sync::atomic::AtomicBool,
    /// what the dismiss route answers instead of its own settled card
    dismiss: Mutex<Option<(u16, Value)>>,
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
            "/v1/host/prove" => {
                let key = if self.impostor.load(std::sync::atomic::Ordering::SeqCst) { "not-the-host-token" } else { TOKEN };
                (200, json!({"ok": true, "proof": buwiz_protocol::proof::broker_proof(key, body["nonce"].as_str().unwrap_or_default())}))
            }
            SUBMIT => self.submit.lock().unwrap().clone(),
            SAVED => self.saved.lock().unwrap().clone(),
            DISMISS if self.dismiss.lock().unwrap().is_some() => self.dismiss.lock().unwrap().clone().unwrap(),
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
        "bots": [{"id": "nova", "name": "Nova", "shape": "blob", "color": "#1884FC"}, {"id": "orbit", "name": "Orbit", "shape": "blob", "color": "#FC8418"}]})
}

fn submitted(state: &str) -> (u16, Value) {
    (200, json!({"ok": true, "notice": {"kind": "user_form", "id": "uf_1", "state": state, "outcomes": [{"id": "email", "filled": true}, {"id": "password", "filled": true}]}}))
}

fn start(submit: (u16, Value)) -> (Headless, Arc<Broker>) {
    let seen = Arc::new(Broker { log: Mutex::new(Vec::new()), submit: Mutex::new(submit), saved: Mutex::new((200, json!({"ok": true, "usable": true}))), impostor: Default::default(), dismiss: Mutex::new(None) });
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
    with_card_as(app, "uf_1");
}

/// [`with_card`], the card's id `id`.
fn with_card_as(app: &mut Headless, id: &'static str) {
    with_card_for(app, id, Some("nova"));
}

/// [`with_card_as`], the card's Bot id `bot` (none: a card that names only its Bot's name).
fn with_card_for(app: &mut Headless, id: &'static str, bot: Option<&'static str>) {
    on(app, move || {
        let st = store();
        st.mcp_loaded.set(true);
        st.set_threads(threads());
        st.active_id.set("t_nova".to_owned());
        st.turns_of.set("t_nova".to_owned());
        st.add_local_wire_turn(&json!({"role": "notice", "text": "Sign in to Shop", "notice": {
            "kind": "user_form", "id": id, "bot": bot, "botName": "Nova", "chat": "t_nova", "title": "Sign in to Shop", "state": "pending",
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
            ("Chip unknown".to_owned(), "  /nosuchcmd now".to_owned()),
            ("Chip share all".to_owned(), "/vault share all".to_owned()),
            ("Chip share".to_owned(), "/vault share shop.example with Orbit".to_owned()),
        ]))
        .child(dynamic(|| {
            let turns: Vec<_> = store().turns.get().into_iter().filter(|t| t.user_form.is_some()).collect();
            view().children(turns.into_iter().map(|t| crate::chat::turn_row(t, "t_nova").into_element())).into_element()
        }))
        // Settings › Logins, while Settings is open on it (`/vault` opens it)
        .child(dynamic(|| {
            let st = store();
            if st.settings_open.get() && st.settings_page.get() == "logins" {
                view().w(px(860.0)).h(px(560.0)).child(crate::vault::settings_page()).into_element()
            } else {
                view().into_element()
            }
        }))
}

/// A login for shop.example as `user`, the fake marker its password, shared with `bots`.
fn save_shared(user: &str, bots: &[&str]) -> crate::vault::SiteLogin {
    assert!(matches!(crate::vault::keychain::kind(), crate::vault::keychain::Kind::Memory(_)), "the test store, never the Keychain");
    let v = crate::vault::vault();
    let row = v.save("shop.example", user, crate::vault::KIND_PASSWORD, &crate::vault::Secret::new(MARK)).unwrap();
    for b in bots {
        v.share(&row.id, b, true).unwrap();
    }
    crate::vault::changed();
    v.get(&row.id).unwrap()
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
    assert!(matches!(crate::vault::keychain::kind(), crate::vault::keychain::Kind::Memory(_)), "the test store, never the Keychain");
    let mut reply = submitted("submitted");
    reply.1["offerSave"] = json!({"origin": "shop.example", "username": "kid@example.com"});
    let (mut app, seen) = start(reply);
    with_card(&mut app);
    assert!(has_text(&app, "Sign in to Shop") && app.has("Open the screen"), "{:?}", app.texts());
    assert!(has_text(&app, "Typed into shop.example on Nova's screen, then Return is pressed to send it. Nothing is typed until you press Log in."), "the site and Return, plainly: {:?}", app.texts());
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
    on(&mut app, || save_shared("kid@example.com", &["nova"]));
    *seen.saved.lock().unwrap() = (200, json!({"ok": true, "usable": false, "code": "shared-computer", "message": "This computer is shared with other bots or people, so a saved login is not used on it."}));
    with_card(&mut app);
    app.press("Use kid@example.com");
    wait_for(&mut app, &seen, SAVED, 1);
    settle(&mut app);
    assert!(has_text(&app, "This computer is shared"), "{:?}", app.texts());
    assert!(heard(&seen, SUBMIT).is_empty());

    *seen.saved.lock().unwrap() = (200, json!({"ok": true, "usable": true}));
    app.press("Type it by hand");
    settle(&mut app);
    app.press("Use a saved login");
    settle(&mut app);
    app.press("Use kid@example.com");
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
    for (n, (chip, words)) in [("Chip computer", "/computer session save shop"), ("Chip takeover", "/computer takeover"), ("Chip messages", "/messages none"), ("Chip vault", "/vault add shop.example"), ("Chip network", "/network allow shop.example"), ("Chip unknown", "/nosuchcmd now")].into_iter().enumerate() {
        app.press(chip);
        let chat = wait_for(&mut app, &seen, "/chat", n + 1);
        assert_eq!(chat[n].0["prompt"], words, "{chip}");
    }
    on(&mut app, || {
        let spec = crate::cards::choice::FormSpec::from_value(&json!({"title": "/computer session load shop", "fields": [{"id": "q", "label": "Pick", "options": ["Yes", "No"]}]})).expect("a form");
        crate::cards::choice::choose("k", &spec, 0, 0);
    });
    let chat = wait_for(&mut app, &seen, "/chat", 7);
    assert_eq!(chat[6].0["prompt"], "/computer session load shop\nPick: Yes");
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
    assert_eq!(heard(&seen, "/chat").len(), 7, "neither went to the AI");
}

/// A fill that may have typed something says so, and offers no blind retry.
#[test]
fn a_fill_that_may_have_typed_says_so_and_offers_no_retry() {
    if !in_child("a_fill_that_may_have_typed_says_so_and_offers_no_retry") {
        return;
    }
    let note = "The browser didn't answer in time. A value may have been typed into the page before it stopped: take a screenshot before anything else.";
    let (mut app, seen) = start((200, json!({"ok": true, "notice": {"kind": "user_form", "id": "uf_1", "state": "fill_failed", "uncertain": true, "note": note}})));
    with_card(&mut app);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, "Not filled") && has_text(&app, "may have been typed"), "{:?}", app.texts());
    assert!(!app.has("Try again") && !app.has("Log in"), "no blind retry: {:?}", app.texts());
}

/// A stranger on the broker's port that can't prove it holds the host token gets no token and
/// no card answer: not on Use, Dismiss, Open the screen or Log in.
#[test]
fn an_unproven_broker_gets_no_token_from_any_card_button() {
    if !in_child("an_unproven_broker_gets_no_token_from_any_card_button") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    seen.impostor.store(true, std::sync::atomic::Ordering::SeqCst);
    on(&mut app, || save_shared("kid@example.com", &["nova"]));
    with_card(&mut app);
    app.press("Use kid@example.com");
    wait_for(&mut app, &seen, "/v1/host/prove", 1);
    settle(&mut app);
    assert!(has_text(&app, "so no saved login was used"), "{:?}", app.texts());
    app.press("Type it by hand");
    settle(&mut app);
    app.press("Open the screen");
    wait_for(&mut app, &seen, "/v1/host/prove", 2);
    settle(&mut app);
    assert!(has_text(&app, "so nothing was sent"), "{:?}", app.texts());
    app.press("Dismiss");
    wait_for(&mut app, &seen, "/v1/host/prove", 3);
    settle(&mut app);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, "/v1/host/prove", 4);
    settle(&mut app);
    let log = seen.log.lock().unwrap();
    let card_calls: Vec<&String> = log.iter().map(|(p, _, _)| p).filter(|p| p.starts_with("/v1/host/computer/login/")).collect();
    assert!(card_calls.is_empty(), "a card call went to an unproven broker: {card_calls:?}");
    assert!(!log.iter().any(|(p, _, token)| *token && p.starts_with("/v1/host/prove")), "no token on a proof");
    assert!(!log.iter().any(|(p, _, token)| *token && (p.starts_with("/v1/host/prove") || p.starts_with("/v1/host/computer/login/"))));
    assert!(!log.iter().any(|(_, b, _)| b.to_string().contains(MARK)), "the password stayed in the app");
}

/// An answer that can't say how the fill went never reads as "nothing was filled": a submit past
/// the broker's patience says to check the screen, and a dismiss that came after the login was
/// typed paints what was typed.
#[test]
fn a_late_or_unsure_answer_paints_what_really_happened() {
    if !in_child("a_late_or_unsure_answer_paints_what_really_happened") {
        return;
    }
    let (mut app, seen) = start((504, json!({"ok": false, "code": "timeout", "message": "It's still being typed.", "notice": {"kind": "user_form", "id": "uf_1", "state": "pending"}})));
    with_card(&mut app);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, "Check the screen") && !app.has("Log in"), "no second answer: {:?}", app.texts());
    assert!(!has_text(&app, "Nothing was filled") && !has_text(&app, "Dismissed"), "{:?}", app.texts());
}

/// A dismiss that came after the broker typed the login paints what was typed.
#[test]
fn a_dismiss_after_the_login_was_typed_paints_it_typed() {
    if !in_child("a_dismiss_after_the_login_was_typed_paints_it_typed") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    *seen.dismiss.lock().unwrap() = Some((409, json!({"ok": false, "code": "stale", "message": "This card was answered already.", "notice": {"kind": "user_form", "id": "uf_1", "state": "submitted"}})));
    with_card(&mut app);
    app.press("Dismiss");
    wait_for(&mut app, &seen, DISMISS, 1);
    settle(&mut app);
    assert!(has_text(&app, "Submitted") && !has_text(&app, "Dismissed without filling"), "{:?}", app.texts());
}

fn asks(app: &mut Headless) -> usize {
    on(app, crate::vault::owner_asks)
}

/// The person's own Enter on `line`.
fn typed(app: &mut Headless, line: &str) {
    let line = line.to_owned();
    on(app, move || {
        store().draft.set(line);
        store().send();
    });
    settle(app);
}

fn no_mark(app: &Headless) {
    assert!(!app.texts().iter().any(|t| t.contains(MARK)), "a secret is on screen: {:?}", app.texts());
}

/// A card for Nova lists only the logins shared with Nova; Share all offers every one, and only
/// for the card's site. A login saved for another site is never listed.
#[test]
fn a_card_offers_only_the_logins_shared_with_its_bot_and_share_all_offers_every_one() {
    if !in_child("a_card_offers_only_the_logins_shared_with_its_bot_and_share_all_offers_every_one") {
        return;
    }
    let (mut app, _seen) = start(submitted("submitted"));
    on(&mut app, || {
        save_shared("kid@example.com", &["nova"]);
        save_shared("ada@example.com", &["orbit"]);
        save_shared("bob@example.com", &[]);
        let v = crate::vault::vault();
        let other = v.save("bank.example", "eve@example.com", crate::vault::KIND_PASSWORD, &crate::vault::Secret::new(MARK)).unwrap();
        v.share(&other.id, "nova", true).unwrap();
    });
    with_card(&mut app);
    assert!(app.has("Use kid@example.com"), "{:?}", app.texts());
    for who in ["ada@example.com", "bob@example.com", "eve@example.com"] {
        assert!(!app.has(&format!("Use {who}")) && !has_text(&app, who), "{who} isn't Nova's: {:?}", app.texts());
    }
    on(&mut app, || {
        crate::vault::vault().set_share_all(true).unwrap();
        crate::vault::changed();
    });
    settle(&mut app);
    for who in ["kid@example.com", "ada@example.com", "bob@example.com"] {
        assert!(app.has(&format!("Use {who}")), "share all offers {who}: {:?}", app.texts());
    }
    assert!(!has_text(&app, "eve@example.com"), "never another site's: {:?}", app.texts());
    on(&mut app, || {
        crate::vault::vault().set_share_all(false).unwrap();
        crate::vault::changed();
    });
    settle(&mut app);
    assert!(app.has("Use kid@example.com") && !app.has("Use ada@example.com"), "{:?}", app.texts());
    no_mark(&app);
}

/// A login that isn't shared with the card's Bot can't be used by its id: refused in the app
/// before the broker is asked or Touch ID shows, and nothing is sent. A share taken back between
/// the pick and Log in stops the submit too.
#[test]
fn an_unshared_logins_id_is_refused_before_touch_id_and_nothing_is_sent() {
    if !in_child("an_unshared_logins_id_is_refused_before_touch_id_and_nothing_is_sent") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    let orbits = on(&mut app, || save_shared("kid@example.com", &["orbit"]));
    with_card(&mut app);
    assert!(!app.has("Use kid@example.com") && !has_text(&app, "kid@example.com"), "not listed: {:?}", app.texts());
    let before = asks(&mut app);
    // the card names the login by id alone
    let id = orbits.id.clone();
    on(&mut app, move || {
        let target = crate::cards::user_form::CardTarget::Login { username_id: "email".into(), password_id: "password".into() };
        crate::cards::user_form::use_saved("uf_1".into(), &id, target, "shop.example".into(), vec!["shop.example".into()], "nova".into(), "Nova");
    });
    settle(&mut app);
    assert!(has_text(&app, "That saved login isn't shared with Nova, so it wasn't used."), "{:?}", app.texts());
    assert_eq!(asks(&mut app), before, "no Touch ID");
    assert!(heard(&seen, SAVED).is_empty() && heard(&seen, SUBMIT).is_empty(), "nothing sent");
    // a login of the card's Bot, but for another site, by id: refused the same way
    let other = on(&mut app, || {
        let v = crate::vault::vault();
        let r = v.save("bank.example", "eve@example.com", crate::vault::KIND_PASSWORD, &crate::vault::Secret::new(MARK)).unwrap();
        v.share(&r.id, "nova", true).unwrap();
        r.id
    });
    on(&mut app, move || {
        let target = crate::cards::user_form::CardTarget::Login { username_id: "email".into(), password_id: "password".into() };
        crate::cards::user_form::use_saved("uf_1".into(), &other, target, "shop.example".into(), vec!["shop.example".into()], "nova".into(), "Nova");
    });
    settle(&mut app);
    assert!(has_text(&app, "is for another site than this card's"), "{:?}", app.texts());
    assert_eq!(asks(&mut app), before, "no Touch ID");

    // shared with Nova, picked, then the share taken back before Log in: nothing goes
    let id = orbits.id.clone();
    on(&mut app, move || {
        crate::vault::vault().share(&id, "nova", true).unwrap();
        crate::vault::changed();
    });
    settle(&mut app);
    app.press("Use kid@example.com");
    wait_for(&mut app, &seen, SAVED, 1);
    settle(&mut app);
    assert_eq!(asks(&mut app), before + 1);
    let id = orbits.id.clone();
    on(&mut app, move || {
        crate::vault::vault().share(&id, "nova", false).unwrap();
    });
    app.press("Log in");
    settle(&mut app);
    assert!(has_text(&app, "isn't shared with Nova"), "{:?}", app.texts());
    assert!(heard(&seen, SUBMIT).is_empty(), "nothing sent");
    assert!(!seen.log.lock().unwrap().iter().any(|(_, b, _)| b.to_string().contains(MARK)), "the password stayed in the app");
    no_mark(&app);
}

/// Touch ID (the test owner) is asked on every use of a saved login: two picks on one card, and
/// one on the next card, are three asks. Nothing read is kept between them.
#[test]
fn touch_id_is_asked_on_every_use_of_a_saved_login() {
    if !in_child("touch_id_is_asked_on_every_use_of_a_saved_login") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    on(&mut app, || save_shared("kid@example.com", &["nova"]));
    with_card(&mut app);
    let before = asks(&mut app);
    app.press("Use kid@example.com");
    wait_for(&mut app, &seen, SAVED, 1);
    settle(&mut app);
    assert_eq!(asks(&mut app), before + 1);
    app.press("Type it by hand");
    settle(&mut app);
    app.press("Use a saved login");
    settle(&mut app);
    app.press("Use kid@example.com");
    wait_for(&mut app, &seen, SAVED, 2);
    settle(&mut app);
    assert_eq!(asks(&mut app), before + 2, "asked again, not remembered");
    app.press("Log in");
    let sent = wait_for(&mut app, &seen, SUBMIT, 1);
    assert_eq!(sent[0].0["savedLoginId"].as_str().map(|s| s.starts_with("sl_")), Some(true));
    settle(&mut app);
    with_card_as(&mut app, "uf_2");
    app.press("Use kid@example.com");
    wait_for(&mut app, &seen, SAVED, 3);
    settle(&mut app);
    assert_eq!(asks(&mut app), before + 3, "a new use, a new ask");
    no_mark(&app);
}

/// Settings › Logins: Add (from `/vault add`), Edit with its secret fields empty, the share
/// switches and Share all, and Delete. No secret is ever on screen or in a field shown back.
#[test]
fn the_logins_page_adds_edits_shares_and_deletes_and_never_shows_a_secret() {
    if !in_child("the_logins_page_adds_edits_shares_and_deletes_and_never_shows_a_secret") {
        return;
    }
    assert!(matches!(crate::vault::keychain::kind(), crate::vault::keychain::Kind::Memory(_)), "the test store, never the Keychain");
    let (mut app, _seen) = start(submitted("submitted"));
    with_card(&mut app);
    typed(&mut app, "/vault add shop.example");
    app.wait_for("Login site");
    assert_eq!(app.value("Login site"), "shop.example", "the site from the line");
    assert_eq!(app.value("Login password"), "");
    app.type_text("Login user name", "kid@example.com");
    app.type_text("Login password", MARK);
    app.type_text("Login authenticator secret", "JBSWY3DPEHPK3PXP");
    no_mark(&app);
    app.press("Save login");
    settle(&mut app);
    assert!(!app.has("Login password"), "the form closed: {:?}", app.texts());
    assert!(has_text(&app, "kid@example.com") && has_text(&app, "shop.example"), "{:?}", app.texts());
    let row = on(&mut app, || crate::vault::vault().list().pop().unwrap());
    assert!(row.shared_with.is_empty(), "added by hand: shared with no Bot");
    assert!(on(&mut app, move || crate::vault::vault().has_code(&row)), "the authenticator secret is kept");
    assert!(!app.has("Use kid@example.com"), "not offered to Nova yet");
    no_mark(&app);

    // a password typed into the Website field is refused, never saved or shown
    app.press("Add a login");
    app.wait_for("Login site");
    app.type_text("Login site", MARK);
    app.type_text("Login user name", "bob@example.com");
    app.type_text("Login password", "fake-other-pw");
    app.press("Save login");
    settle(&mut app);
    assert!(has_text(&app, "Couldn't save the login: the website doesn't look like a site (like shop.example)."), "{:?}", app.texts());
    assert_eq!(on(&mut app, || crate::vault::vault().list().len()), 1, "nothing saved");
    app.press("Cancel");
    settle(&mut app);
    no_mark(&app);

    // Edit: the secret fields are write-only
    app.press("Edit");
    app.wait_for("Login password");
    assert_eq!(app.value("Login user name"), "kid@example.com");
    assert_eq!(app.value("Login password"), "", "never shown back");
    assert_eq!(app.value("Login authenticator secret"), "", "never shown back");
    app.change("Login user name", "kid2@example.com");
    app.press("Save login");
    settle(&mut app);
    assert!(has_text(&app, "kid2@example.com"), "{:?}", app.texts());
    let kept = on(&mut app, || {
        let v = crate::vault::vault();
        let r = v.list().pop().unwrap();
        v.unlock(&r, "test") == crate::vault::Unlock::Secret(crate::vault::Secret::new(MARK))
    });
    assert!(kept, "an empty password field keeps the saved one");
    no_mark(&app);

    // Share: Nova's switch, then the card offers it; Share all
    assert!(app.has("Share with Nova, off") && app.has("Share with Orbit, off"), "{:?}", app.texts());
    app.press("Share with Nova, off");
    settle(&mut app);
    assert!(app.has("Share with Nova, on") && app.has("Share with Orbit, off"));
    assert!(on(&mut app, || crate::vault::vault().list().pop().unwrap().shared_with.contains("nova")));
    assert!(has_text(&app, crate::vault::SHARE_ALL_MEANS));
    app.press("Share all, off");
    settle(&mut app);
    assert!(app.has("Share all, on") && on(&mut app, || crate::vault::vault().share_all()));
    assert!(has_text(&app, "Share all is on, so every Bot is offered this login."));
    app.press("Share all, on");
    settle(&mut app);
    assert!(app.has("Share all, off") && !on(&mut app, || crate::vault::vault().share_all()));

    // a new site stops the shares, and the form says so before Save
    app.press("Edit");
    app.wait_for("Login site");
    assert!(!has_text(&app, crate::vault::SITE_CHANGE_UNSHARES));
    app.change("Login site", "shop3.example");
    settle(&mut app);
    assert!(has_text(&app, crate::vault::SITE_CHANGE_UNSHARES), "{:?}", app.texts());
    app.press("Save login");
    settle(&mut app);
    assert!(on(&mut app, || crate::vault::vault().list().pop().unwrap().shared_with.is_empty()), "moved: shared with no Bot");
    assert!(app.has("Share with Nova, off"));

    // Delete, pressed twice
    app.press("Delete");
    settle(&mut app);
    app.press("Delete — press again");
    settle(&mut app);
    assert!(has_text(&app, "No saved logins yet"), "{:?}", app.texts());
    assert!(on(&mut app, || crate::vault::vault().list().is_empty()));
    no_mark(&app);
}

/// Each `/vault` line, typed: the page, share and unshare with a Bot, asking which of two logins,
/// share all on and off, and an add line carrying more than a site refused with nothing kept.
/// Deleting a Bot takes its shares.
#[test]
fn each_vault_command_does_what_it_says_and_asks_which_of_two() {
    if !in_child("each_vault_command_does_what_it_says_and_asks_which_of_two") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    let kid = on(&mut app, || save_shared("kid@example.com", &[]));
    with_card(&mut app);
    typed(&mut app, "/vault");
    assert!(on(&mut app, || store().settings_open.get_untracked() && store().settings_page.get_untracked() == "logins"));
    assert!(has_text(&app, "kid@example.com"), "{:?}", app.texts());
    on(&mut app, || store().settings_open.set(false));

    typed(&mut app, "/vault share shop.example with Nova");
    let shared = |app: &mut Headless, bot: &'static str| on(app, move || crate::vault::vault().list().iter().filter(|r| r.shared_with.contains(bot)).map(|r| r.username.clone()).collect::<Vec<_>>());
    assert_eq!(shared(&mut app, "nova"), ["kid@example.com"]);
    assert!(has_text(&app, "Shared kid@example.com on shop.example with Nova."), "{:?}", app.texts());
    assert!(app.has("Use kid@example.com"), "the open card offers it now: {:?}", app.texts());

    // a second login for the site: asked which, nothing changed
    on(&mut app, || save_shared("ada@example.com", &[]));
    typed(&mut app, "/vault share shop.example with Orbit");
    assert!(shared(&mut app, "orbit").is_empty(), "never a guess");
    let (head, fills) = on(&mut app, || {
        let st = store();
        let fills: Vec<String> = st.pop_rows.get_untracked().iter().filter_map(|r| match &r.pick { crate::slash::Pick::Fill(t) => Some(t.clone()), _ => None }).collect();
        (st.pop_head.get_untracked(), fills)
    });
    assert_eq!(head, "2 logins are saved for shop.example: which one?");
    assert_eq!(fills, ["/vault share shop.example as ada@example.com with Orbit", "/vault share shop.example as kid@example.com with Orbit"]);
    typed(&mut app, "/vault share shop.example as ada@example.com with Orbit");
    assert_eq!(shared(&mut app, "orbit"), ["ada@example.com"]);

    typed(&mut app, "/vault unshare shop.example as kid@example.com with Nova");
    assert!(shared(&mut app, "nova").is_empty());
    assert!(!app.has("Use kid@example.com"), "gone from Nova's card: {:?}", app.texts());

    typed(&mut app, "/vault share all");
    assert!(on(&mut app, || crate::vault::vault().share_all()));
    assert!(app.has("Use kid@example.com") && app.has("Use ada@example.com"), "{:?}", app.texts());
    typed(&mut app, "/vault share all off");
    assert!(!on(&mut app, || crate::vault::vault().share_all()));

    // no password from the line, and nothing of it kept or shown
    typed(&mut app, &format!("/vault add shop2.example {MARK}"));
    assert!(has_text(&app, "/vault add takes a site only"), "{:?}", app.texts());
    assert!(!on(&mut app, || store().settings_open.get_untracked()), "no form opened");
    assert!(on(&mut app, || store().draft.get_untracked().is_empty()));
    no_mark(&app);
    // a Bot or user name that matches nothing is never shown back
    typed(&mut app, &format!("/vault share shop.example with {MARK}"));
    assert!(has_text(&app, "No Bot has that name. Bots: Nova, Orbit."), "{:?}", app.texts());
    typed(&mut app, &format!("/vault share shop.example as {MARK} with Nova"));
    assert!(has_text(&app, "No login for shop.example has that user name."), "{:?}", app.texts());
    no_mark(&app);

    // a lone word that isn't a site: refused in fixed words, never in the site field or a toast
    typed(&mut app, &format!("/vault add {MARK}"));
    typed(&mut app, &format!("/vault share {MARK} with Nova"));
    assert!(has_text(&app, "That doesn't look like a site"), "{:?}", app.texts());
    assert!(!on(&mut app, || store().settings_open.get_untracked()), "no form opened");
    no_mark(&app);
    assert!(heard(&seen, "/chat").is_empty(), "none of it went to the AI");

    // a deleted Bot's shares go with it
    on(&mut app, || store().delete_bot("orbit".to_owned()));
    wait_for(&mut app, &seen, "/bot/delete", 1);
    settle(&mut app);
    assert!(shared(&mut app, "orbit").is_empty(), "Orbit's share went with it");
    let _ = kid;
}

/// A chip's `/vault share all` (or any `/vault` line) is words for the AI: it runs nothing.
#[test]
fn a_chip_carrying_a_vault_command_runs_nothing() {
    if !in_child("a_chip_carrying_a_vault_command_runs_nothing") {
        return;
    }
    let (mut app, seen) = start(submitted("submitted"));
    on(&mut app, || save_shared("kid@example.com", &[]));
    with_card(&mut app);
    app.press("Chip share all");
    let chat = wait_for(&mut app, &seen, "/chat", 1);
    assert_eq!(chat[0].0["prompt"], "/vault share all");
    app.press("Chip share");
    let chat = wait_for(&mut app, &seen, "/chat", 2);
    assert_eq!(chat[1].0["prompt"], "/vault share shop.example with Orbit");
    settle(&mut app);
    assert!(!on(&mut app, || crate::vault::vault().share_all()), "share all stays off");
    assert!(on(&mut app, || crate::vault::vault().list().iter().all(|r| r.shared_with.is_empty())), "nothing shared");
    on(&mut app, || {
        let spec = crate::cards::choice::FormSpec::from_value(&json!({"title": "/vault share all", "fields": [{"id": "q", "label": "Pick", "options": ["Yes", "No"]}]})).expect("a form");
        crate::cards::choice::choose("k", &spec, 0, 0);
    });
    wait_for(&mut app, &seen, "/chat", 3);
    settle(&mut app);
    assert!(!on(&mut app, || crate::vault::vault().share_all()), "nor from a choice card");
}

/// "Save this login?" on Nova's card keeps the login for Nova alone: shared with the Bot whose
/// card typed it, and no other.
#[test]
fn a_login_saved_from_a_bots_card_is_shared_with_that_bot_only() {
    if !in_child("a_login_saved_from_a_bots_card_is_shared_with_that_bot_only") {
        return;
    }
    assert!(matches!(crate::vault::keychain::kind(), crate::vault::keychain::Kind::Memory(_)), "the test store, never the Keychain");
    let mut reply = submitted("submitted");
    reply.1["offerSave"] = json!({"origin": "shop.example", "username": "kid@example.com"});
    let (mut app, seen) = start(reply);
    with_card(&mut app);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, "Saved, it's offered to Nova only. Share it with other Bots in Settings › Logins."), "{:?}", app.texts());
    app.press("Save");
    settle(&mut app);
    assert!(has_text(&app, "Saved kid@example.com for shop.example"), "{:?}", app.texts());
    let shares = on(&mut app, || crate::vault::vault().list().into_iter().map(|r| r.shared_with.into_iter().collect::<Vec<_>>()).collect::<Vec<_>>());
    assert_eq!(shares, [["nova".to_owned()]]);
    no_mark(&app);
}

/// What was typed into the Logins form's password never outlives the form: closing Settings, or
/// `/vault` with the form open, clears it, and a Save after keeps the saved password, not the
/// one typed before.
#[test]
fn a_password_typed_into_the_logins_form_goes_when_settings_closes_or_vault_reopens_it() {
    if !in_child("a_password_typed_into_the_logins_form_goes_when_settings_closes_or_vault_reopens_it") {
        return;
    }
    let (mut app, _seen) = start(submitted("submitted"));
    let saved = |app: &mut Headless| on(app, || {
        let v = crate::vault::vault();
        let r = v.list().pop().unwrap();
        v.unlock(&r, "test") == crate::vault::Unlock::Secret(crate::vault::Secret::new("fake-old-pw"))
    });
    on(&mut app, || {
        assert!(matches!(crate::vault::keychain::kind(), crate::vault::keychain::Kind::Memory(_)), "the test store, never the Keychain");
        crate::vault::vault().save("shop.example", "kid@example.com", crate::vault::KIND_PASSWORD, &crate::vault::Secret::new("fake-old-pw")).unwrap();
    });
    with_card(&mut app);
    typed(&mut app, "/vault");
    app.press("Edit");
    app.wait_for("Login password");
    app.type_text("Login password", MARK);
    // Settings closes, and opens again
    on(&mut app, || store().settings_open.set(false));
    settle(&mut app);
    typed(&mut app, "/vault");
    app.press("Edit");
    app.wait_for("Login password");
    assert_eq!(app.value("Login password"), "", "nothing held under an empty field");
    app.press("Save login");
    settle(&mut app);
    assert!(saved(&mut app), "the password typed before Settings closed wasn't saved");

    // `/vault` with the form still open
    app.press("Edit");
    app.wait_for("Login password");
    app.type_text("Login password", MARK);
    typed(&mut app, "/vault");
    assert_eq!(app.value("Login password"), "", "the field and what it holds are one");
    app.press("Save login");
    settle(&mut app);
    assert!(saved(&mut app), "the password typed before /vault wasn't saved");
    no_mark(&app);
}

/// The offer's words are true for a login saved already with another Bot: Nova is added, and
/// Orbit keeps it.
#[test]
fn the_save_offer_says_who_else_keeps_a_login_saved_already() {
    if !in_child("the_save_offer_says_who_else_keeps_a_login_saved_already") {
        return;
    }
    let mut reply = submitted("submitted");
    reply.1["offerSave"] = json!({"origin": "shop.example", "username": "kid@example.com"});
    let (mut app, seen) = start(reply);
    on(&mut app, || save_shared("kid@example.com", &["orbit"]));
    with_card(&mut app);
    assert!(!app.has("Use kid@example.com"), "Orbit's, not Nova's");
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, "Saved, it's offered to Nova, and still to Orbit."), "{:?}", app.texts());
    app.press("Save");
    settle(&mut app);
    let shares = on(&mut app, || crate::vault::vault().list().pop().unwrap().shared_with.into_iter().collect::<Vec<_>>());
    assert_eq!(shares, ["nova", "orbit"]);
    no_mark(&app);
}

/// A card that names its Bot only by name records no share, and says so.
#[test]
fn a_card_with_no_bot_id_saves_a_login_shared_with_no_bot_and_says_so() {
    if !in_child("a_card_with_no_bot_id_saves_a_login_shared_with_no_bot_and_says_so") {
        return;
    }
    let mut reply = submitted("submitted");
    reply.1["offerSave"] = json!({"origin": "shop.example", "username": "kid@example.com"});
    let (mut app, seen) = start(reply);
    with_card_for(&mut app, "uf_1", None);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, "Saved, it isn't shared with any Bot yet. Share it in Settings › Logins."), "{:?}", app.texts());
    assert!(!has_text(&app, "offered to Nova"), "{:?}", app.texts());
    app.press("Save");
    settle(&mut app);
    assert!(on(&mut app, || crate::vault::vault().list().pop().unwrap().shared_with.is_empty()), "no share without an id");
    no_mark(&app);
}

/// A vault change redraws only the card's saved-login part: the field being typed into is the
/// same input, with what was typed in it.
#[test]
fn a_vault_change_leaves_the_cards_fields_and_typing_alone() {
    if !in_child("a_vault_change_leaves_the_cards_fields_and_typing_alone") {
        return;
    }
    let (mut app, _seen) = start(submitted("submitted"));
    with_card(&mut app);
    app.type_text("Email", "kid@exa");
    let (email, password) = (app.view("Email"), app.view("Password"));
    on(&mut app, || save_shared("kid@example.com", &["nova"]));
    settle(&mut app);
    assert!(app.has("Use kid@example.com"), "the strip shows the new login: {:?}", app.texts());
    assert_eq!((app.view("Email"), app.view("Password")), (email, password), "the fields weren't rebuilt");
    assert_eq!(app.value("Email"), "kid@exa", "what was typed stays");
}

/// Each of the card's controls is its own accessible button, named, and Log in says it's disabled
/// until the form can be sent; the trust note is its own text. The picker's Use, and the save
/// offer's Save and Not now, too.
#[test]
fn the_cards_controls_are_accessible_buttons_each_with_its_label() {
    if !in_child("the_cards_controls_are_accessible_buttons_each_with_its_label") {
        return;
    }
    let mut reply = submitted("submitted");
    reply.1["offerSave"] = json!({"origin": "shop.example", "username": "kid@example.com"});
    let (mut app, seen) = start(reply);
    on(&mut app, || save_shared("ada@example.com", &["nova"]));
    with_card(&mut app);
    let button = |app: &Headless, label: &str| app.access(label).0 == AccessibilityRole::Button;
    for label in ["Log in", "Open the screen", "Dismiss", "Use ada@example.com", "Type it by hand"] {
        assert!(button(&app, label), "{label} is a button: {:?}", app.access(label));
    }
    assert!(app.access("Log in").1, "Log in says it's disabled with nothing typed");
    assert!(!app.access("Open the screen").1);
    assert!(app.texts().contains(&"Open the screen: You'd use its own browser on its own screen."), "the note is its own text: {:?}", app.texts());
    assert!(!app.texts().iter().any(|t| t.contains("Log in") && t.contains("Open the screen")), "{:?}", app.texts());
    app.press("Type it by hand");
    settle(&mut app);
    typed_login(&mut app);
    assert!(!app.access("Log in").1, "enabled once filled");
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    for label in ["Save", "Not now"] {
        assert!(button(&app, label), "{label} is a button");
    }
    no_mark(&app);
}

/// A password being typed into the open form stays through Share all and Refresh, both in the
/// field and in what Save keeps.
#[test]
fn a_password_being_typed_stays_through_share_all_and_refresh() {
    if !in_child("a_password_being_typed_stays_through_share_all_and_refresh") {
        return;
    }
    let (mut app, _seen) = start(submitted("submitted"));
    on(&mut app, || {
        assert!(matches!(crate::vault::keychain::kind(), crate::vault::keychain::Kind::Memory(_)), "the test store, never the Keychain");
        crate::vault::vault().save("shop.example", "kid@example.com", crate::vault::KIND_PASSWORD, &crate::vault::Secret::new("fake-old-pw")).unwrap();
    });
    with_card(&mut app);
    typed(&mut app, "/vault");
    app.press("Edit");
    app.wait_for("Login password");
    app.type_text("Login password", MARK);
    app.press("Share all, off");
    settle(&mut app);
    app.press("Refresh");
    settle(&mut app);
    assert!(app.value("Login password") == MARK, "the field still holds what was typed");
    app.press("Save login");
    settle(&mut app);
    let kept = on(&mut app, || {
        let v = crate::vault::vault();
        let r = v.list().pop().unwrap();
        v.unlock(&r, "test") == crate::vault::Unlock::Secret(crate::vault::Secret::new(MARK))
    });
    assert!(kept, "Save kept what the field showed");
    no_mark(&app);
}

/// Opening Settings › Logins, or building it again, reads the vault and says nothing changed: an
/// open card's logins aren't read again for it. A write does.
#[test]
fn opening_the_logins_page_changes_no_revision_and_a_write_does() {
    if !in_child("opening_the_logins_page_changes_no_revision_and_a_write_does") {
        return;
    }
    let (mut app, _seen) = start(submitted("submitted"));
    on(&mut app, || save_shared("kid@example.com", &[]));
    with_card(&mut app);
    let rev = |app: &mut Headless| on(app, || crate::vault::revision().get_untracked());
    let before = rev(&mut app);
    typed(&mut app, "/vault");
    on(&mut app, || store().settings_open.set(false));
    settle(&mut app);
    typed(&mut app, "/vault");
    app.press("Refresh");
    settle(&mut app);
    assert_eq!(rev(&mut app), before, "reads change nothing");
    app.press("Share all, off");
    settle(&mut app);
    assert!(rev(&mut app) > before, "a write does");
}

/// With Share all on, the offer says every Bot is offered it, and who has it when Share all is off.
#[test]
fn the_save_offer_under_share_all_says_every_bot_is_offered_it() {
    if !in_child("the_save_offer_under_share_all_says_every_bot_is_offered_it") {
        return;
    }
    let mut reply = submitted("submitted");
    reply.1["offerSave"] = json!({"origin": "shop.example", "username": "kid@example.com"});
    let (mut app, seen) = start(reply);
    on(&mut app, || crate::vault::vault().set_share_all(true).unwrap());
    with_card(&mut app);
    typed_login(&mut app);
    app.press("Log in");
    wait_for(&mut app, &seen, SUBMIT, 1);
    settle(&mut app);
    assert!(has_text(&app, "Saved, every Bot is offered it while Share all is on; with Share all off, it's offered to Nova only."), "{:?}", app.texts());
    assert!(!has_text(&app, "Share all is on, so every Bot"), "one sentence, not two: {:?}", app.texts());
    no_mark(&app);
}

/// A `credential.offer_save` frame's offer says, and does, what the held login's card decided:
/// shared with that card's Bot, or with none when it named no Bot id.
#[test]
fn a_wire_offer_agrees_with_the_held_login_on_who_gets_it() {
    if !in_child("a_wire_offer_agrees_with_the_held_login_on_who_gets_it") {
        return;
    }
    let (mut app, _seen) = start(submitted("submitted"));
    with_card(&mut app);
    let (words, shared) = on(&mut app, || {
        let v = crate::vault::vault();
        v.stash_pending("e_wire", crate::vault::PendingSave { origin: "shop.example".into(), username: "kid@example.com".into(), password: crate::vault::Secret::new(MARK), bot: "nova".into() });
        let frame = json!({"name": "credential.offer_save", "value": {"origin": "shop.example", "username": "kid@example.com", "formEntryId": "e_wire"}});
        let spec = crate::cards::save_login::SaveLoginSpec::from_wire(&frame).unwrap();
        (crate::cards::save_login::saved_note(&spec), spec)
    });
    assert!(words.contains("Saved, it's offered to Nova only."), "{words}");
    on(&mut app, move || crate::cards::save_login::save(&shared, "save-login-e_wire"));
    settle(&mut app);
    assert!(on(&mut app, || crate::vault::vault().list().pop().unwrap().shared_with.contains("nova")), "and the save shares it with Nova");
    // a held login with no Bot id: none, said plainly
    let words = on(&mut app, || {
        crate::vault::vault().stash_pending("e_none", crate::vault::PendingSave { origin: "bank.example".into(), username: "kid@example.com".into(), password: crate::vault::Secret::new(MARK), bot: String::new() });
        let frame = json!({"name": "credential.offer_save", "value": {"origin": "bank.example", "username": "kid@example.com", "formEntryId": "e_none"}});
        crate::cards::save_login::saved_note(&crate::cards::save_login::SaveLoginSpec::from_wire(&frame).unwrap())
    });
    assert!(words.contains("Saved, it isn't shared with any Bot yet."), "{words}");
    no_mark(&app);
}

/// Settings › Logins's controls are buttons and switches a screen reader can press, each with its
/// own label, and a share switch says whether it's on.
#[test]
fn the_logins_pages_controls_are_buttons_and_switches_with_labels_and_state() {
    if !in_child("the_logins_pages_controls_are_buttons_and_switches_with_labels_and_state") {
        return;
    }
    let (mut app, _seen) = start(submitted("submitted"));
    on(&mut app, || save_shared("kid@example.com", &[]));
    with_card(&mut app);
    typed(&mut app, "/vault");
    for label in ["Add a login", "Refresh", "Edit", "Delete", "Show", "kid@example.com on shop.example"] {
        assert_eq!(app.access(label).0, AccessibilityRole::Button, "{label}");
    }
    for (label, on) in [("Share all, off", false), ("Share with Nova, off", false)] {
        let (role, _, checked) = app.access(label);
        assert_eq!((role, checked), (AccessibilityRole::Switch, Some(if on { nsh_core::Checked::True } else { nsh_core::Checked::False })), "{label}");
    }
    app.press("Share with Nova, off");
    settle(&mut app);
    assert_eq!(app.access("Share with Nova, on"), (AccessibilityRole::Switch, false, Some(nsh_core::Checked::True)));
    app.press("Edit");
    app.wait_for("Save login");
    for label in ["Save login", "Cancel"] {
        assert_eq!(app.access(label).0, AccessibilityRole::Button, "{label}");
    }
}
