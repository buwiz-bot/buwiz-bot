//! A person and an agent on one screen: pausing, handing over, who keeps a screen. A real gate and
//! the frames a page sends, against the mock provider and a viewer that hears them.

use std::sync::atomic::AtomicUsize;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::hub::policy::ask;
use crate::hub::routes::host;

const QUIET: Duration = Duration::from_millis(150);
const NO_PATIENCE: Duration = Duration::from_millis(250);

fn timed(label: &str) -> Bed {
    bed_with(label, |p| Computers { quiet: QUIET, ..Computers::new(p) })
}

/// A viewer that takes any socket and hears nothing back.
async fn viewer(b: &Bed) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    *b.mock.viewer_addr.lock().unwrap() = Some(listener.local_addr().unwrap().to_string());
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut b = [0u8; 2048];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&b[..n]),
                    }
                }
                let _ = s.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n").await;
                while matches!(s.read(&mut b).await, Ok(n) if n > 0) {}
            });
        }
    });
}

pub(super) fn open_req(bot: &str, body: Value) -> HubRequest {
    let mut headers = hyper::HeaderMap::new();
    headers.insert("x-buwiz-host", HeaderValue::from_static(host::TEST_TOKEN));
    HubRequest { method: hyper::Method::POST, url: format!("/v1/host/computer/bot/{bot}/open"), headers, body: Bytes::from(body.to_string()), params: vec![bot.to_owned()] }
}

pub(super) async fn answer_of(res: HubResponse) -> (StatusCode, Value) {
    let status = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// What the Computer tab does for a viewer of `mode`, up to a connected socket.
async fn connect(b: &Bed, bot: &str, mode: &str) -> TcpStream {
    let (status, v) = answer_of(open(b.ctx.clone(), open_req(bot, json!({"mode": mode}))).await).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let url = v["url"].as_str().unwrap();
    let (port, target) = url.strip_prefix("http://127.0.0.1:").unwrap().split_once('/').unwrap();
    let (port, target) = (port.parse::<u16>().unwrap(), format!("/{target}"));
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).await.unwrap();
    let cookie = reply.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_owned();
    let clean = target.split('?').next().unwrap();
    let mut ws = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    ws.write_all(format!("GET {clean}ws HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = [0u8; 16];
    ws.read_exact(&mut head).await.unwrap();
    ws
}

/// A command as a page sends it.
fn command(text: &str) -> Vec<u8> {
    let mask = [1u8, 2, 3, 4];
    let mut out = vec![0x81, 0x80 | text.len() as u8];
    out.extend(mask);
    out.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// Keeps sending input until told to stop, as a person busy at the screen.
fn busy_person(mut ws: TcpStream) -> (Arc<AtomicUsize>, tokio::task::JoinHandle<TcpStream>) {
    let stop = Arc::new(AtomicUsize::new(0));
    let until = stop.clone();
    let person = tokio::spawn(async move {
        while until.load(Ordering::SeqCst) == 0 {
            ws.write_all(&command("m,5,5")).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ws
    });
    (stop, person)
}

async fn until_true(what: &str, mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..600 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{what}");
}

fn acted(b: &Bed) -> bool {
    b.mock.calls().iter().any(|c| c.starts_with("input"))
}

#[tokio::test]
async fn an_agent_waits_for_a_person_driving_and_goes_on_when_they_stop() {
    let b = timed("pause");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let ws = connect(&b, &b.ann.0, "drive").await;
    let (stop, person) = busy_person(ws);
    until_true("the person was never seen", async || b.computers.person_on(&b.ann.0)).await;

    let r = Arc::new(run(&b, &b.ann.1, None));
    let call_click = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, "computer_click", json!({"x": 1, "y": 1})).await })
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!acted(&b) && !call_click.is_finished(), "it acted under a person: {:?}", b.mock.calls());
    assert_eq!(b.ctx.jobs.with(r.job, |j| j.detail.clone()).unwrap(), "Waiting for a person on its computer");

    stop.store(1, Ordering::SeqCst);
    let _ws = person.await.unwrap();
    let out = call_click.await.unwrap();
    assert_eq!(out.is_error, None, "{}", words(&out));
    assert!(acted(&b));
}

#[tokio::test]
async fn nothing_it_does_goes_ahead_under_a_person_not_even_a_look() {
    for (name, input) in [("computer_screenshot", json!({})), ("computer_wait", json!({})), ("computer_read_file", json!({"path": "a"})), ("computer_write_file", json!({"path": "a", "content": "x"})), ("computer_run", json!({"command": "ls"})), ("computer_open_url", json!({"url": "https://a.example"})), ("computer_record", json!({"action": "start"}))] {
        let b = bed_with("still", |p| Computers { quiet: Duration::from_secs(30), patience: NO_PATIENCE, ..Computers::new(p) });
        awake(&b, &b.ann.0).await;
        viewer(&b).await;
        let mut ws = connect(&b, &b.ann.0, "drive").await;
        ws.write_all(&command("m,5,5")).await.unwrap();
        until_true("the person was never seen", async || b.computers.person_on(&b.ann.0)).await;
        b.mock.calls.lock().unwrap().clear();
        let r = run(&b, &b.ann.1, None);
        let out = call(&r, name, input).await;
        assert_eq!(out.is_error, Some(true), "{name}: {}", words(&out));
        assert_eq!(words(&out), Problem::PersonOn.sentence(), "{name}");
        assert_eq!(b.mock.calls(), ["guard"], "{name} reached the computer");
    }
}

#[tokio::test]
async fn a_person_who_comes_in_during_an_action_keeps_the_picture_from_the_model() {
    let b = bed_with("picture", |p| Computers { quiet: Duration::from_secs(30), patience: NO_PATIENCE, ..Computers::new(p) });
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    b.mock.calls.lock().unwrap().clear();
    let r = Arc::new(run(&b, &b.ann.1, None));
    let clicking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, "computer_click", json!({"x": 1, "y": 1})).await })
    };
    // the mock settles for half a second after it acts
    until_true("it never acted", async || acted(&b)).await;
    let _ws = connect(&b, &b.ann.0, "drive").await;
    let out = clicking.await.unwrap();
    assert_eq!(out.is_error, None, "the click happened: {}", words(&out));
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("screenshot")), "{:?}", b.mock.calls());
    assert!(words(&out).contains("A person is still using"), "{}", words(&out));
}

