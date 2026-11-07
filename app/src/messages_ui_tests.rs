//! `/messages` through the real composer path on a headless engine: the
//! popup, Enter, `send`, a chip's press and a choice card's, against a
//! stand-in broker that records what reaches it.
//!
//! The store is one per process, so each test runs in a child of its own.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::element::Root;
use nsh_core::platform::{FetchEvent, HttpBody, HttpHead, HttpRequest};
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvents, FetchTask, Network};
use serde_json::{Value, json};

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;

const CHILD: &str = "BUWIZ_MESSAGES_UI_TEST";
const ROUTE: &str = "/v1/host/agents/accepts";
const SAID: &str = "Scout can now message Nova.";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("messages_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// `/threads` with Nova's accepts as `accepts`, a plain chat beside hers.
fn threads(accepts: Value) -> Value {
    let bot = |id: &str, name: &str| json!({"id": id, "name": name, "shape": "blob", "color": "#1884FC"});
    let mut nova = bot("nova", "Nova");
    nova["accepts"] = accepts;
    json!({"list": [{"id": "t_nova", "title": "Nova", "bot": bot("nova", "Nova")}, {"id": "t_plain", "title": "Plain"}],
        "bots": [nova, bot("scout", "Scout"), bot("ada", "Ada")]})
}

/// A stand-in broker behind the engine's network: every request's
/// `METHOD /path` and JSON body. The accepts route answers with `reply`,
/// `/threads` with the Bots as they are.
struct Broker {
    log: Mutex<Vec<(String, Value)>>,
    reply: Value,
    accepts: Mutex<Value>,
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
        // a null `reply`: it applies, and the answer never arrives
        let lost = path == ROUTE && self.reply.is_null();
        if path == ROUTE && (self.reply["ok"] == true || lost) {
            *self.accepts.lock().unwrap() = body["accepts"].clone();
        }
        if path == "/v1/host/tools/undo" {
            *self.accepts.lock().unwrap() = json!("none");
        }
        self.log.lock().unwrap().push((path.clone(), body));
        if lost {
            events(FetchEvent::Failed("the connection broke".to_owned()));
            return Box::new(Done);
        }
        let answer = match path.as_str() {
            ROUTE => self.reply.clone(),
            "/threads" => threads(self.accepts.lock().unwrap().clone()),
            _ => json!({"ok": true}),
        };
        events(FetchEvent::Head(HttpHead { status: 200, ..Default::default() }));
        events(FetchEvent::Data(answer.to_string().into_bytes()));
        events(FetchEvent::End);
        Box::new(Done)
    }
}

fn start(reply: Value) -> (Headless, Arc<Broker>) {
    let seen = Arc::new(Broker { log: Mutex::new(Vec::new()), reply, accepts: Mutex::new(json!("none")) });
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    config.network = Some(seen.clone());
    let mut app = run(config);
    // what the store asks at boot has its answer
    settle(&mut app);
    (app, seen)
}

fn heard(seen: &Broker, path: &str) -> Vec<Value> {
    seen.log.lock().unwrap().iter().filter(|(p, _)| p == path).map(|(_, b)| b.clone()).collect()
}

/// `app` run until `path` has been heard `n` times.
fn wait_for(app: &mut Headless, seen: &Broker, path: &str, n: usize) -> Vec<Value> {
    for _ in 0..200 {
        app.pump(Duration::from_millis(20));
        let got = heard(seen, path);
        if got.len() >= n {
            return got;
        }
    }
    panic!("{path} not heard {n} times: {:?}", seen.log.lock().unwrap());
}

/// Time for anything a press set going to reach the stand-in.
fn settle(app: &mut Headless) {
    for _ in 0..25 {
        app.pump(Duration::from_millis(20));
    }
}

/// What the Stage's `job` press runs next.
static JOB: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// `f` on the engine thread, where the store's signals live.
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

fn open(app: &mut Headless, seen: &Broker, chat: &'static str, accepts: Value) {
    *seen.accepts.lock().unwrap() = accepts.clone();
    on(app, move || {
        let st = store();
        st.mcp_loaded.set(true);
        st.set_threads(threads(accepts));
        st.active_id.set(chat.to_owned());
        st.turns_of.set(chat.to_owned());
    });
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
        .child(crate::mini_v1::ui::suggest_chips(vec![("See more".to_owned(), "/messages allow all".to_owned()), ("Plain".to_owned(), "/nosuch thing".to_owned())]))
        .child(dynamic(|| {
            let turns: Vec<_> = store().turns.get().into_iter().filter(|t| t.tools_change.is_some()).collect();
            view().children(turns.into_iter().map(|t| crate::chat::turn_row(t, "t_nova").into_element())).into_element()
        }))
}

