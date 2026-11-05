//! Computers against the mock provider: seats, screens, wake and sleep, the tools' reach.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use buwiz_computer::mock::{MockProvider, PASSWORD, TOKEN};
use buwiz_computer::{Power, Problem};
use serde_json::{json, Value};

use super::*;
use crate::hub::model::Settings;
use crate::hub::policy::{self, keys, Pos};
use crate::hub::store::Bot;
use crate::hub::tools::computer::{self as tools, own_path, NAMES};
use crate::hub::tools::{exec_tool, tool_list, Out, Run};
use crate::hub::traits::Clock;
use crate::hub::types::TurnCtx;

struct Bed {
    ctx: Arc<HubCtx>,
    computers: Arc<Computers>,
    mock: Arc<MockProvider>,
    /// (bot id, thread)
    ann: (String, String),
    bea: (String, String),
}

fn bed(label: &str) -> Bed {
    let dir = std::env::temp_dir().join(format!("buwiz-computer-hub-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ctx = HubCtx::new(dir, Clock::manual());
    ctx.store.load().unwrap();
    let mock = Arc::new(MockProvider::default());
    let computers = Arc::new(Computers::new(mock.clone()));
    // no catalog: nothing here asks a real opencodex
    computers.set_sight(std::collections::HashMap::new());
    ctx.ext.insert(computers.clone());
    let (a, b) = ctx
        .store
        .write(|s| {
            let (a, b) = (s.fresh_thread("Ann"), s.fresh_thread("Bea"));
            let ids = (a.id.clone(), b.id.clone());
            s.list.push(a);
            s.list.push(b);
            s.bots.push(Bot::new("b_ann", "Ann", "blob", "#FFFFFF", Some(ids.0.clone())));
            s.bots.push(Bot::new("b_bea", "Bea", "blob", "#FFFFFF", Some(ids.1.clone())));
            ids
        })
        .unwrap();
    Bed { ctx: Arc::new(ctx), computers, mock, ann: ("b_ann".into(), a), bea: ("b_bea".into(), b) }
}

fn run(b: &Bed, thread: &str, routine: Option<&str>) -> Run {
    let job = b.ctx.jobs.create(thread, Value::Null, None, false);
    let turn = TurnCtx { thread: thread.to_owned(), routine: routine.map(str::to_owned), ..TurnCtx::default() };
    Run::new(b.ctx.clone(), turn, job, Arc::new(Settings::from(|_| None)))
}

async fn awake(b: &Bed, bot: &str) -> String {
    b.computers.click(&b.ctx, bot).await.unwrap();
    for _ in 0..400 {
        if b.computers.status(&b.ctx, bot).await["state"] == "awake" {
            return Computers::seat(&b.ctx, bot).unwrap().computer;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("it never woke: {}", b.computers.status(&b.ctx, bot).await);
}

async fn call(r: &Run, name: &str, input: Value) -> Out {
    exec_tool(r, &json!({"name": name, "input": input})).await
}

fn words(o: &Out) -> String {
    o.content.as_str().map(str::to_owned).unwrap_or_else(|| o.content.to_string())
}

async fn offered(r: &Run) -> Vec<String> {
    tool_list(r).await.iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).filter(|n| n.starts_with("computer_")).collect()
}

fn set(b: &Bed, bot: &str, key: &str, state: &str) {
    b.ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO bot_tools(bot_id, key, enabled, state, actor, at) VALUES(?,?,?,?,'human',0)", rusqlite::params![bot, key, i64::from(state != "off"), state])).unwrap();
}

#[tokio::test]
async fn a_click_makes_the_computer_and_wakes_it_with_this_bots_screen() {
    let b = bed("click");
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["state"], "absent");
    awake(&b, &b.ann.0).await;
    assert_eq!(b.mock.calls()[1..3], ["wake".to_owned(), "screen_up b_ann".to_owned()]);
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["display"], ":2");
}

#[tokio::test]
async fn it_shows_waking_while_it_wakes() {
    let b = bed("waking");
    b.mock.hold.store(true, Ordering::SeqCst);
    b.computers.click(&b.ctx, &b.ann.0).await.unwrap();
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["state"], "waking");
    // a second click doesn't start a second wake
    b.computers.click(&b.ctx, &b.ann.0).await.unwrap();
    b.mock.hold.store(false, Ordering::SeqCst);
    for _ in 0..400 {
        if b.computers.status(&b.ctx, &b.ann.0).await["state"] == "awake" {
            assert_eq!(b.mock.calls().iter().filter(|c| *c == "wake").count(), 1);
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("it never woke");
}

#[tokio::test]
async fn a_problem_is_a_sentence_in_the_status() {
    let b = bed("sentence");
    *b.mock.problem.lock().unwrap() = Some(Problem::ProviderDown("Docker".into()));
    assert!(b.computers.click(&b.ctx, &b.ann.0).await.is_err());
    let s = b.computers.status(&b.ctx, &b.ann.0).await;
    assert_eq!(s["state"], "error");
    assert!(s["message"].as_str().unwrap().starts_with("Docker isn't running."), "{s}");
    // the next click tries again
    *b.mock.problem.lock().unwrap() = None;
    awake(&b, &b.ann.0).await;
}

/// Every test bed sees: no opencodex is there to ask.
#[test]
fn a_model_no_catalog_names_counts_as_seeing() {
    assert_eq!(sight_of(&json!({})).get("x"), None);
}

#[tokio::test]
async fn two_bots_share_a_computer_and_each_has_its_own_screen() {
    let b = bed("share");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    let (ann, bea) = (Computers::seat(&b.ctx, &b.ann.0).unwrap(), Computers::seat(&b.ctx, &b.bea.0).unwrap());
    assert_eq!(ann.computer, bea.computer);
    let (sa, sb) = (ann.screen.unwrap(), bea.screen.unwrap());
    assert_ne!(sa.display, sb.display);
    assert_ne!(sa.folder, sb.folder);

    // each Bot's tools land on its own screen
    let (ra, rb) = (run(&b, &b.ann.1, None), run(&b, &b.bea.1, None));
    call(&ra, "computer_click", json!({"x": 1, "y": 2})).await;
    call(&rb, "computer_click", json!({"x": 3, "y": 4})).await;
    let calls = b.mock.calls();
    assert!(calls.iter().any(|c| c.starts_with(&format!("input {} Click {{ x: 1", sa.display))), "{calls:?}");
    assert!(calls.iter().any(|c| c.starts_with(&format!("input {} Click {{ x: 3", sb.display))), "{calls:?}");
}

#[tokio::test]
async fn nothing_a_model_sends_moves_it_to_another_bots_screen() {
    let b = bed("reach");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    let theirs = Computers::seat(&b.ctx, &b.bea.0).unwrap().screen.unwrap();
    let r = run(&b, &b.ann.1, None);
    for extra in [json!({"display": theirs.display}), json!({"bot": b.bea.0}), json!({"screen": theirs.display, "computer": id})] {
        let mut input = json!({"x": 1, "y": 1});
        input.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        call(&r, "computer_click", input).await;
    }
    assert!(!b.mock.calls().iter().any(|c| c.starts_with(&format!("input {}", theirs.display))), "{:?}", b.mock.calls());
    // and no schema offers such a field
    for def in tools::defs() {
        for word in ["display", "bot", "computer", "screen"] {
            assert!(def["input_schema"]["properties"].get(word).is_none(), "{def}");
        }
    }
    // files too
    for path in [format!("../{}/x", b.bea.0), format!("/workspace/agents/{}/x", b.bea.0), "/etc/passwd".to_owned(), "a/../../x".to_owned()] {
        let o = call(&r, "computer_read_file", json!({"path": path})).await;
        assert_eq!(o.is_error, Some(true), "{path}");
    }
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("read ")));
}