#[tokio::test]
async fn a_watcher_pauses_nothing() {
    let b = timed("watch");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let mut ws = connect(&b, &b.ann.0, "watch").await;
    ws.write_all(&command("m,5,5")).await.unwrap();
    ws.write_all(&command("k,65,1")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!b.computers.person_on(&b.ann.0));
    let r = run(&b, &b.ann.1, None);
    let out = tokio::time::timeout(Duration::from_secs(3), call(&r, "computer_click", json!({"x": 1, "y": 1}))).await.expect("a watcher held the agent up");
    assert_eq!(out.is_error, None);
}

#[tokio::test]
async fn an_open_drive_viewer_holds_the_agent_until_it_goes_quiet_or_closes() {
    // no input at all: they opened it to press something, and may be about to
    let b = bed_with("idle", |p| Computers { quiet: Duration::from_millis(700), ..Computers::new(p) });
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let _ws = connect(&b, &b.ann.0, "drive").await;
    b.mock.calls.lock().unwrap().clear();
    let r = Arc::new(run(&b, &b.ann.1, None));
    let clicking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, "computer_click", json!({"x": 1, "y": 1})).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!acted(&b), "its click landed under an idle driver");
    assert_eq!(clicking.await.unwrap().is_error, None, "once they've gone quiet it goes on");

    // closed is gone, whatever the quiet says
    let b = bed_with("closed", |p| Computers { quiet: Duration::from_secs(30), ..Computers::new(p) });
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let ws = connect(&b, &b.ann.0, "drive").await;
    assert!(b.computers.person_on(&b.ann.0));
    drop(ws);
    until_true("a closed viewer still holds the agent", async || !b.computers.person_on(&b.ann.0)).await;
}

#[test]
fn a_person_with_a_drive_viewer_counts_for_a_minute_and_the_agent_gives_up_at_ten() {
    assert_eq!((super::super::QUIET, PATIENCE), (Duration::from_secs(60), Duration::from_secs(600)));
}

#[tokio::test]
async fn a_person_on_one_screen_leaves_the_other_bots_alone() {
    let b = bed_with("two", |p| Computers { quiet: Duration::from_secs(30), ..Computers::new(p) });
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    viewer(&b).await;
    let mut ws = connect(&b, &b.ann.0, "drive").await;
    ws.write_all(&command("m,5,5")).await.unwrap();
    until_true("the person was never seen", async || b.computers.person_on(&b.ann.0)).await;
    assert!(!b.computers.person_on(&b.bea.0));
    let r = run(&b, &b.bea.1, None);
    let out = tokio::time::timeout(Duration::from_secs(3), call(&r, "computer_click", json!({"x": 1, "y": 1}))).await.expect("Bea waited on Ann's person");
    assert_eq!(out.is_error, None);
}

#[tokio::test]
async fn the_first_run_to_touch_a_screen_keeps_it_until_it_ends() {
    let b = bed("hold");
    awake(&b, &b.ann.0).await;
    let first = run(&b, &b.ann.1, None);
    let second = run(&b, &b.ann.1, None);
    assert_eq!(call(&first, "computer_screenshot", json!({})).await.is_error, None);
    b.mock.calls.lock().unwrap().clear();
    let out = call(&second, "computer_click", json!({"x": 1, "y": 1})).await;
    assert_eq!(words(&out), Problem::Held.sentence());
    assert_eq!(out.is_error, Some(true));
    assert!(!acted(&b), "{:?}", b.mock.calls());
    assert_eq!(call(&first, "computer_click", json!({"x": 1, "y": 1})).await.is_error, None, "the first goes on");
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["working"], json!(true));
    drop(first);
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["working"], json!(false));
    assert_eq!(call(&second, "computer_click", json!({"x": 1, "y": 1})).await.is_error, None, "it's free once the first ended");
}

#[tokio::test]
async fn a_run_that_never_reached_the_screen_doesnt_keep_it() {
    let b = bed("nohold");
    let first = run(&b, &b.ann.1, None);
    awake(&b, &b.ann.0).await;
    b.computers.sleep(&b.ctx, &seated(&b.ctx, &b.ann.0).unwrap().computer, Sleep::Light).await.unwrap();
    assert_eq!(call(&first, "computer_screenshot", json!({})).await.is_error, Some(true));
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["working"], json!(false));
}

#[tokio::test]
async fn two_runs_at_once_cant_both_be_first_and_another_bots_screen_is_its_own() {
    let b = bed("race");
    let id = awake(&b, &b.ann.0).await;
    let tries: Vec<_> = (0..32)
        .map(|_| {
            let (c, id) = (b.computers.clone(), id.clone());
            tokio::spawn(async move { c.hold("b_ann", &id).ok() })
        })
        .collect();
    let mut won = Vec::new();
    for t in tries {
        won.extend(t.await.unwrap());
    }
    assert_eq!(won.len(), 1, "the screen went to {} runs", won.len());
    assert!(b.computers.hold("b_bea", &id).is_ok());
}

#[tokio::test]
async fn a_delegate_gets_no_computer_tool() {
    let b = bed("delegate");
    awake(&b, &b.ann.0).await;
    b.mock.calls.lock().unwrap().clear();
    let job = b.ctx.jobs.create(&b.ann.1, Value::Null, None, false);
    let turn = TurnCtx { thread: b.ann.1.clone(), acting_bot: Some(b.ann.0.clone()), agent_hop: Some(1), ..TurnCtx::default() };
    let request = Run::new(b.ctx.clone(), turn, job, Arc::new(Settings::from(|_| None)));
    assert_eq!(offered(&request).await, Vec::<String>::new());
    assert_eq!(tools::prompt(&request).await, "");
    for name in NAMES {
        let out = call(&request, name, json!({"x": 1, "y": 1, "command": "ls", "reason": "x"})).await;
        assert_eq!(out.is_error, Some(true), "{name}: {}", words(&out));
    }
    assert!(b.mock.calls().is_empty(), "{:?}", b.mock.calls());

    // a helper is a Bot of its own, working for its parent, even in a chat of its own
    let own = run(&b, &b.ann.1, None);
    assert_eq!(offered(&own).await.len(), NAMES.len(), "its own person's turn has them");
    b.ctx.store.write(|s| s.bots.iter_mut().find(|x| x.id == b.ann.0).unwrap().helper = Some(json!({"parent": "b_bea"}))).unwrap();
    assert_eq!(offered(&own).await, Vec::<String>::new());
    assert_eq!(call(&own, "computer_screenshot", json!({})).await.is_error, Some(true));
    assert!(b.mock.calls().is_empty(), "{:?}", b.mock.calls());
}

