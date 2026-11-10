//! Computer homes on a headless engine: Settings › Computers, the key form, `/computer`, Bot
//! Details' picker and the chat's setup card. A stub broker answers each route and keeps what was
//! sent, so a test reads the very method, path and body.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use buwiz_office_kit::testing::{Headless, run};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvent, FetchEvents, FetchTask, HttpRequest, Network};
use serde_json::{Value, json};

const CHILD: &str = "BUWIZ_HOMES_UI_TEST";
const SECRET: &str = "fake-value-NEVER-SHOWN";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("homes_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// A route's answer: method, the path's end, status and body.
type Route = (&'static str, &'static str, u16, Value);
/// What was sent: method, path and body.
type Sent = (String, String, Value);

static ROUTES: Mutex<Vec<Route>> = Mutex::new(Vec::new());
static SENT: Mutex<Vec<Sent>> = Mutex::new(Vec::new());

/// Answers by the first route that fits (`{}` when none does), and keeps each request.
struct Stub;

struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

impl Network for Stub {
    fn fetch(&self, request: HttpRequest, mut events: FetchEvents) -> Box<dyn FetchTask> {
        std::thread::spawn(move || {
            let path = request.url.strip_prefix("http://127.0.0.1:1").unwrap_or(&request.url).to_owned();
            let body = match &request.body {
                nsh_core::platform::HttpBody::Bytes(b) => serde_json::from_slice(b).unwrap_or(Value::Null),
                _ => Value::Null,
            };
            SENT.lock().unwrap().push((request.method.clone(), path.clone(), body));
            let (status, reply) = ROUTES.lock().unwrap().iter().find(|(m, end, _, _)| *m == request.method && path.ends_with(end)).map(|(_, _, s, b)| (*s, b.clone())).unwrap_or((200, json!({})));
            let head = nsh_engine::HttpHead { status, url: request.url, ..Default::default() };
            events(FetchEvent::Head(head));
            events(FetchEvent::Data(reply.to_string().into_bytes()));
            events(FetchEvent::End);
        });
        Box::new(Done)
    }
}

fn route(method: &'static str, end: &'static str, status: u16, body: Value) {
    ROUTES.lock().unwrap().insert(0, (method, end, status, body));
}

/// The requests other than GETs.
fn changes() -> Vec<Sent> {
    SENT.lock().unwrap().iter().filter(|(m, _, _)| m != "GET").cloned().collect()
}

fn e2b(set: bool) -> Value {
    let key = if set { json!({"key": "api_key", "label": "API key", "placeholder": "e2b_…", "secret": true, "required": true, "set": true, "last4": "_123"}) } else { json!({"key": "api_key", "label": "API key", "placeholder": "e2b_…", "secret": true, "required": true, "set": false}) };
    json!({"kind": "e2b", "label": "E2B", "built": true, "configured": set, "canDefault": true, "isDefault": false, "fields": [key], "notes": ["Billed by E2B."], "computers": []})
}

fn providers(e2b_set: bool) -> Value {
    json!({"default": {"kind": "docker", "label": "Docker on this Mac", "source": "builtin"}, "providers": [
        {"kind": "docker", "label": "Docker on this Mac", "built": true, "configured": true, "canDefault": true, "isDefault": true, "fields": [], "notes": ["Runs on this Mac."], "computers": []},
        {"kind": "ec2", "label": "AWS EC2", "built": true, "configured": true, "canDefault": true, "isDefault": false,
            "fields": [
                {"key": "region", "label": "Region", "placeholder": "ap-southeast-1", "secret": false, "required": true, "hint": "Where it runs", "value": "ap-southeast-1"},
                {"key": "secret_access_key", "label": "Secret access key", "placeholder": "…", "secret": true, "required": false, "set": true, "last4": "x9Qa", "value": SECRET}
            ],
            "notes": ["A limited key is enough."], "computers": [{"id": "cmp_ab12", "bots": ["Ann"]}], "policy": "{\"Version\":\"2012-10-17\"}"},
        e2b(e2b_set),
        {"kind": "boat", "label": "boat.dev", "built": false, "configured": false, "canDefault": true, "isDefault": false, "fields": [], "notes": [], "computers": []},
        {"kind": "remote", "label": "A remote computer", "built": true, "configured": false, "canDefault": false, "isDefault": false, "fields": [], "notes": ["Connect one in a Bot's Computer tab."], "computers": []}
    ]})
}

fn start<T: nsh_core::element::Component<Props = ()>>(viewport: (f32, f32)) -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<T>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(viewport.0), px(viewport.1));
    config.network = Some(Arc::new(Stub));
    run(config)
}