#[test]
fn a_path_stays_in_the_folder() {
    assert_eq!(own_path("agents/a", "x/y.txt").as_deref(), Some("agents/a/x/y.txt"));
    assert_eq!(own_path("agents/a", "/workspace/agents/a/y.txt").as_deref(), Some("agents/a/y.txt"));
    assert_eq!(own_path("agents/a", "agents/a/y.txt").as_deref(), Some("agents/a/y.txt"));
    for bad in ["", "/etc/passwd", "../b/x", "x/../../y", "/workspace/agents/b/x", "./x"] {
        assert_eq!(own_path("agents/a", bad), None, "{bad}");
    }
}

#[tokio::test]
async fn commands_stay_off_on_a_shared_computer() {
    let b = bed("shared-run");
    let id = awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    assert_eq!(call(&r, "computer_run", json!({"command": "ls"})).await.is_error, None, "alone, it runs");
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    let o = call(&r, "computer_run", json!({"command": "ls"})).await;
    assert_eq!(o.is_error, Some(true));
    assert!(words(&o).contains("several Bots share"), "{}", words(&o));
    assert_eq!(b.mock.calls().iter().filter(|c| c.ends_with(" ls")).count(), 1, "the refused one never ran");
}

#[tokio::test]
async fn a_box_is_guarded_before_any_use_after_a_broker_start() {
    let b = bed("guard");
    awake(&b, &b.ann.0).await;
    let again = Computers::new(b.mock.clone());
    b.mock.calls.lock().unwrap().clear();
    again.live(&b.ctx, &b.ann.0).await.unwrap();
    assert_eq!(b.mock.calls(), ["guard"]);
}

