//! `_meta["buwiz/badge"]` against a real host and the fixture plugin (plain
//! Node): read on every path a tool can be called by, kept, collapsed, sent.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::*;
use crate::helper::keystore::Keystore;
use crate::helper::plugins::{Clock as HostClock, Options, Timeouts};
use crate::helper::release::tests::{trust, KEY, SEED};
use crate::hub::ctx::HubCtx;
use crate::hub::model::Settings;
use crate::hub::sources::Caller;
use crate::hub::tools::Run;
use crate::hub::traits::Clock;
use crate::hub::types::TurnCtx;

const FX: &str = "app.buwiz.fixture";
/// `_meta`'s key (`KEY` here is the release key of the tests' trust list)
const BADGE: &str = super::KEY;
const T1: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";
const T2: &str = "tkt_1123456789ABCDEFGHJKMNPQRS";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1")
}

fn node() -> String {
    std::env::var("PATH").ok().and_then(|p| p.split(':').map(|d| std::path::Path::new(d).join("node")).find(|p| p.exists())).map(|p| p.display().to_string()).unwrap_or_else(|| "node".into())
}

struct Bed {
    dir: PathBuf,
    host: Arc<Host>,
    ctx: Arc<HubCtx>,
}

fn host_in(dir: &std::path::Path) -> Arc<Host> {
    let t = trust(&[FX]);
    let pubs: serde_json::Map<String, Value> = t.publishers.iter().map(|(k, p)| (k.clone(), json!({"alg": "ed25519", "publisher": p.publisher, "public_key": p.public_key}))).collect();
    let approved: Vec<Value> = t.approved.iter().map(|(id, k)| json!({"id": id, "publisher_key_id": k})).collect();
    std::fs::write(dir.join("trust.json"), json!({"v": 1, "publishers": pubs, "approved_plugins": approved}).to_string()).unwrap();
    Host::new(Options {
        db: dir.join("plugins.db"),
        keys: Keystore::from_spec(&format!("file:{}", dir.join("keys").display())).unwrap(),
        trust_file: Some(dir.join("trust.json")),
        log_dir: None,
        clock: HostClock::system(),
        timeouts: Timeouts::default(),
        apps_dir: Some(dir.join("v1/apps")),
    })
    .unwrap()
}

/// The fixture's manifest signed for this run, `edit`ed first, written to `at`.
fn sign(dir: &std::path::Path, at: &str, edit: impl FnOnce(&mut Value)) -> PathBuf {
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join("manifest.json")).unwrap()).unwrap();
    m["publisher"]["key_id"] = json!(KEY);
    edit(&mut m);
    let sources: Vec<(String, Vec<u8>)> = ["board", "card"]
        .iter()
        .filter(|id| m["apps"].as_array().unwrap().iter().any(|a| a["id"] == **id))
        .map(|id| (id.to_string(), std::fs::read(fixtures().join(format!("{id}.tsx"))).unwrap()))
        .collect();
    let signed = crate::v1::signing::sign_manifest(&m, &buwiz_protocol::keys::b64(&SEED), &sources).unwrap();
    let file = dir.join(at);
    std::fs::write(&file, signed.to_string()).unwrap();
    file
}

fn spec_of(manifest: &std::path::Path) -> Value {
    json!({"command": node(), "args": [fixtures().join("server.mjs").display().to_string()], "env": {"FX_MANIFEST": manifest.display().to_string()}})
}

async fn install(host: &Host, spec: &Value) {
    let seen = host.inspect(spec).await.unwrap();
    let mut body = spec.clone();
    body["manifest_sha256"] = seen["manifest_sha256"].clone();
    let done = host.install(&body).await.unwrap();
    assert_eq!(done["state"], "running", "{done}");
}