/// The card `computer_ask_person` posted in `chat`.
pub(super) async fn card(b: &Bed, chat: &str) -> Value {
    for _ in 0..600 {
        let found = b.ctx.store.read(|s| s.thread(chat).and_then(|t| t.turns.iter().filter_map(|u| u.notice.clone()).rfind(|n| n["kind"] == "tool_ask" && n["state"] == "pending")));
        if let Some(n) = found {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no card");
}

pub(super) fn answer(b: &Bed, chat: &str, card: &Value, ans: &str) {
    ask::host_answer(&b.ctx, &json!({"chat": chat, "id": card["id"], "answer": ans})).unwrap();
}

/// Asks in a task, says yes on the card, and waits for the screen to be with the person.
async fn taken_over(b: &Bed, input: Value) -> (Arc<Run>, tokio::task::JoinHandle<Out>) {
    let r = Arc::new(run(b, &b.ann.1, None));
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, input).await })
    };
    let c = card(b, &b.ann.1).await;
    answer(b, &b.ann.1, &c, "once");
    until_true("it never reached the person", async || b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_some()).await;
    (r, asking)
}

#[tokio::test]
async fn a_person_takes_over_and_hands_back_and_the_model_sees_only_what_they_left() {
    let b = timed("handover");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    b.mock.calls.lock().unwrap().clear();
    let (r, asking) = taken_over(&b, json!({"reason": "Sign in to the shop", "time_limit_s": 120})).await;
    let status = b.computers.status(&b.ctx, &b.ann.0).await;
    assert_eq!(status["takeover"]["reason"], "Sign in to the shop");
    assert!((110..=120).contains(&status["takeover"]["secondsLeft"].as_u64().unwrap()), "{status}");
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("screenshot")), "it looked while waiting");
    assert_eq!(b.ctx.jobs.with(r.job, |j| j.detail.clone()).unwrap(), ask::WAITING);

    // the person's own viewer drives, and nothing they do lands in the tool's answer yet
    let mut ws = connect(&b, &b.ann.0, "drive").await;
    ws.write_all(&command("k,65,1")).await.unwrap();
    ws.write_all(&command("k,65,0")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!asking.is_finished(), "a quiet person isn't a handback");
    assert!(b.computers.person_on(&b.ann.0), "the screen stays theirs through a pause");

    let (status, back) = answer_of(hand_back(b.ctx.clone(), open_req(&b.ann.0, json!({}))).await).await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert!(back.get("takeover").is_none(), "{back}");
    let out = asking.await.unwrap();
    assert_eq!(out.is_error, None, "{}", words(&out));
    let said = words(&out);
    assert!(said.contains("handed_back") && said.contains("\"image\""), "{said}");
    assert!(b.mock.calls().iter().any(|c| c.starts_with("screenshot")));
    for secret in [TOKEN, PASSWORD, "ticket", "127.0.0.1", "buwiz_viewer"] {
        assert!(!said.contains(secret), "{secret}");
    }
    assert_eq!(b.computers.open_gate().unwrap().mode(&b.ann.0), None, "their drive ticket went with the hand-over");
    assert!(!b.computers.person_on(&b.ann.0));
}

#[tokio::test]
async fn its_own_run_can_do_nothing_while_the_screen_is_handed_over_and_nor_can_another() {
    let b = bed_with("wait", |p| Computers { patience: NO_PATIENCE, ..Computers::new(p) });
    awake(&b, &b.ann.0).await;
    let (r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    let other = run(&b, &b.ann.1, None);
    assert_eq!(words(&call(&other, "computer_screenshot", json!({})).await), Problem::Held.sentence());
    assert_eq!(words(&call(&r, "computer_screenshot", json!({})).await), Problem::PersonOn.sentence(), "a look under a person");
    assert!(b.computers.hand_back(&b.ann.0));
    assert_eq!(asking.await.unwrap().is_error, None);
}

#[tokio::test]
async fn time_running_out_takes_the_screen_back_and_cuts_the_person_off() {
    let b = timed("timeup");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let mut ws = connect(&b, &b.ann.0, "drive").await;
    let live = b.computers.live(&b.ctx, &b.ann.0).await.unwrap();
    let mut handing = b.computers.hand_over(&live, "x", Duration::from_millis(80)).unwrap();
    assert_eq!(handing.ended().await, None, "the time ran out");
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
    assert_eq!(b.computers.open_gate().unwrap().mode(&b.ann.0), None);
    let mut rest = Vec::new();
    assert!(tokio::time::timeout(Duration::from_secs(2), ws.read_to_end(&mut rest)).await.is_ok(), "the person is still connected");
    assert!(!b.computers.hand_back(&b.ann.0), "nothing's left to hand back");
}

#[tokio::test]
async fn the_time_limit_stays_in_its_range() {
    let b = bed("limits");
    awake(&b, &b.ann.0).await;
    for (asked, least, most) in [(json!(5), 25, 30), (json!(100000), 1790, 1800), (Value::Null, 290, 300)] {
        let (_r, asking) = taken_over(&b, json!({"reason": "x", "time_limit_s": asked})).await;
        let left = b.computers.status(&b.ctx, &b.ann.0).await["takeover"]["secondsLeft"].as_u64().unwrap();
        assert!((least..=most).contains(&left), "{asked} left {left}");
        b.computers.hand_back(&b.ann.0);
        asking.await.unwrap();
    }
}

#[tokio::test]
async fn a_hand_back_on_the_stroke_of_the_limit_ends_it_once() {
    let b = bed("stroke");
    awake(&b, &b.ann.0).await;
    let live = b.computers.live(&b.ctx, &b.ann.0).await.unwrap();
    for i in 0..60u64 {
        let mut handing = b.computers.hand_over(&live, "x", Duration::from_millis(3)).unwrap();
        let c = b.computers.clone();
        let click = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_micros(2900 + i * 5)).await;
            c.hand_back("b_ann")
        });
        let ended = handing.ended().await;
        let clicked = click.await.unwrap();
        assert_eq!(ended == Some(Ended::HandedBack), clicked, "the click and the clock disagree: {ended:?} {clicked}");
        assert!(ended.is_none() || clicked);
    }
}

#[tokio::test]
async fn saying_not_now_hands_nothing_over() {
    let b = bed("notnow");
    awake(&b, &b.ann.0).await;
    let r = Arc::new(run(&b, &b.ann.1, None));
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "x"})).await })
    };
    let c = card(&b, &b.ann.1).await;
    assert_eq!(c["title"], "Take over Ann's computer?");
    answer(&b, &b.ann.1, &c, "no");
    let out = asking.await.unwrap();
    assert_eq!(out.is_error, Some(true));
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
}

