//! Jobs against the mock provider: the computer stays up for one, and how one ends.

use std::sync::atomic::Ordering;

use super::*;
use crate::hub::agent::runs::tests::{agent, until, Script};
use crate::hub::computer::jobs::{LIMIT, MAX_LIMIT, PER_BOT};
use crate::hub::policy::ask;
use crate::hub::traits::{ModelApi, TurnRunner};

fn lazy(label: &str) -> Bed {
    bed_with(label, |p| Computers { idle: Duration::ZERO, deep: Duration::ZERO, ..Computers::new(p) })
}

/// What the computer says of every job from here on, and that a start works.
fn job_is(b: &Bed, said: &str) {
    let mut replies = b.mock.replies.lock().unwrap();
    replies.clear();
    replies.push(("setsid".to_owned(), Ok("4242\n".to_owned())));
    replies.push(("echo running".to_owned(), Ok(format!("{said}\n"))));
    replies.push(("tail -c".to_owned(), Ok("what it printed\n".to_owned())));
}

/// `b`'s first Bot's job, started by a person's chat turn; → its id.
async fn started(b: &Bed, input: Value) -> String {
    job_is(b, "running");
    let r = run(b, &b.ann.1, None);
    let o = call(&r, "computer_job_start", input).await;
    assert_eq!(o.is_error, None, "{}", words(&o));
    over(b, &r);
    let v: Value = serde_json::from_str(o.content.as_str().unwrap()).unwrap();
    v["id"].as_str().unwrap().to_owned()
}

/// `r`'s turn has ended: its chat isn't busy.
fn over(b: &Bed, r: &Run) {
    b.ctx.jobs.with(r.job, |j| j.done = true);
}

fn row_of(b: &Bed, id: &str) -> (String, Option<i64>) {
    b.ctx.db.with(|c| c.query_row("SELECT state, exit_code FROM computer_jobs WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap()
}

fn notices(b: &Bed) -> Vec<(String, String)> {
    b.ctx.store.read(|s| {
        s.thread(&b.ann.1).unwrap().turns.iter().filter(|u| u.notice.as_ref().is_some_and(|n| n["kind"] == "computer_job")).map(|u| (u.notice.as_ref().unwrap()["state"].as_str().unwrap().to_owned(), u.text.clone().unwrap())).collect()
    })
}

fn power(b: &Bed) -> Power {
    b.mock.power.lock().unwrap().clone()
}

fn asleep(b: &Bed) -> bool {
    power(b) == Power::Asleep
}

fn with_model(b: &mut Bed, script: &Arc<Script>) {
    let ctx = Arc::get_mut(&mut b.ctx).expect("before anything holds the hub");
    ctx.local = Some(Arc::new(crate::hub::model::testkit::Local::default()));
    ctx.model = Some(script.clone() as Arc<dyn ModelApi>);
    ctx.turns = Some(Arc::new(agent()) as Arc<dyn TurnRunner>);
}

#[tokio::test]
async fn a_job_runs_in_its_bots_folder_from_a_script_the_broker_wrote() {
    let b = bed("job-start");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "sleep 90; echo done > out.txt", "name": "long one"})).await;
    let calls = b.mock.calls();
    assert!(calls.iter().any(|c| c.starts_with(&format!("write /workspace/agents/b_ann/.jobs/{id}/run.sh"))), "{calls:?}");
    assert!(calls.iter().any(|c| c.contains("setsid nohup") && c.contains(&format!(".jobs/{id}/run.sh"))), "{calls:?}");
    assert_eq!(row_of(&b, &id), ("running".to_owned(), None));
    let tab = b.computers.status(&b.ctx, &b.ann.0).await;
    assert_eq!(tab["jobs"][0]["name"], "long one", "{tab}");
    assert_eq!(tab["jobs"][0]["secondsLeft"], LIMIT.as_secs());
}