fn labels(app: &mut Headless) -> Vec<String> {
    on(app, || store().pop_rows.get_untracked().into_iter().map(|r| r.label).collect())
}

fn draft(app: &mut Headless) -> String {
    on(app, || store().draft.get_untracked())
}

/// What the composer's change does as the person types `t`.
fn type_in(app: &mut Headless, t: &'static str) {
    on(app, move || crate::composer::draft_changed(t));
}

/// The box set to `t` with the list not told: rows for an older text, or a
/// plugin's `prompt` answer.
fn place(app: &mut Headless, t: &'static str) {
    on(app, move || store().draft.set(t.to_owned()));
}

fn enter(app: &mut Headless) {
    on(app, crate::slash::enter);
}

/// The composer's own Enter with no list open.
fn send(app: &mut Headless) {
    on(app, || store().send());
}

fn accepts_held(app: &mut Headless) -> Option<crate::agents::Accepts> {
    on(app, || store().bots.with_untracked(|b| b.iter().find(|b| b.id == "nova").map(|b| b.accepts.clone())))
}

fn ok_reply() -> Value {
    json!({"ok": true, "text": SAID, "undo": "u_1"})
}

/// The whole of it: `/mess`, pick, `allow `, narrow, Enter fills, Enter runs,
/// and the receipt's Undo. Nothing is sent until the line is complete, and
/// nothing reaches `/chat`.
#[test]
fn fill_then_enter_runs_it_and_undo_puts_it_back() {
    if !in_child("fill_then_enter_runs_it_and_undo_puts_it_back") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("none"));
    type_in(&mut app, "/mess");
    assert_eq!(labels(&mut app), ["/messages"]);
    enter(&mut app);
    assert_eq!(draft(&mut app), "/messages ");
    assert_eq!(labels(&mut app), ["allow", "none", "Nova", "Scout", "Ada"], "the list follows the pick at once");
    enter(&mut app);
    assert_eq!(draft(&mut app), "/messages allow ");
    assert_eq!(labels(&mut app), ["all", "Scout", "Ada"]);
    type_in(&mut app, "/messages allow sc");
    assert_eq!(labels(&mut app), ["Scout"]);
    enter(&mut app);
    assert_eq!(draft(&mut app), "/messages allow Scout", "the pick only fills");
    assert_eq!(labels(&mut app), ["Scout"], "and reopens the list");
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty(), "nothing yet");
    let refreshes = heard(&seen, "/threads").len();
    enter(&mut app);
    let sent = wait_for(&mut app, &seen, ROUTE, 1);
    assert_eq!(sent[0], json!({"bot": "nova", "accepts": ["scout"]}));
    assert_eq!(draft(&mut app), "");
    assert!(labels(&mut app).is_empty());
    wait_for(&mut app, &seen, "/threads", refreshes + 1);
    settle(&mut app);
    assert_eq!(accepts_held(&mut app), Some(crate::agents::Accepts::List(vec!["scout".to_owned()])), "the real list, from the broker");
    // drawn word by word
    assert!(SAID.split(' ').chain(["Undo"]).all(|w| app.texts().contains(&w)), "the receipt: {:?}", app.texts());
    // the next `allow` no longer offers Scout
    type_in(&mut app, "/messages allow ");
    assert_eq!(labels(&mut app), ["all", "Ada"]);
    type_in(&mut app, "");
    app.press(&format!("Undo: {SAID}"));
    let undone = wait_for(&mut app, &seen, "/v1/host/tools/undo", 1);
    assert_eq!(undone[0], json!({"undo": "u_1"}));
    wait_for(&mut app, &seen, "/threads", refreshes + 2);
    settle(&mut app);
    type_in(&mut app, "/messages allow ");
    assert_eq!(labels(&mut app), ["all", "Scout", "Ada"], "Undo reaches the Bots held here");
    assert!(heard(&seen, "/chat").is_empty());
}

/// `/messages n` and one Enter fills in `none`: a stray Enter can't change
/// who a Bot hears from.
#[test]
fn a_part_word_and_one_enter_changes_nothing() {
    if !in_child("a_part_word_and_one_enter_changes_nothing") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("all"));
    type_in(&mut app, "/messages n");
    assert_eq!(labels(&mut app), ["none", "Nova"]);
    enter(&mut app);
    assert_eq!(draft(&mut app), "/messages none");
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty());
    enter(&mut app);
    assert_eq!(wait_for(&mut app, &seen, ROUTE, 1)[0], json!({"bot": "nova", "accepts": "none"}));
}

