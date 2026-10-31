//! A first-party native screen run for real: its root on a headless engine
//! (`App::Rust`), its bridge on the open's handlers ([`wire`]), with the
//! TSX tests' fake broker and person. A native open gets what a TSX one
//! does: the same checks, signing, pacing and card chrome.

use std::sync::{Arc, Mutex, mpsc};

use nsh_core::{ApproxTextEngine, MountItem};
use nsh_engine::{App, Engine, EngineConfig};
use serde_json::{Value, json};

use super::*;
use crate::mini_v1::{
    load::{NativeScreen, Prepared},
    screens,
    settle::{Phase, Settle},
    test_screens::{self, PLUGIN},
};

/// A running open of the fixture's native `screen`.
struct NativeOpen {
    engine: Option<Engine>,
    rx: mpsc::Receiver<MountBatch>,
    ctx: Arc<OpenContext>,
    broker: Arc<FakeBroker>,
    ui: Arc<TestUi>,
    emit: Emit,
    failed: Arc<Mutex<Vec<String>>>,
    root: PathBuf,
    shown: Vec<String>,
}

fn native_prepared(screen: &str, perms: &Value) -> Arc<Prepared> {
    let mut plugin = plugin(PLUGIN, perms);
    plugin.first_party = true;
    Arc::new(Prepared {
        key: V1Key { plugin: PLUGIN.to_owned(), app: "board".to_owned(), ticket: None, props: Default::default() },
        plugin,
        title: "Fixture Board".to_owned(),
        fingerprint: format!("native:{screen}@1"),
        permissions: Permissions::parse(Some(perms)).unwrap(),
        permissions_json: Some(perms.clone()),
        js: format!("// native:{screen}@1"),
        native: Some(NativeScreen { screen: screen.to_owned(), abi: 1 }),
    })
}

/// The open's context, as `native::start` makes it.
fn context(prepared: Arc<Prepared>, perms: &Value) -> (Arc<OpenContext>, Arc<FakeBroker>, Arc<TestUi>, PathBuf) {
    test_screens::register();
    let session = Session::open(&CertBroker::honest(), PLUGIN, "board", Some(perms), Some(1)).unwrap();
    let broker = Arc::new(FakeBroker::default());
    let ui = Arc::new(TestUi::default());
    let root = crate::mini_v1::storage::scratch_root("native");
    let ctx = OpenContext::new(prepared, session, broker.clone(), ui.clone(), &root, Limits::default());
    (ctx, broker, ui, root)
}

impl NativeOpen {
    fn start(screen: &str, placement: &str, perms: Value) -> NativeOpen {
        let (ctx, broker, ui, root) = context(native_prepared(screen, &perms), &perms);
        let failed: Arc<Mutex<Vec<String>>> = Arc::default();
        let f = failed.clone();
        let fail = Arc::new(move |m: &str| f.lock().unwrap().push(m.to_owned()));
        let viewport = json!({"width": 360, "height": 160, "placement": placement});
        let mounted = screens::mount(&ctx, viewport, fail).expect("the screen mounts");
        let (tx, rx) = mpsc::channel();
        let engine = Engine::start(EngineConfig::new(App::Rust(mounted.root)), || ApproxTextEngine, Sink(tx, Default::default()))
            .expect("engine starts");
        NativeOpen { engine: Some(engine), rx, ctx, broker, ui, emit: mounted.emit, failed, root, shown: Vec::new() }
    }

    /// Waits until the screen shows text starting with `prefix`; that text.
    fn wait(&mut self, prefix: &str) -> String {
        self.wait_for(|t| t.starts_with(prefix)).unwrap_or_else(|| panic!("no {prefix:?}; shown: {:?}", self.shown))
    }

    fn wait_for(&mut self, f: impl Fn(&str) -> bool) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(t) = self.shown.iter().find(|t| f(t)) {
                return Some(t.clone());
            }
            if !self.failed.lock().unwrap().is_empty() {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(batch) = self.rx.recv_timeout(left.min(Duration::from_millis(50))) else {
                if left.is_zero() {
                    return None;
                }
                continue;
            };
            for item in batch.items {
                if let MountItem::Text { text, .. } = item {
                    self.shown.push(text.text.to_string());
                }
            }
        }
    }
}