#[tokio::test]
async fn a_limit_the_model_names_stays_in_bounds() {
    let b = bed("job-limit-bounds");
    awake(&b, &b.ann.0).await;
    job_is(&b, "running");
    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "computer_job_start", json!({"command": "x", "time_limit_s": 999_999})).await;
    assert!(words(&o).contains(&format!("\"time_limit_s\":{}", MAX_LIMIT.as_secs())), "{}", words(&o));
}

#[tokio::test]
async fn a_failed_start_leaves_no_job_behind() {
    let b = lazy("job-failed");
    awake(&b, &b.ann.0).await;
    b.mock.replies.lock().unwrap().push(("setsid".to_owned(), Err(Problem::Failed("no".to_owned()))));
    let r = run(&b, &b.ann.1, None);
    assert_eq!(call(&r, "computer_job_start", json!({"command": "x"})).await.is_error, Some(true));
    assert!(Computers::jobs_of(&b.ctx, &b.ann.0).is_empty());
    drop(r);
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(asleep(&b), "nothing holds the computer up");
}

#[tokio::test]
async fn a_bot_has_two_jobs_at_most() {
    let b = bed("job-two");
    awake(&b, &b.ann.0).await;
    for _ in 0..PER_BOT {
        started(&b, json!({"command": "x"})).await;
    }
    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "computer_job_start", json!({"command": "x"})).await;
    assert_eq!(o.is_error, Some(true));
    assert!(words(&o).contains("most"), "{}", words(&o));
}

#[tokio::test]
async fn a_running_job_keeps_the_computer_up_past_idle() {
    let b = lazy("job-idle");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    for _ in 0..3 {
        b.computers.sleep_the_idle(&b.ctx).await;
        assert_eq!(power(&b), Power::Awake, "it slept under a job");
    }
    job_is(&b, "exit 0");
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(row_of(&b, &id), ("done".to_owned(), Some(0)));
    assert!(asleep(&b), "with the job over, idle is idle");
}

#[tokio::test]
async fn a_running_job_keeps_the_computer_up_until_its_limit() {
    let b = lazy("job-limit");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x", "time_limit_s": 3600})).await;
    b.ctx.clock.manual_step(Some(b.ctx.clock.now() + 3_599_000.0), false);
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(power(&b), Power::Awake, "not yet");
    assert!(!b.mock.calls().iter().any(|c| c.contains("kill -TERM")));
    b.ctx.clock.manual_step(Some(b.ctx.clock.now() + 1_000.0), false);
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(b.mock.calls().iter().any(|c| c.contains("kill -TERM")), "it's stopped, not left");
    assert_eq!(row_of(&b, &id).0, "limit");
    assert_eq!(notices(&b)[0].1, "Job reached its time limit and was stopped.");
    assert!(asleep(&b));
}

#[tokio::test]
async fn a_job_past_its_limit_that_wont_stop_doesnt_hold_the_computer() {
    let b = lazy("job-stuck");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x", "time_limit_s": 3600})).await;
    b.mock.replies.lock().unwrap().insert(0, ("kill -TERM".to_owned(), Err(Problem::Failed("no".to_owned()))));
    b.ctx.clock.manual_step(Some(b.ctx.clock.now() + 3_600_000.0), false);
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(asleep(&b));
    assert_eq!(row_of(&b, &id).0, "running", "the next look it can reach stops it");
}

#[tokio::test]
async fn a_persons_sleep_still_wins() {
    let b = lazy("job-sleep");
    let computer = awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    b.computers.sleep(&b.ctx, &computer, Sleep::Light).await.unwrap();
    assert!(asleep(&b), "a person's click isn't refused");
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["jobs"][0]["frozen"], true);

    // frozen, it isn't ended, and it isn't swept deeper, which would end it
    b.mock.calls.lock().unwrap().clear();
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(row_of(&b, &id).0, "running");
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("sleep")), "{:?}", b.mock.calls());

    // waking lets it go on
    awake(&b, &b.ann.0).await;
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["jobs"][0]["frozen"], false);
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(power(&b), Power::Awake);
}