#[tokio::test]
async fn a_box_that_fails_its_guard_is_refused_and_stays_refused() {
    let b = bed("unsafe");
    awake(&b, &b.ann.0).await;
    *b.mock.unsafe_now.lock().unwrap() = Some(Problem::Unwalled("no python3".into()));
    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "computer_screenshot", json!({})).await;
    assert!(words(&o).contains("couldn't be walled off"), "{}", words(&o));
    // even once the check would pass: a person has to wake it again
    *b.mock.unsafe_now.lock().unwrap() = None;
    b.mock.set_power(Power::Awake);
    assert_eq!(b.computers.live(&b.ctx, &b.ann.0).await.err().map(|p| p.code()), Some("unwalled"));
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["state"], "error");
    awake(&b, &b.ann.0).await;
    assert!(b.computers.live(&b.ctx, &b.ann.0).await.is_ok());
}

#[tokio::test]
async fn a_link_out_of_the_folder_is_refused_for_reads_and_writes() {
    let b = bed("link");
    awake(&b, &b.ann.0).await;
    let (folder, r) = (format!("agents/{}", b.ann.0), run(&b, &b.ann.1, None));
    b.mock.links.lock().unwrap().insert(format!("{folder}/out"), format!("/workspace/agents/{}/x", b.bea.0));
    b.mock.calls.lock().unwrap().clear();
    assert_eq!(call(&r, "computer_read_file", json!({"path": "out"})).await.is_error, Some(true));
    assert_eq!(call(&r, "computer_write_file", json!({"path": "out", "content": "x"})).await.is_error, Some(true));
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("read ") || c.starts_with("write ")), "{:?}", b.mock.calls());
    assert_eq!(call(&r, "computer_read_file", json!({"path": "fine"})).await.is_error, None);
}