#[tokio::test]
async fn a_takeover_card_takes_no_standing_yes_whatever_a_client_sends() {
    let b = bed("grant");
    awake(&b, &b.ann.0).await;
    set(&b, &b.ann.0, tools::KEY, "ask");
    let r = Arc::new(run(&b, &b.ann.1, None));
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "x"})).await })
    };
    let c = card(&b, &b.ann.1).await;
    let refused = ask::host_answer(&b.ctx, &json!({"chat": b.ann.1, "id": c["id"], "answer": "chat"}));
    assert!(refused.is_err(), "the host took a standing yes");
    assert!(!ask::granted(&b.ctx, &b.ann.1, tools::ASK_KEY) && !ask::granted(&b.ctx, &b.ann.1, tools::KEY));
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
    // the card is still there to answer
    answer(&b, &b.ann.1, &card(&b, &b.ann.1).await, "once");
    until_true("no hand-over", async || b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_some()).await;
    b.computers.hand_back(&b.ann.0);
    asking.await.unwrap();
}

#[tokio::test]
async fn the_card_says_what_a_hand_over_doesnt_keep_from_the_ai() {
    let b = bed("words");
    awake(&b, &b.ann.0).await;
    let r = Arc::new(run(&b, &b.ann.1, None));
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "Sign in"})).await })
    };
    let c = card(&b, &b.ann.1).await;
    let words = c["lines"].to_string();
    for said in ["Commands it ran can keep running, but they can't see this screen or what you type", "pages it opened, and anything it set going by clicking on this screen, could", "Don't type a real password unless you trust this Bot's work so far"] {
        assert!(words.contains(said), "{words}");
    }
    assert!(!words.contains("shown nothing"), "{words}");
    answer(&b, &b.ann.1, &c, "no");
    asking.await.unwrap();
}

#[tokio::test]
async fn what_the_model_left_running_ends_before_the_person_starts_and_the_model_is_told() {
    let b = bed("strays");
    awake(&b, &b.ann.0).await;
    *b.mock.strays.lock().unwrap() = vec!["python3".to_owned()];
    b.mock.calls.lock().unwrap().clear();
    let (_r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    assert!(b.mock.calls().iter().any(|c| c == "clear_strays"), "{:?}", b.mock.calls());
    b.computers.hand_back(&b.ann.0);
    let said = words(&asking.await.unwrap());
    assert!(said.contains("programs_ended") && said.contains("python3"), "{said}");
}

#[tokio::test]
async fn a_computer_that_cant_be_cleared_isnt_handed_over() {
    let b = bed("noclear");
    awake(&b, &b.ann.0).await;
    *b.mock.stray_problem.lock().unwrap() = Some(Problem::Failed("no".into()));
    let r = Arc::new(run(&b, &b.ann.1, None));
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "x"})).await })
    };
    answer(&b, &b.ann.1, &card(&b, &b.ann.1).await, "once");
    let out = tokio::time::timeout(Duration::from_secs(5), asking).await.expect("it was handed over all the same").unwrap();
    assert_eq!(out.is_error, Some(true));
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
}

#[tokio::test]
async fn a_hand_over_that_fails_after_the_recording_stopped_still_says_where_it_is() {
    let b = timed("record-lost");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let r = Arc::new(run(&b, &b.ann.1, None));
    call(&r, "computer_record", json!({"action": "start"})).await;
    *b.mock.stray_problem.lock().unwrap() = Some(Problem::Failed("no".into()));
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "x"})).await })
    };
    answer(&b, &b.ann.1, &card(&b, &b.ann.1).await, "once");
    let out = tokio::time::timeout(Duration::from_secs(5), asking).await.expect("it was handed over all the same").unwrap();
    assert_eq!(out.is_error, Some(true));
    assert!(words(&out).contains("7.mp4"), "{}", words(&out));
}

#[tokio::test]
async fn a_computer_not_confirmed_walled_off_isnt_handed_over() {
    for answer_given in [Ok(false), Err(Problem::Failed("docker said no".into()))] {
        let b = bed("notapart");
        awake(&b, &b.ann.0).await;
        *b.mock.apart.lock().unwrap() = Some(answer_given);
        let r = Arc::new(run(&b, &b.ann.1, None));
        let asking = {
            let r = r.clone();
            tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "x"})).await })
        };
        let out = tokio::time::timeout(Duration::from_secs(5), asking).await.expect("it asked anyway").unwrap();
        assert_eq!(out.is_error, Some(true));
        assert!(words(&out).contains("nothing was handed over"), "{}", words(&out));
        assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
        assert!(!b.mock.calls().iter().any(|c| c == "clear_strays"), "{:?}", b.mock.calls());
        assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("recording").is_none());
    }
}

#[tokio::test]
async fn mock_computer_reaches_only_a_computer_tool_this_turn_is_offered() {
    let b = bed("mockhook");
    awake(&b, &b.ann.0).await;
    b.mock.calls.lock().unwrap().clear();
    let r = run(&b, &b.ann.1, None);
    for words in ["shell_run {\"command\": \"ls\"}", "memory_add {\"text\": \"x\"}", "tools_set {\"key\": \"computer\", \"state\": \"on\"}"] {
        let tried = tokio::time::timeout(Duration::from_secs(5), crate::hub::agent::slash_mock::mock_computer(&r, words)).await;
        assert!(tried.expect("it got as far as a card").is_err(), "{words}");
    }
    assert!(b.mock.calls().is_empty(), "{:?}", b.mock.calls());
    crate::hub::agent::slash_mock::mock_computer(&r, "computer_screenshot {}").await.unwrap();
    assert!(b.mock.calls().iter().any(|c| c.starts_with("screenshot")));

    // what a delegate isn't offered, it can't get this way either
    b.mock.calls.lock().unwrap().clear();
    let job = b.ctx.jobs.create(&b.ann.1, Value::Null, None, false);
    let turn = TurnCtx { thread: b.ann.1.clone(), acting_bot: Some(b.ann.0.clone()), ..TurnCtx::default() };
    let request = Run::new(b.ctx.clone(), turn, job, Arc::new(Settings::from(|_| None)));
    assert!(crate::hub::agent::slash_mock::mock_computer(&request, "computer_screenshot {}").await.is_err());
    assert!(b.mock.calls().is_empty(), "{:?}", b.mock.calls());
}

#[test]
fn mock_computer_is_debug_only() {
    let source = include_str!("../../agent/slash_mock.rs");
    for line in ["pub(crate) async fn mock_computer(", "if let Some(words) = p.strip_prefix(\"/mock computer \")"] {
        let at = source.find(line).expect(line);
        let before = source[..at].trim_end().lines().last().unwrap().trim();
        assert_eq!(before, "#[cfg(debug_assertions)]", "{line}");
    }
}