#[tokio::test]
async fn a_job_frozen_past_its_limit_lets_go_and_is_stopped_once_it_can_be() {
    let b = lazy("job-frozen-limit");
    let computer = awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x", "time_limit_s": 3600})).await;
    b.computers.sleep(&b.ctx, &computer, Sleep::Light).await.unwrap();
    b.ctx.clock.manual_step(Some(b.ctx.clock.now() + 3_600_000.0), false);
    b.mock.calls.lock().unwrap().clear();
    b.computers.sleep_the_idle(&b.ctx).await;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(b.mock.calls().iter().any(|c| c.starts_with("sleep")), "the deep sleep can take it: {:?}", b.mock.calls());
    assert_eq!(row_of(&b, &id).0, "running", "nothing could reach it to stop it");
    awake(&b, &b.ann.0).await;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(row_of(&b, &id).0, "limit");
}

#[tokio::test]
async fn a_job_survives_a_broker_restart() {
    let b = lazy("job-restart");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;

    // what the next broker has: the row, and an empty `Now`
    let fresh = Arc::new(Computers { idle: Duration::ZERO, deep: Duration::ZERO, ..Computers::new(b.mock.clone()) });
    fresh.load_jobs(&b.ctx);
    fresh.sleep_the_idle(&b.ctx).await;
    assert_eq!(power(&b), Power::Awake, "a new broker slept what the last one's job needs");
    assert_eq!(row_of(&b, &id).0, "running");

    job_is(&b, "exit 0");
    fresh.sleep_the_idle(&b.ctx).await;
    assert_eq!(row_of(&b, &id), ("done".to_owned(), Some(0)));
    assert_eq!(notices(&b).len(), 1, "it's told of by the one that saw it end");
}

#[tokio::test]
async fn a_shared_computer_refuses_a_job_unless_apart() {
    let b = bed("job-shared");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    job_is(&b, "running");
    *b.mock.apart.lock().unwrap() = None;
    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "computer_job_start", json!({"command": "x"})).await;
    assert_eq!(o.is_error, Some(true));
    assert!(words(&o).contains("several Bots share"), "{}", words(&o));
    assert!(!b.mock.calls().iter().any(|c| c.contains("run.sh")), "nothing was written or started");
    *b.mock.apart.lock().unwrap() = Some(Err(Problem::Failed("no".to_owned())));
    assert_eq!(call(&r, "computer_job_start", json!({"command": "x"})).await.is_error, Some(true), "a failed read is a no");
    *b.mock.apart.lock().unwrap() = Some(Ok(true));
    assert_eq!(call(&r, "computer_job_start", json!({"command": "x"})).await.is_error, None);
}

#[tokio::test]
async fn a_finished_job_posts_a_notice_and_calls_no_model() {
    let mut b = lazy("job-free");
    let script = Arc::new(Script::default());
    with_model(&mut b, &script);
    awake(&b, &b.ann.0).await;
    started(&b, json!({"command": "x", "name": "the build"})).await;
    job_is(&b, "exit 3");
    b.computers.sleep_the_idle(&b.ctx).await;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(notices(&b), [("done".to_owned(), "Job \"the build\" failed with exit code 3.".to_owned())], "told once");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 0);
    assert!(b.ctx.jobs.running().is_empty());
}

#[tokio::test]
async fn with_routines_on_a_finished_job_starts_one_turn() {
    let mut b = lazy("job-turn");
    let script = Arc::new(Script::default());
    script.say("It built.");
    with_model(&mut b, &script);
    set(&b, &b.ann.0, tools::ROUTINES_KEY, "on");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    job_is(&b, "exit 0");
    b.computers.sleep_the_idle(&b.ctx).await;
    until("no turn started", || script.calls.load(Ordering::SeqCst) == 1).await;
    until("the turn never ended", || !b.ctx.jobs.busy(&b.ann.1)).await;
    let prompt = script.bodies.lock().unwrap()[0].to_string();
    assert!(prompt.contains(&id) && prompt.contains("computer_job_status"), "{prompt}");
    b.computers.sleep_the_idle(&b.ctx).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 1, "one turn, once");
    assert_eq!(notices(&b).len(), 1);
}

