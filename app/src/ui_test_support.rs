//! What the headless composer tests share: the child-process runner (the store is one per
//! process, so each test runs in a child of its own), the stand-in network's answer, the wait
//! helpers, and the stage whose `job` press runs a closure on the engine thread.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use nsh_core::platform::{FetchEvent, HttpHead};
use nsh_core::prelude::*;
use nsh_engine::{FetchEvents, FetchTask};
use serde_json::Value;

use buwiz_office_kit::testing::Headless;

use crate::live::store;

const CHILD: &str = "BUWIZ_UI_TEST_CHILD";

/// Whether this process is the test's child. When it is not, runs the test `module::name` in a
/// child (no backend, one thread), asserts it passed, and says false.
pub(crate) fn in_child(module: &str, name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("{module}::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{log}");
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// A fetch that has nothing more to say.
pub(crate) struct Done;

impl FetchTask for Done {
    fn send(&mut self, _: Vec<u8>) {}
    fn finish(&mut self) {}
    fn pause(&mut self, _: bool) {}
    fn cancel(self: Box<Self>) {}
}

/// Answers a fetch with `answer` as a 200.
pub(crate) fn finish(mut events: FetchEvents, answer: Value) -> Box<dyn FetchTask> {
    events(FetchEvent::Head(HttpHead { status: 200, ..Default::default() }));
    events(FetchEvent::Data(answer.to_string().into_bytes()));
    events(FetchEvent::End);
    Box::new(Done)
}

/// How long a test waits for what it expects before it fails: generous, for a loaded runner.
const PATIENCE: Duration = Duration::from_secs(60);

/// `app` pumped until `done()` (run on the engine thread) says so.
pub(crate) fn wait_until(app: &mut Headless, what: &str, done: impl Fn() -> bool + Send + Clone + 'static) {
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if on(app, done.clone()) {
            return;
        }
        app.pump(Duration::from_millis(10));
    }
    panic!("waited for {what}");
}

/// What the Stage's `job` press runs next.
static JOB: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// `f` on the engine thread, where the store's signals live.
pub(crate) fn on<R: Send + 'static>(app: &mut Headless, f: impl FnOnce() -> R + Send + 'static) -> R {
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

/// The root of these tests: the store, the toasts, the keymap's bindings, and the `job` press.
#[component]
pub(crate) fn Stage() -> impl IntoElement {
    Owner::detached().run(|| {
        let _ = store();
    });
    view()
        .size_full()
        .child(text("probe ready=1"))
        .child(crate::toast::layer())
        .child(crate::keys::bindings())
        .child(view().label("job").w(px(40.0)).h(px(24.0)).on_press(|| {
            if let Some(job) = JOB.lock().unwrap().take() {
                job();
            }
        }))
}