#[tokio::test]
async fn a_turn_with_nobody_there_cant_ask() {
    let b = bed("nobody");
    awake(&b, &b.ann.0).await;
    set(&b, &b.ann.0, tools::ROUTINES_KEY, "on");
    let r = run(&b, &b.ann.1, Some("rt_1"));
    assert!(!offered(&r).await.iter().any(|n| n == tools::ASK_PERSON));
    assert_eq!(offered(&r).await.len(), NAMES.len() - 1);
    let out = call(&r, tools::ASK_PERSON, json!({"reason": "x"})).await;
    assert_eq!(out.is_error, Some(true));
    assert_eq!(words(&out), ask::NOBODY);
}

#[tokio::test]
async fn a_reason_is_needed_and_kept_short() {
    let b = bed("reason");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    assert_eq!(call(&r, tools::ASK_PERSON, json!({"reason": "  "})).await.is_error, Some(true));
    let (_r, asking) = {
        drop(r);
        taken_over(&b, json!({"reason": "y".repeat(5000)})).await
    };
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["takeover"]["reason"].as_str().unwrap().len(), 200);
    b.computers.hand_back(&b.ann.0);
    asking.await.unwrap();
}

#[tokio::test]
async fn a_run_that_goes_away_leaves_no_person_holding_a_screen() {
    let b = timed("abandon");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let (r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    let _ws = connect(&b, &b.ann.0, "drive").await;
    asking.abort();
    let _ = asking.await;
    drop(r);
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["working"], json!(false));
    assert_eq!(b.computers.open_gate().unwrap().mode(&b.ann.0), None, "a drive ticket outlived its hand-over");
}

#[tokio::test]
async fn a_person_who_sleeps_the_computer_ends_the_hand_over() {
    let b = bed("sleeps");
    let id = awake(&b, &b.ann.0).await;
    let (_r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(5), asking).await.expect("sleeping the computer left the hand-over running").unwrap();
    assert_eq!(words(&out), Problem::Asleep.sentence());
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
}

#[tokio::test]
async fn a_sleep_that_fails_leaves_the_person_where_they_were() {
    let b = bed("sleepfails");
    let id = awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let (_r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    let _ws = connect(&b, &b.ann.0, "drive").await;
    *b.mock.sleep_problem.lock().unwrap() = Some(Problem::ProviderDown("Docker".into()));
    assert!(b.computers.sleep(&b.ctx, &id, Sleep::Light).await.is_err());
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_some(), "the hand-over ended on a sleep that never happened");
    assert_eq!(b.computers.open_gate().unwrap().mode(&b.ann.0), Some(Mode::Drive), "their viewer was cut for nothing");
    assert!(!asking.is_finished());
    *b.mock.sleep_problem.lock().unwrap() = None;
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    assert_eq!(b.computers.open_gate().unwrap().mode(&b.ann.0), None);
    let out = tokio::time::timeout(Duration::from_secs(5), asking).await.expect("the hand-over went on asleep").unwrap();
    assert_eq!(words(&out), Problem::Asleep.sentence());
}

#[tokio::test]
async fn sleeping_stops_what_the_model_was_recording() {
    let b = bed("sleeprec");
    let id = awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    call(&r, "computer_record", json!({"action": "start"})).await;
    b.mock.calls.lock().unwrap().clear();
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    let calls = b.mock.calls();
    let at = |word: &str| calls.iter().position(|c| c == word).unwrap_or_else(|| panic!("{word}: {calls:?}"));
    assert!(at("record_stop 7") < at("sleep Light"), "{calls:?}");
    assert!(b.computers.now().recordings.is_empty());
}

#[tokio::test]
async fn a_bot_deleted_during_a_hand_over_ends_it_and_keeps_the_rule_on_files() {
    let b = bed("deleted");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    let (_r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    b.computers.detach(&b.ctx, &b.ann.0).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(5), asking).await.expect("the Bot's going left the hand-over running").unwrap();
    assert_eq!(words(&out), Problem::Gone.sentence());
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(!b.mock.calls().iter().any(|c| c == "destroy"), "Bea is still there");
}

fn lazy(label: &str) -> Bed {
    bed_with(label, |p| Computers { idle: Duration::ZERO, deep: Duration::ZERO, ..Computers::new(p) })
}

#[tokio::test]
async fn a_screen_in_use_keeps_its_computer_up_however_long_it_looks_idle() {
    let b = lazy("sweep");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    call(&r, "computer_screenshot", json!({})).await;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Awake, "it slept under a run");
    drop(r);
    let (r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Awake, "it slept under a person");
    b.computers.hand_back(&b.ann.0);
    asking.await.unwrap();
    drop(r);
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Asleep, "free, it sleeps");
}

#[tokio::test]
async fn a_failed_read_of_the_seats_doesnt_sleep_a_screen_in_use() {
    let b = lazy("sweepread");
    let id = awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    call(&r, "computer_screenshot", json!({})).await;
    break_the_seats_read(&b);
    assert_eq!(b.computers.sleep_if_unused(&b.ctx, &id, Sleep::Light).await, Ok(false));
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Awake);
}

#[tokio::test]
async fn a_sweep_under_way_stops_runs_and_hand_overs_and_a_click_wakes_it_after() {
    let b = lazy("sweeprace");
    let id = awake(&b, &b.ann.0).await;
    let live = b.computers.live(&b.ctx, &b.ann.0).await.unwrap();
    b.mock.hang_sleep.store(true, Ordering::SeqCst);
    let sweep = {
        let (c, ctx) = (b.computers.clone(), b.ctx.clone());
        tokio::spawn(async move { c.sleep_the_idle(&ctx).await })
    };
    until_true("the sweep never started", async || b.computers.now().sleeping.contains(&id)).await;
    assert!(matches!(b.computers.hold(&b.ann.0, &id), Err(Problem::Asleep)), "a run started on a computer going to sleep");
    assert!(matches!(b.computers.hand_over(&live, "x", Duration::from_secs(5)), Err(Problem::Asleep)));
    assert!(matches!(b.computers.live(&b.ctx, &b.ann.0).await, Err(Problem::Asleep)));

    // a click in the middle of it: the sleep finishes, and then the wake, in that order
    b.computers.click(&b.ctx, &b.ann.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!b.mock.calls().iter().any(|c| c == "sleep Light"));
    b.mock.calls.lock().unwrap().clear();
    b.mock.hang_sleep.store(false, Ordering::SeqCst);
    sweep.await.unwrap();
    until_true("the click was lost", async || b.computers.status(&b.ctx, &b.ann.0).await["state"] == "awake").await;
    let calls = b.mock.calls();
    let at = |word: &str| calls.iter().position(|c| c == word).unwrap_or_else(|| panic!("{word}: {calls:?}"));
    assert!(at("sleep Light") < at("wake"), "{calls:?}");
    assert_eq!(*b.mock.power.lock().unwrap(), Power::Awake);
}

