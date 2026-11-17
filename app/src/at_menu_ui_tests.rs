//! The composer's `@` menu through the real composer path on a headless engine: it lists the
//! plugins and MCP servers the Bot of the chat in front has on, asks the broker's switches each
//! time it opens, and follows the chat to another Bot. A stand-in broker answers what the broker
//! does: the servers' manifest, the chats, and which keys are off for each chat's Bot.
//!
//! The store is one per process, so each test runs in a child of its own.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::element::Root;
use nsh_core::platform::{FetchEvent, HttpHead, HttpRequest};
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvents, FetchTask, Network};
use serde_json::{Value, json};

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;

const CHILD: &str = "BUWIZ_AT_MENU_UI_TEST";

fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("at_menu_ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

fn bot(id: &str, name: &str) -> Value {
    json!({"id": id, "name": name, "shape": "blob", "color": "#1884FC"})
}

/// `/threads`: a chat each for Nova and Ada, and a plain one.
fn threads() -> Value {
    json!({"list": [{"id": "t_nova", "title": "Nova", "bot": bot("nova", "Nova")}, {"id": "t_ada", "title": "Ada", "bot": bot("ada", "Ada")}, {"id": "t_plain", "title": "Plain"}],
        "bots": [bot("nova", "Nova"), bot("ada", "Ada")]})
}

/// What `/mcp/manifest` lists: two servers with `@` scopes.
fn manifest() -> Value {
    json!({"servers": [
        {"id": "bir", "title": "BIR", "scopes": [{"name": "taxpayer", "description": "A taxpayer"}]},
        {"id": "gallery", "title": "Gallery", "scopes": [{"name": "photo", "description": "A photo"}]},
    ]})
}

/// A stand-in broker: every request's path, and the keys off for each chat's Bot, which a test
/// changes between the menu's openings.
struct Broker {
    log: Mutex<Vec<String>>,
    off: Mutex<HashMap<String, Vec<String>>>,
    /// The off route answers `ok: false`.
    refuse: AtomicBool,
    /// Answers for this chat's off route wait here until `release`.
    hold: Mutex<Option<String>>,
    held: Mutex<Vec<(FetchEvents, Value)>>,
}

struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

impl Network for Broker {
    fn fetch(&self, request: HttpRequest, events: FetchEvents) -> Box<dyn FetchTask> {
        let path = request.url.strip_prefix(crate::client::backend()).unwrap_or(&request.url).to_owned();
        self.log.lock().unwrap().push(path.clone());
        let answer = match path.as_str() {
            "/threads" => threads(),
            "/mcp/manifest" => manifest(),
            p if p.starts_with("/v1/host/tools/off?thread=") => {
                let thread = p.trim_start_matches("/v1/host/tools/off?thread=");
                if self.refuse.load(Ordering::SeqCst) {
                    return finish(events, json!({"ok": false, "code": "internal"}));
                }
                let answer = json!({"ok": true, "off": self.off.lock().unwrap().get(thread).cloned().unwrap_or_default()});
                if self.hold.lock().unwrap().as_deref() == Some(thread) {
                    self.held.lock().unwrap().push((events, answer));
                    return Box::new(Done);
                }
                answer
            }
            _ => json!({"ok": true}),
        };
        finish(events, answer)
    }
}

impl Broker {
    /// Lets the answers held back go.
    fn release(&self) {
        *self.hold.lock().unwrap() = None;
        for (events, answer) in self.held.lock().unwrap().drain(..) {
            finish(events, answer);
        }
    }
}

fn finish(mut events: FetchEvents, answer: Value) -> Box<dyn FetchTask> {
    events(FetchEvent::Head(HttpHead { status: 200, ..Default::default() }));
    events(FetchEvent::Data(answer.to_string().into_bytes()));
    events(FetchEvent::End);
    Box::new(Done)
}

/// How long a test waits for what it expects before it fails: generous, for a loaded runner.
const PATIENCE: Duration = Duration::from_secs(60);

/// `app` pumped until `done()` (run on the engine thread) says so.
fn wait_until(app: &mut Headless, what: &str, done: impl Fn() -> bool + Send + Clone + 'static) {
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if on(app, done.clone()) {
            return;
        }
        app.pump(Duration::from_millis(10));
    }
    panic!("waited for {what}");
}

fn start() -> (Headless, Arc<Broker>) {
    let seen = Arc::new(Broker { log: Mutex::new(Vec::new()), off: Mutex::new(HashMap::new()), refuse: AtomicBool::new(false), hold: Mutex::new(None), held: Mutex::new(Vec::new()) });
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    config.network = Some(seen.clone());
    let mut app = run(config);
    // the draft's watch, as boot sets it, and the servers' manifest it asks for
    on(&mut app, crate::slash::boot);
    wait_until(&mut app, "the manifest", || store().mcp_loaded.get() && store().mcp_manifest.with_untracked(|m| m.len()) == 2);
    (app, seen)
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
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if let Ok(r) = rx.try_recv() {
            return r;
        }
        app.pump(Duration::from_millis(5));
    }
    panic!("the job never ran");
}