/// Rows for an older text: Enter sends what's typed, never the old row.
#[test]
fn enter_on_stale_rows_runs_what_is_typed() {
    if !in_child("enter_on_stale_rows_runs_what_is_typed") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("none"));
    type_in(&mut app, "/messages allow s");
    assert_eq!(labels(&mut app), ["Scout"]);
    // typed on, the rows not caught up
    place(&mut app, "/messages allow ada");
    enter(&mut app);
    assert_eq!(wait_for(&mut app, &seen, ROUTE, 1)[0], json!({"bot": "nova", "accepts": ["ada"]}));
    assert_eq!(draft(&mut app), "");
}

/// A chip carrying the command goes to the AI as words; the box keeps
/// what the person was writing.
#[test]
fn a_chip_press_does_not_run_it() {
    if !in_child("a_chip_press_does_not_run_it") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("none"));
    place(&mut app, "my own words");
    app.press("See more");
    let chat = wait_for(&mut app, &seen, "/chat", 1);
    assert_eq!(chat[0]["prompt"], "/messages allow all");
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty(), "{:?}", seen.log.lock().unwrap());
    assert_eq!(draft(&mut app), "my own words");
}

/// A choice card whose words read as the command: its press doesn't run it.
#[test]
fn a_choice_press_does_not_run_it() {
    if !in_child("a_choice_press_does_not_run_it") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("all"));
    on(&mut app, || {
        let spec = crate::cards::choice::FormSpec::from_value(&json!({"title": "/messages none", "fields": [{"id": "q", "label": "Pick", "options": ["Yes", "No"]}]})).expect("a form");
        crate::cards::choice::choose("k", &spec, 0, 0);
    });
    let chat = wait_for(&mut app, &seen, "/chat", 1);
    assert_eq!(chat[0]["prompt"], "/messages none\nPick: Yes");
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty(), "{:?}", seen.log.lock().unwrap());
}

/// The receipt belongs to the chat the line was typed in: gone from there
/// before the answer, it is a toast saying where Undo is, not a line in
/// whichever chat is open.
#[test]
fn a_change_answered_after_leaving_its_chat_is_a_toast() {
    if !in_child("a_change_answered_after_leaving_its_chat_is_a_toast") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("none"));
    place(&mut app, "/messages allow scout");
    on(&mut app, || {
        let st = store();
        st.send();
        st.active_id.set("t_plain".to_owned());
        st.turns_of.set("t_plain".to_owned());
    });
    wait_for(&mut app, &seen, ROUTE, 1);
    settle(&mut app);
    assert!(on(&mut app, || store().local_turns.borrow().is_empty()), "no receipt anywhere");
    assert!(app.texts().iter().any(|t| t.contains(SAID) && t.contains("Undo it in /tools.")), "{:?}", app.texts());
}

/// No answer doesn't mean nothing changed: the app says it couldn't confirm
/// and shows what the broker really has.
#[test]
fn a_lost_answer_is_unconfirmed_and_the_real_state_comes_back() {
    if !in_child("a_lost_answer_is_unconfirmed_and_the_real_state_comes_back") {
        return;
    }
    let (mut app, seen) = start(Value::Null);
    open(&mut app, &seen, "t_nova", json!("all"));
    let refreshes = heard(&seen, "/threads").len();
    place(&mut app, "/messages none");
    send(&mut app);
    wait_for(&mut app, &seen, ROUTE, 1);
    wait_for(&mut app, &seen, "/threads", refreshes + 1);
    settle(&mut app);
    assert_eq!(accepts_held(&mut app), Some(crate::agents::Accepts::None), "it did apply");
    assert!(app.texts().iter().any(|t| t.contains("Couldn't confirm")), "{:?}", app.texts());
    assert!(!app.texts().iter().any(|t| t.contains("nothing changed")), "{:?}", app.texts());
    assert!(on(&mut app, || store().local_turns.borrow().is_empty()), "no receipt for what isn't known");
}

/// A line in the box runs on the person's own Enter, and not before.
#[test]
fn a_line_in_the_box_runs_on_the_persons_enter() {
    if !in_child("a_line_in_the_box_runs_on_the_persons_enter") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("all"));
    place(&mut app, "/messages none");
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty());
    send(&mut app);
    assert_eq!(wait_for(&mut app, &seen, ROUTE, 1)[0], json!({"bot": "nova", "accepts": "none"}));
    assert!(heard(&seen, "/chat").is_empty());
}