#[tokio::test]
async fn what_the_model_recorded_stops_before_a_person_is_on_the_screen() {
    let b = timed("record");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let r = run(&b, &b.ann.1, None);
    call(&r, "computer_record", json!({"action": "start"})).await;
    drop(r);
    let (r, asking) = taken_over(&b, json!({"reason": "x"})).await;
    assert!(b.mock.calls().iter().any(|c| c == "record_stop 7"), "{:?}", b.mock.calls());
    b.computers.hand_back(&b.ann.0);
    let said = words(&asking.await.unwrap());
    assert!(said.contains("recording_stopped") && said.contains("7.mp4"), "{said}");
    drop(r);

    // a plain drive viewer the same; a stop that fails keeps the person's ticket from being made
    let r = run(&b, &b.ann.1, None);
    call(&r, "computer_record", json!({"action": "start"})).await;
    *b.mock.record_problem.lock().unwrap() = Some(Problem::Failed("no".into()));
    let (status, v) = answer_of(open(b.ctx.clone(), open_req(&b.ann.0, json!({"mode": "drive"}))).await).await;
    assert_eq!(status, StatusCode::CONFLICT, "{v}");
    *b.mock.record_problem.lock().unwrap() = None;
    b.mock.calls.lock().unwrap().clear();
    let _ws = connect(&b, &b.ann.0, "drive").await;
    assert!(b.mock.calls().iter().any(|c| c == "record_stop 7"), "{:?}", b.mock.calls());
    // with a drive viewer open, a person can start typing at any moment
    assert_eq!(call(&r, "computer_record", json!({"action": "start"})).await.is_error, Some(true));
}

#[tokio::test]
async fn a_recording_past_its_life_or_lost_with_a_restart_keeps_nobody_out() {
    let b = bed("recordstale");
    let id = awake(&b, &b.ann.0).await;
    viewer(&b).await;
    *b.mock.record_problem.lock().unwrap() = Some(Problem::Failed("no such recording".into()));
    let drive = async |b: &Bed| answer_of(open(b.ctx.clone(), open_req(&b.ann.0, json!({"mode": "drive"}))).await).await.0;
    let recording = |began: Instant| Recording { computer: id.clone(), screen: seated(&b.ctx, &b.ann.0).unwrap().screen.unwrap(), id: Some(7), stopping: false, began };

    b.computers.now().recordings.insert(b.ann.0.clone(), recording(Instant::now()));
    assert_eq!(drive(&b).await, StatusCode::CONFLICT, "it may be recording still");
    b.computers.now().recordings.insert(b.ann.0.clone(), recording(Instant::now() - RECORDING_LIFE));
    assert_eq!(drive(&b).await, StatusCode::OK, "it ended by itself long ago");

    b.computers.now().recordings.insert(b.ann.0.clone(), recording(Instant::now()));
    b.computers.sleep(&b.ctx, &id, Sleep::Deep).await.unwrap();
    b.computers.wake_and_wait(&b.ctx, &id).await.unwrap();
    assert_eq!(drive(&b).await, StatusCode::OK, "a fresh start took the recording with it");
}

#[tokio::test]
async fn one_recording_at_a_time_so_none_is_left_with_nothing_to_stop_it() {
    let b = bed("onerec");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    assert_eq!(call(&r, "computer_record", json!({"action": "start"})).await.is_error, None);
    let second = call(&r, "computer_record", json!({"action": "start"})).await;
    assert_eq!(second.is_error, Some(true), "{}", words(&second));
    assert_eq!(b.mock.calls().iter().filter(|c| *c == "record_start").count(), 1, "a second one started");
    assert_eq!(call(&r, "computer_record", json!({"action": "stop", "id": 7})).await.is_error, None);
    assert_eq!(call(&r, "computer_record", json!({"action": "start"})).await.is_error, None, "stopped, it can start again");
}

#[tokio::test]
async fn a_recording_starting_as_a_person_comes_is_stopped_before_their_ticket() {
    let b = timed("recrace");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    b.mock.hold_record.store(true, Ordering::SeqCst);
    let r = Arc::new(run(&b, &b.ann.1, None));
    let starting = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, "computer_record", json!({"action": "start"})).await })
    };
    until_true("the start never got there", async || b.mock.calls().iter().any(|c| c == "record_start")).await;
    let ticket = {
        let ctx = b.ctx.clone();
        let req = open_req(&b.ann.0, json!({"mode": "drive"}));
        tokio::spawn(async move { answer_of(open(ctx, req).await).await })
    };
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!ticket.is_finished(), "their ticket went out with a recording on its way");
    b.mock.hold_record.store(false, Ordering::SeqCst);
    let (status, v) = ticket.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(b.mock.calls().iter().any(|c| c == "record_stop 7"), "it recorded the person: {:?}", b.mock.calls());
    let out = starting.await.unwrap();
    assert_eq!(out.is_error, Some(true), "{}", words(&out));
    assert!(b.computers.now().recordings.is_empty());
}

#[tokio::test]
async fn no_recording_starts_while_a_drive_ticket_is_being_made() {
    let b = bed("recmark");
    awake(&b, &b.ann.0).await;
    let live = b.computers.live(&b.ctx, &b.ann.0).await.unwrap();
    let coming = Coming::new(&b.computers, &b.ann.0);
    assert!(b.computers.begin_recording(&live).is_err());
    drop(coming);
    assert!(b.computers.begin_recording(&live).is_ok());
    // past the check, and a person came: the start is told to stop
    let coming = Coming::new(&b.computers, &b.ann.0);
    assert!(!b.computers.recording_started(&b.ann.0, 7));
    drop(coming);
}

#[tokio::test]
async fn a_recording_that_wont_stop_hands_nothing_over() {
    let b = bed("recordfail");
    awake(&b, &b.ann.0).await;
    let r = Arc::new(run(&b, &b.ann.1, None));
    call(&r, "computer_record", json!({"action": "start"})).await;
    *b.mock.record_problem.lock().unwrap() = Some(Problem::Failed("no".into()));
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "x"})).await })
    };
    let c = card(&b, &b.ann.1).await;
    answer(&b, &b.ann.1, &c, "once");
    assert_eq!(asking.await.unwrap().is_error, Some(true));
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none());
}