#[tokio::test]
async fn a_jobs_turn_waits_for_a_run_under_way_and_a_switch_turned_off_cancels_it() {
    let mut b = lazy("job-busy");
    let script = Arc::new(Script::default());
    with_model(&mut b, &script);
    set(&b, &b.ann.0, tools::ROUTINES_KEY, "on");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    let busy = b.ctx.jobs.create(&b.ann.1, Value::Null, None, false);
    job_is(&b, "exit 0");
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(notices(&b).len(), 1, "the notice doesn't wait");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 0, "two turns at once in a chat");
    let owed = |b: &Bed| b.ctx.db.with(|c| c.query_row("SELECT turn_owed FROM computer_jobs WHERE id=?", [&id], |r| r.get::<_, i64>(0))).unwrap();
    assert_eq!(owed(&b), 1);
    set(&b, &b.ann.0, tools::ROUTINES_KEY, "off");
    b.ctx.jobs.with(busy, |j| j.done = true);
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(owed(&b), 0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 0, "a person took that back");
}

#[tokio::test]
async fn a_status_tells_the_model_and_owes_no_turn() {
    let mut b = bed("job-status");
    let script = Arc::new(Script::default());
    with_model(&mut b, &script);
    set(&b, &b.ann.0, tools::ROUTINES_KEY, "on");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "computer_job_status", json!({"id": id})).await;
    assert!(words(&o).contains("what it printed") && words(&o).contains("\"state\":\"running\""), "{}", words(&o));
    job_is(&b, "exit 0");
    let o = call(&r, "computer_job_status", json!({})).await;
    assert!(words(&o).contains("\"exit_code\":0"), "{}", words(&o));
    assert_eq!(notices(&b).len(), 1);
    over(&b, &r);
    b.computers.sleep_the_idle(&b.ctx).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(script.calls.load(Ordering::SeqCst), 0, "the model read it already");
}

#[tokio::test]
async fn a_job_gone_without_a_word_says_so() {
    let b = bed("job-lost");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    job_is(&b, "gone");
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(row_of(&b, &id), ("lost".to_owned(), None));
    assert!(notices(&b)[0].1.contains("left no exit code"), "{:?}", notices(&b));
}

#[tokio::test]
async fn a_look_that_cant_be_read_ends_nothing() {
    let b = bed("job-garbled");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    job_is(&b, "sh: ps: not found");
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(row_of(&b, &id).0, "running");
}

#[tokio::test]
async fn a_model_stops_its_own_job_and_no_other_bots() {
    let b = bed("job-stop");
    let computer = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &computer).await.unwrap();
    let id = started(&b, json!({"command": "x"})).await;
    let theirs = run(&b, &b.bea.1, None);
    for name in ["computer_job_stop", "computer_job_status"] {
        let o = call(&theirs, name, json!({"id": id})).await;
        assert_eq!(o.is_error, Some(true), "{name}: {}", words(&o));
    }
    assert_eq!(row_of(&b, &id).0, "running");
    let r = run(&b, &b.ann.1, None);
    assert_eq!(call(&r, "computer_job_stop", json!({"id": id})).await.is_error, None);
    assert_eq!(row_of(&b, &id).0, "stopped");
    assert_eq!(notices(&b).len(), 1);
    assert_eq!(call(&r, "computer_job_stop", json!({"id": id})).await.is_error, None, "stopped twice is stopped");
    assert_eq!(notices(&b).len(), 1);
}

#[tokio::test]
async fn at_ask_a_job_waits_for_a_card_and_its_status_doesnt() {
    let b = bed("job-ask");
    awake(&b, &b.ann.0).await;
    job_is(&b, "running");
    set(&b, &b.ann.0, tools::KEY, "ask");
    let r = run(&b, &b.ann.1, Some("r1"));
    set(&b, &b.ann.0, tools::ROUTINES_KEY, "on");
    let o = call(&r, "computer_job_start", json!({"command": "x"})).await;
    assert_eq!(words(&o), ask::WAITS);
    assert!(!b.mock.calls().iter().any(|c| c.contains("run.sh")));
    assert_eq!(call(&r, "computer_job_status", json!({})).await.is_error, None);
}