fn shows(app: &mut Headless, word: &str) -> bool {
    app.pump(Duration::from_millis(40));
    app.texts().iter().any(|t| t.contains(word))
}

fn wait_for_text(app: &mut Headless, word: &str) {
    for _ in 0..100 {
        if shows(app, word) {
            return;
        }
    }
    panic!("{word} never showed: {:?}", app.texts());
}

fn wait_for_change(app: &mut Headless, n: usize) -> Vec<Sent> {
    for _ in 0..100 {
        if changes().len() >= n {
            return changes();
        }
        app.pump(Duration::from_millis(20));
    }
    panic!("nothing reached the broker: {:?}", SENT.lock().unwrap());
}

fn never_painted(app: &Headless, secret: &str) {
    assert!(!app.texts().iter().any(|t| t.contains(secret)), "{secret} was painted: {:?}", app.texts());
}

#[component]
fn Page() -> impl IntoElement {
    let _ = crate::live::store();
    view()
        .size_full()
        .child(crate::homes::settings_page())
        .child(text(|| format!("probe ready=1 empty={} open={}", crate::homes::page_form().is_empty(), crate::homes::page_form().kind.get())).text_size(px(8.0)))
}

#[test]
fn the_page_lists_every_home_and_never_paints_a_secret() {
    if !in_child("the_page_lists_every_home_and_never_paints_a_secret") {
        return;
    }
    route("GET", "/v1/host/computer/providers", 200, providers(false));
    let mut app = start::<Page>((900.0, 1400.0));
    wait_for_text(&mut app, "AWS EC2");
    for words in ["Docker on this Mac", "E2B", "boat.dev", "A remote computer", "Key set ••x9Qa", "No key", "Computers here: cmp_ab12 (Ann)", "A limited key is enough.", "Not available in this build yet.", "New computers are made on Docker on this Mac (the built-in choice)."] {
        assert!(shows(&mut app, words), "{words}: {:?}", app.texts());
    }
    assert!(app.has("Change key for AWS EC2") && app.has("Add key for E2B") && app.has("Remove the key for AWS EC2"));
    assert!(app.has("Make AWS EC2 the default") && !app.has("Make E2B the default"), "a home with no key can't be the default");
    never_painted(&app, SECRET);

    app.press("Change key for AWS EC2");
    app.wait_for("Copy the limited policy");
    assert!(app.has("Secret access key") && shows(&mut app, "Leave it blank to keep it"), "{:?}", app.texts());
    never_painted(&app, SECRET);
}

#[test]
fn a_submit_sends_the_put_and_clears_what_was_typed() {
    if !in_child("a_submit_sends_the_put_and_clears_what_was_typed") {
        return;
    }
    route("GET", "/v1/host/computer/providers", 200, providers(false));
    route("PUT", "/v1/host/computer/providers/e2b", 200, json!({"ok": true, "provider": e2b(true)}));
    let mut app = start::<Page>((900.0, 1400.0));
    app.wait_for("Add key for E2B");
    app.press("Add key for E2B");
    app.wait_for("API key");
    app.type_text("API key", "e2b_sk_123");
    wait_for_text(&mut app, "empty=false");
    app.press("Save the key for E2B");
    let sent = wait_for_change(&mut app, 1);
    assert_eq!((sent[0].0.as_str(), sent[0].1.as_str()), ("PUT", "/v1/host/computer/providers/e2b"));
    assert_eq!(sent[0].2, json!({"values": {"api_key": "e2b_sk_123"}, "clear": []}));
    wait_for_text(&mut app, "empty=true");
    assert!(shows(&mut app, "open=") && !app.has("Save the key for E2B"), "the form closed: {:?}", app.texts());
    wait_for_text(&mut app, "Key set ••_123");
    never_painted(&app, "e2b_sk_123");
}

#[test]
fn a_refusal_keeps_the_form_open_with_its_sentence() {
    if !in_child("a_refusal_keeps_the_form_open_with_its_sentence") {
        return;
    }
    route("GET", "/v1/host/computer/providers", 200, providers(false));
    route("PUT", "/v1/host/computer/providers/e2b", 409, json!({"ok": false, "code": "bad_key", "message": "E2B says that key is wrong."}));
    let mut app = start::<Page>((900.0, 1400.0));
    app.wait_for("Add key for E2B");
    app.press("Add key for E2B");
    app.wait_for("API key");
    app.type_text("API key", "nope");
    app.press("Save the key for E2B");
    wait_for_text(&mut app, "E2B says that key is wrong.");
    assert!(app.has("Save the key for E2B") && shows(&mut app, "open=e2b"), "the form stays: {:?}", app.texts());
    assert!(shows(&mut app, "No key"), "nothing was saved");
    never_painted(&app, "nope");
    app.press("Cancel the key for E2B");
    wait_for_text(&mut app, "empty=true");
}