#[component]
fn Stage() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view().size_full().child(text("probe ready=1")).child(crate::toast::layer()).child(view().label("job").w(px(40.0)).h(px(24.0)).on_press(|| {
        if let Some(job) = JOB.lock().unwrap().take() {
            job();
        }
    }))
}

/// Chat `chat` in front.
fn open(app: &mut Headless, chat: &'static str) {
    on(app, move || {
        let st = store();
        st.set_threads(threads());
        st.active_id.set(chat.to_owned());
        st.turns_of.set(chat.to_owned());
    });
}

fn install_plugins(app: &mut Headless) {
    on(app, || {
        let plugin = |short: &str, title: &str, scope: &str| crate::mini_v1::load::PluginEntry {
            id: format!("app.buwiz.{short}"),
            short: short.to_owned(),
            title: title.to_owned(),
            enabled: true,
            scopes: vec![json!({"name": scope, "description": format!("{scope} rows")})],
            ..Default::default()
        };
        store().v1.entries.set(vec![plugin("ebirforms", "eBIR Forms", "taxprofile"), plugin("books", "Books", "person")]);
    });
}

/// Whether the opening has an answer (or a failure) for the chat in front.
fn answered() -> bool {
    let thread = store().active_id.get_untracked();
    match &*store().at_off.borrow() {
        crate::slash::AtOff::Known(t, _) | crate::slash::AtOff::Failed(t) => *t == thread,
        _ => false,
    }
}

/// The popup's rows once the answer for the typed text has been applied.
fn menu(app: &mut Headless, typed: &'static str) -> Vec<String> {
    on(app, move || crate::composer::draft_changed(typed));
    wait_until(app, "the broker's answer to be applied", answered);
    on(app, || store().pop_rows.get_untracked().into_iter().map(|r| r.label).collect())
}

/// The menu closed, as the person leaves it by clearing the box.
fn close_menu(app: &mut Headless) {
    on(app, || crate::composer::draft_changed(""));
}

fn asked(seen: &Broker, thread: &str) -> usize {
    let path = format!("/v1/host/tools/off?thread={thread}");
    seen.log.lock().unwrap().iter().filter(|p| **p == path).count()
}

/// With a plugin and an MCP server off for Nova, her `@` menu lists neither and lists the rest;
/// Ada's lists all four; the chat in front decides.
#[test]
fn the_at_menu_lists_only_what_the_active_bot_has_on() {
    if !in_child("the_at_menu_lists_only_what_the_active_bot_has_on") {
        return;
    }
    let (mut app, seen) = start();
    install_plugins(&mut app);
    seen.off.lock().unwrap().insert("t_nova".to_owned(), vec!["mcp:gallery".to_owned(), "v1:app.buwiz.books".to_owned()]);
    open(&mut app, "t_nova");
    assert_eq!(menu(&mut app, "@"), ["@bir", "@ebirforms"], "Nova has the gallery and Books off");
    assert!(menu(&mut app, "@bo").is_empty(), "narrowing to Books finds nothing: it is off");
    assert!(menu(&mut app, "@gal").is_empty(), "nor the gallery");
    assert!(menu(&mut app, "@books:").is_empty(), "no scopes of a plugin that is off");
    assert!(menu(&mut app, "@gallery:").is_empty(), "no scopes of a server that is off");
    assert_eq!(menu(&mut app, "@ebirforms:"), ["@ebirforms:taxprofile"]);
    assert_eq!(menu(&mut app, "@bir:"), ["@bir:taxpayer"]);

    // the chat of another Bot: the switches are hers
    close_menu(&mut app);
    open(&mut app, "t_ada");
    assert_eq!(menu(&mut app, "@"), ["@bir", "@gallery", "@ebirforms", "@books"], "Ada has everything on");
    assert_eq!(menu(&mut app, "@books:"), ["@books:person"]);
    assert!(asked(&seen, "t_ada") >= 1, "{:?}", seen.log.lock().unwrap());

    // a plain chat has its own, and it is all on here
    close_menu(&mut app);
    open(&mut app, "t_plain");
    assert_eq!(menu(&mut app, "@"), ["@bir", "@gallery", "@ebirforms", "@books"]);

    // and back in Nova's
    close_menu(&mut app);
    open(&mut app, "t_nova");
    assert_eq!(menu(&mut app, "@"), ["@bir", "@ebirforms"]);
}