#[tokio::test]
async fn a_lost_record_doesnt_strand_a_bot() {
    for broken in [None, Some("{\"provider")] {
        let b = bed("lost");
        let old = awake(&b, &b.ann.0).await;
        b.computers.attach(&b.ctx, &b.bea.0, &old).await.unwrap();
        let path = Computers::dir(&b.ctx, &old).join("machine.json");
        match broken {
            Some(text) => std::fs::write(&path, text).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        let fresh = Computers::new(b.mock.clone());
        let fresh = Arc::new(fresh);
        assert_eq!(fresh.live(&b.ctx, &b.ann.0).await.err(), Some(Problem::Gone));
        assert_eq!(fresh.status(&b.ctx, &b.ann.0).await["code"], "gone");
        fresh.click(&b.ctx, &b.ann.0).await.unwrap();
        for _ in 0..400 {
            if fresh.status(&b.ctx, &b.ann.0).await["state"] == "awake" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let new = Computers::seat(&b.ctx, &b.ann.0).unwrap().computer;
        assert_ne!(new, old);
        assert_eq!(Computers::seat(&b.ctx, &b.bea.0).unwrap().computer, new, "the Bots at it come along");
        assert!(b.mock.calls().contains(&format!("sweep {old}")), "what's left of the old one goes: {:?}", b.mock.calls());
        assert_eq!(Computers::ids(&b.ctx), [new]);
    }
}

#[tokio::test]
async fn leaving_with_the_record_lost_is_leaving() {
    let b = bed("lost-leave");
    let id = awake(&b, &b.ann.0).await;
    std::fs::remove_file(Computers::dir(&b.ctx, &id).join("machine.json")).unwrap();
    let fresh = Computers::new(b.mock.clone());
    fresh.detach(&b.ctx, &b.ann.0).await.unwrap();
    assert!(Computers::seat(&b.ctx, &b.ann.0).is_none());
    assert_eq!(Computers::ids_where(&b.ctx, "1=1"), Vec::<String>::new());
}

#[tokio::test]
async fn a_bot_goes_with_docker_off_and_the_computer_follows_when_it_is_back() {
    let b = bed("docker-off");
    let id = awake(&b, &b.ann.0).await;
    *b.mock.problem.lock().unwrap() = Some(Problem::ProviderDown("Docker".into()));
    b.computers.detach(&b.ctx, &b.ann.0).await.unwrap();
    assert!(Computers::seat(&b.ctx, &b.ann.0).is_none());
    assert_eq!(Computers::ids(&b.ctx), Vec::<String>::new());
    assert_eq!(Computers::ids_where(&b.ctx, "gone_at IS NOT NULL"), std::slice::from_ref(&id), "it's still owed a removal");
    *b.mock.problem.lock().unwrap() = None;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(Computers::ids_where(&b.ctx, "1=1"), Vec::<String>::new());
    assert!(!Computers::dir(&b.ctx, &id).exists());
}

#[tokio::test]
async fn a_bot_that_leaves_a_sleeping_computer_loses_its_screen_at_the_next_wake() {
    let b = bed("leave-asleep");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    b.computers.detach(&b.ctx, &b.bea.0).await.unwrap();
    awake(&b, &b.ann.0).await;
    for _ in 0..400 {
        if b.mock.calls().contains(&"screen_down b_bea".to_owned()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(b.mock.calls().contains(&"screen_down b_bea".to_owned()), "{:?}", b.mock.calls());
    assert_eq!(*b.mock.screens.lock().unwrap(), ["b_ann"]);
}

#[tokio::test]
async fn a_computer_id_is_never_a_path() {
    let b = bed("id");
    awake(&b, &b.ann.0).await;
    for bad in ["../../etc", "cmp_../x", "cmp_", "/tmp/x", "cmp_zz"] {
        assert_eq!(b.computers.attach(&b.ctx, &b.bea.0, bad).await, Err(Problem::Gone), "{bad}");
    }
}

#[tokio::test]
async fn a_model_that_cant_see_gets_no_tools_and_a_plain_reason() {
    let b = bed("blind");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    b.computers.set_sight(std::collections::HashMap::from([(r.settings.model.clone(), false)]));
    assert_eq!(offered(&r).await, Vec::<String>::new());
    assert!(tools::prompt(&r).await.contains("can't see images"));
    let o = call(&r, "computer_screenshot", json!({})).await;
    assert!(words(&o).contains("can't see images"), "{}", words(&o));
    b.computers.set_sight(std::collections::HashMap::from([(r.settings.model.clone(), true)]));
    assert_eq!(offered(&r).await.len(), NAMES.len());
    assert!(tools::prompt(&r).await.contains("the screen is for acting"));
}

#[test]
fn the_catalog_says_who_sees() {
    let known = sight_of(&json!({"data": [
        {"id": "a", "capabilities": {"supports_vision": true}},
        {"id": "b", "capabilities": {"input_modalities": ["text"]}},
        {"id": "c", "capabilities": {"input_modalities": ["text", "image"]}},
        {"id": "d"},
    ]}));
    assert_eq!((known.get("a"), known.get("b"), known.get("c"), known.get("d")), (Some(&true), Some(&false), Some(&true), None));
}

#[tokio::test]
async fn sleep_is_the_computers_and_a_wake_from_any_bot_wakes_it_for_all() {
    let b = bed("sleep-all");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    b.computers.sleep(&b.ctx, &id, Sleep::Deep).await.unwrap();
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["state"], "asleep");
    assert_eq!(b.computers.status(&b.ctx, &b.bea.0).await["state"], "asleep");
    b.mock.calls.lock().unwrap().clear();
    awake(&b, &b.bea.0).await;
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["state"], "awake");
    let calls = b.mock.calls();
    assert!(calls.contains(&"screen_up b_ann".to_owned()) && calls.contains(&"screen_up b_bea".to_owned()), "a start brings every screen up: {calls:?}");
}

#[tokio::test]
async fn a_resumed_computer_only_brings_up_the_screens_it_lacks() {
    let b = bed("resume");
    let id = awake(&b, &b.ann.0).await;
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    b.mock.calls.lock().unwrap().clear();
    awake(&b, &b.ann.0).await;
    let calls = b.mock.calls();
    assert!(!calls.contains(&"screen_up b_ann".to_owned()), "its screen is as it was: {calls:?}");
    assert!(calls.contains(&"screen_up b_bea".to_owned()), "and the Bot that joined gets one: {calls:?}");
}

#[tokio::test]
async fn a_computer_whose_container_is_gone_is_made_again_on_a_click() {
    let b = bed("absent");
    let old = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &old).await.unwrap();
    b.mock.set_power(Power::Absent);
    assert_eq!(b.computers.live(&b.ctx, &b.ann.0).await.err(), Some(Problem::Gone));
    let new = awake(&b, &b.ann.0).await;
    assert_ne!(new, old, "the record still read, and there was nothing to wake");
    assert_eq!(Computers::seat(&b.ctx, &b.bea.0).unwrap().computer, new, "the Bots at it come along");
    assert_eq!(Computers::ids(&b.ctx), [new]);
    assert!(!Computers::dir(&b.ctx, &old).exists());
}

#[tokio::test]
async fn a_docker_that_is_down_doesnt_read_as_a_gone_computer() {
    let b = bed("down-click");
    let old = awake(&b, &b.ann.0).await;
    b.computers.sleep(&b.ctx, &old, Sleep::Light).await.unwrap();
    *b.mock.problem.lock().unwrap() = Some(Problem::ProviderDown("Docker".into()));
    b.computers.click(&b.ctx, &b.ann.0).await.unwrap();
    assert_eq!(Computers::seat(&b.ctx, &b.ann.0).unwrap().computer, old);
}

#[tokio::test]
async fn two_first_clicks_make_one_computer() {
    let b = bed("race");
    b.mock.create_ms.store(30, Ordering::SeqCst);
    let (one, two) = tokio::join!(b.computers.click(&b.ctx, &b.ann.0), b.computers.click(&b.ctx, &b.ann.0));
    one.unwrap();
    two.unwrap();
    assert_eq!(b.mock.calls().iter().filter(|c| c.starts_with("create")).count(), 1, "{:?}", b.mock.calls());
    assert_eq!(Computers::ids_where(&b.ctx, "1=1").len(), 1);
}

#[tokio::test]
async fn a_new_computer_that_cant_get_its_seat_is_removed() {
    let b = bed("no-seat");
    b.ctx.db.with(|c| c.execute("ALTER TABLE computer_seats RENAME TO seats_away", [])).unwrap();
    let got = b.computers.click(&b.ctx, &b.ann.0).await;
    b.ctx.db.with(|c| c.execute("ALTER TABLE seats_away RENAME TO computer_seats", [])).unwrap();
    assert!(got.is_err());
    assert!(b.mock.calls().contains(&"destroy".to_owned()), "{:?}", b.mock.calls());
    assert_eq!(Computers::ids_where(&b.ctx, "1=1"), Vec::<String>::new());
}

#[tokio::test]
async fn a_computer_that_cant_be_recorded_doesnt_stay_behind() {
    // its record can't be saved
    let b = bed("no-save");
    b.mock.poison_save.store(true, Ordering::SeqCst);
    assert!(b.computers.click(&b.ctx, &b.ann.0).await.is_err());
    assert!(b.mock.calls().contains(&"destroy".to_owned()), "{:?}", b.mock.calls());
    assert_eq!(std::fs::read_dir(b.ctx.data.join("computers")).unwrap().count(), 0, "nor its folder");
    // its row can't be written
    let b = bed("no-row");
    b.ctx.db.with(|c| c.execute("ALTER TABLE computers RENAME TO computers_away", [])).unwrap();
    assert!(b.computers.click(&b.ctx, &b.ann.0).await.is_err());
    b.ctx.db.with(|c| c.execute("ALTER TABLE computers_away RENAME TO computers", [])).unwrap();
    assert!(b.mock.calls().contains(&"destroy".to_owned()), "{:?}", b.mock.calls());
    assert_eq!(std::fs::read_dir(b.ctx.data.join("computers")).unwrap().count(), 0);
}

#[tokio::test]
async fn a_bot_delete_that_cant_be_written_leaves_its_computer_alone() {
    let b = bed("delete");
    let id = awake(&b, &b.ann.0).await;
    b.ctx.db.with(|c| c.execute_batch("CREATE TRIGGER no_delete BEFORE DELETE ON bots BEGIN SELECT RAISE(ABORT, 'disk full'); END;")).unwrap();
    let body = bytes::Bytes::from(json!({"id": b.ann.0}).to_string());
    let req = HubRequest { method: hyper::Method::POST, url: "/bot/delete".into(), headers: hyper::HeaderMap::new(), body, params: Vec::new() };
    let res = crate::hub::routes::bots::bot_delete(b.ctx.clone(), req).await;
    assert_ne!(res.status(), StatusCode::OK);
    assert!(!b.mock.calls().contains(&"destroy".to_owned()), "a live Bot lost its files: {:?}", b.mock.calls());
    assert_eq!(Computers::seat(&b.ctx, &b.ann.0).map(|s| s.computer), Some(id));
}

#[tokio::test]
async fn a_bot_delete_takes_its_computer_with_it() {
    let b = bed("delete-ok");
    awake(&b, &b.ann.0).await;
    let body = bytes::Bytes::from(json!({"id": b.ann.0}).to_string());
    let req = HubRequest { method: hyper::Method::POST, url: "/bot/delete".into(), headers: hyper::HeaderMap::new(), body, params: Vec::new() };
    assert_eq!(crate::hub::routes::bots::bot_delete(b.ctx.clone(), req).await.status(), StatusCode::OK);
    assert!(b.mock.calls().contains(&"destroy".to_owned()));
    assert_eq!(Computers::ids_where(&b.ctx, "1=1"), Vec::<String>::new());
}

#[tokio::test]
async fn it_sleeps_when_nobody_needs_it() {
    let mut b = bed("idle");
    let computers = Arc::new(Computers { idle: Duration::from_millis(0), deep: Duration::from_millis(0), ..Computers::new(b.mock.clone()) });
    b.computers = computers.clone();
    // the same records, under this provider
    let ctx = b.ctx.clone();
    let id = {
        computers.click(&ctx, &b.ann.0).await.unwrap();
        for _ in 0..400 {
            if computers.status(&ctx, &b.ann.0).await["state"] == "awake" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Computers::seat(&ctx, &b.ann.0).unwrap().computer
    };
    computers.sleep_the_idle(&ctx).await;
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Asleep);
    assert!(b.mock.calls().contains(&"sleep Light".to_owned()));
    // and later lets go of its memory, once
    computers.sleep_the_idle(&ctx).await;
    assert!(b.mock.calls().contains(&"sleep Deep".to_owned()));
    let deep = b.mock.calls().iter().filter(|c| *c == "sleep Deep").count();
    computers.sleep_the_idle(&ctx).await;
    assert_eq!(b.mock.calls().iter().filter(|c| *c == "sleep Deep").count(), deep);
    let _ = id;
}

#[tokio::test]
async fn a_recently_used_computer_stays_up() {
    let b = bed("used");
    awake(&b, &b.ann.0).await;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Awake);
}