#[test]
fn a_computer_being_deleted_shows_where_to_look_and_forget_sends_its_id() {
    if !in_child("a_computer_being_deleted_shows_where_to_look_and_forget_sends_its_id") {
        return;
    }
    let mut stuck = e2b(true);
    stuck["computers"] = json!([{"id": "cmp_cd34", "bots": [], "leaving": true, "look": "In the E2B dashboard, kill any sandbox whose metadata has buwiz-computer=cmp_cd34"}]);
    let mut list = providers(true);
    list["providers"][2] = stuck;
    route("GET", "/v1/host/computer/providers", 200, list);
    route("POST", "/v1/host/computer/providers/e2b/forget", 200, json!({"ok": true, "provider": e2b(true)}));
    let mut app = start::<Page>((900.0, 1600.0));
    wait_for_text(&mut app, "cmp_cd34 is being deleted");
    assert!(shows(&mut app, "buwiz-computer=cmp_cd34, then press Forget cmp_cd34."), "{:?}", app.texts());
    // the broker's list once it's forgotten, which a Forget asks for again
    route("GET", "/v1/host/computer/providers", 200, providers(true));
    app.press("Forget cmp_cd34 on E2B");
    let sent = wait_for_change(&mut app, 1);
    assert_eq!((sent[0].0.as_str(), sent[0].1.as_str()), ("POST", "/v1/host/computer/providers/e2b/forget"));
    assert_eq!(sent[0].2, json!({"id": "cmp_cd34"}));
    wait_for_text(&mut app, "cmp_cd34 is forgotten.");
    assert!(!shows(&mut app, "cmp_cd34 is being deleted"), "{:?}", app.texts());
}

#[test]
fn a_lost_computer_shows_where_to_look_and_keep_and_delete_send_its_id() {
    if !in_child("a_lost_computer_shows_where_to_look_and_keep_and_delete_send_its_id") {
        return;
    }
    let mut found = e2b(true);
    found["computers"] = json!([{"id": "cmp_ef56", "bots": [], "lost": true, "look": "In the E2B dashboard, kill any sandbox whose metadata has buwiz-computer=cmp_ef56"}]);
    let mut list = providers(true);
    list["providers"][2] = found;
    route("GET", "/v1/host/computer/providers", 200, list);
    route("POST", "/v1/host/computer/providers/e2b/keep", 409, json!({"ok": false, "code": "still_lost", "message": "Computer cmp_ef56 on E2B still can't be found with the keys in use. Nothing was deleted."}));
    route("POST", "/v1/host/computer/providers/e2b/delete", 200, json!({"ok": true, "provider": e2b(true)}));
    let mut app = start::<Page>((900.0, 1600.0));
    wait_for_text(&mut app, "cmp_ef56 was found with its record lost");
    assert!(shows(&mut app, "To check by hand: In the E2B dashboard, kill any sandbox whose metadata has buwiz-computer=cmp_ef56."), "{:?}", app.texts());
    assert!(!shows(&mut app, "cmp_ef56 is being deleted"), "it isn't on its way out: {:?}", app.texts());
    app.press("Keep cmp_ef56 on E2B");
    let sent = wait_for_change(&mut app, 1);
    assert_eq!((sent[0].0.as_str(), sent[0].1.as_str()), ("POST", "/v1/host/computer/providers/e2b/keep"));
    assert_eq!(sent[0].2, json!({"id": "cmp_ef56"}));
    wait_for_text(&mut app, "Nothing was deleted.");
    assert!(shows(&mut app, "cmp_ef56 was found with its record lost"), "a refused Keep changes nothing: {:?}", app.texts());
    // the broker's list once it's deleted, which a Delete asks for again
    route("GET", "/v1/host/computer/providers", 200, providers(true));
    app.press("Delete cmp_ef56 on E2B");
    let sent = wait_for_change(&mut app, 2);
    assert_eq!((sent[1].0.as_str(), sent[1].1.as_str()), ("POST", "/v1/host/computer/providers/e2b/delete"));
    assert_eq!(sent[1].2, json!({"id": "cmp_ef56"}));
    wait_for_text(&mut app, "cmp_ef56 is deleted.");
    assert!(!shows(&mut app, "cmp_ef56 was found with its record lost"), "{:?}", app.texts());
}

