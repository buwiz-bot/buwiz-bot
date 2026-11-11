//! A remote computer: what a person adds, what the hub never does to it, and the tools against a
//! real `sshd`, and a real box behind it, on loopback. The real ones are ignored in a plain run:
//! `cargo test -p buwiz-broker --lib computer::tests::remote -- --ignored`.

use buwiz_computer::loopback::{self, Sshd};
use buwiz_computer::DockerProvider;

use super::together::{answer, answer_of, card, open_req};
use super::*;

const KEYLESS_PIN: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA==";

fn asked_for(sshd: &Sshd) -> Value {
    json!({"url": sshd.url(), "keyFile": sshd.key, "pin": sshd.pin})
}

fn route_body(bot: &str, tail: &str, body: Value) -> HubRequest {
    let mut req = open_req(bot, body);
    req.url = format!("/v1/host/computer/bot/{bot}{tail}");
    req
}

/// A key file with no passphrase, as far as `Asked` can tell.
fn key_at(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("id");
    std::fs::write(&path, "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----\n").unwrap();
    path
}

fn far_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("c3h-{name}-{}-{}", std::process::id(), buwiz_computer::random_hex(3).unwrap()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A row for a remote computer at `bot`, with no saved record behind it.
fn lost_remote(b: &Bed, bot: &str) -> String {
    let id = "cmp_0123456789abcdef".to_owned();
    b.ctx.db.with(|c| c.execute("INSERT INTO computers(id, provider, created_at, created_for, reach) VALUES(?,'remote',1,?,?)", rusqlite::params![id, bot, r#"{"kind":"ssh","host":"h","user":"u","port":null}"#])).unwrap();
    Computers::add_seat(&b.ctx, bot, &id, "member").unwrap();
    id
}

#[tokio::test]
async fn no_yes_no_computer() {
    let b = bed("remote-consent");
    let (status, body) = answer_of(super::add_remote(b.ctx.clone(), route_body("b_ann", "/remote", json!({"url": "ssh://me@h", "pin": KEYLESS_PIN}))).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["message"].as_str().unwrap().contains("say yes first"), "{body}");
    assert_eq!(seated(&b.ctx, "b_ann"), None);
    let (status, _) = answer_of(super::add_remote(b.ctx.clone(), route_body("b_ann", "/remote", json!({"url": "ssh://me@h", "consent": "yes"}))).await).await;
    assert_eq!(status, StatusCode::CONFLICT, "a yes is true and nothing else");
}

#[tokio::test]
async fn only_the_apps_token_reaches_the_routes() {
    let b = bed("remote-token");
    let mut req = route_body("b_ann", "/remote", json!({"consent": true}));
    req.headers.clear();
    assert_eq!(super::add_remote(b.ctx.clone(), req).await.status(), StatusCode::UNAUTHORIZED);
    let mut req = route_body("b_ann", "/remote/remove", json!({}));
    req.headers.clear();
    assert_eq!(super::remove_remote(b.ctx.clone(), req).await.status(), StatusCode::UNAUTHORIZED);
    let mut req = route_body("b_ann", "", json!({}));
    req.headers.clear();
    assert_eq!(super::test_remote(b.ctx.clone(), req).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_bot_with_a_computer_cant_add_another_and_a_local_one_isnt_removed_here() {
    let b = bed("remote-second");
    awake(&b, "b_ann").await;
    let body = json!({"url": "ssh://me@h", "pin": KEYLESS_PIN, "consent": true});
    let (status, said) = answer_of(super::add_remote(b.ctx.clone(), route_body("b_ann", "/remote", body)).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(said["message"].as_str().unwrap().contains("already has a computer"), "{said}");
    let (status, said) = answer_of(super::remove_remote(b.ctx.clone(), route_body("b_ann", "/remote/remove", json!({}))).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(said["message"].as_str().unwrap().contains("Only a remote computer"), "{said}");
    assert!(seated(&b.ctx, "b_ann").is_some());
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("destroy")));
}

#[tokio::test]
async fn a_lost_record_is_never_swapped_for_a_local_computer() {
    let b = bed("remote-lost");
    let id = lost_remote(&b, "b_ann");
    let said = b.computers.click(&b.ctx, "b_ann").await.unwrap_err().sentence();
    assert!(said.contains("saved connection is lost"), "{said}");
    assert_eq!(seated(&b.ctx, "b_ann").unwrap().computer, id, "the seat stays where it was");
    assert_eq!(b.mock.calls(), Vec::<String>::new(), "the local provider had no part in it");

    let status = b.computers.status(&b.ctx, "b_ann").await;
    assert_eq!((status["state"].as_str(), status["remote"]["lost"].as_bool()), (Some("error"), Some(true)), "{status}");
    assert!(status["message"].as_str().unwrap().contains("remove it and add it again"), "{status}");

    b.computers.remove_remote(&b.ctx, "b_ann").await.unwrap();
    assert_eq!(seated(&b.ctx, "b_ann"), None);
    assert!(Computers::ids(&b.ctx).is_empty());
    assert_eq!(b.mock.calls(), Vec::<String>::new(), "no sweep by the local provider either");
}

#[tokio::test]
async fn a_remote_computer_is_never_made_again_or_wiped_from_here() {
    let b = bed("remote-noremake");
    lost_remote(&b, "b_ann");
    for said in [b.computers.update(&b.ctx, "b_ann").await.unwrap_err().sentence(), b.computers.reset(&b.ctx, "b_ann").await.unwrap_err().sentence()] {
        assert!(said.contains("connected by a person"), "{said}");
    }
    assert_eq!(b.mock.calls(), Vec::<String>::new(), "no local provider had a part in it");
}

#[tokio::test]
async fn a_key_lost_from_the_keystore_says_so_and_nothing_else_stands_in() {
    let b = bed("remote-nokey");
    let (id, dir) = ("cmp_00ff".to_owned(), far_dir("nokey"));
    let m = Asked { url: "ssh://me@h.example".into(), key_file: Some(key_at(&dir)), pin: Some(KEYLESS_PIN.into()), ..Asked::default() }.machine(&Computers::dir(&b.ctx, &id)).unwrap();
    std::fs::create_dir_all(Computers::dir(&b.ctx, &id)).unwrap();
    Computers::record(&b.ctx, &id, &m, "b_ann").unwrap();
    Computers::add_seat(&b.ctx, "b_ann", &id, "member").unwrap();
    assert_eq!(b.computers.machine(&b.ctx, &id).unwrap().link.unwrap().key, None);
    assert!(b.computers.machines.lock().unwrap().is_empty(), "a key not found now may be found later");
    // once it's there, the same record has it
    keys(&b.ctx).unwrap().put_computer_key(&id, "the key").unwrap();
    assert_eq!(b.computers.machine(&b.ctx, &id).unwrap().link.unwrap().key.unwrap().expose(), "the key");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failed_read_of_the_row_keeps_the_local_provider_off_it() {
    let b = bed("remote-noread");
    let id = lost_remote(&b, "b_ann");
    b.ctx.db.with(|c| c.execute("ALTER TABLE computers RENAME COLUMN provider TO provider_gone", [])).unwrap();
    assert!(b.computers.click(&b.ctx, "b_ann").await.is_err());
    let _ = b.computers.finish_retire(&b.ctx, &id).await;
    assert_eq!(b.mock.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn not_being_able_to_read_a_record_counts_as_remote() {
    let b = bed("remote-unread");
    assert_eq!(b.computers.remote_at(&b.ctx, "b_ann"), None, "no computer, nothing to say");
    let id = awake(&b, "b_bea").await;
    assert_eq!(b.computers.remote_at(&b.ctx, "b_bea"), None, "a local one");
    lost_remote(&b, "b_ann");
    assert_eq!(b.computers.remote_at(&b.ctx, "b_ann"), Some(true), "a remote row with its record lost");
    b.computers.machines.lock().unwrap().clear();
    std::fs::remove_file(Computers::dir(&b.ctx, &id).join("machine.json")).unwrap();
    assert_eq!(b.computers.remote_at(&b.ctx, "b_bea"), None, "a local row says so, record or not");
    b.ctx.db.with(|c| c.execute("ALTER TABLE computers RENAME COLUMN provider TO provider_gone", [])).unwrap();
    assert_eq!(b.computers.remote_at(&b.ctx, "b_bea"), Some(true), "no row to read: not known to be local");
    let status = b.computers.status(&b.ctx, "b_bea").await;
    assert!(status.get("remote").is_none(), "a local computer isn't called remote: {status}");
    assert!(status["message"].as_str().unwrap().contains("couldn't be read"), "{status}");
    b.ctx.db.with(|c| c.execute("ALTER TABLE computer_seats RENAME TO seats_gone", [])).unwrap();
    assert_eq!(b.computers.remote_at(&b.ctx, "b_bea"), Some(true), "no seat to read");
}

#[tokio::test]
async fn ask_comes_with_a_remote_computer_and_a_persons_choice_stands() {
    let b = bed("remote-ask-default");
    assert_eq!(policy::tool_state(&b.ctx, "b_ann", tools::KEY), Pos::On, "a Bot with no computer");
    super::ask_first(&b.ctx, "b_ann").unwrap();
    assert_eq!(policy::tool_state(&b.ctx, "b_ann", tools::KEY), Pos::Ask);
    super::drop_ask(&b.ctx, "b_ann");
    assert_eq!(policy::tool_state(&b.ctx, "b_ann", tools::KEY), Pos::On, "it goes with the computer");

    set(&b, "b_ann", tools::KEY, "on");
    super::ask_first(&b.ctx, "b_ann").unwrap();
    assert_eq!(policy::tool_state(&b.ctx, "b_ann", tools::KEY), Pos::Ask, "an On from before says nothing of this machine");

    set(&b, "b_ann", tools::KEY, "on");
    super::drop_ask(&b.ctx, "b_ann");
    assert_eq!(policy::tool_state(&b.ctx, "b_ann", tools::KEY), Pos::On, "what a person set after isn't taken back");
    set(&b, "b_ann", tools::KEY, "ask");
    super::drop_ask(&b.ctx, "b_ann");
    assert_eq!(b.ctx.db.with(|c| c.query_row("SELECT actor FROM bot_tools WHERE bot_id='b_ann'", [], |r| r.get::<_, String>(0))).unwrap(), "human", "nor a person's Ask");

    set(&b, "b_bea", tools::KEY, "off");
    super::ask_first(&b.ctx, "b_bea").unwrap();
    assert_eq!(policy::tool_state(&b.ctx, "b_bea", tools::KEY), Pos::Off, "off isn't widened to Ask");
}

/// What every kind must say, whichever sentences it drops.
fn says_the_rest(words: &str) {
    for part in ["every right this login has", "that includes deleting things", "Nothing walls it off", "Sleep only disconnects", "puts its computer switch at Ask, unless it's Off", "with its computer switch at On"] {
        assert!(words.contains(part), "{part}: {words}");
    }
}

#[test]
fn each_kind_of_computer_is_told_only_what_is_true_of_it() {
    let ssh = Reach::Ssh { host: "h".into(), user: None, port: None };
    let https = Reach::Url { url: "https://a.example".into() };
    let (box_ssh, plain, box_https) = (consent_for(&ssh, true), consent_for(&ssh, false), consent_for(&https, true));
    for words in [&box_ssh, &plain, &box_https] {
        says_the_rest(words);
    }
    assert!(box_ssh.contains("use the screen there") && box_ssh.contains("kept in this Mac's Keychain"), "{box_ssh}");
    assert!(!plain.contains("screen") && plain.contains("read and change files there") && plain.contains("kept in this Mac's Keychain"), "{plain}");
    assert!(box_https.contains("use the screen there") && !box_https.contains("Keychain") && !box_https.contains("SSH"), "{box_https}");
}

#[test]
fn a_yes_covers_what_was_tested_and_nothing_else() {
    let dir = far_dir("tested");
    let b = bed("remote-tested");
    let c = &b.computers;
    let other = far_dir("tested-other");
    std::fs::write(key_at(&other), "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU=\nZZ==\n-----END OPENSSH PRIVATE KEY-----\n").unwrap();
    let asked = |url: &str, key: &std::path::Path, token: Option<&str>, viewer: Option<&str>, pin: &str| Asked { url: url.into(), key_file: Some(key.to_owned()), token: token.map(Secret::new), viewer_password: viewer.map(Secret::new), pin: Some(pin.into()) }.machine(&dir).unwrap();
    let (key_a, key_b) = (key_at(&dir), other.join("id"));
    let tested = asked("ssh://me@h", &key_a, Some("t"), Some("v"), KEYLESS_PIN);
    let test_it = || c.tested.lock().unwrap().insert(c.fingerprint(&tested, "b_ann").unwrap(), Instant::now());
    let pass = |m: &Machine| {
        test_it();
        c.take_tested(m, "b_ann")
    };
    assert!(pass(&tested));
    assert!(!c.take_tested(&tested, "b_ann"), "a yes is good once");
    test_it();
    assert!(!c.take_tested(&tested, "b_bea"), "another Bot's yes");
    c.tested.lock().unwrap().clear();
    for (what, m) in [
        ("the login", asked("ssh://root@h", &key_a, Some("t"), Some("v"), KEYLESS_PIN)),
        ("the host", asked("ssh://me@other", &key_a, Some("t"), Some("v"), KEYLESS_PIN)),
        ("the key", asked("ssh://me@h", &key_b, Some("t"), Some("v"), KEYLESS_PIN)),
        ("the token", asked("ssh://me@h", &key_a, Some("x"), Some("v"), KEYLESS_PIN)),
        ("the viewer password", asked("ssh://me@h", &key_a, Some("t"), Some("x"), KEYLESS_PIN)),
        ("the host key", asked("ssh://me@h", &key_a, Some("t"), Some("v"), "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIQ==")),
    ] {
        assert!(!pass(&m), "{what} changed after the Test");
        c.tested.lock().unwrap().clear();
    }
    c.tested.lock().unwrap().insert(c.fingerprint(&tested, "b_ann").unwrap(), Instant::now() - TESTED_LIFE);
    assert!(!c.take_tested(&tested, "b_ann"), "a Test from long ago");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&other);
}

/// With no randomness a Test's mark would be the same on every run, so there's no Test at all.
#[tokio::test]
async fn no_randomness_refuses_the_test_and_reaches_nobody() {
    let dir = far_dir("no-salt");
    let b = bed_with("remote-no-salt", |p| Computers { tested_salt: None, ..Computers::new(p) });
    let body = json!({"url": "ssh://me@127.0.0.1:9", "keyFile": key_at(&dir), "pin": KEYLESS_PIN});
    let (status, said) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_ann", "", body)).await).await;
    assert_ne!(status, StatusCode::OK, "{said}");
    assert!(said["message"].as_str().unwrap().contains("nothing was tested"), "{said}");
    assert!(b.computers.tested.lock().unwrap().is_empty());
    let machine = Asked { url: "ssh://me@h".into(), key_file: Some(key_at(&dir)), pin: Some(KEYLESS_PIN.into()), ..Asked::default() }.machine(&dir).unwrap();
    assert!(!b.computers.take_tested(&machine, "b_ann"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_test_is_for_a_bot_that_exists() {
    let b = bed("remote-no-bot");
    let (status, said) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_nobody", "", json!({"url": "https://box.example.com"}))).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{said}");
}

#[tokio::test]
async fn a_start_clears_what_a_crash_left_in_the_computers_folder() {
    let b = bed("remote-crash");
    let staged = b.ctx.data.join("computers/cmp_1/ssh");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("key"), "x").unwrap();
    let mut routes = crate::hub::Registry::default();
    let mut ctx = HubCtx::new(b.ctx.data.clone(), Clock::manual());
    ctx.ext.insert(b.computers.clone());
    super::install(&mut routes, &mut ctx);
    let ctx = Arc::new(ctx);
    for run in std::mem::take(&mut *routes.start.lock().unwrap()) {
        run(ctx.clone());
    }
    assert!(!staged.join("key").exists());
}

#[tokio::test]
async fn the_picker_says_a_computer_is_remote() {
    let b = bed("remote-list");
    lost_remote(&b, "b_ann");
    awake(&b, "b_bea").await;
    let list = Computers::list(&b.ctx);
    let remote: Vec<bool> = list["computers"].as_array().unwrap().iter().map(|c| c["remote"].as_bool().unwrap()).collect();
    assert_eq!(remote.iter().filter(|r| **r).count(), 1, "{list}");
}

#[test]
fn the_switch_doesnt_promise_a_wall_for_a_remote_computer() {
    let b = bed("remote-words");
    let (_, sub) = keys::label_of(&b.ctx, tools::KEY).unwrap();
    assert!(sub.contains("isn't walled off"), "{sub}");
}

/// A Bed with a real far end's Bot `b_ann` added and connected.
async fn added(b: &Bed, body: Value) -> String {
    let (_, tested) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_ann", "", body.clone())).await).await;
    assert_eq!(tested["ok"], true, "{tested}");
    let mut body = body;
    body["consent"] = json!(true);
    let (status, said) = answer_of(super::add_remote(b.ctx.clone(), route_body("b_ann", "/remote", body)).await).await;
    assert_eq!(status, StatusCode::OK, "{said}");
    assert_eq!(said["state"], "asleep", "added is not connected: {said}");
    assert_eq!(policy::tool_state(&b.ctx, "b_ann", tools::KEY), Pos::Ask, "a remote computer starts at Ask");
    // a person widens it, for the tests that aren't about the card
    set(b, "b_ann", tools::KEY, "on");
    awake(b, "b_ann").await
}

/// The file store of a test broker: never the Keychain.
fn kept_key(b: &Bed, id: &str) -> std::path::PathBuf {
    b.ctx.data.join("keys/computer-keys").join(format!("{id}.key"))
}

fn tree(dir: &std::path::Path) -> String {
    let out = std::process::Command::new("/usr/bin/find").arg(dir).output().unwrap();
    let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_owned).collect();
    lines.sort();
    lines.join("\n")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn a_plain_host_through_the_tools_and_the_routes() {
    let dir = far_dir("plain");
    let sshd = Sshd::start(&dir);
    let b = bed("remote-plain");

    // the test route: first the key to look at, then a yes, and no secret comes back
    let (_, offered) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_ann", "", json!({"url": sshd.url(), "keyFile": sshd.key}))).await).await;
    assert_eq!((offered["code"].as_str(), offered["pin"].as_str()), (Some("trust"), Some(sshd.pin.as_str())), "{offered}");
    let (_, tested) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_ann", "", asked_for(&sshd))).await).await;
    assert_eq!((tested["ok"].as_bool(), tested["screen"].as_bool()), (Some(true), Some(false)), "{tested}");
    let yes = tested["consent"].as_str().unwrap();
    says_the_rest(yes);
    assert!(!yes.contains("screen") && yes.contains("kept in this Mac's Keychain"), "a plain host: {yes}");
    assert_eq!(std::fs::read_dir(b.ctx.data.join("computers")).unwrap().count(), 0, "a test leaves nothing");

    let id = added(&b, asked_for(&sshd)).await;
    let key = std::fs::read_to_string(&sshd.key).unwrap();
    let saved = Computers::dir(&b.ctx, &id).join("machine.json");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(!std::fs::read_to_string(&saved).unwrap().contains("PRIVATE KEY"), "the record holds no key");
    assert_eq!(std::fs::read_to_string(kept_key(&b, &id)).unwrap(), key, "the keystore has it");
    let in_db: Option<String> = b.ctx.db.with(|c| c.query_row("SELECT reach FROM computers WHERE id=?", [&id], |r| r.get(0))).unwrap();
    assert!(!in_db.unwrap().contains("PRIVATE KEY"), "app.db holds no secret");
    let status = b.computers.status(&b.ctx, "b_ann").await;
    assert_eq!((status["state"].as_str(), status["remote"]["screen"].as_bool()), (Some("awake"), Some(false)), "{status}");
    assert!(!status.to_string().contains("PRIVATE KEY"));

    let r = run(&b, &b.ann.1, None);
    assert!(tools::offered(&r).await);
    let defs = tools::defs_for(&r);
    assert!(!defs.iter().any(|d| d["name"] == tools::ASK_PERSON), "no hand-over is offered");
    let said = defs.iter().find(|d| d["name"] == "computer_run").unwrap()["description"].as_str().unwrap().to_owned();
    assert!(said.contains("nothing walls it off") && !said.contains("as a user of its own"), "{said}");
    let prompt = tools::prompt(&r).await;
    assert!(prompt.contains("remote machine") && prompt.contains("no screen"), "{prompt}");

    let o = call(&r, "computer_run", json!({"command": "echo hello-from-far; pwd"})).await;
    assert!(words(&o).contains("hello-from-far") && words(&o).contains("agents/b_ann"), "{}", words(&o));
    assert!(call(&r, "computer_write_file", json!({"path": "a/b.txt", "content": "kept"})).await.is_error.is_none());
    assert!(words(&call(&r, "computer_read_file", json!({"path": "a/b.txt"})).await).contains("kept"));
    assert_eq!(std::fs::read_to_string(sshd.home.join("buwiz-computer/agents/b_ann/a/b.txt")).unwrap(), "kept");
    for bad in ["../../secret", "/etc/hosts", "a/../../x"] {
        assert_eq!(call(&r, "computer_read_file", json!({"path": bad})).await.is_error, Some(true), "{bad}");
    }
    call(&r, "computer_run", json!({"command": "ln -s /etc/hosts out"})).await;
    assert_eq!(call(&r, "computer_read_file", json!({"path": "out"})).await.is_error, Some(true), "a link out of the folder");

    // nothing here needs eyes: a model that can't see still has it
    b.computers.set_sight(std::collections::HashMap::from([(r.settings.model.clone(), false)]));
    assert!(tools::offered(&r).await);
    assert!(words(&call(&r, "computer_run", json!({"command": "echo blind-ok"})).await).contains("blind-ok"));
    b.computers.set_sight(std::collections::HashMap::new());

    // what comes back cut says so, or it gets written back over the file
    std::fs::write(sshd.home.join("buwiz-computer/agents/b_ann/long"), "y".repeat(40_000)).unwrap();
    let o = call(&r, "computer_read_file", json!({"path": "long"})).await;
    assert!(words(&o).contains("only the first 30000 characters"), "{}", &words(&o)[words(&o).len().saturating_sub(100)..]);

    // the screen tools are refused, in words
    for (name, input, word) in [
        ("computer_screenshot", json!({}), "show its screen"),
        ("computer_click", json!({"x": 1, "y": 1}), "take input"),
        ("computer_key", json!({"key": "Return"}), "take input"),
        ("computer_open_url", json!({"url": "https://example.com"}), "open a page"),
        ("computer_record", json!({"action": "start"}), "record its screen"),
        ("computer_wait", json!({}), "show its screen"),
    ] {
        let o = call(&r, name, input).await;
        assert_eq!(o.is_error, Some(true), "{name}");
        assert!(words(&o).contains(word), "{name}: {}", words(&o));
    }
    assert!(words(&call(&r, tools::ASK_PERSON, json!({"reason": "sign in"})).await).contains("can't be handed over"));
    let (status, said) = answer_of(super::open(b.ctx.clone(), open_req("b_ann", json!({}))).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(said["message"].as_str().unwrap().contains("show a live desktop"), "{said}");

    // the rules that don't change: a delegate, and a shared computer
    let delegate = {
        let job = b.ctx.jobs.create(&b.ann.1, Value::Null, None, false);
        let turn = TurnCtx { thread: b.ann.1.clone(), acting_bot: Some("b_bea".to_owned()), ..TurnCtx::default() };
        Run::new(b.ctx.clone(), turn, job, Arc::new(Settings::from(|_| None)))
    };
    assert!(!tools::offered(&delegate).await);
    assert!(words(&call(&delegate, "computer_run", json!({"command": "true"})).await).contains("working for another"));
    let r2 = run(&b, &b.ann.1, Some("routine-1"));
    assert!(words(&call(&r2, "computer_run", json!({"command": "true"})).await).contains("only when a person turns that on"));
    b.computers.attach(&b.ctx, "b_bea", &id).await.unwrap();
    assert_eq!(policy::tool_state(&b.ctx, "b_bea", tools::KEY), Pos::Ask, "a Bot that joins starts at Ask too");
    assert!(words(&call(&r, "computer_run", json!({"command": "true"})).await).contains("several Bots share"));
    b.computers.leave(&b.ctx, "b_bea").await.unwrap();
    assert_eq!(policy::tool_state(&b.ctx, "b_bea", tools::KEY), Pos::On, "and leaves it behind");

    // what the model gets back has no secret in it, even if the far end says it
    std::fs::write(sshd.home.join("buwiz-computer/agents/b_ann/leak"), &key).unwrap();
    let o = call(&r, "computer_read_file", json!({"path": "leak"})).await;
    assert!(!words(&o).contains("PRIVATE KEY") && words(&o).contains("[hidden]"), "{}", words(&o));

    // sleep is hanging up, and the sweep's sleep too
    let far = tree(&sshd.home);
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    assert_eq!(b.computers.status(&b.ctx, "b_ann").await["state"], "asleep");
    assert!(words(&call(&r, "computer_run", json!({"command": "true"})).await).contains("asleep"));
    // a new broker has only the record and the keystore
    b.computers.machines.lock().unwrap().clear();
    awake(&b, "b_ann").await;
    assert!(words(&call(&r, "computer_run", json!({"command": "echo back"})).await).contains("back"));

    // removing is forgetting
    let (status, said) = answer_of(super::remove_remote(b.ctx.clone(), route_body("b_ann", "/remote/remove", json!({}))).await).await;
    assert_eq!(status, StatusCode::OK, "{said}");
    assert_eq!(said["state"], "absent", "{said}");
    assert!(!Computers::dir(&b.ctx, &id).exists(), "its record went");
    assert!(!kept_key(&b, &id).exists(), "the key went with it");
    assert!(Computers::ids(&b.ctx).is_empty());
    assert_eq!(tree(&sshd.home), far, "removing touched the machine");
    assert_eq!(b.mock.calls(), Vec::<String>::new(), "the local provider had no part in it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn the_form_gets_a_boxs_words_over_ssh_and_a_yes_to_another_login_is_refused() {
    let dir = far_dir("words");
    let sshd = Sshd::start(&dir);
    let b = bed("remote-words-ssh");
    let mut body = asked_for(&sshd);
    body["token"] = json!("a-token");
    body["viewerPassword"] = json!("a-password");
    let (_, tested) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_ann", "", body.clone())).await).await;
    assert_eq!((tested["ok"].as_bool(), tested["screen"].as_bool()), (Some(true), Some(true)), "{tested}");
    let words = tested["consent"].as_str().unwrap();
    says_the_rest(words);
    assert!(words.contains("use the screen there") && words.contains("kept in this Mac's Keychain"), "{words}");

    // the login changed after the Test: same pinned host, so only the binding can say no
    let mut other = body.clone();
    other["url"] = json!(format!("ssh://nobody-else@127.0.0.1:{}", sshd.port));
    other["consent"] = json!(true);
    let (status, said) = answer_of(super::add_remote(b.ctx.clone(), route_body("b_ann", "/remote", other)).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(said["message"].as_str().unwrap().contains("isn't what was tested"), "{said}");
    assert_eq!(seated(&b.ctx, "b_ann"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs openssl"]
async fn the_form_gets_https_words_and_no_token_goes_on_a_test() {
    let dir = far_dir("words-tls");
    let (exec, host) = ("exec.box.c3t.test".to_owned(), "host.box.c3t.test".to_owned());
    let (root, cert, key) = loopback::vouched_certificate(&dir, &[exec.clone(), host.clone()]);
    let behind = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let to = behind.local_addr().unwrap().to_string();
    let at = loopback::terminator(cert.clone(), key, std::collections::HashMap::from([(exec, to.clone()), (host, to)])).await;
    let b = bed_with("remote-words-tls", |p| Computers::new(p).with_remote(buwiz_computer::RemoteProvider::new().with_dial(at).with_root(root)));
    let mut body = json!({"url": "https://box.c3t.test", "token": "a-token", "viewerPassword": "a-password"});
    let (_, offered) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_ann", "", body.clone())).await).await;
    assert_eq!((offered["code"].as_str(), offered["vouched"].as_bool()), (Some("trust"), Some(true)), "{offered}");
    body["pin"] = offered["pin"].clone();
    let (_, tested) = answer_of(super::test_remote(b.ctx.clone(), route_body("b_ann", "", body)).await).await;
    assert_eq!((tested["ok"].as_bool(), tested["screen"].as_bool()), (Some(true), Some(true)), "{tested}");
    let words = tested["consent"].as_str().unwrap();
    says_the_rest(words);
    assert!(words.contains("use the screen there") && !words.contains("Keychain") && !words.contains("SSH"), "{words}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn deleting_the_last_bot_leaves_the_machine_alone() {
    let dir = far_dir("delete");
    let sshd = Sshd::start(&dir);
    let b = bed("remote-delete");
    let id = added(&b, asked_for(&sshd)).await;
    let r = run(&b, &b.ann.1, None);
    call(&r, "computer_write_file", json!({"path": "kept.txt", "content": "x"})).await;
    let far = tree(&sshd.home);
    b.computers.detach(&b.ctx, "b_ann").await.unwrap();
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(Computers::ids_where(&b.ctx, "1=1").is_empty(), "the record is gone");
    assert!(!Computers::dir(&b.ctx, &id).exists());
    assert!(!kept_key(&b, &id).exists(), "deleting the last Bot took the key too");
    assert_eq!(tree(&sshd.home), far);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn a_click_during_the_sweeps_sleep_isnt_lost_on_a_remote_computer() {
    let dir = far_dir("sweep");
    let sshd = Sshd::start(&dir);
    let b = bed_with("remote-sweep", |p| Computers { idle: Duration::ZERO, ..Computers::new(p) });
    let id = added(&b, asked_for(&sshd)).await;
    let woke = {
        let (ctx, computers) = (b.ctx.clone(), b.computers.clone());
        let sweep = tokio::spawn({
            let (ctx, computers) = (ctx.clone(), computers.clone());
            async move { computers.sleep_if_unused(&ctx, &id, Sleep::Light).await }
        });
        tokio::task::yield_now().await;
        computers.click(&ctx, "b_ann").await.unwrap();
        sweep.await.unwrap().unwrap();
        ctx
    };
    for _ in 0..600 {
        if b.computers.status(&woke, "b_ann").await["state"] == "awake" {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the click was lost: {}", b.computers.status(&woke, "b_ann").await);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd"]
async fn at_ask_a_remote_computer_waits_for_a_card() {
    let dir = far_dir("ask");
    let sshd = Sshd::start(&dir);
    let b = bed("remote-ask");
    added(&b, asked_for(&sshd)).await;
    set(&b, "b_ann", tools::KEY, "ask");
    set(&b, "b_ann", tools::ROUTINES_KEY, "on");
    let r = run(&b, &b.ann.1, Some("routine-1"));
    // a routine has nobody to answer: the card is refused, and nothing ran
    let o = call(&r, "computer_run", json!({"command": "touch ran"})).await;
    assert_eq!(o.is_error, Some(true), "{}", words(&o));
    assert!(!sshd.home.join("buwiz-computer/agents/b_ann/ran").exists());

    // a person there gets a card that says what this is, and nothing runs before their yes
    let r = Arc::new(run(&b, &b.ann.1, None));
    let asking = tokio::spawn({
        let r = r.clone();
        async move { call(&r, "computer_run", json!({"command": "touch ran"})).await }
    });
    let c = card(&b, &b.ann.1).await;
    assert!(c["lines"].to_string().contains("remote machine"), "{c}");
    assert!(!sshd.home.join("buwiz-computer/agents/b_ann/ran").exists());
    answer(&b, &b.ann.1, &c, "once");
    assert_eq!(asking.await.unwrap().is_error, None);
    assert!(sshd.home.join("buwiz-computer/agents/b_ann/ran").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sshd, Docker and the computer image"]
async fn a_real_box_through_the_tools_the_tile_and_the_viewer() {
    let image = std::env::var("BUWIZ_COMPUTER_IMAGE").unwrap_or_else(|_| "buwiz-computer:local".to_owned());
    let dir = far_dir("box");
    let docker = DockerProvider::new(image, "c3th");
    docker.check().await.unwrap_or_else(|e| panic!("this test needs Docker and the image: {e}"));
    let box_dir = far_dir("box-record");
    let local = docker.create(&Spec { id: "cmp_c3th".into(), dir: box_dir.clone(), network: Default::default() }).await.unwrap();
    docker.wake(&local).await.unwrap();
    let (_, seen) = docker.look(&local).await.unwrap();
    let ports = seen.seen.as_ref().unwrap().ports.clone();
    for port in [1337u16, 1340, 6901, 6902, 6903, 6904, 6905] {
        loopback::forward(port, ports[&port].clone()).await;
    }
    let sshd = Sshd::start(&dir);
    let b = bed("remote-box");
    let mut body = asked_for(&sshd);
    body["token"] = json!(local.token.expose());
    body["viewerPassword"] = json!(local.viewer_password.expose());
    let id = added(&b, body).await;
    assert_eq!(b.computers.status(&b.ctx, "b_ann").await["remote"]["screen"], true);

    let r = run(&b, &b.ann.1, None);
    let o = call(&r, "computer_screenshot", json!({})).await;
    assert!(o.content.to_string().contains("image/jpeg"), "a real picture came back");
    assert_eq!(call(&r, "computer_click", json!({"x": 30, "y": 30})).await.is_error, None);
    assert!(words(&call(&r, "computer_run", json!({"command": "pwd"})).await).contains("/workspace/agents/b_ann"));
    assert!(call(&r, "computer_write_file", json!({"path": "n.txt", "content": "kept"})).await.is_error.is_none());
    assert!(words(&call(&r, "computer_read_file", json!({"path": "n.txt"})).await).contains("kept"));
    assert!(words(&call(&r, tools::ASK_PERSON, json!({"reason": "sign in"})).await).contains("can't be handed over"));

    // the tile and the viewer, through the same routes the app calls
    let res = super::screen(b.ctx.clone(), route_body("b_ann", "/screen", json!({}))).await;
    assert_eq!(res.status(), StatusCode::OK);
    let (status, opened) = answer_of(super::open(b.ctx.clone(), open_req("b_ann", json!({"mode": "drive"}))).await).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert!(opened["url"].as_str().unwrap().contains("ticket="));
    assert!(!opened.to_string().contains(local.viewer_password.expose()));

    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    assert_eq!(b.computers.status(&b.ctx, "b_ann").await["state"], "asleep");
    assert_eq!(docker.power(&local).await.unwrap(), Power::Awake, "a remote computer's sleep doesn't reach it");
    assert_eq!(docker.read_file(&local, "/workspace/agents/b_ann/n.txt").await.unwrap(), b"kept");
    b.computers.remove_remote(&b.ctx, "b_ann").await.unwrap();
    assert!(docker.screens(&local).await.unwrap().iter().any(|s| s == "b_ann"), "removing took its screen down");

    docker.destroy(&local).await.unwrap();
    let _ = std::process::Command::new("docker").args(["network", "rm", "c3th-net"]).output();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&box_dir);
}
