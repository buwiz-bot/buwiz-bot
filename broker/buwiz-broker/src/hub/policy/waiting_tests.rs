//! A card that outlives its run (`ask.rs`): real turns on a scripted model, with the app there or
//! gone as each test says.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::ask;
use super::{RealChatsDefaults, DAY};
use crate::hub::agent::runs::tests::{agent, until, Script};
use crate::hub::ctx::HubCtx;
use crate::hub::library::botctx;
use crate::hub::model::Settings;
use crate::hub::tools::{exec_tool, Out, Run};
use crate::hub::traits::Clock;
use crate::hub::types::TurnCtx;

const START: f64 = 1_800_000_000_000.0;

fn temp(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("buwiz-waiting-{label}-{}", crate::hub::store::uuidv7()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Bed {
    ctx: Arc<HubCtx>,
    chat: String,
    model: Arc<Script>,
}

/// A broker on `dir`, with a plain chat, whose memory switch asks by default.
fn hub(dir: &Path, chat: Option<String>, tune: impl FnOnce(&mut HubCtx)) -> Bed {
    let mut ctx = HubCtx::new(dir.to_owned(), Clock::manual());
    ctx.ext.insert(RealChatsDefaults);
    botctx::register(&mut ctx);
    ctx.clock.manual_step(Some(START), false);
    ctx.store.load().unwrap();
    let model = Arc::new(Script::default());
    ctx.local = Some(Arc::new(crate::hub::model::testkit::Local::default()));
    ctx.model = Some(model.clone());
    ctx.turns = Some(Arc::new(agent()));
    tune(&mut ctx);
    let chat = chat.unwrap_or_else(|| {
        ctx.store
            .write(|s| {
                let t = s.fresh_thread("plain");
                let id = t.id.clone();
                s.list.push(t);
                id
            })
            .unwrap()
    });
    Bed { ctx: Arc::new(ctx), chat, model }
}

fn gone(label: &str) -> Bed {
    hub(&temp(label), None, |_| {})
}

fn run_of(b: &Bed, turn: TurnCtx) -> Run {
    let job = b.ctx.jobs.create(&b.chat, Value::Null, None, false);
    Run::new(b.ctx.clone(), TurnCtx { thread: b.chat.clone(), ..turn }, job, Arc::new(Settings::from(|_| None)))
}

/// One call in a turn of `turn`'s kind that ends with it. A card that parks fails, and doesn't hang.
async fn call_in(b: &Bed, turn: TurnCtx, name: &str, input: Value) -> Out {
    let run = run_of(b, turn);
    let out = tokio::time::timeout(Duration::from_secs(2), exec_tool(&run, &json!({"name": name, "input": input}))).await.expect("the call parked");
    b.ctx.jobs.with(run.job, |j| j.done = true);
    out
}

async fn call(b: &Bed, name: &str, input: Value) -> Out {
    call_in(b, TurnCtx::default(), name, input).await
}

fn cards(b: &Bed) -> Vec<Value> {
    b.ctx.store.read(|s| s.thread(&b.chat).unwrap().turns.iter().filter_map(|u| u.notice.clone()).filter(|n| n["kind"] == "tool_ask").collect())
}

fn states(b: &Bed) -> Vec<String> {
    cards(b).iter().map(|c| c["state"].as_str().unwrap().to_owned()).collect()
}

fn memories(ctx: &HubCtx) -> Vec<String> {
    botctx::chats_memories(ctx).iter().map(|m| crate::js::string_of(&m["text"])).collect()
}

fn answer(b: &Bed, id: &Value, ans: &str) -> Result<Value, crate::v1::V1Error> {
    ask::host_answer(&b.ctx, &json!({"chat": b.chat, "id": id, "answer": ans}))
}

async fn idle(b: &Bed) {
    until("the turn never ended", || !b.ctx.jobs.busy(&b.chat)).await;
}

fn rows(ctx: &HubCtx) -> i64 {
    ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM ask_waiting", [], |r| r.get(0))).unwrap()
}

fn tools_of(body: &Value) -> Vec<String> {
    body["tools"].as_array().unwrap().iter().map(|t| crate::js::string_of(&t["name"])).collect()
}

#[tokio::test]
async fn a_card_with_the_app_gone_waits_and_parks_nothing() {
    let b = gone("gone");
    let run = run_of(&b, TurnCtx::default());
    let out = tokio::time::timeout(Duration::from_secs(2), exec_tool(&run, &json!({"name": "memory_add", "input": {"text": "tea"}}))).await.expect("the call parked");
    assert_eq!(out, Out::err(ask::WAITS.into()), "the model is told it hasn't run");
    assert_eq!(states(&b), ["waiting"]);
    assert_eq!(cards(&b)[0]["tool"], "memory_add");
    assert!(memories(&b.ctx).is_empty());
    assert_ne!(b.ctx.jobs.with(run.job, |j| j.detail.clone()).unwrap(), ask::WAITING, "no run sits waiting");
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn with_the_app_there_a_card_still_parks() {
    let b = hub(&temp("there"), None, |ctx| ctx.ui.app_is_there());
    let run = run_of(&b, TurnCtx::default());
    let call = tokio::spawn(async move { exec_tool(&run, &json!({"name": "memory_add", "input": {"text": "tea"}})).await });
    until("no card", || !cards(&b).is_empty()).await;
    assert_eq!(states(&b), ["pending"]);
    answer(&b, &cards(&b)[0]["id"], "once").unwrap();
    assert!(crate::js::string_of(&call.await.unwrap().content).starts_with("remembered"));
    assert_eq!(rows(&b.ctx), 0);
}

#[tokio::test]
async fn a_routines_card_waits_and_is_its_routines() {
    let b = gone("routine");
    b.ctx.ui.app_is_there();
    let turn = || TurnCtx { routine: Some("r1".into()), ..TurnCtx::default() };
    let run = run_of(&b, turn());
    run.state().external = true;
    let out = tokio::time::timeout(Duration::from_secs(2), exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi", "cwd": "/"}}))).await.expect("the call parked");
    b.ctx.jobs.with(run.job, |j| j.done = true);
    assert_eq!(out, Out::err(ask::WAITS.into()), "nobody there, whether or not the app is");
    assert_eq!(states(&b), ["waiting"]);
    assert!(ask::routine_waits(&b.ctx, "r1"));
    assert!(!ask::routine_waits(&b.ctx, "r2"), "another routine isn't held by it");

    // a yes is the person's own turn, whoever's the card was: their tools, and no routine's limits
    b.model.say("Done.");
    answer(&b, &cards(&b)[0]["id"], "once").unwrap();
    idle(&b).await;
    assert!(!ask::routine_waits(&b.ctx, "r1"), "answered, it runs again");
    assert!(tools_of(&b.model.bodies.lock().unwrap()[0]).iter().any(|t| t == "routine_list"), "a routine's own turn has no routine tools");

    // a v1 routine is held by its instance
    let surface = TurnCtx { routine: Some("p/r".into()), surface: Some(json!({"plugin": "p", "routine": "r", "instance": "i1"})), ..TurnCtx::default() };
    let run = run_of(&b, surface);
    run.state().external = true;
    assert_eq!(exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo v1", "cwd": "/"}})).await, Out::err(ask::WAITS.into()));
    b.ctx.jobs.with(run.job, |j| j.done = true);
    assert!(ask::routine_waits(&b.ctx, "i1"));
}

#[tokio::test]
async fn when_the_ten_minutes_run_out_the_card_waits_and_a_yes_still_works() {
    let b = hub(&temp("timeout"), None, |ctx| {
        ctx.ext.insert(ask::AskWait(Duration::from_millis(30)));
        ctx.ui.app_is_there();
    });
    assert_eq!(call(&b, "memory_add", json!({"text": "tea"})).await, Out::err(ask::WAITS.into()));
    assert_eq!(states(&b), ["waiting"]);
    b.model.tool("memory_add", json!({"text": "tea"}));
    b.model.say("Saved.");
    answer(&b, &cards(&b)[0]["id"], "once").unwrap();
    idle(&b).await;
    assert_eq!(memories(&b.ctx), ["tea"]);
}

#[tokio::test]
async fn a_waiting_card_survives_a_restart() {
    let dir = temp("restart");
    let first = hub(&dir, None, |_| {});
    call(&first, "memory_add", json!({"text": "tea"})).await;
    let id = cards(&first)[0]["id"].clone();
    let chat = first.chat.clone();
    drop(first);

    let b = hub(&dir, Some(chat), |_| {});
    ask::expire_pending(&b.ctx);
    ask::expire_waiting(&b.ctx);
    assert_eq!(states(&b), ["waiting"], "a restart leaves it");
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 0, "it starts no model on its own");
    b.model.tool("memory_add", json!({"text": "tea"}));
    b.model.say("Saved.");
    let said = answer(&b, &id, "once").unwrap();
    assert!(said["job"].is_u64() && said["thread"] == json!(b.chat), "{said}");
    idle(&b).await;
    assert_eq!(memories(&b.ctx), ["tea"]);
    assert_eq!(states(&b), ["allowed"]);
    assert_eq!(rows(&b.ctx), 0);
}

#[tokio::test]
async fn a_waiting_card_expires_after_seven_days() {
    let b = gone("expires");
    ask::sweep_waiting(b.ctx.clone());
    call(&b, "memory_add", json!({"text": "tea"})).await;
    let id = cards(&b)[0]["id"].clone();
    b.ctx.clock.manual_step(Some(START + (7 * DAY - 1) as f64), true);
    assert_eq!(states(&b), ["waiting"], "still inside the week");
    b.ctx.clock.manual_step(Some(START + (7 * DAY + 3_600_000) as f64), true);
    assert_eq!(states(&b), ["expired"], "the sweep got it");
    assert_eq!(rows(&b.ctx), 0);
    assert_eq!(answer(&b, &id, "once").unwrap_err().code, "stale");
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 0);
    assert!(super::activity(&b.ctx, None).iter().any(|r| r["outcome"] == "expired"));
}

#[tokio::test]
async fn an_answer_past_the_week_finds_it_expired_whatever_the_sweep_did() {
    let b = gone("late");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    b.ctx.clock.manual_step(Some(START + (7 * DAY) as f64), false);
    assert_eq!(answer(&b, &cards(&b)[0]["id"], "once").unwrap_err().code, "stale");
    assert_eq!(states(&b), ["expired"]);
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_yes_covers_only_that_exact_call() {
    let b = gone("exact");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    let id = cards(&b)[0]["id"].clone();
    // the yes's turn: another input, that exact call, and that exact call again
    b.model.tool("memory_add", json!({"text": "coffee"}));
    b.model.tool("memory_add", json!({"text": "tea"}));
    b.model.tool("memory_add", json!({"text": "tea"}));
    b.model.say("Done.");
    answer(&b, &id, "once").unwrap();
    idle(&b).await;
    assert_eq!(memories(&b.ctx), ["tea"], "that call, once, and nothing else");
    assert_eq!(states(&b), ["allowed", "waiting", "waiting"], "the rest asked for themselves");
    let bodies = b.model.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4);
    let asked = crate::js::stringify(&bodies[0]["messages"]);
    assert!(asked.contains("memory_add") && asked.contains(r#"{\"text\":\"tea\"}"#), "the model is given the call: {asked}");
    assert!(!ask::granted(&b.ctx, &b.chat, "memory"), "a yes grants nothing that lasts");

    // the turn is over, and the yes with it
    assert_eq!(call(&b, "memory_add", json!({"text": "tea"})).await, Out::err(ask::WAITS.into()));
    assert_eq!(memories(&b.ctx), ["tea"]);
}

#[tokio::test]
async fn a_yes_on_the_shell_is_bound_to_the_folder_too() {
    let b = gone("shell");
    let run = run_of(&b, TurnCtx { yes: Some(ask::yes_for_test("shell_run", json!({"command": "echo hi", "cwd": "/"}))), ..TurnCtx::default() });
    let other = exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi", "cwd": "/tmp"}})).await;
    assert_eq!(other, Out::err(ask::WAITS.into()));
    let same = exec_tool(&run, &json!({"name": "shell_run", "input": {"cwd": "/", "command": "echo hi"}})).await;
    assert_eq!(same.content, json!("hi"), "{same:?}");
    assert_eq!(exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi", "cwd": "/"}})).await, Out::err(ask::WAITS.into()), "spent");
}

#[tokio::test]
async fn a_yes_passes_a_card_and_no_switch() {
    let b = gone("off");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    super::host_set(&b.ctx, &json!({"bot": "chats", "key": "memory", "state": "off"})).unwrap();
    b.model.tool("memory_add", json!({"text": "tea"}));
    b.model.say("It's off.");
    answer(&b, &cards(&b)[0]["id"], "once").unwrap();
    idle(&b).await;
    assert!(memories(&b.ctx).is_empty(), "off stays off");

    // and a block stays a block, whatever was said yes to
    let rule = super::Change::RuleAdd { kind: "block".into(), pattern: "echo".into(), risk: None, path: None };
    super::commit(&b.ctx, &rule, "human", None).unwrap();
    let run = run_of(&b, TurnCtx { yes: Some(ask::yes_for_test("shell_run", json!({"command": "echo hi", "cwd": "/"}))), ..TurnCtx::default() });
    let out = exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi", "cwd": "/"}})).await;
    assert_eq!(out, Out::err(super::gate::BLOCKED.into()));
}

#[tokio::test]
async fn no_starts_no_turn() {
    let b = gone("no");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    let said = answer(&b, &cards(&b)[0]["id"], "no").unwrap();
    assert!(said.get("job").is_none(), "{said}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 0, "no model call");
    assert!(b.ctx.jobs.running().is_empty() && !b.ctx.jobs.busy(&b.chat), "no job");
    assert_eq!(states(&b), ["declined"]);
    assert_eq!(rows(&b.ctx), 0);
    assert!(memories(&b.ctx).is_empty());
    assert_eq!(answer(&b, &cards(&b)[0]["id"], "once").unwrap_err().code, "stale", "a no stands");
    // the model hears of it when it next runs here, and not before
    assert!(crate::js::stringify(&json!(super::take_notes(&b.ctx, &b.chat, None))).contains("Not now"));
}

fn sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs" || x == "json") {
            out.push((p.clone(), std::fs::read_to_string(&p).unwrap()));
        }
    }
}

/// A card's answer comes in on the host route, which only the host token opens (401 without it:
/// `routines::tests`), and a yes lives on the turn that route starts. So a model has the card's
/// id and the call's input and still has no way to say yes.
#[tokio::test]
async fn a_model_cant_answer_its_own_card() {
    let b = gone("own");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    let id = cards(&b)[0]["id"].clone();

    // the model asks again, and again as if answering; its own turns are no yes
    for input in [json!({"text": "tea"}), json!({"text": "tea", "id": id, "answer": "once"})] {
        assert_eq!(call(&b, "memory_add", input).await, Out::err(ask::WAITS.into()));
    }
    assert!(memories(&b.ctx).is_empty());
    assert_eq!(states(&b)[0], "waiting");

    // nothing a model reaches calls the answer, or makes a yes
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut all = Vec::new();
    sources(&root, &mut all);
    assert!(all.len() > 100);
    let (answering, route, making) = (concat!("host_", "answer("), concat!("/v1/host/tools", "/ask\""), concat!("Yes", " {"));
    let this = Path::new(file!()).file_name().unwrap();
    let rel = |p: &Path| p.strip_prefix(&root).unwrap().display().to_string();
    let named = |word: &str| -> Vec<String> {
        let mut v: Vec<String> = all.iter().filter(|(p, t)| p.file_name() != Some(this) && !rel(p).contains("tests") && t.contains(word)).map(|(p, _)| rel(p)).collect();
        v.sort();
        v
    };
    assert_eq!(named(answering), ["hub/policy/ask.rs", "hub/routes/host.rs"], "only the route answers a card");
    assert_eq!(named(route), ["hub/routes/host.rs"], "only the route names it");
    assert_eq!(named(making), ["hub/policy/ask.rs"], "only an answer makes a yes");
}

#[tokio::test]
async fn the_same_call_waits_on_one_card() {
    let b = gone("once");
    for _ in 0..3 {
        assert_eq!(call(&b, "memory_add", json!({"text": "tea"})).await, Out::err(ask::WAITS.into()));
    }
    assert_eq!(states(&b), ["waiting"]);
    call(&b, "memory_add", json!({"text": "coffee"})).await;
    assert_eq!(states(&b), ["waiting", "waiting"], "another call is another question");
}

#[tokio::test]
async fn a_call_holding_a_secret_is_never_kept() {
    let b = gone("secret");
    let out = call(&b, "memory_add", json!({"text": "my TIN is 123-456-789"})).await;
    assert_eq!(out, Out::err(ask::NOBODY.into()));
    assert!(cards(&b).is_empty());
    assert_eq!(rows(&b.ctx), 0);

    // its 10 minutes run out as they always did
    let b = hub(&temp("secret-parked"), None, |ctx| {
        ctx.ext.insert(ask::AskWait(Duration::from_millis(30)));
        ctx.ui.app_is_there();
    });
    assert_eq!(call(&b, "memory_add", json!({"text": "ticket tkt_01ARZ3NDEKTSV4RRFFQ69G5FAV"})).await, Out::err(ask::NO_ANSWER.into()));
    assert_eq!(states(&b), ["expired"]);
    assert_eq!(rows(&b.ctx), 0);
}

#[tokio::test]
async fn a_waiting_card_takes_nothing_that_lasts() {
    let b = gone("lasts");
    call(&b, "shell_run", json!({"command": "git status --short -- x", "cwd": "/"})).await;
    let c = cards(&b)[0].clone();
    assert_eq!(c["state"], "waiting");
    for ans in ["chat", "rule"] {
        assert_eq!(answer(&b, &c["id"], ans).unwrap_err().code, "invalid_args", "{ans}");
    }
    assert_eq!(states(&b), ["waiting"], "the card stays");
    assert!(!ask::granted(&b.ctx, &b.chat, "shell"));
    assert!(super::rules(&b.ctx, "allow").is_empty());
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_yes_while_the_chat_works_leaves_the_card_waiting() {
    let b = gone("busy");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    let working = b.ctx.jobs.create(&b.chat, Value::Null, None, false);
    let id = cards(&b)[0]["id"].clone();
    assert_eq!(answer(&b, &id, "once").unwrap_err().code, "busy");
    assert_eq!(states(&b), ["waiting"]);
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 0);
    b.ctx.jobs.with(working, |j| j.done = true);
    b.model.say("Done.");
    answer(&b, &id, "once").unwrap();
    idle(&b).await;
    assert_eq!(states(&b), ["allowed"]);
}

#[tokio::test]
async fn a_second_answer_finds_the_card_settled() {
    let b = gone("twice");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    let id = cards(&b)[0]["id"].clone();
    b.model.say("Done.");
    answer(&b, &id, "once").unwrap();
    assert_eq!(answer(&b, &id, "once").unwrap_err().code, "stale");
    idle(&b).await;
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 1, "one turn");
}

#[tokio::test]
async fn a_routine_turn_leaves_one_card() {
    let b = gone("one-card");
    let run = run_of(&b, TurnCtx { routine: Some("r1".into()), ..TurnCtx::default() });
    run.state().external = true;
    let shell = |c: &str| json!({"name": "shell_run", "input": {"command": c, "cwd": "/"}});
    assert_eq!(exec_tool(&run, &shell("echo one")).await, Out::err(ask::WAITS.into()));
    assert_eq!(exec_tool(&run, &shell("echo two")).await, Out::err(ask::ONE_CARD.into()), "told, and no new card");
    assert_eq!(exec_tool(&run, &shell("echo three")).await, Out::err(ask::ONE_CARD.into()));
    assert_eq!(states(&b), ["waiting"]);
    assert_eq!(rows(&b.ctx), 1);
    // a person's turn in that chat isn't held to it
    assert_eq!(call(&b, "memory_add", json!({"text": "tea"})).await, Out::err(ask::WAITS.into()));
    assert_eq!(states(&b), ["waiting", "waiting"]);
}

/// Told there's a card already, a turn asked for nothing new: Activity has no line for it.
#[tokio::test]
async fn a_call_turned_away_by_the_one_card_rule_logs_nothing() {
    let b = gone("one-card-log");
    let run = run_of(&b, TurnCtx { routine: Some("r1".into()), ..TurnCtx::default() });
    run.state().external = true;
    let shell = |c: &str| json!({"name": "shell_run", "input": {"command": c, "cwd": "/"}});
    exec_tool(&run, &shell("echo one")).await;
    let logged = |b: &Bed| super::activity(&b.ctx, None).iter().filter(|a| a["outcome"] == "needs_ok").count();
    assert_eq!(logged(&b), 1);
    assert_eq!(exec_tool(&run, &shell("echo two")).await, Out::err(ask::ONE_CARD.into()));
    assert_eq!(logged(&b), 1);
}

/// A stop between the card and its row: nothing can answer that card, and it doesn't stay.
#[tokio::test]
async fn a_card_that_lost_its_row_expires() {
    let b = gone("stranded");
    assert_eq!(call(&b, "memory_add", json!({"text": "tea"})).await, Out::err(ask::WAITS.into()));
    assert_eq!(call(&b, "memory_add", json!({"text": "coffee"})).await, Out::err(ask::WAITS.into()));
    let lost = cards(&b)[1]["id"].as_str().unwrap().to_owned();
    b.ctx.db.with(|c| c.execute("DELETE FROM ask_waiting WHERE id=?", [&lost])).unwrap();
    ask::expire_waiting(&b.ctx);
    assert_eq!(states(&b), ["waiting", "waiting"], "a card just posted may have its row on the way");
    b.ctx.clock.manual_step(Some(START + 60_000.0), false);
    ask::expire_waiting(&b.ctx);
    assert_eq!(states(&b), ["waiting", "expired"], "the card with its row stays");
    assert_eq!(rows(&b.ctx), 1);
}

/// Not reading the rows isn't reading that there are none.
#[tokio::test]
async fn a_failed_read_of_the_rows_expires_no_card() {
    let b = gone("stranded-read");
    call(&b, "memory_add", json!({"text": "tea"})).await;
    b.ctx.db.with(|c| c.execute("ALTER TABLE ask_waiting RENAME TO ask_gone", [])).unwrap();
    b.ctx.clock.manual_step(Some(START + 60_000.0), false);
    ask::expire_waiting(&b.ctx);
    assert_eq!(states(&b), ["waiting"]);
}

/// With the switch On no card comes, so nothing but the call itself can spend the yes: left
/// over, it would pass the outside-content guard on a later call.
#[tokio::test]
async fn a_yes_goes_on_its_first_call_whichever_way_that_goes() {
    let b = gone("spent");
    super::host_set(&b.ctx, &json!({"bot": "chats", "key": "memory", "state": "on"})).unwrap();
    let input = json!({"text": "tea"});
    let run = run_of(&b, TurnCtx { yes: Some(ask::yes_for_test("memory_add", input.clone())), ..TurnCtx::default() });
    let block = json!({"name": "memory_add", "input": input});
    assert!(crate::js::string_of(&exec_tool(&run, &block).await.content).starts_with("remembered"), "On: no card");
    assert!(cards(&b).is_empty());
    run.state().external = true;
    assert_eq!(exec_tool(&run, &block).await, Out::err(ask::WAITS.into()), "the guard asks: the yes is gone");
    assert_eq!(states(&b), ["waiting"]);
    assert_eq!(memories(&b.ctx).len(), 1);
}

/// A sweep landing between a card and its row, as another thread could.
fn swept_between(ctx: &mut HubCtx) {
    ctx.ext.insert(ask::Between(Arc::new(|c| ask::expire_waiting(c))));
}

#[tokio::test]
async fn a_sweep_between_a_card_and_its_row_loses_neither() {
    let b = hub(&temp("between"), None, swept_between);
    assert_eq!(call(&b, "memory_add", json!({"text": "tea"})).await, Out::err(ask::WAITS.into()));
    assert_eq!((states(&b), rows(&b.ctx)), (vec!["waiting".to_owned()], 1), "the row is there for the answer");

    // and a parked card whose wait runs out
    let b = hub(&temp("between-parked"), None, |ctx| {
        swept_between(ctx);
        ctx.ext.insert(ask::AskWait(Duration::from_millis(30)));
        ctx.ui.app_is_there();
    });
    assert_eq!(call(&b, "memory_add", json!({"text": "tea"})).await, Out::err(ask::WAITS.into()));
    assert_eq!((states(&b), rows(&b.ctx)), (vec!["waiting".to_owned()], 1));
}

/// A second yes landing just after the first passed its check, as another thread could.
#[tokio::test]
async fn two_yeses_on_two_cards_start_one_turn() {
    let second: Arc<std::sync::Mutex<Option<(String, Value)>>> = Arc::default();
    let code: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
    let (s, c) = (second.clone(), code.clone());
    let b = hub(&temp("two-yeses"), None, move |ctx| {
        ctx.ext.insert(ask::Between(Arc::new(move |hub| {
            let Some((chat, id)) = s.lock().unwrap().take() else { return };
            let said = ask::host_answer(hub, &json!({"chat": chat, "id": id, "answer": "once"}));
            *c.lock().unwrap() = Some(said.map_or_else(|e| e.code, |_| "started".to_owned()));
        })));
    });
    call(&b, "memory_add", json!({"text": "tea"})).await;
    call(&b, "memory_add", json!({"text": "coffee"})).await;
    let both = cards(&b);
    b.model.say("Done.");
    b.model.say("Done again.");
    *second.lock().unwrap() = Some((b.chat.clone(), both[1]["id"].clone()));
    answer(&b, &both[0]["id"], "once").unwrap();
    idle(&b).await;
    assert_eq!(code.lock().unwrap().as_deref(), Some("busy"), "the second finds the chat taken");
    assert_eq!(states(&b), ["allowed", "waiting"], "and its card still waits");
    assert_eq!(b.model.calls.load(Ordering::SeqCst), 1, "one turn");
}