/// A plugin's `prompt` answer can't stage the command, however it is
/// dressed: the box keeps what it held, and the person is told.
#[test]
fn a_plugins_prompt_cannot_place_the_command_in_the_box() {
    if !in_child("a_plugins_prompt_cannot_place_the_command_in_the_box") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("none"));
    for line in ["/messages allow all", "  /messages none\n", "/messages", "/messages:none"] {
        on(&mut app, move || store().v1_place_prompt(line));
        assert_eq!(draft(&mut app), "", "{line:?}");
    }
    app.pump(Duration::from_millis(100));
    assert!(app.texts().iter().any(|t| t.contains("can't put /messages in the box")), "{:?}", app.texts());
    place(&mut app, "my own words");
    on(&mut app, || store().v1_place_prompt("/messages allow all"));
    assert_eq!(draft(&mut app), "my own words", "kept");
    // any other prompt still lands
    place(&mut app, "");
    on(&mut app, || store().v1_place_prompt("Tell me about /messages"));
    assert_eq!(draft(&mut app), "Tell me about /messages");
    assert!(heard(&seen, ROUTE).is_empty() && heard(&seen, "/chat").is_empty());
}

/// Before the servers' commands are known a chip's send waits for them.
/// It's the chip's words that go then, not the line in the box.
#[test]
fn a_chip_before_the_manifest_never_sends_the_box() {
    if !in_child("a_chip_before_the_manifest_never_sends_the_box") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("all"));
    on(&mut app, || store().mcp_loaded.set(false));
    place(&mut app, "/messages none");
    app.press("Plain");
    let chat = wait_for(&mut app, &seen, "/chat", 1);
    assert_eq!(chat[0]["prompt"], "/nosuch thing");
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty(), "{:?}", seen.log.lock().unwrap());
    assert_eq!(draft(&mut app), "/messages none", "still the person's to send");
}

/// A plugin's token in the line, with a file waiting: `v1_send` runs before
/// `dispatch`, and would rewrite the token and send the line to the model.
#[test]
fn a_line_with_a_token_and_a_file_stays_out_of_chat() {
    if !in_child("a_line_with_a_token_and_a_file_stays_out_of_chat") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("all"));
    on(&mut app, || {
        store().v1.entries.set(vec![crate::mini_v1::load::PluginEntry {
            id: "app.buwiz.books".to_owned(),
            short: "books".to_owned(),
            enabled: true,
            scopes: vec![json!({"name": "person"})],
            ..Default::default()
        }]);
        store().attachments.set(vec![crate::store::Attach { name: "a.txt".to_owned(), ..Default::default() }]);
    });
    place(&mut app, "/messages allow @books:person:x");
    send(&mut app);
    for _ in 0..4 {
        settle(&mut app);
    }
    assert!(heard(&seen, "/chat").is_empty(), "{:?}", seen.log.lock().unwrap());
    assert_eq!(draft(&mut app), "");
}

/// No typed form reaches the model, a plugin's token in the line included.
#[test]
fn no_typed_form_reaches_chat() {
    if !in_child("no_typed_form_reaches_chat") {
        return;
    }
    let (mut app, seen) = start(ok_reply());
    open(&mut app, &seen, "t_nova", json!("all"));
    on(&mut app, || {
        store().v1.entries.set(vec![crate::mini_v1::load::PluginEntry {
            id: "app.buwiz.books".to_owned(),
            short: "books".to_owned(),
            enabled: true,
            scopes: vec![json!({"name": "person"})],
            ..Default::default()
        }])
    });
    for line in ["/messages", "/messages nova", "/messages nobody at all", "/messages allow", "/messages allow scout", "/messages all", "/messages:none", "/messages allow @books:person:x", "/messages @books:person:x"] {
        place(&mut app, line);
        send(&mut app);
        assert_eq!(draft(&mut app), "", "{line}");
    }
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty() && heard(&seen, "/chat").is_empty(), "nothing leaves: {:?}", seen.log.lock().unwrap());
    for line in ["/messages allow all", "/messages none"] {
        place(&mut app, line);
        send(&mut app);
    }
    wait_for(&mut app, &seen, ROUTE, 2);
    settle(&mut app);
    assert!(heard(&seen, "/chat").is_empty());
}

/// A plain chat and a refusal: no change, no receipt; the Bots stay as they were.
#[test]
fn a_plain_chat_and_a_refusal_change_nothing() {
    if !in_child("a_plain_chat_and_a_refusal_change_nothing") {
        return;
    }
    let (mut app, seen) = start(json!({"ok": false, "code": "invalid_args", "message": "no"}));
    open(&mut app, &seen, "t_plain", json!("all"));
    place(&mut app, "/messages none");
    send(&mut app);
    settle(&mut app);
    assert!(heard(&seen, ROUTE).is_empty() && heard(&seen, "/chat").is_empty());
    open(&mut app, &seen, "t_nova", json!("all"));
    place(&mut app, "/messages none");
    send(&mut app);
    wait_for(&mut app, &seen, ROUTE, 1);
    settle(&mut app);
    assert_eq!(accepts_held(&mut app), Some(crate::agents::Accepts::All), "put back");
    assert!(on(&mut app, || store().turns.with_untracked(|t| t.iter().all(|t| t.tools_change.is_none()))), "no receipt");
}