#[tokio::test]
async fn a_bot_that_leaves_takes_its_jobs_with_it() {
    let b = lazy("job-leave");
    let computer = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &computer).await.unwrap();
    let id = started(&b, json!({"command": "x"})).await;
    b.computers.leave(&b.ctx, &b.ann.0).await.unwrap();
    assert!(b.mock.calls().iter().any(|c| c.contains("kill -TERM")), "{:?}", b.mock.calls());
    assert_eq!(row_of(&b, &id).0, "stopped");
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(asleep(&b));
}

#[tokio::test]
async fn a_deleted_bots_jobs_go_with_its_computer() {
    let b = lazy("job-delete");
    awake(&b, &b.ann.0).await;
    let id = started(&b, json!({"command": "x"})).await;
    b.computers.detach(&b.ctx, &b.ann.0).await.unwrap();
    assert!(b.ctx.db.with(|c| c.query_row("SELECT 1 FROM computer_jobs WHERE id=?", [&id], |r| r.get::<_, i64>(0)).optional()).unwrap().is_none());
    assert!(b.computers.now().jobs.is_empty());
}

/// What `GET /threads` marks on `b`'s two chats, as (working, waiting).
fn marks(b: &Bed) -> [(Value, Value); 2] {
    let mut body = json!({"list": [{"id": b.ann.1}, {"id": b.bea.1}]});
    crate::hub::routes::chat::mark_work(&b.ctx, &mut body);
    [0, 1].map(|i| (body["list"][i]["working"].clone(), body["list"][i]["waiting"].clone()))
}

#[tokio::test]
async fn a_row_says_a_job_is_running_and_a_card_waits() {
    let b = bed("job-rows");
    let computer = awake(&b, &b.ann.0).await;
    let clear = (Value::Null, Value::Null);
    assert_eq!(marks(&b), [clear.clone(), clear.clone()], "a row with nothing going on reads as it did");

    let id = started(&b, json!({"command": "x"})).await;
    assert_eq!(marks(&b), [(json!(true), Value::Null), clear.clone()]);

    // frozen, it's going nowhere, and a dot would say otherwise
    b.computers.sleep(&b.ctx, &computer, Sleep::Light).await.unwrap();
    assert_eq!(marks(&b)[0], clear);
    awake(&b, &b.ann.0).await;
    assert_eq!(marks(&b)[0].0, json!(true));

    job_is(&b, "exit 0");
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(row_of(&b, &id).0, "done");
    assert_eq!(marks(&b)[0], clear);

    let at = policy::now(&b.ctx);
    b.ctx.db.with(|c| c.execute("INSERT INTO ask_waiting(id, chat, tool, input, routine, at) VALUES('a_1',?,'computer_run',NULL,NULL,?)", rusqlite::params![b.bea.1, at])).unwrap();
    assert_eq!(marks(&b)[1], (Value::Null, json!(true)));
    b.ctx.clock.manual_step(Some(at as f64 + ask::KEEP as f64 + 1.0), false);
    assert_eq!(marks(&b)[1], clear, "past its week it isn't waiting");
}

#[tokio::test]
async fn a_run_under_way_marks_its_row_for_an_app_that_wasnt_there() {
    let b = bed("run-rows");
    let job = b.ctx.jobs.create(&b.ann.1, Value::Null, None, false);
    assert_eq!(marks(&b)[0].0, Value::Null);
    b.ctx.jobs.with(job, |j| j.abort = Some(Arc::new(tokio::sync::Notify::new())));
    assert_eq!(marks(&b)[0].0, json!(true));
}