#[tokio::test]
async fn a_viewer_watches_unless_a_person_says_drive() {
    let b = bed("modes");
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let mode = |b: &Bed| b.computers.open_gate().unwrap().mode(&b.ann.0);
    assert_eq!(answer_of(open(b.ctx.clone(), open_req(&b.ann.0, json!({}))).await).await.0, StatusCode::OK);
    assert_eq!(mode(&b), Some(Mode::Watch), "saying nothing is watching");
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["viewer"], "watch");
    assert_eq!(answer_of(open(b.ctx.clone(), open_req(&b.ann.0, json!({"mode": "drive"}))).await).await.0, StatusCode::OK);
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["viewer"], "drive");
    for bad in [json!("admin"), json!(1), json!(true)] {
        let (status, v) = answer_of(open(b.ctx.clone(), open_req(&b.ann.0, json!({"mode": bad}))).await).await;
        assert_eq!(status, StatusCode::CONFLICT, "{v}");
    }
    assert_eq!(mode(&b), Some(Mode::Drive), "a refused word leaves what's there");
}

#[tokio::test]
async fn a_hand_back_is_a_persons_and_only_of_a_screen_they_have() {
    let b = bed("handback");
    awake(&b, &b.ann.0).await;
    let req = HubRequest { method: hyper::Method::POST, url: "/v1/host/computer/bot/b_ann/handback".into(), headers: hyper::HeaderMap::new(), body: Bytes::new(), params: vec![b.ann.0.clone()] };
    assert_eq!(hand_back(b.ctx.clone(), req).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(answer_of(hand_back(b.ctx.clone(), open_req(&b.ann.0, json!({}))).await).await.0, StatusCode::CONFLICT);
}

/// A sign-in page, served inside the box: all of it is one link, and only following that link
/// sets the cookie. What each browser sends back goes in a log.
const SIGN_IN: &str = r#"
import http.server
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        out = b'ok'
        self.send_response(200)
        if self.path == '/login':
            out = b'<body style="margin:0"><a href="/signed-in" style="display:block;width:100vw;height:100vh;background:#fff">sign in</a>'
        if self.path == '/signed-in':
            self.send_header('Set-Cookie', 'session=abc; Max-Age=3600')
        self.send_header('Content-Type', 'text/html')
        self.end_headers()
        self.wfile.write(out)
        with open('/tmp/c2b-signin.log', 'a') as f:
            f.write(self.path + ' cookie=' + str(self.headers.get('Cookie')) + '\n')
http.server.ThreadingHTTPServer(('127.0.0.1', 8123), H).serve_forever()
"#;

async fn real_until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..240 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("{what}");
}

/// The whole of it on a real computer, in the real browser, through the real gate and viewer. Only
/// the model is a test's calls. `cargo test -p buwiz-broker --all-features real_docker -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and the computer image"]
async fn real_docker_a_person_drives_signs_in_for_a_bot_and_hands_back() {
    let image = std::env::var("BUWIZ_COMPUTER_IMAGE").unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let docker = Arc::new(buwiz_computer::DockerProvider::new(image, "c2b"));
    docker.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let b = Arc::new(bed_with("real", |_| Computers { quiet: Duration::from_secs(1), ..Computers::new(docker) }));
    // a task of its own, so a failed check still gets to the cleaning up
    let local = tokio::task::LocalSet::new();
    let flow = local.spawn_local({
        let b = b.clone();
        async move { real_flow(&b).await }
    });
    let outcome = local.run_until(flow).await;
    for bot in [&b.bea.0, &b.ann.0] {
        b.computers.detach(&b.ctx, bot).await.unwrap();
    }
    b.computers.sleep_the_idle(&b.ctx).await;
    let _ = std::process::Command::new("docker").args(["network", "rm", "c2b-net"]).output();
    outcome.unwrap();
}