#[tokio::test]
async fn the_records_outlive_the_broker() {
    let b = bed("restart");
    let id = awake(&b, &b.ann.0).await;
    // a new broker: the same data dir, nothing in memory
    let again = Computers::new(b.mock.clone());
    assert_eq!(again.status(&b.ctx, &b.ann.0).await["computer"], json!(id));
    assert_eq!(again.status(&b.ctx, &b.ann.0).await["state"], "awake");
    assert!(again.live(&b.ctx, &b.ann.0).await.is_ok(), "and it finds the saved computer and screen");
}

#[tokio::test]
async fn the_last_bot_leaving_removes_the_computer_and_an_earlier_one_doesnt() {
    let b = bed("detach");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    b.computers.detach(&b.ctx, &b.ann.0).await.unwrap();
    assert!(b.mock.calls().contains(&"screen_down b_ann".to_owned()) && !b.mock.calls().contains(&"destroy".to_owned()));
    assert!(Computers::seat(&b.ctx, &b.ann.0).is_none());
    b.computers.detach(&b.ctx, &b.bea.0).await.unwrap();
    assert!(b.mock.calls().contains(&"destroy".to_owned()));
    assert!(!Computers::dir(&b.ctx, &id).exists());
    assert_eq!(Computers::ids(&b.ctx), Vec::<String>::new());
}

