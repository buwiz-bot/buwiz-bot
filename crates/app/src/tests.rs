use std::{
    sync::{Arc, Mutex, mpsc},
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use nsh_core::{
    ApproxTextEngine, MountBatch, MountItem,
    element::{AnyElement, Root, signal, text},
};
use nsh_engine::{App, Engine, EngineConfig, MountSink};
use serde_json::{Value, json};

use super::*;

/// A host that records what the screen sent and answers every call with
/// its data, or `answer`'s.
#[derive(Default)]
struct FakeHost {
    granted: Vec<&'static str>,
    sent: Mutex<Vec<(String, Value, bool)>>,
    answer: Mutex<Option<Result<Value, BuwizError>>>,
    listeners: Mutex<Vec<Listener>>,
    failed: Mutex<Vec<String>>,
}

impl FakeHost {
    fn emit(&self, name: &str, data: Value) {
        for l in self.listeners.lock().unwrap().iter() {
            l(name, &data);
        }
    }
}

impl Host for FakeHost {
    fn params(&self) -> Map<String, Value> {
        let p = json!({
            "props": {"page": 2}, "app": {"plugin": "app.buwiz.fixture", "app": "board"},
            "granted": self.granted, "viewport": {"width": 360, "height": 160, "placement": "inline"},
        });
        p.as_object().unwrap().clone()
    }
    fn post(&self, method: &str, data: Value) {
        self.sent.lock().unwrap().push((method.to_owned(), data, false));
    }
    fn call(&self, method: &str, data: Value, done: Done) {
        self.sent.lock().unwrap().push((method.to_owned(), data.clone(), true));
        match self.answer.lock().unwrap().clone() {
            Some(a) => done(a),
            None => done(Ok(data)),
        }
    }
    fn listen(&self, listener: Listener) {
        self.listeners.lock().unwrap().push(listener);
    }
    fn fail(&self, message: &str) {
        self.failed.lock().unwrap().push(message.to_owned());
    }
}

struct Wakes;
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {}
}

/// `f`'s output: the fake host answers at once, so one poll is enough.
fn now<T>(f: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(Wakes));
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("not answered"),
    }
}

fn bridge(granted: Vec<&'static str>) -> (Arc<FakeHost>, Bridge) {
    let host = Arc::new(FakeHost { granted, ..Default::default() });
    (host.clone(), Bridge::new(host))
}

#[test]
fn the_params_reach_the_bridge() {
    let (_, b) = bridge(vec![]);
    assert_eq!(b.props(), json!({"page": 2}));
    assert_eq!(b.app()["app"], "board");
    assert_eq!(b.viewport_now(), Viewport { width: 360.0, height: 160.0, placement: Placement::Inline });
}

#[test]
fn calls_carry_the_tsx_bridges_data() {
    let (host, b) = bridge(vec!["buwiz.call"]);
    assert_eq!(now(b.call("fx_echo", Value::Null)).unwrap(), json!({"tool": "fx_echo", "args": {}}));
    now(b.actions(vec![Action::new("save", "Save").primary()])).unwrap();
    now(b.meta(Some(Meta { text: "Draft".into(), pill: true }))).unwrap();
    now(b.settle("Saved")).unwrap();
    b.fit(120.4).unwrap();
    b.ready();
    let sent = host.sent.lock().unwrap();
    let names: Vec<(&str, bool)> = sent.iter().map(|(m, _, call)| (m.as_str(), *call)).collect();
    assert_eq!(
        names,
        [("buwiz.call", true), ("buwiz.actions", true), ("buwiz.meta", true), ("buwiz.settle", true), ("buwiz.fit", false), ("buwiz.ready", false)]
    );
    assert_eq!(sent[1].1, json!({"actions": [{"id": "save", "label": "Save", "primary": true, "disabled": false}]}));
    assert_eq!(sent[2].1, json!({"meta": {"text": "Draft", "pill": true}}));
    assert_eq!(sent[4].1, json!({"height": 121.0}));
}

#[test]
fn a_rejection_is_a_buwiz_error_and_a_dropped_answer_is_internal() {
    let (host, b) = bridge(vec![]);
    *host.answer.lock().unwrap() = Some(Err(BuwizError::new("permission_denied", false, "no")));
    assert_eq!(now(b.call("x", json!({}))).unwrap_err().code, "permission_denied");
    struct Drops;
    impl Host for Drops {
        fn params(&self) -> Map<String, Value> {
            Map::new()
        }
        fn post(&self, _: &str, _: Value) {}
        fn call(&self, _: &str, _: Value, done: Done) {
            drop(done);
        }
        fn listen(&self, _: Listener) {}
        fn fail(&self, _: &str) {}
    }
    let e = now(Bridge::new(Arc::new(Drops)).call("x", json!({}))).unwrap_err();
    assert_eq!((e.code.as_str(), e.retryable), ("internal", true));
}

#[test]
fn a_bad_fit_is_refused_here() {
    let (host, b) = bridge(vec![]);
    assert_eq!(b.fit(f32::NAN).unwrap_err().code, "invalid_args");
    assert_eq!(b.fit(0.0).unwrap_err().code, "invalid_args");
    assert!(host.sent.lock().unwrap().is_empty());
}