/// A stand-in for `caffeinate`: it writes down how it was called and its pid, and waits.
fn fake_caffeinate(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("buwiz-awake-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (program, log) = (dir.join("caffeinate"), dir.join("log"));
    std::fs::write(&program, format!("#!/bin/sh\necho \"$$ $*\" >> '{}'\nexec sleep 300\n", log.display())).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    (program, log)
}

fn awake_bed(label: &str) -> (Bed, std::path::PathBuf) {
    let (program, log) = fake_caffeinate(label);
    (bed_with(label, |p| Computers { idle: Duration::ZERO, deep: Duration::ZERO, keep_awake: awake::KeepAwake::with(program), ..Computers::new(p) }), log)
}

fn calls_of(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_owned).collect()
}

async fn held(log: &std::path::Path, times: usize) -> String {
    until("it was never held", || calls_of(log).len() == times).await;
    calls_of(log).last().unwrap().clone()
}

fn running(line: &str) -> bool {
    let pid = line.split(' ').next().unwrap();
    std::process::Command::new("kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().unwrap().success()
}

#[tokio::test]
async fn the_mac_is_kept_awake_for_a_job_by_this_brokers_pid_and_let_go_after() {
    let (b, log) = awake_bed("hold");
    awake(&b, &b.ann.0).await;
    assert!(calls_of(&log).is_empty(), "a computer awake isn't a job");
    let id = started(&b, json!({"command": "x"})).await;
    let line = held(&log, 1).await;
    assert!(line.ends_with(&format!(" -s -w {}", std::process::id())), "{line}");
    assert!(running(&line));

    // a second job, and a look, take nothing more
    started(&b, json!({"command": "y"})).await;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(calls_of(&log).len(), 1, "held once");

    let r = run(&b, &b.ann.1, None);
    assert_eq!(call(&r, "computer_job_stop", json!({"id": id})).await.is_error, None);
    assert!(running(&line), "one job still runs");
    over(&b, &r);
    job_is(&b, "exit 0");
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(!running(&line), "no job runs, so nothing holds the Mac");
}

#[tokio::test]
async fn a_frozen_job_doesnt_keep_the_mac_awake() {
    let (b, log) = awake_bed("frozen");
    let computer = awake(&b, &b.ann.0).await;
    started(&b, json!({"command": "x"})).await;
    let line = held(&log, 1).await;
    b.computers.sleep(&b.ctx, &computer, Sleep::Light).await.unwrap();
    assert!(!running(&line), "nothing runs");
    awake(&b, &b.ann.0).await;
    let line = held(&log, 2).await;
    assert!(running(&line), "it runs again");
}

#[tokio::test]
async fn a_restart_holds_the_mac_for_a_job_it_found_and_one_that_died_is_held_again() {
    let (b, log) = awake_bed("restart");
    awake(&b, &b.ann.0).await;
    started(&b, json!({"command": "x"})).await;
    let first = held(&log, 1).await;
    let (program, next_log) = fake_caffeinate("restart-next");
    let fresh = Arc::new(Computers { idle: Duration::ZERO, deep: Duration::ZERO, keep_awake: awake::KeepAwake::with(program), ..Computers::new(b.mock.clone()) });
    fresh.load_jobs(&b.ctx);
    let line = held(&next_log, 1).await;
    assert!(running(&line));
    let pid = line.split(' ').next().unwrap();
    std::process::Command::new("kill").arg(pid).status().unwrap();
    // killed, it's unreaped until a look finds it gone, and then it's started again
    for _ in 0..200 {
        fresh.sleep_the_idle(&b.ctx).await;
        if calls_of(&next_log).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(running(&held(&next_log, 2).await));
    fresh.let_the_mac_sleep();
    assert!(!running(calls_of(&next_log).last().unwrap()), "a broker on its way out lets go");
    b.computers.let_the_mac_sleep();
    assert!(!running(&first));
}

#[tokio::test]
async fn a_program_that_wont_start_doesnt_stop_a_job() {
    let b = bed_with("no-caffeinate", |p| Computers { keep_awake: awake::KeepAwake::with("/nonexistent/caffeinate".into()), ..Computers::new(p) });
    awake(&b, &b.ann.0).await;
    started(&b, json!({"command": "x"})).await;
}