async fn bed(label: &str) -> Bed {
    let dir = std::env::temp_dir().join(format!("buwiz-badges-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let host = host_in(&dir);
    install(&host, &spec_of(&sign(&dir, "manifest.json", |_| {}))).await;
    let mut ctx = HubCtx::new(dir.clone(), Clock::manual());
    ctx.store.load().unwrap();
    ctx.plugins = Some(host.clone());
    Bed { dir, host, ctx: Arc::new(ctx) }
}

impl Bed {
    /// The fixture's `fx_badge` called as the routine's query tool is.
    async fn call(&self, args: Value) -> Value {
        self.host.call_tool(FX, "fx_badge", &args, Some(&json!({"buwiz/caller": {"origin": "routine", "plugin": FX, "ts": 1}})), None).await.unwrap()
    }

    fn badges(&self) -> Vec<Value> {
        strip(self.host.badges_view(FX).1)
    }

    async fn done(self) {
        self.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A badge list or event without its `rev`, which is checked where it matters.
fn strip(v: Value) -> Vec<Value> {
    v.as_array().unwrap().iter().map(|b| without_rev(b.clone())).collect()
}

fn without_rev(mut b: Value) -> Value {
    assert!(b["rev"].as_i64().is_some_and(|r| r > 0), "a rev: {b}");
    b.as_object_mut().unwrap().remove("rev");
    b
}

fn one(app: &str, ticket: Option<&str>, dot: &str, text: &str) -> Value {
    json!({"app": app, "ticket": ticket, "dot": dot, "text": text})
}

/// An open of the fixture's board, and the signed calls it makes: the app's path.
struct Board {
    sessions: Arc<crate::helper::sessions::Sessions>,
    key: buwiz_protocol::keys::SessionKey,
    open_id: String,
}

impl Board {
    async fn open(host: &Arc<Host>) -> Board {
        let sessions = crate::helper::sessions::Sessions::new(host.clone());
        let key = buwiz_protocol::keys::SessionKey::generate();
        let open_id = buwiz_protocol::keys::b64(&buwiz_protocol::keys::random::<16>());
        let m = host.row(FX).unwrap().manifest();
        let board = m["apps"].as_array().unwrap().iter().find(|a| a["id"] == "board").unwrap();
        let digest = crate::v1::manifest::perms_digest(board.get("permissions")).unwrap();
        let ask = json!({"plugin": FX, "app": "board", "open_id": open_id, "ks_pub": key.public(), "perms_sha256": digest, "not_after": crate::hub::store::now_ms() + 3_600_000});
        sessions.open(&ask).await.unwrap();
        Board { sessions, key, open_id }
    }

    async fn call(&self, args: Value) -> Result<Value, crate::v1::V1Error> {
        let envelope = json!({
            "v": 1, "typ": "buwiz-call", "plugin": FX, "app": "board", "open_id": self.open_id, "tool": "fx_badge", "args": args, "actor": "person",
            "ts": crate::hub::store::now_ms(), "nonce": buwiz_protocol::keys::b64(&buwiz_protocol::keys::random::<16>()),
        });
        let sig = self.key.sign(&envelope).unwrap();
        self.sessions.call(&json!({"envelope": envelope, "sig": sig})).await
    }
}

fn run_in(ctx: &Arc<HubCtx>) -> Run {
    let thread = ctx.store.read(|s| s.active.clone());
    let job = ctx.jobs.create(&thread, Value::Null, None, false);
    Run::new(ctx.clone(), TurnCtx { thread, ..TurnCtx::default() }, job, Arc::new(Settings::from(|_| None)))
}

const VECTORS: &str = include_str!("../../../../buwiz-protocol/src/badge-vectors.json");

/// The vectors every side shares (`vectors/badge.json` of the SDK): well formed, or malformed for the reason.
#[test]
fn what_a_badge_may_be() {
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    let apps: Vec<String> = v["apps"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect();
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() > 40);
    for c in cases {
        let got = parse(&apps, &c["badge"]);
        match c["ok"].as_bool().unwrap() {
            true => assert!(got.is_ok(), "{}: {got:?}", c["badge"]),
            false => assert_eq!(got.unwrap_err(), c["why"].as_str().unwrap(), "{}", c["badge"]),
        }
    }
    // what a well formed one comes to
    let set = |dot: &str, text: &str| Parsed { app: "board".into(), ticket: None, set: Some((dot.into(), text.into())) };
    assert_eq!(parse(&apps, &json!({"app": "board", "dot": "green"})), Ok(set("green", "")));
    assert_eq!(parse(&apps, &json!({"app": "board", "text": "18h"})), Ok(set("none", "18h")), "no dot is none");
    assert_eq!(parse(&apps, &json!({"app": "board", "dot": "red", "ticket": null})), Ok(Parsed { app: "board".into(), ticket: None, set: Some(("red".into(), String::new())) }), "a null ticket is absent");
    assert_eq!(parse(&apps, &json!({"app": "card", "clear": true, "ticket": T1})), Ok(Parsed { app: "card".into(), ticket: Some(T1.into()), set: None }));
}

/// The key is read on a routine's query call, the model's call, an app's signed
/// call and a widget's call, and reaches neither the model nor the app.
#[tokio::test]
async fn the_key_is_read_on_every_path_and_reaches_neither_the_model_nor_the_app() {
    let b = bed("paths").await;
    let badge = |app: &str, ticket: Option<&str>, dot: &str, text: &str| {
        let mut b = json!({"app": app, "dot": dot, "text": text});
        if let Some(t) = ticket {
            b["ticket"] = json!(t);
        }
        json!({"badge": b})
    };
    // the routine's query
    let raw = b.call(badge("board", None, "red", "routine")).await;
    assert_eq!(raw["_meta"][BADGE]["text"], "routine", "the host's own call hands the raw result back; each caller strips it");
    // the model
    let run = run_in(&b.ctx);
    let out = crate::hub::tools::v1::call(&run, "fx_badge", &badge("card", None, "amber", "model")).await;
    assert_eq!(out.is_error, Some(false), "{:?}", out.content);
    let said = out.content.to_string();
    assert!(!said.contains("buwiz/badge") && said.contains("badged"), "the model sees the content only: {said}");
    // an app, signed
    let board = Board::open(&b.host).await;
    let answer = board.call(badge("board", Some(T1), "green", "app")).await.unwrap();
    assert_eq!(answer["ok"], true);
    assert!(answer["result"].get("_meta").is_none(), "the app's result has no _meta: {answer}");
    assert!(!answer.to_string().contains("buwiz/badge"), "{answer}");
    // a widget
    let data = crate::hub::sources::plugin::run(&b.ctx, Caller::Widget("w1"), FX, "fx", "badge", &badge("card", Some(T2), "red", "widget")).await.unwrap();
    assert_eq!(data["badged"], true);
    assert!(!data.to_string().contains("badge\":"), "{data}");
    assert_eq!(
        b.badges(),
        vec![one("board", None, "red", "routine"), one("board", Some(T1), "green", "app"), one("card", None, "amber", "model"), one("card", Some(T2), "red", "widget")]
    );
    b.done().await;
}

/// A malformed badge is dropped, and the result still returns whole.
#[tokio::test]
async fn a_malformed_badge_is_dropped_and_the_result_still_returns() {
    let b = bed("malformed").await;
    for bad in [
        json!("red"),
        json!({"app": "board", "dot": "blue"}),
        json!({"app": "board", "text": "123456789"}),
        json!({"app": "board", "text": "a\nb"}),
        json!({"app": "board"}),
        json!({"app": "board", "clear": true, "dot": "red"}),
        json!({"app": "board", "dot": "red", "ticket": "nope"}),
        // another plugin's app, or none of its apps
        json!({"app": "forms", "dot": "red"}),
        json!({"app": "app.buwiz.other/board", "dot": "red"}),
    ] {
        let r = b.call(json!({"badge": bad})).await;
        assert_eq!(r["structuredContent"]["badged"], true, "{bad}: the result stands");
        assert_ne!(r["isError"], true);
        assert_eq!(b.badges(), Vec::<Value>::new(), "{bad}");
    }
    // the model's call: the answer is the tool's, the badge dropped
    let out = crate::hub::tools::v1::call(&run_in(&b.ctx), "fx_badge", &json!({"badge": {"app": "forms", "dot": "red"}})).await;
    assert_eq!(out.is_error, Some(false));
    assert_eq!(b.badges(), Vec::<Value>::new());
    b.done().await;
}

/// A failed result's badge is ignored; a clear removes one; a ticket narrows it;
/// a plugin that is not approved sets nothing.
#[tokio::test]
async fn a_failed_result_sets_nothing_a_clear_removes_it_and_a_ticket_narrows_it() {
    let b = bed("rules").await;
    let r = b.call(json!({"fail": true, "badge": {"app": "board", "dot": "red"}})).await;
    assert_eq!(r["isError"], true);
    assert_eq!(b.badges(), Vec::<Value>::new(), "isError: ignored");
    b.call(json!({"badge": {"app": "board", "dot": "green", "text": "ok"}})).await;
    b.call(json!({"badge": {"app": "board", "ticket": T1, "dot": "red", "text": "late"}})).await;
    assert_eq!(b.badges(), vec![one("board", None, "green", "ok"), one("board", Some(T1), "red", "late")], "a ticket's own, and the app's for every pin");
    b.call(json!({"badge": {"app": "board", "dot": "amber"}})).await;
    assert_eq!(b.badges()[0], one("board", None, "amber", ""), "a new badge replaces the old, it does not merge");
    b.call(json!({"fail": true, "badge": {"app": "board", "clear": true}})).await;
    assert_eq!(b.badges().len(), 2, "a clear on a failed result is ignored too");
    b.call(json!({"badge": {"app": "board", "clear": true, "ticket": T1}})).await;
    assert_eq!(b.badges(), vec![one("board", None, "amber", "")], "a clear with a ticket takes that one only");
    b.call(json!({"badge": {"app": "board", "clear": true}})).await;
    assert_eq!(b.badges(), Vec::<Value>::new());
    b.call(json!({"badge": {"app": "board", "dot": "none", "text": ""}})).await;
    assert_eq!(b.badges(), Vec::<Value>::new(), "a badge that shows nothing is malformed: not kept");
    // approval is the host's own when it started the plugin: the trust file is read then, not per result
    std::fs::write(b.dir.join("trust.json"), json!({"v": 1, "publishers": {}, "approved_plugins": []}).to_string()).unwrap();
    b.host.disable(FX).await.unwrap();
    let _ = b.host.enable(FX).await;
    let r = b.host.call_tool(FX, "fx_badge", &json!({"badge": {"app": "board", "dot": "red"}}), None, None).await;
    assert!(r.is_err(), "a plugin that is not approved does not run, so no tool of it is called");
    assert_eq!(b.badges(), Vec::<Value>::new(), "not approved when it started: nothing set");
    b.done().await;
}

/// Badges are in the database: a new host on it has them. Uninstall, and an
/// update whose manifest has lost the app, take them away and say so.
#[tokio::test]
async fn badges_survive_a_restart_and_go_with_the_plugin_or_the_app() {
    let b = bed("keep").await;
    b.call(json!({"badge": {"app": "board", "dot": "red", "text": "2"}})).await;
    b.call(json!({"badge": {"app": "card", "ticket": T1, "dot": "green"}})).await;
    let was = b.badges();
    assert_eq!(was.len(), 2);
    b.host.stop_all().await;
    let again = host_in(&b.dir);
    assert_eq!(strip(again.badges_view(FX).1), was, "a restart keeps them");
    // an update without the card app (a newer version): its badge goes
    let mut events = again.badge_events();
    let next = sign(&b.dir, "next.json", |m| {
        m["version"] = json!("1.0.1");
        m["apps"].as_array_mut().unwrap().retain(|a| a["id"] == "board");
        m["apps"][0]["permissions"]["open"]["apps"] = json!([]);
    });
    install(&again, &spec_of(&next)).await;
    assert_eq!(strip(again.badges_view(FX).1), vec![one("board", None, "red", "2")], "the card app left the manifest");
    let gone = without_rev(events.try_recv().unwrap());
    assert_eq!(gone, json!({"plugin": FX, "app": "card", "ticket": T1, "dot": "none", "text": ""}), "and it is said");
    // uninstall
    again.uninstall(FX).await.unwrap();
    assert_eq!(again.badges_view(FX).1, json!([]));
    assert_eq!(without_rev(events.try_recv().unwrap()), json!({"plugin": FX, "app": "board", "ticket": null, "dot": "none", "text": ""}));
    again.stop_all().await;
    let _ = std::fs::remove_dir_all(&b.dir);
}

/// The event's shape, and the listing the app loads carries the badge.
#[tokio::test]
async fn the_event_is_emitted_and_the_plugin_listing_carries_the_badge() {
    let b = bed("event").await;
    let mut events = b.host.badge_events();
    b.call(json!({"badge": {"app": "card", "ticket": T1, "dot": "amber", "text": "18h"}})).await;
    let e = without_rev(events.recv().await.unwrap());
    assert_eq!(e, json!({"plugin": FX, "app": "card", "ticket": T1, "dot": "amber", "text": "18h"}));
    b.call(json!({"badge": {"app": "card", "ticket": T1, "dot": "amber", "text": "18h"}})).await;
    assert!(events.try_recv().is_err(), "the same badge again is no change and no event");
    let row = b.host.row(FX).unwrap();
    let listed = crate::hub::routines::plugins::describe(&b.host, &row, &|_, _| json!({}));
    assert_eq!(strip(listed["badges"].clone()), vec![one("card", Some(T1), "amber", "18h")], "what a freshly started app loads");
    assert!(listed["badges_rev"].as_i64().unwrap() >= listed["badges"][0]["rev"].as_i64().unwrap(), "the list says what rev it is as of");
    b.done().await;
}

/// A plugin that changes one badge more than once a second is collapsed to the
/// latest value: the first goes out, and one more with the last, a second on.
#[tokio::test]
async fn rapid_changes_collapse_to_the_latest() {
    let b = bed("rate").await;
    let mut events = b.host.badge_events();
    for n in 1..=6 {
        b.call(json!({"badge": {"app": "board", "dot": "red", "text": n.to_string()}})).await;
    }
    assert_eq!(b.badges(), vec![one("board", None, "red", "6")], "the store has the latest at once");
    assert_eq!(events.recv().await.unwrap()["text"], "1", "the first goes out at once");
    assert!(events.try_recv().is_err(), "the rest wait");
    let last = tokio::time::timeout(Duration::from_millis(1800), events.recv()).await.expect("the latest follows").unwrap();
    assert_eq!(last["text"], "6", "the one that follows is the latest, not each");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(events.try_recv().is_err(), "nothing between: two events for six changes");
    // another badge has its own second, and a quiet one goes out at once
    b.call(json!({"badge": {"app": "card", "dot": "green"}})).await;
    assert_eq!(events.recv().await.unwrap()["app"], "card");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    b.call(json!({"badge": {"app": "board", "dot": "amber"}})).await;
    assert_eq!(events.recv().await.unwrap()["dot"], "amber");
    b.done().await;
}

/// A plugin holds 32 badges; a set past that is dropped, a clear always works and frees a place,
/// a change of one it holds still goes through, and no gate is kept for a badge that is gone.
#[tokio::test]
async fn a_plugin_holds_at_most_32_badges() {
    let b = bed("cap").await;
    let tickets: Vec<String> = (0..CAP + 1).map(|i| format!("tkt_{:0>26}", format!("{i:X}"))).collect();
    for t in &tickets[..CAP] {
        b.call(json!({"badge": {"app": "board", "ticket": t, "dot": "green"}})).await;
    }
    assert_eq!(b.badges().len(), CAP);
    b.call(json!({"badge": {"app": "board", "ticket": &tickets[CAP], "dot": "red"}})).await;
    b.call(json!({"badge": {"app": "card", "dot": "red"}})).await;
    assert_eq!(b.badges().len(), CAP, "past the cap: dropped");
    b.call(json!({"badge": {"app": "board", "ticket": &tickets[0], "dot": "red", "text": "late"}})).await;
    assert_eq!(b.badges().iter().find(|x| x["ticket"] == json!(tickets[0])).unwrap()["dot"], "red", "one it holds can change");
    b.call(json!({"badge": {"app": "board", "ticket": &tickets[1], "clear": true}})).await;
    assert_eq!(b.badges().len(), CAP - 1, "a clear works");
    b.call(json!({"badge": {"app": "card", "dot": "red"}})).await;
    assert_eq!(b.badges().len(), CAP, "and frees a place");
    assert!(b.host.badge_gates.lock().unwrap().len() <= CAP + 1);
    b.done().await;
}

/// An announcement still waiting when the plugin is uninstalled says nothing and leaves no gate.
#[tokio::test]
async fn a_waiting_announcement_does_nothing_after_an_uninstall() {
    let b = bed("late").await;
    let mut events = b.host.badge_events();
    for n in 1..=4 {
        b.call(json!({"badge": {"app": "board", "dot": "red", "text": n.to_string()}})).await;
    }
    assert_eq!(events.recv().await.unwrap()["text"], "1", "the first went out");
    b.host.uninstall(FX).await.unwrap();
    let gone = events.recv().await.unwrap();
    assert_eq!((gone["dot"].clone(), gone["text"].clone()), (json!("none"), json!("")), "the removal's own");
    assert!(b.host.badge_gates.lock().unwrap().is_empty(), "no gate is left");
    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert!(events.try_recv().is_err(), "nothing after it: the timer found nothing to do");
    assert!(b.host.badge_gates.lock().unwrap().is_empty(), "and made no gate");
    b.done().await;
}

/// Each badge, event and removal has a rev that only goes up, and the list says the rev it is as of.
#[tokio::test]
async fn revs_only_go_up() {
    let b = bed("rev").await;
    let mut events = b.host.badge_events();
    b.call(json!({"badge": {"app": "board", "dot": "red"}})).await;
    let first = events.recv().await.unwrap()["rev"].as_i64().unwrap();
    let (at, list) = b.host.badges_view(FX);
    assert_eq!(list[0]["rev"].as_i64().unwrap(), first, "the event and the list say the same rev");
    assert!(at >= first);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    b.call(json!({"badge": {"app": "board", "clear": true}})).await;
    let cleared = events.recv().await.unwrap();
    assert_eq!(cleared["dot"], "none");
    assert!(cleared["rev"].as_i64().unwrap() > first, "a clear's rev is newer than what it clears");
    let (later, none) = b.host.badges_view(FX);
    assert_eq!(none, json!([]));
    assert!(later >= cleared["rev"].as_i64().unwrap() && later > at, "the list's rev covers the removal");
    b.done().await;
}