#[tokio::test]
async fn the_model_cant_make_or_wake_one() {
    let b = bed("no-wake");
    let r = run(&b, &b.ann.1, None);
    assert_eq!(offered(&r).await, Vec::<String>::new(), "no computer, no tools");
    let o = call(&r, "computer_screenshot", json!({})).await;
    assert_eq!(o.is_error, Some(true));
    assert!(b.mock.calls().is_empty(), "{:?}", b.mock.calls());

    let id = awake(&b, &b.ann.0).await;
    assert_eq!(offered(&r).await.len(), NAMES.len());
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    b.mock.calls.lock().unwrap().clear();
    let o = call(&r, "computer_click", json!({"x": 1, "y": 1})).await;
    assert_eq!(o.is_error, Some(true));
    assert!(words(&o).contains("Only a person can wake it"), "{}", words(&o));
    assert!(!b.mock.calls().contains(&"wake".to_owned()));
}

#[tokio::test]
async fn a_plain_chat_has_none() {
    let b = bed("chat");
    awake(&b, &b.ann.0).await;
    let thread = b.ctx.store.read(|s| s.list.iter().find(|t| t.id != b.ann.1 && t.id != b.bea.1).map(|t| t.id.clone())).unwrap();
    let r = run(&b, &thread, None);
    assert_eq!(offered(&r).await, Vec::<String>::new());
    assert_eq!(call(&r, "computer_screenshot", json!({})).await.is_error, Some(true));
    assert!(!keys::all_keys(&b.ctx, policy::CHATS).contains(&"computer".to_owned()));
}