impl Drop for NativeOpen {
    fn drop(&mut self) {
        self.engine.take();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn nat_01_a_native_open_mounts_the_screens_root() {
    let mut o = NativeOpen::start("fx.board", "full", board_perms());
    assert_eq!(o.wait("vp"), "vp 360 Full | - | waiting");
    let done = o.wait("vp 360 Full | - | done");
    assert!(!done.contains("needs a newer"), "{done}");
    assert!(o.failed.lock().unwrap().is_empty());
}

#[test]
fn nat_02_fit_and_ready_drive_the_cards_settle_hold() {
    let mut o = NativeOpen::start("fx.board", "inline", board_perms());
    o.wait("vp 360 Inline | - | done");
    // clamped to the manifest's range (no inline range: the default) and
    // paced, as a TSX app's
    let fits = o.ui.fits.lock().unwrap().clone();
    assert_eq!(fits, [crate::mini_v1::cards::fit_height(&o.ctx.prepared.plugin.app("board").cloned().unwrap(), 130.0)]);
    assert_eq!(o.ui.readies.load(Ordering::SeqCst), 1);
    // the card's hold, as `mini.rs` drives it: wait_ready holds until the
    // ready, then two stable ticks settle it
    let t0 = Instant::now();
    let mut s = Settle::new(t0);
    s.loaded(true);
    assert_eq!(s.tick(t0 + settle_ms(400), Some(1)), None);
    assert_eq!(s.tick(t0 + settle_ms(433), Some(1)), None);
    assert_eq!(s.tick(t0 + settle_ms(466), Some(1)), None, "held for its ready");
    for _ in 0..o.ui.readies.load(Ordering::SeqCst) {
        s.ready();
    }
    // its layout counts as stable only from the ready
    assert_eq!(s.tick(t0 + settle_ms(500), Some(1)), None);
    assert_eq!(s.tick(t0 + settle_ms(533), Some(1)), Some(Phase::Settled));
}

fn settle_ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn nat_03_a_call_goes_out_signed_by_the_session_and_an_ungranted_tool_is_denied() {
    let mut o = NativeOpen::start("fx.board", "full", board_perms());
    let done = o.wait("vp 360 Full | - | done");
    assert!(done.ends_with(r#"{"code":"1601C"} permission_denied"#), "{done}");
    let calls = o.broker.calls.lock().unwrap();
    // only the granted tool went out, signed by the open's session key
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (env, sig) = &calls[0];
    let s = &o.ctx.session;
    assert_eq!((env["plugin"].as_str(), env["app"].as_str(), env["tool"].as_str()), (Some(PLUGIN), Some("board"), Some("fx_echo")));
    assert_eq!(env["open_id"], s.open_id.as_str());
    keys::verify(&s.public_key(), env, sig).expect("the session key signed it");
}

#[test]
fn nat_04_ungranted_capabilities_are_none() {
    let mut granted = NativeOpen::start("fx.board", "full", board_perms());
    // the board's grants: storage, attach, pick_file, copy; no preview
    assert!(granted.wait("vp 360 Full | - | done").contains(" storage,attach,pick,copy "));
    let mut none = NativeOpen::start("fx.board", "full", json!({"tools": ["fx_echo"]}));
    assert!(none.wait("vp 360 Full | - | done").contains("done true - "));
}

#[test]
fn nat_05_a_footer_press_reaches_on_action() {
    let mut o = NativeOpen::start("fx.board", "inline", board_perms());
    o.wait("vp 360 Inline | - | done");
    let actions = o.ui.actions.lock().unwrap().clone();
    assert_eq!(actions.len(), 1);
    assert_eq!((actions[0][0].id.as_str(), actions[0][0].primary), ("save", true));
    let press = o.ui.press.lock().unwrap().clone().expect("the card got its press");
    // a button the screen does not list reaches nothing
    press("delete");
    press("save");
    assert!(o.wait("vp 360 Inline | save").starts_with("vp 360 Inline | save | done"));
    assert!(!o.shown.iter().any(|t| t.contains("| delete |")));
}

#[test]
fn nat_06_viewport_updates_reach_the_screen() {
    let mut o = NativeOpen::start("fx.board", "full", board_perms());
    o.wait("vp 360 Full | - | done");
    (o.emit)("buwiz.viewport", json!({"width": 800, "height": 600, "placement": "full"}));
    o.wait("vp 800 Full");
}

#[test]
fn nat_07_a_panicking_screen_shows_the_error_banner() {
    let mut o = NativeOpen::start("fx.panics", "full", board_perms());
    assert_eq!(o.wait_for(|_| false), None);
    assert_eq!(*o.failed.lock().unwrap(), ["the ledger broke"]);
    // the banner's text, as `native::start` sets it
    assert_eq!(load::say::runtime("Fixture Board", "the ledger broke"), "Fixture Board stopped: the ledger broke.");
}

#[test]
fn nat_08_a_missing_screen_says_it_needs_a_newer_buwiz_bot() {
    let perms = board_perms();
    for (screen, abi) in [("fx.ledger", 1), ("fx.board", 2)] {
        let mut prepared = (*native_prepared(screen, &perms)).clone();
        prepared.native = Some(NativeScreen { screen: screen.to_owned(), abi });
        let (ctx, _, _, root) = context(Arc::new(prepared), &perms);
        let e = screens::mount(&ctx, json!({"width": 1, "height": 1, "placement": "full"}), Arc::new(|_: &str| {})).err().unwrap();
        assert_eq!((e.code.as_str(), e.message.as_str()), ("not_supported", "Fixture Board: this screen needs a newer Buwiz Bot."));
        let _ = std::fs::remove_dir_all(root);
    }
    // and a screen whose build panics fails as a load does
    let (ctx, _, _, root) = context(native_prepared("fx.build-panics", &perms), &perms);
    let e = screens::mount(&ctx, json!({"width": 1, "height": 1, "placement": "full"}), Arc::new(|_: &str| {})).err().unwrap();
    assert_eq!((e.code.as_str(), e.message.as_str()), ("internal", "Fixture Board stopped: no ledger to build."));
    let _ = std::fs::remove_dir_all(root);
}

/// The open-time comparison the PR records: the board as a native screen
/// and as the equivalent TSX app, from the open's start (its session, its
/// bridge, its engine) until its calls are answered, the median of seven.
/// The TSX open compiles its module (`load::build`, what the module cache
/// skips on a later open); that compile alone is measured too.
#[test]
fn nat_09_native_and_tsx_open_times() {
    const TSX: &str = r#"
        await buwiz.actions([{ id: "save", label: "Save", primary: true }]);
        buwiz.fit(130);
        buwiz.ready();
        const echo = await buwiz.call("fx_echo", { code: "1601C" });
        let secret = "ran";
        try { await buwiz.call("fx_secret", {}); } catch (e) { secret = e.code; }
        return "done " + JSON.stringify(echo) + " " + secret;
    "#;
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let (mut native, mut tsx, mut compile) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..7 {
        let t0 = Instant::now();
        let mut o = NativeOpen::start("fx.board", "inline", board_perms());
        o.wait("vp 360 Inline | - | done");
        native.push(ms(t0.elapsed()));
        drop(o);
        let source = app(TSX);
        let t1 = Instant::now();
        load::build("board", "Fixture Board", &source).expect("compiles");
        compile.push(ms(t1.elapsed()));
        let t2 = Instant::now();
        let mut o = Open::start_placed("inline", FIXTURE, board_perms(), &source, Limits::default(), |_| {});
        o.wait("done");
        tsx.push(ms(t2.elapsed()));
    }
    let median = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let line = format!(
        "mini v1 timing: test board open (median of 7): native {:.1} ms, tsx {:.1} ms of which compile {:.1} ms",
        median(&native),
        median(&tsx),
        median(&compile)
    );
    log::info!("{line}");
    eprintln!("{line}; runs native {native:.1?} tsx {tsx:.1?} compile {compile:.1?}");
}