#[test]
fn a_lost_computer_that_cant_be_kept_offers_only_delete_and_says_why() {
    if !in_child("a_lost_computer_that_cant_be_kept_offers_only_delete_and_says_why") {
        return;
    }
    let mut found = e2b(true);
    found["computers"] = json!([{"id": "cmp_ef56", "bots": [], "lost": true, "look": "In the E2B dashboard, kill any sandbox whose metadata has buwiz-computer=cmp_ef56", "keep": false, "why": "An E2B computer can't be given new secrets once its record is lost, so it can't be put back in use: it can only be deleted, or left as it is."}]);
    let mut list = providers(true);
    list["providers"][2] = found;
    route("GET", "/v1/host/computer/providers", 200, list);
    let mut app = start::<Page>((900.0, 1600.0));
    wait_for_text(&mut app, "cmp_ef56 was found with its record lost");
    assert!(shows(&mut app, "it can only be deleted, or left as it is. Delete deletes it there for good."), "{:?}", app.texts());
    assert!(!shows(&mut app, "Keep looks for it again"), "{:?}", app.texts());
    assert!(app.has("Delete cmp_ef56 on E2B") && !app.has("Keep cmp_ef56 on E2B"), "only Delete is offered");
}

#[test]
fn a_deleted_lost_computer_takes_its_leftovers_off_the_list() {
    if !in_child("a_deleted_lost_computer_takes_its_leftovers_off_the_list") {
        return;
    }
    let mut list = providers(true);
    list["providers"][0]["computers"] = json!([{"id": "cmp_9a8b", "bots": [], "lost": true, "look": "In Docker, remove any container labelled app.buwiz.computer.id=cmp_9a8b"}]);
    list["providers"][0]["leftovers"] = json!([{"name": "buwiz-computer-0a1b2c3d-home", "id": "cmp_old"}]);
    route("GET", "/v1/host/computer/providers", 200, list);
    // the reply has the one home, with no list of leftovers: only the full list says
    let mut docker = providers(true)["providers"][0].clone();
    docker["computers"] = json!([]);
    route("POST", "/v1/host/computer/providers/docker/delete", 200, json!({"ok": true, "provider": docker}));
    let mut app = start::<Page>((900.0, 1600.0));
    wait_for_text(&mut app, "buwiz-computer-0a1b2c3d-home");
    let mut after = providers(true);
    after["providers"][0]["leftovers"] = json!([]);
    route("GET", "/v1/host/computer/providers", 200, after);
    app.press("Delete cmp_9a8b on Docker on this Mac");
    let sent = wait_for_change(&mut app, 1);
    assert_eq!((sent[0].0.as_str(), sent[0].1.as_str()), ("POST", "/v1/host/computer/providers/docker/delete"));
    wait_for_text(&mut app, "cmp_9a8b is deleted.");
    for _ in 0..100 {
        if !shows(&mut app, "buwiz-computer-0a1b2c3d-home") {
            return;
        }
    }
    panic!("the deleted computer's volumes are still listed: {:?}", app.texts());
}

#[component]
fn Composer() -> impl IntoElement {
    let _ = crate::live::store();
    view().size_full().child(view().label("send").w(px(40.0)).h(px(20.0)).on_press(|| {
        assert!(crate::slash::dispatch("/computer default e2b", true), "a built-in takes it");
    }))
    .child(text("probe ready=1").text_size(px(8.0)))
}

#[test]
fn computer_default_sends_the_default_put() {
    if !in_child("computer_default_sends_the_default_put") {
        return;
    }
    route("GET", "/v1/host/computer/providers", 200, providers(true));
    route("PUT", "/v1/host/computer/providers/default", 200, json!({"ok": true, "default": {"kind": "e2b", "label": "E2B", "source": "saved"}}));
    let mut app = start::<Composer>((600.0, 400.0));
    app.press("send");
    let sent = wait_for_change(&mut app, 1);
    assert_eq!(sent, [("PUT".to_owned(), "/v1/host/computer/providers/default".to_owned(), json!({"kind": "e2b"}))]);
}

#[component]
fn Details() -> impl IntoElement {
    let _ = crate::live::store();
    view().size_full().child(crate::homes::bot_row("b1", true)).child(text("probe ready=1").text_size(px(8.0)))
}