#[tokio::test]
async fn the_switch_is_on_by_default_and_off_takes_the_tools_away() {
    let b = bed("switch");
    awake(&b, &b.ann.0).await;
    assert_eq!(policy::tool_state(&b.ctx, &b.ann.0, tools::KEY), Pos::On);
    let r = run(&b, &b.ann.1, None);
    assert!(keys::all_keys(&b.ctx, &b.ann.0).contains(&"computer".to_owned()));
    set(&b, &b.ann.0, tools::KEY, "off");
    assert_eq!(offered(&r).await, Vec::<String>::new());
    b.mock.calls.lock().unwrap().clear();
    let o = call(&r, "computer_click", json!({"x": 1, "y": 1})).await;
    assert!(words(&o).contains("computer is off"), "{}", words(&o));
    assert!(b.mock.calls().is_empty());
}

#[tokio::test]
async fn at_ask_a_read_runs_and_a_change_needs_someone_to_say_yes() {
    let b = bed("ask");
    awake(&b, &b.ann.0).await;
    set(&b, &b.ann.0, tools::KEY, "ask");
    // an agent's request has nobody to ask: the change is refused rather than parked
    let job = b.ctx.jobs.create(&b.ann.1, Value::Null, None, false);
    let turn = TurnCtx { thread: b.ann.1.clone(), acting_bot: Some(b.ann.0.clone()), ..TurnCtx::default() };
    let r = Run::new(b.ctx.clone(), turn, job, Arc::new(Settings::from(|_| None)));
    assert_eq!(call(&r, "computer_screenshot", json!({})).await.is_error, None);
    b.mock.calls.lock().unwrap().clear();
    let o = call(&r, "computer_click", json!({"x": 1, "y": 1})).await;
    assert_eq!(o.is_error, Some(true), "{}", words(&o));
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("input")));
}

#[tokio::test]
async fn a_routine_gets_it_only_if_a_person_turned_that_on() {
    let b = bed("routine");
    let id = awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, Some("rt_1"));
    assert_eq!(policy::tool_state(&b.ctx, &b.ann.0, tools::ROUTINES_KEY), Pos::Off);
    assert_eq!(offered(&r).await, Vec::<String>::new());
    let o = call(&r, "computer_click", json!({"x": 1, "y": 1})).await;
    assert_eq!(o.is_error, Some(true));

    set(&b, &b.ann.0, tools::ROUTINES_KEY, "on");
    assert_eq!(offered(&r).await.len(), NAMES.len());
    // then it may wake it, with nobody to click
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    assert_eq!(call(&r, "computer_screenshot", json!({})).await.is_error, None);
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Awake);

    // Ask stays Ask: nobody can answer, so a change is refused
    set(&b, &b.ann.0, tools::KEY, "ask");
    assert_eq!(call(&r, "computer_click", json!({"x": 1, "y": 1})).await.is_error, Some(true));
}

#[tokio::test]
async fn no_tool_can_widen_the_routine_switch() {
    let b = bed("widen");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "tools_set", json!({"key": tools::ROUTINES_KEY, "state": "on"})).await;
    assert!(words(&o).contains("proposed"), "{}", words(&o));
    assert_eq!(policy::tool_state(&b.ctx, &b.ann.0, tools::ROUTINES_KEY), Pos::Off);
}