#[test]
fn ungranted_capabilities_are_none() {
    let (_, none) = bridge(vec!["buwiz.call"]);
    assert!(none.storage().is_none());
    assert!(none.attach("item", "x").is_none());
    assert!(none.pick_file(PickOptions::default()).is_none());
    assert!(none.copy("x").is_none());
    assert!(none.preview("fx_doc", Value::Null, None).is_none());
    let (host, all) = bridge(vec!["buwiz.storage.get", "buwiz.attach", "buwiz.pickFile", "buwiz.copy", "buwiz.preview"]);
    let s = all.storage().expect("granted");
    now(s.set("k", "v")).unwrap();
    now(all.copy("hi").expect("granted")).unwrap();
    now(all.preview("fx_doc", Value::Null, Some("Doc")).expect("granted")).unwrap();
    let sent = host.sent.lock().unwrap();
    assert_eq!(sent[0], ("buwiz.storage.set".to_owned(), json!({"key": "k", "value": "v"}), true));
    assert_eq!(sent[2].1, json!({"tool": "fx_doc", "title": "Doc"}));
}

#[test]
fn picked_files_read_their_bytes() {
    let (host, b) = bridge(vec!["buwiz.pickFile"]);
    *host.answer.lock().unwrap() = Some(Ok(json!([{"id": "f1", "name": "a.csv", "size": 3, "type": "text/csv"}])));
    let files = now(b.pick_file(PickOptions { extensions: vec![".csv".into()], multiple: false }).unwrap()).unwrap();
    assert_eq!((files[0].name.as_str(), files[0].size, files[0].mime.as_str()), ("a.csv", 3, "text/csv"));
    *host.answer.lock().unwrap() = Some(Ok(json!("YWJj")));
    assert_eq!(now(files[0].read()).unwrap(), b"abc");
    assert_eq!(host.sent.lock().unwrap()[1].1, json!({"read": "f1"}));
}

#[test]
fn a_viewport_event_updates_the_bridge() {
    let (host, b) = bridge(vec![]);
    host.emit("buwiz.viewport", json!({"width": 800, "height": 600, "placement": "full"}));
    assert_eq!(b.viewport_now(), Viewport { width: 800.0, height: 600.0, placement: Placement::Full });
}

// ---------------------------------------------------------------------------
// On an engine: the boundary, the signals and the listeners.
// ---------------------------------------------------------------------------

struct Sink(mpsc::Sender<MountBatch>);

impl MountSink for Sink {
    fn submit(&self, batch: MountBatch) {
        let _ = self.0.send(batch);
    }
}

/// Runs `root` until a text starting with `prefix` shows, or `until` holds.
/// `during` runs each round with what showed so far.
fn run(root: Root, prefix: &str, until: impl Fn() -> bool, during: impl Fn(&[String])) -> Vec<String> {
    let (tx, rx) = mpsc::channel();
    let engine = Engine::start(EngineConfig::new(App::Rust(root)), || ApproxTextEngine, Sink(tx)).expect("engine starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut shown = Vec::new();
    while Instant::now() < deadline {
        during(&shown);
        if let Ok(batch) = rx.recv_timeout(Duration::from_millis(20)) {
            for item in batch.items {
                if let MountItem::Text { text, .. } = item {
                    shown.push(text.text.to_string());
                }
            }
        }
        if until() || (!prefix.is_empty() && shown.iter().any(|t| t.starts_with(prefix))) {
            break;
        }
    }
    drop(engine);
    shown
}

#[test]
fn a_panicking_screen_reaches_the_host() {
    let (host, b) = bridge(vec![]);
    let h = host.clone();
    run(
        root(b, |_| -> AnyElement { panic!("the ledger broke") }),
        "",
        move || !h.failed.lock().unwrap().is_empty(),
        |_| {},
    );
    assert_eq!(*host.failed.lock().unwrap(), ["the ledger broke"]);
}

#[test]
fn a_panic_in_a_listener_reaches_the_host() {
    let (host, b) = bridge(vec![]);
    let (h, e) = (host.clone(), host.clone());
    run(
        root(b, |b| {
            b.on_action(|id| panic!("pressed {id}"));
            text("up")
        }),
        "",
        move || !h.failed.lock().unwrap().is_empty(),
        move |shown| {
            if !shown.is_empty() {
                e.emit("buwiz.action", json!({"id": "save"}));
            }
        },
    );
    assert_eq!(*host.failed.lock().unwrap(), ["pressed save"]);
}

#[test]
fn the_viewport_signal_and_actions_follow_the_host() {
    let (host, b) = bridge(vec![]);
    let e = host.clone();
    let shown = run(
        root(b, |b| {
            let vp = b.viewport();
            let pressed = signal(String::from("-"));
            b.on_action(move |id| pressed.set(id.to_owned()));
            text(move || format!("vp {} {:?} {}", vp.get().width, vp.get().placement, pressed.get()))
        }),
        "vp 800 Full save",
        || false,
        move |shown| {
            if !shown.is_empty() {
                e.emit("buwiz.viewport", json!({"width": 800, "height": 600, "placement": "full"}));
                e.emit("buwiz.action", json!({"id": "save"}));
            }
        },
    );
    assert!(shown.iter().any(|t| t == "vp 360 Inline -"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "vp 800 Full save"), "{shown:?}");
}

#[test]
fn a_panic_in_a_bridge_task_reaches_the_host() {
    let (host, b) = bridge(vec![]);
    let h = host.clone();
    run(
        root(b, |b| {
            b.spawn(async { panic!("the call broke") });
            text("up")
        }),
        "",
        move || !h.failed.lock().unwrap().is_empty(),
        |_| {},
    );
    assert_eq!(*host.failed.lock().unwrap(), ["the call broke"]);
}