/// A switch moved while the menu was closed shows the next time it opens, not before; a menu
/// open keeps what it was asked.
#[test]
fn a_switch_moved_while_the_menu_was_closed_shows_when_it_opens_again() {
    if !in_child("a_switch_moved_while_the_menu_was_closed_shows_when_it_opens_again") {
        return;
    }
    let (mut app, seen) = start();
    install_plugins(&mut app);
    open(&mut app, "t_ada");
    assert_eq!(menu(&mut app, "@"), ["@bir", "@gallery", "@ebirforms", "@books"]);
    let before = asked(&seen, "t_ada");
    assert_eq!(menu(&mut app, "@bo"), ["@books"], "typing on is no new opening");
    assert_eq!(asked(&seen, "t_ada"), before, "the open menu asked nothing more");

    close_menu(&mut app);
    seen.off.lock().unwrap().insert("t_ada".to_owned(), vec!["v1:app.buwiz.ebirforms".to_owned()]);
    assert_eq!(menu(&mut app, "@"), ["@bir", "@gallery", "@books"], "Ebirforms went off while it was closed");
    assert_eq!(asked(&seen, "t_ada"), before + 1);

    close_menu(&mut app);
    seen.off.lock().unwrap().remove("t_ada");
    assert_eq!(menu(&mut app, "@"), ["@bir", "@gallery", "@ebirforms", "@books"], "and back on");
}

/// A draft cleared with no close (a token sent) ends the opening too: the next `@` at the same
/// place asks again.
#[test]
fn a_draft_cleared_without_a_close_still_asks_at_the_next_at() {
    if !in_child("a_draft_cleared_without_a_close_still_asks_at_the_next_at") {
        return;
    }
    let (mut app, seen) = start();
    install_plugins(&mut app);
    open(&mut app, "t_ada");
    assert_eq!(menu(&mut app, "@").len(), 4);
    let before = asked(&seen, "t_ada");
    // sent: the box is emptied by a plain set
    on(&mut app, || store().draft.set(String::new()));
    seen.off.lock().unwrap().insert("t_ada".to_owned(), vec!["v1:app.buwiz.books".to_owned()]);
    assert_eq!(menu(&mut app, "@"), ["@bir", "@gallery", "@ebirforms"], "Books went off since");
    assert_eq!(asked(&seen, "t_ada"), before + 1);
}

/// Switching chats while the first one's answer is on its way: the chat in front is asked, and
/// the first one's answer, landing late, changes nothing.
#[test]
fn an_answer_for_a_chat_no_longer_in_front_is_dropped_and_the_chat_in_front_is_asked() {
    if !in_child("an_answer_for_a_chat_no_longer_in_front_is_dropped_and_the_chat_in_front_is_asked") {
        return;
    }
    let (mut app, seen) = start();
    install_plugins(&mut app);
    seen.off.lock().unwrap().insert("t_nova".to_owned(), vec!["mcp:gallery".to_owned()]);
    *seen.hold.lock().unwrap() = Some("t_nova".to_owned());
    open(&mut app, "t_nova");
    on(&mut app, || crate::composer::draft_changed("@"));
    wait_until(&mut app, "Nova's ask to be held", {
        let seen = seen.clone();
        move || !seen.held.lock().unwrap().is_empty()
    });
    // another chat, the `@` still at the same place
    open(&mut app, "t_ada");
    assert_eq!(menu(&mut app, "@"), ["@bir", "@gallery", "@ebirforms", "@books"], "Ada's own answer, not Nova's");
    seen.release();
    // Nova's answer lands; the rows stay Ada's
    wait_until(&mut app, "the late answer to be heard", {
        let seen = seen.clone();
        move || seen.held.lock().unwrap().is_empty()
    });
    on(&mut app, || ());
    assert_eq!(on(&mut app, || store().pop_rows.get_untracked().len()), 4);
    assert!(matches!(on(&mut app, || store().at_off.borrow().clone()), crate::slash::AtOff::Known(t, _) if t == "t_ada"));
}

/// A broker that will not answer lists nothing, asks once for the opening (not at each key), and
/// is asked again at the next opening.
#[test]
fn a_menu_the_broker_will_not_answer_lists_nothing_and_asks_once_per_opening() {
    if !in_child("a_menu_the_broker_will_not_answer_lists_nothing_and_asks_once_per_opening") {
        return;
    }
    let (mut app, seen) = start();
    install_plugins(&mut app);
    open(&mut app, "t_ada");
    seen.refuse.store(true, Ordering::SeqCst);
    assert!(menu(&mut app, "@").is_empty());
    assert!(menu(&mut app, "@b").is_empty());
    assert!(menu(&mut app, "@bo").is_empty());
    assert_eq!(asked(&seen, "t_ada"), 1, "one ask for the whole opening");
    seen.refuse.store(false, Ordering::SeqCst);
    close_menu(&mut app);
    assert_eq!(menu(&mut app, "@").len(), 4, "asked again at the next opening");
    assert_eq!(asked(&seen, "t_ada"), 2);
}