#[test]
fn the_bot_details_picker_sends_null_for_the_default() {
    if !in_child("the_bot_details_picker_sends_null_for_the_default") {
        return;
    }
    route("GET", "/v1/host/computer/providers", 200, providers(true));
    route("GET", "/v1/host/computer/bot/b1", 200, json!({"state": "absent", "home": {"kind": "e2b", "label": "E2B", "source": "bot"}}));
    route("PUT", "/v1/host/computer/bot/b1/provider", 200, json!({"state": "absent", "home": {"kind": "docker", "label": "Docker on this Mac", "source": "default"}}));
    let mut app = start::<Details>((600.0, 600.0));
    wait_for_text(&mut app, "E2B ▾");
    assert!(shows(&mut app, "Applies to this Bot's next new computer"));
    app.press("Computer");
    app.wait_for("Default (Docker on this Mac)");
    assert!(app.has("E2B") && app.has("AWS EC2") && !app.has("A remote computer") && !app.has("boat.dev"), "ready homes only");
    app.press("Default (Docker on this Mac)");
    let sent = wait_for_change(&mut app, 1);
    assert_eq!(sent, [("PUT".to_owned(), "/v1/host/computer/bot/b1/provider".to_owned(), json!({"kind": null}))]);
    wait_for_text(&mut app, "Default (Docker on this Mac) ▾");
}

#[component]
fn ReadDetails() -> impl IntoElement {
    let _ = crate::live::store();
    view().size_full().child(crate::homes::bot_row("b1", false)).child(text("probe ready=1").text_size(px(8.0)))
}

#[test]
fn bot_details_open_to_read_shows_the_home_and_offers_no_picker() {
    if !in_child("bot_details_open_to_read_shows_the_home_and_offers_no_picker") {
        return;
    }
    route("GET", "/v1/host/computer/providers", 200, providers(true));
    route("GET", "/v1/host/computer/bot/b1", 200, json!({"state": "absent", "home": {"kind": "e2b", "label": "E2B", "source": "bot"}}));
    let mut app = start::<ReadDetails>((600.0, 600.0));
    wait_for_text(&mut app, "E2B");
    app.press("Computer");
    assert!(!app.has("E2B ▾") && !app.has("AWS EC2") && !app.has("Default (Docker on this Mac)"), "no picker when it only reads");
}

#[component]
fn Chat() -> impl IntoElement {
    let _ = crate::live::store();
    let n = crate::cards::computer_setup::Notice::of(&json!({"kind": "computer_setup", "id": "cs_1", "bot": "b1", "botName": "Ann", "chat": "t1", "reason": "This needs a browser that stays up.", "state": "pending", "at": 5})).unwrap();
    view()
        .size_full()
        .child(crate::cards::computer_setup::row(n))
        .child(text(|| format!("probe ready=1 empty={}", crate::cards::computer_setup::form_is_empty("cs_1"))).text_size(px(8.0)))
}

#[test]
fn the_notice_draws_a_pick_a_home_card_that_leads_to_a_key_and_a_choice() {
    if !in_child("the_notice_draws_a_pick_a_home_card_that_leads_to_a_key_and_a_choice") {
        return;
    }
    route("GET", "/v1/host/computer/providers", 200, providers(false));
    route("PUT", "/v1/host/computer/providers/e2b", 200, json!({"ok": true, "provider": e2b(true)}));
    route("PUT", "/v1/host/computer/bot/b1/provider", 200, json!({"state": "absent", "home": {"kind": "e2b", "label": "E2B", "source": "bot"}}));
    let mut app = start::<Chat>((800.0, 900.0));
    wait_for_text(&mut app, "This needs a browser that stays up.");
    assert!(shows(&mut app, "Where should Ann's next new computer be made?"), "{:?}", app.texts());
    wait_for_text(&mut app, "Needs a key");
    assert!(app.has("Docker on this Mac") && app.has("E2B") && app.has("A remote computer"));
    app.press("E2B");
    wait_for_text(&mut app, "Enter a key for E2B");
    app.type_text("API key", "e2b_sk_9");
    app.press("Save the key for E2B");
    wait_for_text(&mut app, "E2B works");
    assert!(shows(&mut app, "empty=true"), "the typed key is gone");
    never_painted(&app, "e2b_sk_9");
    assert!(app.has("Make it the default"));
    app.press("Only for Ann");
    wait_for_text(&mut app, "Ann's next new computer will be made on E2B.");
    let sent = changes();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].2, json!({"values": {"api_key": "e2b_sk_9"}, "clear": []}));
    assert_eq!((sent[1].1.as_str(), &sent[1].2), ("/v1/host/computer/bot/b1/provider", &json!({"kind": "e2b"})));
}