async fn real_flow(b: &Bed) {
    b.computers.click(&b.ctx, &b.ann.0).await.unwrap();
    real_until("it never woke", async || b.computers.status(&b.ctx, &b.ann.0).await["state"] == "awake").await;
    let id = seated(&b.ctx, &b.ann.0).unwrap().computer;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    let live = b.computers.live(&b.ctx, &b.ann.0).await.unwrap();
    let box_run = async |cmd: &str| live.provider.run(&live.machine, &live.screen, cmd, 10_000).await.unwrap();
    let quoted = SIGN_IN.replace('\'', "'\\''");
    let started = box_run(&format!("setsid nohup python3 -c '{quoted}' >/dev/null 2>&1 &")).await;
    assert_eq!(started.exit_code, Some(0), "{}", started.stderr);
    let log = async || box_run("cat /tmp/c2b-signin.log 2>/dev/null").await.stdout;
    // left running on the desktop, as something a click started is: it goes before the person starts
    let buwiz_computer::Reach::Container { name } = &live.machine.reach else { panic!("a container") };
    let in_box = |args: &[&str]| {
        let out = std::process::Command::new("docker").args(["exec", "--user", "0", name]).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let bot = live.screen.bot().unwrap().to_owned();
    let desktop = 20001 + 2 * in_box(&["cat", &format!("/home/box/.box/bots/{bot}")]).parse::<u32>().unwrap();
    in_box(&["sh", "-c", &format!("setpriv --reuid={desktop} --regid={desktop} --clear-groups setsid nohup python3 -c 'import time; time.sleep(300)' >/dev/null 2>&1 &")]);
    let pointer = || in_box(&["sh", "-c", &format!("DISPLAY={} XAUTHORITY=/run/box/x/svc.auth setpriv --reuid=990 --regid=990 --clear-groups xdotool getmouselocation --shell | head -2 | tr '\\n' ' '", live.screen.display)]);

    // two Bots, one computer: the check from outside lets a real command through
    let r = Arc::new(run(b, &b.ann.1, None));
    let shared = call(&r, "computer_run", json!({"command": "id -u"})).await;
    assert_eq!(shared.is_error, None, "{}", words(&shared));
    assert!(words(&shared).contains(&(desktop - 1).to_string()), "{}", words(&shared));

    // a person drives: what the agent does waits, then lands after they stop
    let opened = call(&r, "computer_open_url", json!({"url": "http://127.0.0.1:8123/login"})).await;
    assert_eq!(opened.is_error, None, "{}", words(&opened));
    let mut ws = connect(b, &b.ann.0, "drive").await;
    ws.write_all(&command("m,20,20")).await.unwrap();
    real_until("the person was never seen", async || b.computers.person_on(&b.ann.0)).await;
    let scrolling = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, "computer_scroll", json!({"x": 700, "y": 500, "dy": 120})).await })
    };
    for _ in 0..4 {
        ws.write_all(&command("m,21,21")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(!scrolling.is_finished(), "it went on under a person");
    let where_now = pointer();
    assert!(where_now.contains("X=21"), "the agent moved the pointer under a person: {where_now}");
    assert_eq!(scrolling.await.unwrap().is_error, None, "it goes on once they stop");
    let where_now = pointer();
    assert!(where_now.contains("X=700"), "{where_now}");
    drop(ws);

    // the agent asks, and the person signs in with the browser that's already there
    let asking = {
        let r = r.clone();
        tokio::spawn(async move { call(&r, tools::ASK_PERSON, json!({"reason": "Sign in to the shop", "time_limit_s": 120})).await })
    };
    let c = card(b, &b.ann.1).await;
    answer(b, &b.ann.1, &c, "once");
    real_until("no hand-over", async || b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_some()).await;
    let mut ws = connect(b, &b.ann.0, "drive").await;
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["viewer"], "drive");
    for step in ["m,640,420", "b,1,1", "b,1,0"] {
        ws.write_all(&command(step)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    real_until("the person's click never reached the page", async || log().await.contains("/signed-in")).await;
    assert!(!asking.is_finished(), "it went on before the hand-back");
    assert!(b.computers.hand_back(&b.ann.0));
    let out = asking.await.unwrap();
    assert_eq!(out.is_error, None, "{}", words(&out));
    let said = words(&out);
    assert!(said.contains("handed_back") && said.contains("image/jpeg"), "{}", &said[..said.len().min(300)]);
    assert!(said.contains("programs_ended") && said.contains("python3"), "{}", &said[..said.len().min(400)]);
    assert!(!said.contains(live.machine.viewer_password.expose()) && !said.contains(live.machine.token.expose()));
    let mut rest = Vec::new();
    assert!(tokio::time::timeout(Duration::from_secs(5), ws.read_to_end(&mut rest)).await.is_ok(), "the person's drive ticket outlived the hand-back");

    // its own browser kept the sign-in; the other Bot's profile never had it
    let again = call(&r, "computer_open_url", json!({"url": "http://127.0.0.1:8123/who-ann"})).await;
    assert_eq!(again.is_error, None, "{}", words(&again));
    let bea = run(b, &b.bea.1, None);
    let other = call(&bea, "computer_open_url", json!({"url": "http://127.0.0.1:8123/who-bea"})).await;
    assert_eq!(other.is_error, None, "{}", words(&other));
    real_until("the browsers never came back", async || {
        let l = log().await;
        l.contains("/who-ann") && l.contains("/who-bea")
    })
    .await;
    let l = log().await;
    println!("what the browsers sent:\n{l}");
    assert!(l.contains("/who-ann cookie=session=abc"), "{l}");
    assert!(l.contains("/who-bea cookie=None"), "{l}");
    let own = box_run(&format!("pgrep -u {desktop} -af -- --user-data-dir= | head -1")).await.stdout;
    assert!(own.contains(&format!("/desktops/{bot}/")), "{own}");
}

/// A chat turn of the real tool loop with a model that uses its computer: the card it parks on,
/// the screen it holds and the hand-over it waits on all go with a person's Stop.
#[tokio::test]
async fn a_stopped_run_frees_its_screen_and_expires_its_card() {
    use crate::hub::agent::runs::tests::{chat, ended, rows, stop, until, Script};

    let mut b = timed("stopped");
    let script = Arc::new(Script::default());
    let ctx = Arc::get_mut(&mut b.ctx).unwrap();
    ctx.model = Some(script.clone());
    ctx.local = Some(Arc::new(crate::hub::model::testkit::Local::default()));
    awake(&b, &b.ann.0).await;
    viewer(&b).await;
    let (bot, chat_id) = b.ann.clone();
    let other = run(&b, &chat_id, None);

    // it holds the screen and parks on a card
    script.tool("computer_click", json!({"x": 1, "y": 1}));
    script.tool(tools::ASK_PERSON, json!({"reason": "Sign in to the shop"}));
    let (job, done) = chat(&b.ctx, &chat_id);
    let asked = card(&b, &chat_id).await;
    assert_eq!(words(&call(&other, "computer_click", json!({"x": 2, "y": 2})).await), Problem::Held.sentence(), "the run has the screen");
    assert_eq!(rows(&b.ctx), 1);
    let calls = script.calls.load(Ordering::SeqCst);

    assert_eq!(answer_of(stop(&b.ctx, job).await).await.0, StatusCode::OK);
    done.await.unwrap().unwrap();
    let state = |id: &Value| b.ctx.store.read(|s| s.thread(&chat_id).unwrap().turns.iter().filter_map(|u| u.notice.clone()).find(|n| n["id"] == *id).unwrap()["state"].clone());
    assert_eq!(state(&asked["id"]), "expired", "nobody can answer a stopped run's card");
    assert_eq!(call(&other, "computer_click", json!({"x": 3, "y": 3})).await.is_error, None, "the screen is free");
    other.state().screen_hold = None;
    assert_eq!(b.ctx.jobs.with(job, |j| (j.done, j.detail.clone())).unwrap(), (true, "stopped".to_owned()));
    assert_eq!(rows(&b.ctx), 0);
    assert_eq!(ended(&b.ctx, &chat_id).len(), 1);
    assert!(ask::host_answer(&b.ctx, &json!({"chat": chat_id, "id": asked["id"], "answer": "once"})).is_err(), "a late yes finds nothing");
    assert_eq!(script.calls.load(Ordering::SeqCst), calls, "no model call after the stop");

    // it has handed the screen over and waits for the person
    script.tool(tools::ASK_PERSON, json!({"reason": "Sign in again"}));
    let (job, done) = chat(&b.ctx, &chat_id);
    let asked = card(&b, &chat_id).await;
    answer(&b, &chat_id, &asked, "once");
    until("it never reached the person", || b.computers.now().handovers.contains_key(&bot)).await;
    let mut ws = connect(&b, &bot, "drive").await;
    assert!(b.computers.status(&b.ctx, &bot).await.get("takeover").is_some());
    assert_eq!(answer_of(stop(&b.ctx, job).await).await.0, StatusCode::OK);
    done.await.unwrap().unwrap();
    assert!(b.computers.status(&b.ctx, &bot).await.get("takeover").is_none(), "the hand-over ended with the run");
    assert!(!b.computers.hand_back(&bot), "nothing's left to hand back");
    assert_eq!(b.computers.open_gate().unwrap().mode(&bot), None, "their drive ticket went with it");
    let mut rest = Vec::new();
    assert!(tokio::time::timeout(Duration::from_secs(2), ws.read_to_end(&mut rest)).await.is_ok(), "the person is still connected");
    until("the job never ended", || b.ctx.jobs.json(job as f64)["done"] == true).await;
}