#[tokio::test]
async fn what_a_computer_shows_counts_as_outside_content() {
    let b = bed("outside");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    assert!(!r.state().external);
    call(&r, "computer_screenshot", json!({})).await;
    assert!(r.state().external && r.state().beyond_screens);
    // and it doesn't make the computer's own next step ask
    assert_eq!(call(&r, "computer_click", json!({"x": 1, "y": 1})).await.is_error, None);
}

#[tokio::test]
async fn an_action_answers_with_the_settled_screen_for_every_provider() {
    let b = bed("image");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "computer_click", json!({"x": 1, "y": 1})).await;
    let blocks = o.content.as_array().unwrap();
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[1]["source"]["media_type"], "image/jpeg");
    let facts: Value = serde_json::from_str(blocks[0]["text"].as_str().unwrap()).unwrap();
    assert!(facts.get("waited_ms").is_some() && facts.get("timed_out").is_some(), "{facts}");
}

#[tokio::test]
async fn the_secrets_never_reach_the_model_the_status_or_the_list() {
    let b = bed("secrets");
    let id = awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    *b.mock.stdout.lock().unwrap() = format!("leak {TOKEN} and {PASSWORD}");
    for (name, input) in [("computer_run", json!({"command": "env"})), ("computer_read_file", json!({"path": "x"}))] {
        let shown = words(&call(&r, name, input).await);
        assert!(!shown.contains(TOKEN) && !shown.contains(PASSWORD) && shown.contains("[hidden]"), "{shown}");
    }
    let shown = format!("{} {}", b.computers.status(&b.ctx, &b.ann.0).await, Computers::list(&b.ctx));
    assert!(!shown.contains(TOKEN) && !shown.contains(PASSWORD), "{shown}");
    // nor app.db, and the file that holds them is the owner's alone
    let dump: String = b.ctx.db.with(|c| c.query_row("SELECT id || provider || coalesce(created_for,'') FROM computers", [], |r| r.get(0))).unwrap();
    assert!(!dump.contains(TOKEN));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(Computers::dir(&b.ctx, &id).join("machine.json")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn the_open_url_holds_a_ticket_and_no_password() {
    let b = bed("open");
    awake(&b, &b.ann.0).await;
    let live = b.computers.live(&b.ctx, &b.ann.0).await.unwrap();
    let viewer = live.provider.viewer(&live.machine, &live.screen).await.unwrap().unwrap();
    let url = b.computers.gate().await.unwrap().open(&b.ann.0, viewer).unwrap();
    assert!(url.contains("ticket=") && !url.contains(PASSWORD), "{url}");
}

#[tokio::test]
async fn a_route_answers_only_the_apps_token() {
    let b = bed("routes");
    let req = HubRequest { method: hyper::Method::POST, url: "/v1/host/computer/bot/b_ann/wake".into(), headers: hyper::HeaderMap::new(), body: bytes::Bytes::new(), params: vec![b.ann.0.clone()] };
    assert_eq!(wake(b.ctx.clone(), req).await.status(), StatusCode::UNAUTHORIZED);
    assert!(b.mock.calls().is_empty());
}

#[test]
fn fetch_url_cant_reach_a_computers_ports() {
    let port = buwiz_computer::PORT_FIRST + 3;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for host in ["127.0.0.1", "localhost", "LOCALHOST", "127.1", "[::1]", "0.0.0.0"] {
        let url = format!("http://{host}:{port}/v1/health");
        let out = runtime.block_on(crate::hub::tools::builtin::fetch_url(&json!({"url": url})));
        assert_eq!(out.unwrap_err(), "fetch failed", "{host}");
    }
}

#[test]
fn a_computer_can_be_pinned_to_a_group() {
    let b = bed("pin");
    let group = crate::hub::library::groups::new_group(&b.ctx, &json!({"name": "Desks"})).unwrap();
    let item = json!({"id": group["id"], "kind": "app", "ref": "computer:b_ann", "title": "Ann's computer", "op": "add"});
    crate::hub::library::groups::group_item(&b.ctx, &item).unwrap();
    let bad = json!({"id": group["id"], "kind": "app", "ref": "computer:../x", "title": "x", "op": "add"});
    assert!(crate::hub::library::groups::group_item(&b.ctx, &bad).is_err());
}
