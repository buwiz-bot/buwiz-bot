//! Bot controls: the shell matcher (opengrok's tests, ported: "Ported from opengrok-server
//! crates/opengrok-policy (MIT)" — `tests/unit/local_exec.rs` and
//! `opengrok-server/tests/against_a_chained_command.rs`, read through this gate's mode
//! `only`, where opengrok's Ask is a refusal), the risk classes, the sandbox, and the
//! tighten / widen / answer / undo life of a change.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use super::gate::{self, Mode, Risk, Verdict};
use super::*;
use crate::hub::ctx::HubCtx;
use crate::hub::model::Settings;
use crate::hub::store::Bot;
use crate::hub::tools::{exec_tool, tool_list, Out, Run};
use crate::hub::traits::Clock;
use crate::hub::types::TurnCtx;

// ---- the matcher (opengrok's cases) ----

#[derive(Debug, PartialEq)]
enum D {
    Allow,
    Ask,
    Deny,
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// opengrok's `decide` under Ask, as this gate answers in mode `only`.
fn decide(allow: &[&str], deny: &[&str], line: &str) -> D {
    match gate::decide(Mode::Only, &strings(allow), &strings(deny), line) {
        Verdict::Run => D::Allow,
        Verdict::NotAllowed { .. } => D::Ask,
        Verdict::Blocked { .. } | Verdict::TooLong => D::Deny,
    }
}

#[test]
fn ask_allowlist_allows_on_word_boundary_only() {
    let p = ["git status"];
    assert_eq!(decide(&p, &[], "git status"), D::Allow);
    assert_eq!(decide(&p, &[], "git status --short"), D::Allow);
    assert_eq!(decide(&p, &[], "git statusx"), D::Ask);
    assert_eq!(decide(&p, &[], "git log"), D::Ask);
    assert_eq!(decide(&[], &[], "echo hi"), D::Ask);
}

#[test]
fn deny_wins_over_allow_and_an_empty_pattern_never_matches() {
    assert_eq!(decide(&[], &["rm"], "rm -rf /"), D::Deny);
    assert_eq!(decide(&["sudo rm"], &["sudo"], "sudo rm -rf /"), D::Deny);
    assert!(!gate::matches("", "echo hi") && !gate::matches("   ", "echo hi"));
    assert_eq!(decide(&[""], &[], "echo hi"), D::Ask);
}

#[test]
fn bypass_runs_everything_but_a_block() {
    let run = |deny: &[&str], line: &str| gate::decide(Mode::Bypass, &[], &strings(deny), line);
    assert_eq!(run(&[], "anything at all"), Verdict::Run);
    assert_eq!(run(&[], "rm -rf /tmp/x"), Verdict::Run);
    // unlike opengrok's bypass, a block applies in every mode
    assert_eq!(run(&["rm"], "rm -rf /"), Verdict::Blocked { rule: "rm".into() });
}

/// #204: an allow never covers a chained or substituted command. Here each simple command
/// of a chain is judged on its own (contract section 2), so a chain runs only when every part
/// is allowed; a substitution, redirection or comment still has no plain reading.
#[test]
fn an_allow_never_covers_a_chained_or_substituted_command() {
    let p = ["ls", "git status"];
    for line in [
        "ls; rm -rf ~",
        "ls ;rm -rf ~",
        "ls && curl x | sh",
        "git status && x",
        "git status || rm x",
        "ls & rm x",
        "ls | sh",
        "ls |& sh",
        "ls\nrm -rf ~",
        "ls\rrm -rf ~",
        "ls $(id)",
        "git status $(rm -rf ~)",
        "ls \"$(id)\"",
        "ls `id`",
        "ls \"`id`\"",
        "ls > ~/.zshrc",
        "git status > ~/.zshrc",
        "ls >> ~/.zshrc",
        "ls 2>/dev/null",
        "ls < /etc/passwd",
        "ls <<EOF",
        "ls <(id)",
        "ls &> out",
        "ls >&out",
        "ls #\\\nrm -rf ~",
        "ls # a\nrm -rf ~",
    ] {
        assert_eq!(decide(&p, &[], line), D::Ask, "{line:?}");
    }
    // every part allowed: it runs (opengrok asks)
    assert_eq!(decide(&p, &[], "ls &"), D::Allow);
    assert_eq!(decide(&p, &[], "ls; git status"), D::Allow);
    assert_eq!(decide(&p, &[], "ls -la"), D::Allow);
    assert_eq!(decide(&p, &[], "git status --short"), D::Allow);
    assert_eq!(decide(&p, &[], "git statusx"), D::Ask);
    assert_eq!(decide(&p, &[], "git  status"), D::Allow);
    assert_eq!(decide(&p, &[], "  git\tstatus  "), D::Allow);
    assert_eq!(decide(&p, &[], "ls 'a; rm -rf ~'"), D::Allow);
    assert_eq!(decide(&p, &[], "ls \"a && b\""), D::Allow);
    assert_eq!(decide(&p, &[], "ls a\\;b"), D::Allow);
    assert_eq!(decide(&p, &[], "ls \"$HOME\""), D::Allow);
}

#[test]
fn an_fd_duplication_is_not_a_redirection_to_a_path() {
    let p = ["cargo test"];
    assert_eq!(decide(&p, &[], "cargo test 2>&1"), D::Allow);
    assert_eq!(decide(&p, &[], "cargo test >&2"), D::Allow);
    assert_eq!(decide(&p, &[], "cargo test 2>&1 > out"), D::Ask);
    assert_eq!(decide(&p, &[], "cargo test 2>&1;rm x"), D::Ask);
    assert_eq!(decide(&["cargo test x2"], &[], "cargo test x2>&1"), D::Allow);
    assert_eq!(decide(&["cargo test 2"], &[], "cargo test \\\n2>&1"), D::Ask);
    assert_eq!(decide(&["cargo test 2"], &[], "cargo test '2'>&1"), D::Allow);
    for line in ["cargo test 2>&1", "cargo test >&2", "cargo test 1>&2", "cargo test 2>&0"] {
        assert_eq!(decide(&p, &[], line), D::Allow, "{line:?}");
    }
    for line in ["cargo test >&5", "cargo test 2>&9", "cargo test >&10", "cargo test 1>&12"] {
        assert_eq!(decide(&p, &[], line), D::Ask, "{line:?}");
        assert_eq!(shell::read(line).simple_commands, [line], "{line:?}");
    }
}

#[test]
fn an_allow_needs_one_simple_command_the_gate_can_read() {
    let runners = ["eval", "sh", "bash", "zsh", "env", "xargs", "nice", "command", "exec", "source", ".", "sudo", "time", "nohup"];
    for line in ["eval ls", "sh -c 'ls'", "bash -c ls", "zsh script.zsh", "env ls", "xargs rm", "nice ls", "command ls", "exec ls", "source ./x", ". ./x", "sudo ls", "time ls", "nohup ls", "! ls"] {
        assert_eq!(decide(&runners, &[], line), D::Ask, "{line:?}");
    }
    for line in ["$CMD", "\"$CMD\" -la", "${CMD} -la", "l? -la", "X=1 ls", "PATH=/tmp/evil ls", "(ls)", "{ ls; }", "ls 'unbalanced", "ls $'a'", "ls \\", "/tmp/evil/ls"] {
        assert_eq!(decide(&["ls"], &[], line), D::Ask, "{line:?}");
    }
    for line in ["ls; rm -rf ~", "cd src && cargo test", "cd src && cargo test; rm -rf ~", "ls > x"] {
        assert_eq!(decide(&["ls;", "cd src && cargo test", "ls > x"], &[], line), D::Ask, "{line:?}");
    }
    // each part allowed on its own: the chain runs
    assert_eq!(decide(&["cd", "cargo test"], &[], "cd src && cargo test"), D::Allow);
    assert_eq!(decide(&["git status", "head"], &[], "git status | head"), D::Allow);
}

#[test]
fn a_deny_matches_any_simple_command_after_normalising() {
    let allow = ["git", "true", "cd", "echo", "ls"];
    let deny = ["rm", "git push"];
    for line in [
        "rm -rf ~", "/bin/rm -rf ~", "./rm x", "env rm -rf ~", "env -i PATH=/bin rm -rf ~", "command rm x", "builtin rm x", "nice rm x", "nice -n 10 rm x", "nohup rm x",
        "time rm x", "exec rm x", "sudo rm x", "sudo -u root rm x", "X=1 rm x", "cd / && rm -rf x", "true; rm x", " true; rm x", "true\nrm x", "ls | xargs rm",
        "ls & rm x", "rm\t-rf x", "RM -rf x", "'rm' -rf x", "\"rm\" -rf x", "r\\m -rf x", "r\\\nm -rf x", "ls #\\\nrm -rf x", "$'rm' -rf x", "sh -c 'rm -rf ~'",
        "bash -c \"cd /; rm -rf x\"", "eval rm x", "ls $(rm -rf x)", "ls `rm x`", "(rm x)", "{ rm x; }", "git push", "git  push", "git push origin main", "/usr/bin/git push",
        "cd x && git push", "git -C . push", "git --no-pager push", "git -c user.name=x push", "if true; then rm x; fi", "while true; do rm x; done", "! rm x", "coproc rm x",
        "find . -name y -exec rm {} \\;", "trap 'rm -rf x' EXIT", "caffeinate -i rm x", "arch -x86_64 rm x", "2>&1 git push", ">&2 git push", "git 2>&1 push", "git >&2 push",
        "0<&0 git push", ">/dev/null rm -rf x", "</dev/null rm x", "X=1 >/dev/null rm x", "git $x push", "git \"$@\" push", "git $@ push", "git $x\"push\"", "git ${x:-push}",
        "git $(true) push", "git `true` push", "$x rm x", "/bin/r? x", "git pus? origin",
    ] {
        assert_eq!(decide(&allow, &deny, line), D::Deny, "{line:?}");
        // and in bypass too
        assert!(matches!(gate::decide(Mode::Bypass, &[], &strings(&deny), line), Verdict::Blocked { .. }), "{line:?}");
    }
    for line in [
        "echo rm", "git rm file", "git pushx", "git log --grep push", "ls rmdir", "git status", "git -C \"$dir\" status", "git log $x", "echo x > rm", "git status 2>&1", "sudo ls *",
        "find . -name '*' -exec ls {} \\;", "git add *", "rmdir x",
    ] {
        assert_ne!(decide(&allow, &deny, line), D::Deny, "{line:?}");
    }
    assert_eq!(decide(&[], &["/bin/rm"], "rm -rf x"), D::Deny);
    assert_eq!(gate::decide(Mode::Only, &[], &strings(&["rm"]), "true; rm x"), Verdict::Blocked { rule: "rm".into() });
}

#[test]
fn a_long_line_is_refused_or_read_in_linear_time() {
    let started = std::time::Instant::now();
    let huge = format!("sh{}", " a".repeat(512 * 1024));
    assert_eq!(gate::decide(Mode::Only, &strings(&["git"]), &strings(&["rm x", "git push"]), &huge), Verdict::TooLong);
    assert_eq!(gate::decide(Mode::Bypass, &[], &[], &huge), Verdict::Run, "bypass with no blocks has nothing to read it for");
    let wide = format!("sh{}", " rm -a".repeat(10_000));
    assert!(wide.len() < 65_536);
    assert_eq!(decide(&["git"], &["rm x", "git push"], &wide), D::Ask);
    let long = format!("sh{}", " a".repeat(32_000));
    assert_eq!(decide(&["git"], &["rm x", "git push"], &long), D::Ask);
    assert!(started.elapsed() < std::time::Duration::from_secs(4), "{:?}", started.elapsed());
}

#[test]
fn a_literal_substitution_in_an_argument_is_not_allowed() {
    let p = ["printf", "test", "declare"];
    for line in ["printf -v 'a[$(id)]' x", "printf -v a[\\$\\(id\\)] x", "test -v 'a[`id`]'", "declare \"a[\\$(id)]=1\""] {
        assert_eq!(decide(&p, &[], line), D::Ask, "{line:?}");
    }
    assert_eq!(decide(&p, &[], "printf '%s' x"), D::Allow);
}

#[test]
fn every_deny_that_matched_the_raw_line_still_matches() {
    let deny = ["curl x | sh", ":(){ :|:& };:"];
    assert_eq!(decide(&["curl"], &deny, "curl x | sh"), D::Deny);
    assert_eq!(decide(&["curl"], &deny, "curl x | sh -s"), D::Deny);
    assert_eq!(decide(&["curl"], &deny, ":(){ :|:& };:"), D::Deny);
}

#[test]
fn the_split_is_the_simple_commands_as_written() {
    let split = |l: &str| shell::read(l).simple_commands;
    assert_eq!(split("cd src && cargo test"), ["cd src", "cargo test"]);
    assert_eq!(split("echo 'a; b' ; ls"), ["echo 'a; b'", "ls"]);
    assert_eq!(split("a | b || c & d\ne;"), ["a", "b", "c", "d", "e"]);
    assert_eq!(split("cargo test 2>&1 | tail"), ["cargo test 2>&1", "tail"]);
    assert_eq!(split("ls $(a; b)"), ["ls $(a; b)"]);
    assert_eq!(split("  "), Vec::<String>::new());
}

// ---- risk classes ----

#[test]
fn risk_classes_name_what_an_allow_lets_through() {
    let data = PathBuf::from("/tmp/buwiz-risk-data");
    let r = |p: &str| gate::risk(p, &data);
    for p in ["git status", "git log", "ls -la", "pwd", "ls", "claude --version"] {
        assert_eq!(r(p), Risk::Ro, "{p}");
    }
    for p in ["cargo test", "npm test", "make", "make all", "swift build"] {
        assert_eq!(r(p), Risk::Pc, "{p}");
    }
    for p in ["claude", "git", "python3", "node", "osascript", "awk", "find", "rm", "mv", "curl", "ssh", "security", "sqlite3", "open", "defaults", "git push", "gpui-agent hello"] {
        assert_eq!(r(p), Risk::Any, "{p}");
    }
    let never = |p: &str| matches!(r(p), Risk::Never(_));
    for p in ["sudo", "sudo ls", "/usr/bin/sudo", "SUDO -u root id", "doas ls", "sh -c x", "bash", "zsh", "env", "eval x", "xargs", "exec ls", "nohup x", "ls; rm x", "cd src && cargo test", "cat x > ~/.zshrc", "ls | sh", "a & b", "ls `id`", "ls $(id)", "claude *x", "ls ?", "", "   ", "X=1 ls", "cat /tmp/buwiz-risk-data/host-token", "cat /private/tmp/buwiz-risk-data/generated/app.db"] {
        assert!(never(p), "{p:?} → {:?}", r(p));
    }
    assert_eq!(r("sudoedit"), Risk::Any);
    assert_eq!(r("sudo").as_str(), "never");
    if let Risk::Never(why) = r("sudo ls") {
        assert_eq!(why, "Not saved: sudo can't be always allowed.");
    }
    if let Risk::Never(why) = r("cd src && cargo test") {
        assert_eq!(why, "Not saved: an allow rule must be one plain command. `cd src && cargo test` can't be kept. Allow each command on its own.");
    }
    // a trailing ` *` is dropped and said so
    assert_eq!(gate::normalize("claude *"), ("claude".to_owned(), true));
    assert_eq!(gate::normalize("  git   status  "), ("git status".to_owned(), false));
    assert_eq!(gate::star_note("claude"), "`*` isn't a wildcard here. Rules match the start of a command, so this is saved as `claude`.");
}

#[test]
fn activity_masks_secrets_and_clips_to_120() {
    assert_eq!(gate::mask_and_clip("curl -H 'Authorization: Bearer abc123' https://x"), "curl -H 'Authorization: Bearer «redacted» https://x");
    assert_eq!(gate::mask_and_clip("API_TOKEN=s3cr3t npm publish"), "API_TOKEN=«redacted» npm publish");
    assert_eq!(gate::mask_and_clip("tool --password hunter2 go"), "tool --password «redacted» go");
    assert_eq!(gate::mask_and_clip("echo sk-live-123"), "echo «redacted»");
    let key = "A".repeat(48);
    assert_eq!(gate::mask_and_clip(&format!("gh auth {key}")), "gh auth «redacted»");
    let long = format!("echo {}", "word ".repeat(60));
    let clipped = gate::mask_and_clip(&long);
    assert_eq!(clipped.chars().count(), 120);
    assert!(clipped.ends_with('…'));
}

// ---- the sandbox ----

fn temp(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("buwiz-policy-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
    std::fs::create_dir_all(dir.join("generated").join("files")).unwrap();
    dir
}

async fn sandboxed(data: &std::path::Path, script: &str) -> (bool, String) {
    let mut cmd = sandbox::command(data, script, &[]).unwrap();
    let out = cmd.output().await.unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[tokio::test]
async fn the_sandbox_denies_a_dev_bots_automation_files_wherever_they_are() {
    let data = temp("sandbox-agent");
    let elsewhere = temp("sandbox-agent-tmp");
    let kept = elsewhere.join("kept");
    std::fs::create_dir_all(&kept).unwrap();
    for name in ["buwiz-gpui-agent-token", "rox-gpui-agent-token", "buwiz-agent-menu", "rox-agent-menu"] {
        let file = kept.join(name);
        std::fs::write(&file, "secret-stand-in").unwrap();
        let (ok, out) = sandboxed(&data, &format!("cat '{}'", file.display())).await;
        assert!(!ok && !out.contains("secret-stand-in"), "{name}: {out}");
        let (ok, _) = sandboxed(&data, &format!("echo cmd > '{}'", file.display())).await;
        assert!(!ok, "{name} written");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "secret-stand-in");
        // not by another name either
        let (ok, out) = sandboxed(&data, &format!("ln '{0}' '{1}/hard' && cat '{1}/hard'", file.display(), kept.display())).await;
        assert!(!ok && !out.contains("secret-stand-in"), "{name} through a hard link: {out}");
        let (ok, out) = sandboxed(&data, &format!("ln -sf '{0}' '{1}/soft' && cat '{1}/soft'", file.display(), kept.display())).await;
        assert!(!ok && !out.contains("secret-stand-in"), "{name} through a link: {out}");
    }
    // moving its folder doesn't free it: a path rule would stop matching here
    let moved = elsewhere.join("moved");
    let (ok, out) = sandboxed(&data, &format!("mv '{}' '{1}' && cat '{1}/buwiz-gpui-agent-token'", kept.display(), moved.display())).await;
    assert!(!ok && !out.contains("secret-stand-in"), "{out}");
    // the rule is the file's name, not its neighbours
    std::fs::write(elsewhere.join("my-agent-menu"), "fine").unwrap();
    let (ok, out) = sandboxed(&data, &format!("cat '{}'", elsewhere.join("my-agent-menu").display())).await;
    assert!(ok && out.contains("fine"), "{out}");
}

#[tokio::test]
async fn the_sandbox_denies_the_token_and_the_database_but_not_the_files_folder() {
    let data = temp("sandbox");
    std::fs::write(data.join("host-token"), "secret-token").unwrap();
    std::fs::write(data.join("generated").join("app.db"), "db").unwrap();
    std::fs::write(data.join("generated").join("files").join("a.txt"), "hello").unwrap();
    let token = data.join("host-token");
    let (ok, out) = sandboxed(&data, &format!("cat '{}'", token.display())).await;
    assert!(!ok && out.contains("Operation not permitted") && !out.contains("secret-token"), "{out}");
    // through /private/tmp too: the profile names the real path
    let (ok, out) = sandboxed(&data, &format!("cat '{}'", gate::real(&token).display())).await;
    assert!(!ok && !out.contains("secret-token"), "{out}");
    let (ok, out) = sandboxed(&data, &format!("cat '{}'", data.join("generated/app.db").display())).await;
    assert!(!ok && !out.contains("db\n") && out.contains("Operation not permitted"), "{out}");
    let (ok, _) = sandboxed(&data, &format!("echo x > '{}'", data.join("generated/app.db-wal").display())).await;
    assert!(!ok);
    // the root takes no new file and no rename; the files folder reads and writes
    let (ok, _) = sandboxed(&data, &format!("echo x > '{}'", data.join("planted").display())).await;
    assert!(!ok && !data.join("planted").exists());
    let (ok, _) = sandboxed(&data, &format!("mv '{}' '{}'", data.join("generated").display(), data.join("gone").display())).await;
    assert!(!ok && data.join("generated").exists());
    let (ok, out) = sandboxed(&data, &format!("cat '{0}/a.txt' && echo made > '{0}/b.txt'", data.join("generated/files").display())).await;
    assert!(ok && out.contains("hello"), "{out}");
    assert_eq!(std::fs::read_to_string(data.join("generated/files/b.txt")).unwrap().trim(), "made");
    // shell_run runs in it
    let r = crate::hub::tools::builtin::shell_run(&data, &json!({"command": format!("cat '{}'", token.display()), "cwd": "/"})).await;
    assert_eq!(r.is_error, Some(true));
    assert!(!crate::js::string_of(&r.content).contains("secret-token"));
    // a program is found, never run
    let found = sandbox::find_program(&data, "zsh").await.unwrap();
    assert!(found.iter().any(|(p, _)| p.ends_with("/zsh")), "{found:?}");
    assert!(sandbox::find_program(&data, "a;b").await.is_err());
    assert!(sandbox::find_program(&data, "no-such-program-buwiz").await.unwrap().is_empty());
}

// ---- the life of a change ----

const START: f64 = 1_793_745_245_000.0;

struct Bed {
    ctx: Arc<HubCtx>,
    hex: String,
    plain: String,
}

fn bed(label: &str) -> Bed {
    let dir = temp(label);
    let mut ctx = HubCtx::new(dir, Clock::manual());
    ctx.ext.insert(RealChatsDefaults);
    crate::hub::library::botctx::register(&mut ctx);
    ctx.clock.manual_step(Some(START), false);
    ctx.store.load().unwrap();
    let (hex, plain) = ctx
        .store
        .write(|s| {
            let t = s.fresh_thread("Hex");
            let hex = t.id.clone();
            s.bots.push(Bot::new("hex", "Hex", "blob", "#FFFFFF", Some(t.id.clone())));
            s.list.push(t);
            let p = s.fresh_thread("plain");
            let plain = p.id.clone();
            s.list.push(p);
            (hex, plain)
        })
        .unwrap();
    Bed { ctx: Arc::new(ctx), hex, plain }
}

fn run_in(ctx: &Arc<HubCtx>, thread: &str) -> Run {
    let job = ctx.jobs.create(thread, Value::Null, None, false);
    Run::new(ctx.clone(), TurnCtx { thread: thread.to_owned(), ..TurnCtx::default() }, job, Arc::new(Settings::from(|_| None)))
}

async fn call(run: &Run, name: &str, input: Value) -> Value {
    let out = exec_tool(run, &json!({"name": name, "input": input})).await;
    let text = crate::js::string_of(&out.content);
    serde_json::from_str(&text).unwrap_or(json!({"text": text, "error": out.is_error}))
}

fn notices(ctx: &HubCtx, chat: &str) -> Vec<Value> {
    ctx.store.read(|s| s.thread(chat).unwrap().turns.iter().filter_map(|u| u.notice.clone()).filter(|n| n["kind"] == "tools_change").collect())
}

fn step(ctx: &HubCtx, ms: f64) {
    let now = ctx.clock.now();
    ctx.clock.manual_step(Some(now + ms), false);
}

fn names(tools: &[Value]) -> Vec<String> {
    tools.iter().map(|t| crate::js::string_of(&t["name"])).collect()
}

#[tokio::test]
async fn each_tighten_applies_with_a_receipt_and_its_undo_puts_it_back() {
    let b = bed("tighten");
    let run = run_in(&b.ctx, &b.hex);
    // tool off
    let r = call(&run, "tools_set", json!({"key": "shell", "enabled": false})).await;
    assert_eq!((r["status"].clone(), r["key"].clone()), (json!("off"), json!("shell")));
    assert!(!tool_on(&b.ctx, "hex", "shell"));
    let n = notices(&b.ctx, &b.hex).pop().unwrap();
    assert_eq!((n["op"].clone(), n["state"].clone(), n["text"].clone(), n["undo"].clone()), (json!("tool_off"), json!("applied"), json!("Hex can't use the shell now."), r["undo"].clone()));
    assert_eq!(n["botName"], "Hex");
    assert_eq!(n["label"], "Shell on this Mac");
    assert_eq!(host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap(), json!({"ok": true, "text": "Put back."}));
    assert!(tool_on(&b.ctx, "hex", "shell"));
    assert_eq!(host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap_err().code, "stale", "once");
    // allow removed, block added, mode to only
    Change::RuleAdd { kind: "allow".into(), pattern: "git status".into(), risk: Some("ro".into()), path: None }.apply(&b.ctx, "human").unwrap();
    let r = call(&run, "shell_rule_remove", json!({"kind": "allow", "pattern": "git status"})).await;
    assert_eq!(r["status"], "removed");
    assert!(rules(&b.ctx, "allow").is_empty());
    assert_eq!(notices(&b.ctx, &b.hex).pop().unwrap()["text"], "Removed `git status` from Always allowed. It still runs while Bypass is on.");
    host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap();
    assert_eq!(rules(&b.ctx, "allow")[0].risk.as_deref(), Some("ro"), "put back as it was");
    // in mode only, the words say it is refused now
    Change::Mode(Mode::Only).apply(&b.ctx, "human").unwrap();
    let r = host_rules(&b.ctx, &json!({"op": "remove", "kind": "allow", "pattern": "git status"})).unwrap();
    assert_eq!(r["text"], "Removed `git status` from Always allowed. Bots can't run it now unless you allow it again.");
    host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap();
    Change::Mode(Mode::Bypass).apply(&b.ctx, "human").unwrap();
    let r = call(&run, "shell_rule_add", json!({"kind": "deny", "pattern": "rm -rf *"})).await;
    assert_eq!((r["status"].clone(), r["pattern"].clone()), (json!("added"), json!("rm -rf")));
    assert!(r["note"].as_str().unwrap().contains("saved as `rm -rf`"));
    assert_eq!(notices(&b.ctx, &b.hex).pop().unwrap()["text"], "Blocked on this Mac: `rm -rf`.");
    host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap();
    assert!(rules(&b.ctx, "block").is_empty());
    assert_eq!(mode(&b.ctx), Mode::Bypass, "no row is bypass");
    let r = call(&run, "shell_mode_set", json!({"mode": "only"})).await;
    assert_eq!(r["status"], "applied");
    assert_eq!(mode(&b.ctx), Mode::Only);
    assert_eq!(notices(&b.ctx, &b.hex).pop().unwrap()["text"], "Mode: Only allowed commands. Unlisted commands are refused.");
    // a later change on the same target makes the undo stale
    host_mode(&b.ctx, &json!({"mode": "bypass"})).unwrap();
    assert_eq!(host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap_err().message, UNDO_STALE);
    // and after 24 h every undo is
    let r = call(&run, "tools_set", json!({"key": "web", "enabled": false})).await;
    step(&b.ctx, DAY as f64 + 1.0);
    assert_eq!(host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap_err().code, "stale");
    assert!(!tool_on(&b.ctx, "hex", "web"));
    // the actor is the Bot
    assert_eq!(tool_rows(&b.ctx, "hex").iter().find(|r| r.0 == "web").unwrap().2, "bot:hex");
}

#[tokio::test]
async fn each_widen_proposes_and_never_applies() {
    let b = bed("widen");
    let run = run_in(&b.ctx, &b.hex);
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": false})).unwrap();
    host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "rm"})).unwrap();
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    let r = call(&run, "tools_set", json!({"key": "shell", "enabled": true})).await;
    assert_eq!((r["status"].clone(), r["note"].clone()), (json!("proposed"), json!("Not in effect until the user confirms.")));
    assert!(r["proposal"].as_str().unwrap().starts_with("p_"));
    let r = call(&run, "shell_rule_add", json!({"kind": "allow", "pattern": "claude", "program_path": "/Users/x/.local/bin/claude"})).await;
    assert_eq!((r["status"].clone(), r["risk"].clone()), (json!("proposed"), json!("any")));
    let r = call(&run, "shell_rule_add", json!({"kind": "allow", "pattern": "git status"})).await;
    assert_eq!(r["risk"], "ro");
    let r = call(&run, "shell_rule_remove", json!({"kind": "block", "pattern": "rm"})).await;
    assert_eq!(r["status"], "proposed");
    let r = call(&run, "shell_mode_set", json!({"mode": "bypass"})).await;
    assert_eq!(r["status"], "proposed");
    // nothing applied
    assert!(!tool_on(&b.ctx, "hex", "shell"));
    assert!(rules(&b.ctx, "allow").is_empty());
    assert_eq!(rules(&b.ctx, "block").len(), 1);
    assert_eq!(mode(&b.ctx), Mode::Only);
    let ns = notices(&b.ctx, &b.hex);
    assert!(ns.iter().all(|n| n["state"] == "pending"));
    let by_op = |op: &str| ns.iter().find(|n| n["op"] == op).unwrap().clone();
    assert_eq!(by_op("tool_on")["steps"], 1);
    assert_eq!(by_op("mode")["steps"], 2);
    let claude = ns.iter().find(|n| n["rule"]["pattern"] == "claude").unwrap();
    assert_eq!(claude["rule"], json!({"kind": "allow", "pattern": "claude", "risk": "any", "path": "/Users/x/.local/bin/claude"}));
    assert_eq!(claude["steps"], 2);
    assert_eq!(ns.iter().find(|n| n["rule"]["pattern"] == "git status").unwrap()["steps"], 1);
    // refusals: never, a crafted argument, another Bot, exists
    for p in ["sudo ls", "sh -c 'x'", "ls; rm x", "claude ?"] {
        let r = call(&run, "shell_rule_add", json!({"kind": "allow", "pattern": p})).await;
        assert_eq!(r["status"], "refused", "{p}");
    }
    let r = call(&run, "tools_set", json!({"key": "shell", "enabled": true, "bot": "other"})).await;
    assert_eq!(r["why"], "Hex can't change another Bot's tools from here.");
    let r = call(&run, "tools_set", json!({"key": "nope", "enabled": true})).await;
    assert_eq!(r["status"], "refused");
    let r = call(&run, "shell_rule_add", json!({"kind": "allow", "pattern": "claude *"})).await;
    assert!(r["star"].as_str().unwrap().contains("saved as `claude`"));
    assert_eq!(r["status"], "proposed");
    // a new proposal supersedes the old one for the same (bot, op, pattern)
    let ns = notices(&b.ctx, &b.hex);
    let claudes: Vec<&Value> = ns.iter().filter(|n| n["rule"]["pattern"] == "claude").collect();
    assert_eq!(claudes.iter().map(|n| n["state"].clone()).collect::<Vec<_>>(), [json!("superseded"), json!("pending")]);
    // a plain chat switches the Chats profile (PR 2)
    let plain = run_in(&b.ctx, &b.plain);
    let r = call(&plain, "tools_set", json!({"key": "shell", "enabled": false})).await;
    assert_eq!(r["status"], "off");
    assert_eq!(tool_state(&b.ctx, CHATS, "shell"), Pos::Off);
    // a pending card older than 24 h expires
    step(&b.ctx, DAY as f64 + 1.0);
    sweep_expired(&b.ctx);
    assert!(notices(&b.ctx, &b.hex).iter().all(|n| n["state"] != "pending"));
    assert!(notices(&b.ctx, &b.hex).iter().any(|n| n["state"] == "expired"));
}

#[tokio::test]
async fn answers_apply_or_decline_once_and_the_bot_is_told() {
    let b = bed("answer");
    let run = run_in(&b.ctx, &b.hex);
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    let p = call(&run, "shell_rule_add", json!({"kind": "allow", "pattern": "claude"})).await["proposal"].clone();
    let q = call(&run, "shell_mode_set", json!({"mode": "bypass"})).await["proposal"].clone();
    let chat = b.hex.clone();
    // yes applies, with at, undo and text
    let a = answer(&b.ctx, &json!({"chat": chat, "id": p, "answer": "yes"})).unwrap();
    let n = &a["notice"];
    assert_eq!((n["state"].clone(), n["text"].clone()), (json!("applied"), json!("Always allowed on this Mac: `claude`.")));
    assert!(n["undo"].as_str().unwrap().starts_with("u_") && n["at"].as_i64().is_some());
    assert_eq!(rules(&b.ctx, "allow")[0].pattern, "claude");
    assert_eq!(judge(&b.ctx, "claude -p hi"), Verdict::Run);
    // only once
    assert_eq!(answer(&b.ctx, &json!({"chat": chat, "id": p, "answer": "yes"})).unwrap_err().code, "stale");
    // no declines and changes nothing
    let a = answer(&b.ctx, &json!({"chat": chat, "id": q, "answer": "no"})).unwrap();
    assert_eq!(a["notice"]["state"], "declined");
    assert_eq!(mode(&b.ctx), Mode::Only);
    // the Bot reads one note per decision on its next call, once
    let notes = take_notes(&b.ctx, &b.hex, Some("hex"));
    let texts: Vec<String> = notes.iter().map(|m| crate::js::string_of(&m["content"])).collect();
    assert_eq!(texts.len(), 2);
    assert!(texts[0].ends_with("The user allowed `claude` on this Mac."), "{texts:?}");
    assert!(texts[1].ends_with("The user said no to Bypass on this Mac."), "{texts:?}");
    assert_eq!(notes[0]["role"], "user");
    assert!(take_notes(&b.ctx, &b.hex, Some("hex")).is_empty());
    // after Not now: no new proposal for it in this thread for 24 h, unless the user names it
    let r = call(&run, "shell_mode_set", json!({"mode": "bypass"})).await;
    assert_eq!((r["status"].clone(), r["why"].clone()), (json!("refused"), json!(SAID_NO)));
    run.state().prompt = Some("please switch to bypass".into());
    assert_eq!(call(&run, "shell_mode_set", json!({"mode": "bypass"})).await["status"], "proposed");
    run.state().prompt = Some("hi".into());
    step(&b.ctx, DAY as f64 + 1.0);
    let other = call(&run, "tools_set", json!({"key": "web", "enabled": false})).await;
    assert_eq!(other["status"], "off");
    host_set(&b.ctx, &json!({"bot": "hex", "key": "web", "enabled": false})).unwrap();
    // turning a tool on: the human note names the Bot
    let p = call(&run, "tools_set", json!({"key": "web", "enabled": true})).await["proposal"].clone();
    answer(&b.ctx, &json!({"chat": chat, "id": p, "answer": "no"})).unwrap();
    let t = take_notes(&b.ctx, &b.hex, Some("hex"));
    assert!(crate::js::string_of(&t.last().unwrap()["content"]).ends_with("The user said no to turning on the web for Hex."));
    // a bad answer, an unknown card
    assert_eq!(answer(&b.ctx, &json!({"chat": chat, "id": p, "answer": "maybe"})).unwrap_err().code, "invalid_args");
    assert_eq!(answer(&b.ctx, &json!({"chat": chat, "id": "p_nope", "answer": "yes"})).unwrap_err().code, "not_found");
}

#[tokio::test]
async fn a_click_on_a_changed_target_is_stale() {
    let b = bed("stale");
    let run = run_in(&b.ctx, &b.hex);
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": false})).unwrap();
    step(&b.ctx, 10.0);
    let p = call(&run, "tools_set", json!({"key": "shell", "enabled": true})).await["proposal"].clone();
    // the user turns it on and off again in the sheet: the target moved since
    step(&b.ctx, 10.0);
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": true})).unwrap();
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": false})).unwrap();
    let e = answer(&b.ctx, &json!({"chat": b.hex, "id": p, "answer": "yes"})).unwrap_err();
    assert_eq!((e.code.as_str(), e.message.as_str()), ("stale", "That changed since you looked. Ask Hex again."));
    assert!(!tool_on(&b.ctx, "hex", "shell"));
    assert_eq!(notices(&b.ctx, &b.hex).iter().find(|n| n["id"] == p).unwrap()["state"], "stale");
    // already in effect: stale too
    host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "rm"})).unwrap();
    step(&b.ctx, 10.0);
    let p = call(&run, "shell_rule_remove", json!({"kind": "block", "pattern": "rm"})).await["proposal"].clone();
    host_rules(&b.ctx, &json!({"op": "remove", "kind": "block", "pattern": "rm"})).unwrap();
    assert_eq!(answer(&b.ctx, &json!({"chat": b.hex, "id": p, "answer": "yes"})).unwrap_err().code, "stale");
}

#[tokio::test]
async fn the_ceiling_hides_and_refuses_an_off_tool() {
    let b = bed("ceiling");
    let run = run_in(&b.ctx, &b.hex);
    let before = names(&tool_list(&run).await);
    for t in ["shell_run", "fetch_url", "memory_add", "routine_create", "agent_send", "tools_set", "shell_rule_add"] {
        assert!(before.contains(&t.to_owned()), "{t}");
    }
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": false})).unwrap();
    host_set(&b.ctx, &json!({"bot": "hex", "key": "web", "enabled": false})).unwrap();
    host_set(&b.ctx, &json!({"bot": "hex", "key": "memory", "enabled": false})).unwrap();
    let after = names(&tool_list(&run).await);
    for t in ["shell_run", "fetch_url", "query_source", "web_search", "memory_add", "memory_list"] {
        assert!(!after.contains(&t.to_owned()), "{t}");
    }
    for t in ["routine_create", "agent_send", "tools_set", "shell_rules", "generate_app"] {
        assert!(after.contains(&t.to_owned()), "{t}");
    }
    let r = exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi"}})).await;
    assert_eq!(r, Out::err("shell is off for this Bot. Only the user can turn it on; tools_set proposes it.".into()));
    let r = exec_tool(&run, &json!({"name": "fetch_url", "input": {"url": "http://127.0.0.1:9/"}})).await;
    assert_eq!(crate::js::string_of(&r.content), "web is off for this Bot. Only the user can turn it on; tools_set proposes it.");
    // a plain chat has no ceiling
    let plain = run_in(&b.ctx, &b.plain);
    assert!(names(&tool_list(&plain).await).contains(&"shell_run".to_owned()));
}

#[tokio::test]
async fn shell_run_goes_through_the_gate_and_into_activity() {
    let b = bed("gate");
    let run = run_in(&b.ctx, &b.hex);
    let shell = |cmd: &str| {
        let block = json!({"name": "shell_run", "input": {"command": cmd, "cwd": "/"}});
        let run = &run;
        async move { exec_tool(run, &block).await }
    };
    assert_eq!(shell("echo hi").await, Out { content: json!("hi"), is_error: Some(false) });
    host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "rm"})).unwrap();
    assert_eq!(shell("true; rm -rf /tmp/nothing-here").await, Out::err(gate::BLOCKED.into()));
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    assert_eq!(shell("npm test").await, Out::err("`npm test` is not allowed on this Mac. Ask the user, or propose a rule with shell_rule_add.".into()));
    host_rules(&b.ctx, &json!({"op": "add", "kind": "allow", "pattern": "echo"})).unwrap();
    assert_eq!(shell("echo a && echo b").await.content, json!("a\nb"));
    assert_eq!(shell("echo a && pwd").await, Out::err("`pwd` is not allowed on this Mac. Ask the user, or propose a rule with shell_rule_add.".into()));
    let secret = format!("echo API_TOKEN=abc {}", "hi there ".repeat(30));
    shell(&secret).await;
    // a run that exits non-zero (the sandbox's denial among them) failed, with its exit
    host_rules(&b.ctx, &json!({"op": "add", "kind": "allow", "pattern": "cat"})).unwrap();
    let token = b.ctx.data.join("host-token");
    std::fs::write(&token, "secret").unwrap();
    assert_eq!(shell(&format!("cat '{}'", token.display())).await.is_error, Some(true));
    let a = activity(&b.ctx, None);
    let outcomes: Vec<String> = a.iter().map(|r| crate::js::string_of(&r["outcome"])).collect();
    assert_eq!(outcomes, ["failed", "ran", "refused_mode", "ran", "refused_mode", "refused_block", "ran"]);
    assert_eq!((a[0]["result"].clone(), a[1]["result"].clone(), a[2]["result"].clone()), (json!("exit 1"), json!("exit 0"), json!("not allowed")));
    let a: Vec<Value> = a.into_iter().skip(1).collect();
    assert_eq!(a[0]["bot"], "hex");
    assert_eq!(a[0]["botName"], "Hex");
    let shown = crate::js::string_of(&a[0]["command"]);
    assert!(shown.starts_with("echo API_TOKEN=«redacted» hi there") && shown.chars().count() == 120, "{shown}");
    // rows past 7 days go on the next write
    step(&b.ctx, (8 * DAY) as f64);
    shell("echo later").await;
    assert_eq!(activity(&b.ctx, None).len(), 1);
    let n: i64 = b.ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM shell_activity", [], |r| r.get(0))).unwrap();
    assert_eq!(n, 1, "pruned");
    // a row logged before `failed` existed reads as failed by its exit
    log_activity(&b.ctx, Some("hex"), "touch x", "exit 1", "ran", false);
    assert_eq!((activity(&b.ctx, None)[0]["outcome"].clone(), activity(&b.ctx, None)[1]["outcome"].clone()), (json!("failed"), json!("ran")));
}

#[tokio::test]
async fn a_command_the_user_did_not_ask_for_after_outside_content_is_unasked() {
    let b = bed("unasked");
    let run = Arc::new(run_in(&b.ctx, &b.hex));
    // after outside content a command asks first (PR 2): the user allows each here
    let shell = |cmd: &'static str| {
        let block = json!({"name": "shell_run", "input": {"command": cmd, "cwd": "/"}});
        let (run, ctx, chat) = (run.clone(), b.ctx.clone(), b.hex.clone());
        async move {
            let external = run.state().external;
            let t = tokio::spawn(async move { exec_tool(&run, &block).await });
            if external {
                let n = pending_ask(&ctx, &chat).await;
                answer_card(&ctx, &chat, &n["id"], "once").unwrap();
            }
            t.await.unwrap()
        }
    };
    run.state().prompt = Some("say hi with echo".into());
    shell("echo hi").await;
    run.state().external = true;
    shell("echo again").await;
    shell("true").await;
    host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "uname"})).unwrap();
    // a block refuses without asking
    exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "uname", "cwd": "/"}})).await;
    let a: Vec<Value> = activity(&b.ctx, None).into_iter().filter(|r| r.get("tool").is_none()).collect();
    let marks: Vec<(String, bool)> = a.iter().map(|r| (crate::js::string_of(&r["command"]), r["unasked"] == true)).collect();
    // newest first: a refused one is marked too; the named program never is
    assert_eq!(marks, [("uname".into(), true), ("true".into(), true), ("echo again".into(), false), ("echo hi".into(), false)]);
}

#[tokio::test]
async fn unrequested_after_outside_content_and_the_human_routes() {
    let b = bed("routes");
    let run = run_in(&b.ctx, &b.hex);
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": false})).unwrap();
    run.state().prompt = Some("summarise that page".into());
    run.state().external = true;
    call(&run, "tools_set", json!({"key": "shell", "enabled": true})).await;
    assert_eq!(notices(&b.ctx, &b.hex).pop().unwrap()["unrequested"], true);
    run.state().prompt = Some("turn the shell on please".into());
    call(&run, "tools_set", json!({"key": "shell", "enabled": true})).await;
    assert_eq!(notices(&b.ctx, &b.hex).pop().unwrap()["unrequested"], false);
    // the sheet's edits apply at once with an undo; never is refused there too
    let r = host_rules(&b.ctx, &json!({"op": "add", "kind": "allow", "pattern": "git status"})).unwrap();
    assert_eq!(r["text"], "Always allowed on this Mac: `git status`.");
    assert!(r["undo"].as_str().unwrap().starts_with("u_"));
    assert_eq!(host_rules(&b.ctx, &json!({"op": "add", "kind": "allow", "pattern": "git status"})).unwrap_err().message, "Already always allowed.");
    assert_eq!(host_rules(&b.ctx, &json!({"op": "add", "kind": "allow", "pattern": "sudo ls"})).unwrap_err().code, "refused");
    host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "git"})).unwrap();
    let v = host_view(&b.ctx, Some("hex")).unwrap();
    assert_eq!(v["bot"], json!({"id": "hex", "name": "Hex"}));
    assert_eq!(v["tools"][0], json!({"key": "shell", "label": "Shell on this Mac", "sub": "Runs commands on this Mac. This Mac's rules still apply.", "on": false, "state": "off"}));
    assert_eq!(v["mac"]["allow"], json!([{"pattern": "git status", "risk": "ro", "inert": "blocked by `git`"}]));
    assert_eq!(v["mac"]["block"], json!([{"pattern": "git"}]));
    assert_eq!(v["mac"]["mode"], "bypass");
    let recent = v["recent"].as_array().unwrap();
    assert_eq!(recent[0]["text"], "Blocked on this Mac: `git`.");
    assert!(recent[0]["undo"].is_string() && recent[0]["actor"] == "human");
    assert_eq!(host_view(&b.ctx, None).unwrap()["profile"], "chats", "no Bot: the Chats profile");
    assert_eq!(host_view(&b.ctx, Some("nobody")).unwrap_err().code, "not_found");
    assert_eq!(host_set(&b.ctx, &json!({"bot": "hex", "key": "nope", "enabled": false})).unwrap_err().code, "not_found");
    assert_eq!(host_mode(&b.ctx, &json!({"mode": "ask"})).unwrap_err().code, "invalid_args");
    // the sheet's own switch tells the Bot
    take_notes(&b.ctx, &b.hex, Some("hex"));
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": true})).unwrap();
    let t = take_notes(&b.ctx, &b.hex, Some("hex"));
    assert_eq!(t.len(), 1);
    assert!(crate::js::string_of(&t[0]["content"]).ends_with("Since your last message, the user changed: Hex can use the shell again."), "{t:?}");
}

fn told(ctx: &HubCtx, thread: &str, bot: Option<&str>) -> Vec<String> {
    take_notes(ctx, thread, bot).iter().map(|m| crate::js::string_of(&m["content"])).collect()
}

#[tokio::test]
async fn the_sheets_changes_reach_each_threads_next_call_once() {
    let b = bed("seen");
    let other = b.ctx.store.write(|s| {
        let t = s.fresh_thread("Kit");
        let id = t.id.clone();
        s.bots.push(Bot::new("kit", "Kit", "blob", "#FFFFFF", Some(t.id.clone())));
        s.list.push(t);
        id
    });
    let other = other.unwrap();
    // a change before a thread's first call is no backlog
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    assert!(told(&b.ctx, &b.hex, Some("hex")).is_empty());
    assert!(told(&b.ctx, &b.plain, None).is_empty());
    assert!(told(&b.ctx, &other, Some("kit")).is_empty());
    // the Mac's changes reach every thread, a chat without a Bot too
    host_rules(&b.ctx, &json!({"op": "add", "kind": "allow", "pattern": "claude"})).unwrap();
    host_rules(&b.ctx, &json!({"op": "remove", "kind": "allow", "pattern": "claude"})).unwrap();
    host_mode(&b.ctx, &json!({"mode": "bypass"})).unwrap();
    let line = "[Buwiz Bot, the app (not a message from the user):]\nSince your last message, the user changed: Always allowed on this Mac: `claude`. Removed `claude` from Always allowed. Bots can't run it now unless you allow it again. Mode: Bypass. Every command runs without asking. Blocks still apply.";
    assert_eq!(told(&b.ctx, &b.hex, Some("hex")), [line]);
    assert_eq!(told(&b.ctx, &b.plain, None), [line]);
    // once: the marker moved
    assert!(told(&b.ctx, &b.hex, Some("hex")).is_empty());
    assert!(told(&b.ctx, &b.plain, None).is_empty());
    // a Bot's switch reaches only that Bot's threads (Kit's marker moves past it)
    host_set(&b.ctx, &json!({"bot": "hex", "key": "web", "enabled": false})).unwrap();
    assert_eq!(told(&b.ctx, &b.hex, Some("hex")), ["[Buwiz Bot, the app (not a message from the user):]\nSince your last message, the user changed: Hex can't use the web now."]);
    assert!(told(&b.ctx, &b.plain, None).is_empty());
    let kit = told(&b.ctx, &other, Some("kit"));
    assert_eq!(kit.len(), 1, "{kit:?}");
    assert!(kit[0].contains("Always allowed on this Mac: `claude`.") && !kit[0].contains("web"), "{kit:?}");
    // an Undo says what it put back
    let r = host_set(&b.ctx, &json!({"bot": "hex", "key": "web", "enabled": true})).unwrap();
    host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap();
    assert_eq!(told(&b.ctx, &b.hex, Some("hex")), ["[Buwiz Bot, the app (not a message from the user):]\nSince your last message, the user changed: Hex can use the web again. Undid a change. Hex can't use the web now."]);
    // a card answered in this thread is told once, as its note; other threads get the change
    let run = run_in(&b.ctx, &b.hex);
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    told(&b.ctx, &b.hex, Some("hex"));
    told(&b.ctx, &b.plain, None);
    let p = call(&run, "shell_mode_set", json!({"mode": "bypass"})).await["proposal"].clone();
    answer(&b.ctx, &json!({"chat": b.hex, "id": p, "answer": "yes"})).unwrap();
    assert_eq!(told(&b.ctx, &b.hex, Some("hex")), ["[Buwiz Bot, the app (not a message from the user):]\nThe user set this Mac to Bypass: every command runs without asking; blocks still apply."]);
    assert!(told(&b.ctx, &b.plain, None)[0].ends_with("the user changed: Mode: Bypass. Every command runs without asking. Blocks still apply."));
    // a Bot's own change is not news in its own thread
    call(&run, "tools_set", json!({"key": "memory", "enabled": false})).await;
    assert!(told(&b.ctx, &b.hex, Some("hex")).is_empty());
    // a thread first called later starts there
    let late = b.ctx.store.write(|s| {
        let t = s.fresh_thread("late");
        let id = t.id.clone();
        s.list.push(t);
        id
    });
    assert!(told(&b.ctx, &late.unwrap(), None).is_empty());
}

#[tokio::test]
async fn a_bots_changes_reach_the_other_threads_but_never_its_own() {
    let b = bed("botmade");
    let (hex2, kit) = b
        .ctx
        .store
        .write(|s| {
            let t = s.fresh_thread("Hex again");
            let hex2 = t.id.clone();
            s.list.push(t);
            let k = s.fresh_thread("Kit");
            let kit = k.id.clone();
            s.bots.push(Bot::new("kit", "Kit", "blob", "#FFFFFF", Some(k.id.clone())));
            s.list.push(k);
            (hex2, kit)
        })
        .unwrap();
    let all = |b: &Bed| [told(&b.ctx, &b.hex, Some("hex")), told(&b.ctx, &hex2, Some("hex")), told(&b.ctx, &b.plain, None), told(&b.ctx, &kit, Some("kit"))];
    let none: [Vec<String>; 4] = Default::default();
    assert_eq!(all(&b), none, "markers start now");
    let run = run_in(&b.ctx, &b.hex);
    let head = "[Buwiz Bot, the app (not a message from the user):]\n";
    // a Bot's own switch: that Bot's other threads, not the one it was made in
    call(&run, "tools_set", json!({"key": "memory", "enabled": false})).await;
    let mine = format!("{head}Since your last message, in other chats: Hex made this change: Hex can't use its memory now.");
    assert_eq!(all(&b), [vec![], vec![mine], vec![], vec![]]);
    // a Mac-wide change: every other thread
    call(&run, "shell_rule_add", json!({"kind": "block", "pattern": "rm"})).await;
    let mac = format!("{head}Since your last message, in other chats: Hex made this change: Blocked on this Mac: `rm`.");
    assert_eq!(all(&b), [vec![], vec![mac.clone()], vec![mac.clone()], vec![mac]]);
    // the user's and a Bot's in one call: a line each
    host_rules(&b.ctx, &json!({"op": "add", "kind": "allow", "pattern": "ls"})).unwrap();
    call(&run, "shell_rule_remove", json!({"kind": "allow", "pattern": "ls"})).await;
    assert_eq!(
        told(&b.ctx, &b.plain, None),
        [
            format!("{head}Since your last message, the user changed: Always allowed on this Mac: `ls`."),
            format!("{head}Since your last message, in other chats: Hex made this change: Removed `ls` from Always allowed. It still runs while Bypass is on."),
        ]
    );
    assert_eq!(told(&b.ctx, &b.hex, Some("hex")), [format!("{head}Since your last message, the user changed: Always allowed on this Mac: `ls`.")]);
    // the thread is on the row
    let t: Option<String> = b.ctx.db.with(|c| c.query_row("SELECT thread FROM policy_change WHERE actor='bot:hex' ORDER BY id DESC LIMIT 1", [], |r| r.get(0))).unwrap();
    assert_eq!(t.as_deref(), Some(b.hex.as_str()));
}

#[tokio::test]
async fn a_card_whose_change_is_made_elsewhere_settles_in_every_chat() {
    let b = bed("moot");
    host_set(&b.ctx, &json!({"bot": "hex", "key": "web", "enabled": false})).unwrap();
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    let run = run_in(&b.ctx, &b.hex);
    let web = call(&run, "tools_set", json!({"key": "web", "enabled": true})).await["proposal"].clone();
    let bypass = call(&run, "shell_mode_set", json!({"mode": "bypass"})).await["proposal"].clone();
    let other = run_in(&b.ctx, &b.plain);
    let there = call(&other, "shell_mode_set", json!({"mode": "bypass"})).await["proposal"].clone();
    let state = |chat: &str, id: &Value| notices(&b.ctx, chat).into_iter().find(|n| &n["id"] == id).unwrap()["state"].clone();
    // the sheet turns the web on: that card goes, the others wait
    host_set(&b.ctx, &json!({"bot": "hex", "key": "web", "enabled": true})).unwrap();
    assert_eq!((state(&b.hex, &web), state(&b.hex, &bypass), state(&b.plain, &there)), (json!("stale"), json!("pending"), json!("pending")));
    // a yes on one chat's card settles the same change's card in the other
    let a = answer(&b.ctx, &json!({"chat": b.plain, "id": there, "answer": "yes"})).unwrap();
    assert_eq!(a["notice"]["state"], "applied");
    assert_eq!((state(&b.plain, &there), state(&b.hex, &bypass)), (json!("applied"), json!("stale")));
}

#[tokio::test]
async fn an_undo_marks_the_row_it_undid_instead_of_adding_one() {
    let b = bed("undone");
    told(&b.ctx, &b.plain, None);
    let r = host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "rm"})).unwrap();
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap();
    let rec = recent(&b.ctx);
    assert_eq!(rec.len(), 2, "{rec:?}");
    assert_eq!(rec[0]["text"], "Mode: Only allowed commands. Unlisted commands are refused.");
    assert!(rec[0]["undo"].is_string() && rec[0].get("undone").is_none());
    assert_eq!(rec[1], json!({"at": rec[1]["at"], "actor": "human", "text": "Blocked on this Mac: `rm`.", "undone": true}));
    // the model still reads what the Undo put back
    let t = told(&b.ctx, &b.plain, None);
    assert!(t[0].ends_with("Mode: Only allowed commands. Unlisted commands are refused. Undid a change. Stopped blocking `rm` on this Mac."), "{t:?}");
}

#[tokio::test]
async fn changes_keep_30_days_for_the_notes_but_24_h_for_recent_and_undo() {
    let b = bed("keep");
    told(&b.ctx, &b.hex, Some("hex"));
    told(&b.ctx, &b.plain, None);
    let r = host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "rm"})).unwrap();
    // a thread idle for two days still hears of it
    step(&b.ctx, (2 * DAY) as f64);
    assert_eq!(told(&b.ctx, &b.plain, None), ["[Buwiz Bot, the app (not a message from the user):]\nSince your last message, the user changed: Blocked on this Mac: `rm`."]);
    // Recent changes and Undo stay 24 h
    assert!(recent(&b.ctx).is_empty());
    assert_eq!(host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap_err().code, "stale");
    let rows = |b: &Bed| -> i64 { b.ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM policy_change", [], |r| r.get(0))).unwrap() };
    let marked = |b: &Bed, t: &str| -> i64 { b.ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM policy_seen WHERE thread=?", [t], |r| r.get(0))).unwrap() };
    host_mode(&b.ctx, &json!({"mode": "only"})).unwrap();
    assert_eq!(rows(&b), 2, "kept under 30 days");
    // past 30 days a row goes on the next write, and so does a gone thread's marker
    step(&b.ctx, (KEEP - DAY) as f64);
    b.ctx.store.write(|s| s.list.retain(|t| t.id != b.plain)).unwrap();
    assert_eq!(marked(&b, &b.plain), 1);
    host_mode(&b.ctx, &json!({"mode": "bypass"})).unwrap();
    let blocks: i64 = b.ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM policy_change WHERE target='rule:block:rm'", [], |r| r.get(0))).unwrap();
    assert_eq!((rows(&b), blocks), (2, 0), "the 31-day-old row pruned, the 29-day-old kept");
    assert_eq!((marked(&b, &b.plain), marked(&b, &b.hex)), (0, 1));
}

#[tokio::test]
async fn skill_enable_and_mcp_enable_propose_and_disable_acts_as_before() {
    let b = bed("f3");
    let mut ctx = HubCtx::new(b.ctx.data.clone(), Clock::manual());
    ctx.clock.manual_step(Some(START), false);
    crate::hub::market::skills::register(&mut ctx);
    ctx.store.load().unwrap();
    let ctx = Arc::new(ctx);
    crate::hub::market::schema(&ctx).unwrap();
    ctx.db.with(|c| c.execute("INSERT INTO skills(id,name,description,body,enabled,created_at) VALUES('tone','Tone','d','b',0,1)", [])).unwrap();
    ctx.db.with(|c| c.execute("INSERT INTO mcp_servers(id,name,command,args,env,enabled,created_at) VALUES('fx','Fx','node','[]','{}',0,1)", [])).unwrap();
    let run = run_in(&ctx, &b.hex);
    let r = call(&run, "skill_enable", json!({"id": "tone"})).await;
    assert_eq!(r["status"], "proposed");
    let on: i64 = ctx.db.with(|c| c.query_row("SELECT enabled FROM skills WHERE id='tone'", [], |r| r.get(0))).unwrap();
    assert_eq!(on, 0, "not applied");
    let n = notices(&ctx, &b.hex).pop().unwrap();
    assert_eq!((n["op"].clone(), n["key"].clone()), (json!("tool_on"), json!("skill:tone")));
    answer(&ctx, &json!({"chat": b.hex, "id": n["id"], "answer": "yes"})).unwrap();
    let on: i64 = ctx.db.with(|c| c.query_row("SELECT enabled FROM skills WHERE id='tone'", [], |r| r.get(0))).unwrap();
    assert_eq!(on, 1);
    let r = call(&run, "mcp_enable", json!({"id": "fx"})).await;
    assert_eq!(r["status"], "proposed");
    let on: i64 = ctx.db.with(|c| c.query_row("SELECT enabled FROM mcp_servers WHERE id='fx'", [], |r| r.get(0))).unwrap();
    assert_eq!(on, 0);
    // disable acts as it did
    let r = call(&run, "skill_disable", json!({"id": "tone"})).await;
    assert!(crate::js::string_of(&r["text"]).contains("disabled"), "{r}");
    // a skill off for this Bot leaves its prompt
    ctx.db.with(|c| c.execute("UPDATE skills SET enabled=1", [])).unwrap();
    let prompt = |ctx: &HubCtx| {
        let off = off_keys(ctx, "hex");
        crate::hub::tools::defs::base_system_for(ctx, false, &|id| off.contains(&format!("skill:{id}")))
    };
    assert!(prompt(&ctx).contains("### Tone"));
    host_set(&ctx, &json!({"bot": "hex", "key": "skill:tone", "enabled": false})).unwrap();
    assert!(!prompt(&ctx).contains("### Tone"));
}

// ---- asking before acting (PR 2) ----

/// A [`bed`] whose cards wait `wait` for an answer.
fn bed_waiting(label: &str, wait: std::time::Duration) -> Bed {
    let dir = temp(label);
    let mut ctx = HubCtx::new(dir, Clock::manual());
    ctx.ext.insert(ask::AskWait(wait));
    ctx.ext.insert(RealChatsDefaults);
    crate::hub::library::botctx::register(&mut ctx);
    ctx.clock.manual_step(Some(START), false);
    ctx.store.load().unwrap();
    let (hex, plain) = ctx
        .store
        .write(|s| {
            let t = s.fresh_thread("Hex");
            let hex = t.id.clone();
            s.bots.push(Bot::new("hex", "Hex", "blob", "#FFFFFF", Some(t.id.clone())));
            s.list.push(t);
            let p = s.fresh_thread("plain");
            let plain = p.id.clone();
            s.list.push(p);
            (hex, plain)
        })
        .unwrap();
    Bed { ctx: Arc::new(ctx), hex, plain }
}

fn asks(ctx: &HubCtx, chat: &str) -> Vec<Value> {
    ctx.store.read(|s| s.thread(chat).unwrap().turns.iter().filter_map(|u| u.notice.clone()).filter(|n| n["kind"] == "tool_ask").collect())
}

/// The card `chat` waits on, once it is posted.
async fn pending_ask(ctx: &HubCtx, chat: &str) -> Value {
    for _ in 0..500 {
        if let Some(n) = asks(ctx, chat).into_iter().find(|n| n["state"] == "pending") {
            return n;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("no pending card in {chat}");
}

/// Calls `name` in `thread` (a task: it may wait on a card).
fn spawn_call(ctx: &Arc<HubCtx>, thread: &str, name: &str, input: Value) -> tokio::task::JoinHandle<Out> {
    let run = run_in(ctx, thread);
    let block = json!({"name": name, "input": input});
    tokio::spawn(async move { exec_tool(&run, &block).await })
}

fn answer_card(ctx: &HubCtx, chat: &str, id: &Value, ans: &str) -> Result<Value, crate::v1::V1Error> {
    ask::host_answer(ctx, &json!({"chat": chat, "id": id, "answer": ans}))
}

fn chats_memories(ctx: &HubCtx) -> Vec<String> {
    crate::hub::library::botctx::chats_memories(ctx).iter().map(|m| crate::js::string_of(&m["text"])).collect()
}

#[test]
fn positions_default_by_profile_and_a_row_before_state_reads_as_enabled() {
    assert_eq!((default_pos(CHATS, "web"), default_pos(CHATS, "office"), default_pos(CHATS, "memory"), default_pos(CHATS, "shell"), default_pos(CHATS, "mcp:x")), (Pos::On, Pos::On, Pos::Ask, Pos::Ask, Pos::Ask));
    assert_eq!((default_pos("hex", "shell"), default_pos("hex", "agents")), (Pos::On, Pos::On));
    let b = bed("positions");
    b.ctx.db.with(|c| c.execute("INSERT INTO bot_tools(bot_id, key, enabled, actor, at) VALUES('hex','web',0,'human',1)", [])).unwrap();
    assert_eq!(tool_state(&b.ctx, "hex", "web"), Pos::Off, "a PR 1 row is off");
    // on to ask and ask to off tighten; off to ask and ask to on widen
    let ch = |from, state| Change::Tool { bot: "hex".into(), key: "web".into(), state, from };
    assert!(!ch(Pos::On, Pos::Ask).widens() && !ch(Pos::Ask, Pos::Off).widens() && !ch(Pos::On, Pos::Off).widens());
    assert!(ch(Pos::Off, Pos::Ask).widens() && ch(Pos::Ask, Pos::On).widens() && ch(Pos::Off, Pos::On).widens());
    // an undo written by PR 1 still reads
    assert_eq!(Change::from_json(&json!({"op": "tool", "bot": "hex", "key": "web", "enabled": true})), Some(ch(Pos::Off, Pos::On)));
    assert_eq!(Change::from_json(&ch(Pos::On, Pos::Ask).to_json()), Some(ch(Pos::On, Pos::Ask)));
}

#[tokio::test]
async fn tools_set_takes_a_state_and_the_host_route_sets_chats() {
    let b = bed("states");
    let run = run_in(&b.ctx, &b.hex);
    // on to ask: a tighten, at once, with its receipt and undo
    let r = call(&run, "tools_set", json!({"key": "web", "state": "ask"})).await;
    assert_eq!(r["status"], "ask");
    assert_eq!(tool_state(&b.ctx, "hex", "web"), Pos::Ask);
    let n = notices(&b.ctx, &b.hex).pop().unwrap();
    assert_eq!((n["op"].clone(), n["state"].clone(), n["text"].clone()), (json!("tool_ask"), json!("applied"), json!("Hex asks you before using the web.")));
    // ask to on: a card, not applied
    let r = call(&run, "tools_set", json!({"key": "web", "state": "on"})).await;
    assert_eq!(r["status"], "proposed");
    assert_eq!(tool_state(&b.ctx, "hex", "web"), Pos::Ask);
    let p = notices(&b.ctx, &b.hex).pop().unwrap();
    assert_eq!((p["op"].clone(), p["from"].clone()), (json!("tool_on"), json!("ask")));
    answer(&b.ctx, &json!({"chat": b.hex, "id": p["id"], "answer": "yes"})).unwrap();
    assert_eq!(tool_state(&b.ctx, "hex", "web"), Pos::On);
    // the old enabled still works
    assert_eq!(call(&run, "tools_set", json!({"key": "web", "enabled": false})).await["status"], "off");
    // tools_list shows the state and the profile
    let l = call(&run, "tools_list", json!({})).await;
    assert_eq!((l["profile"].clone(), l["bot"].clone()), (json!("bot"), json!("Hex")));
    assert_eq!(l["tools"].as_array().unwrap().iter().find(|t| t["key"] == "web").unwrap()["state"], "off");
    // a plain chat changes the Chats profile: memory ask to off at once, off to ask a card
    let plain = run_in(&b.ctx, &b.plain);
    let l = call(&plain, "tools_list", json!({})).await;
    assert_eq!(l["profile"], "chats");
    assert_eq!(l["tools"].as_array().unwrap().iter().find(|t| t["key"] == "memory").unwrap()["state"], "ask");
    assert_eq!(call(&plain, "tools_set", json!({"key": "memory", "state": "off"})).await["status"], "off");
    assert_eq!(tool_state(&b.ctx, CHATS, "memory"), Pos::Off);
    assert_eq!(call(&plain, "tools_set", json!({"key": "memory", "state": "ask"})).await["status"], "proposed");
    let p = notices(&b.ctx, &b.plain).pop().unwrap();
    assert_eq!((p["bot"].clone(), p["botName"].clone(), p["op"].clone(), p["from"].clone()), (json!(CHATS), json!("Plain chats"), json!("tool_ask"), json!("off")));
    // the person's route: {bot: "chats", key, state}, a widen at once
    let r = host_set(&b.ctx, &json!({"bot": "chats", "key": "memory", "state": "on"})).unwrap();
    assert_eq!(r["text"], "Plain chats can use memory again.");
    assert_eq!(tool_state(&b.ctx, CHATS, "memory"), Pos::On);
    host_undo(&b.ctx, &json!({"undo": r["undo"]})).unwrap();
    assert_eq!(tool_state(&b.ctx, CHATS, "memory"), Pos::Off);
    host_set(&b.ctx, &json!({"bot": "hex", "key": "shell", "enabled": false})).unwrap();
    assert_eq!(tool_state(&b.ctx, "hex", "shell"), Pos::Off);
    assert_eq!(host_set(&b.ctx, &json!({"bot": "chats", "key": "web", "state": "maybe"})).unwrap_err().code, "invalid_args");
    // the Chats view
    let v = host_view(&b.ctx, None).unwrap();
    assert_eq!((v["profile"].clone(), v["bot"].clone()), (json!("chats"), Value::Null));
    let row = |k: &str| v["tools"].as_array().unwrap().iter().find(|t| t["key"] == k).unwrap().clone();
    assert_eq!(row("web"), json!({"key": "web", "label": "Web", "sub": "Reads web pages and searches the web.", "on": true, "state": "on"}));
    assert_eq!((row("shell")["state"].clone(), row("memory")["state"].clone(), row("memory")["on"].clone()), (json!("ask"), json!("off"), json!(false)));
    assert_eq!(host_view(&b.ctx, Some("chats")).unwrap()["profile"], "chats");
    // a plain chat's change is news in the other plain chats, never in a Bot's
    let other = b.ctx.store.write(|s| {
        let t = s.fresh_thread("other");
        let id = t.id.clone();
        s.list.push(t);
        id
    }).unwrap();
    told(&b.ctx, &other, None);
    told(&b.ctx, &b.hex, Some("hex"));
    host_set(&b.ctx, &json!({"bot": "chats", "key": "web", "state": "off"})).unwrap();
    assert!(told(&b.ctx, &other, None).iter().any(|t| t.ends_with("Plain chats can't use web search now.")));
    assert_eq!(host_set(&b.ctx, &json!({"bot": "chats", "key": "web", "state": "ask"})).unwrap()["text"], "Plain chats ask you before using web search.");
    assert!(told(&b.ctx, &b.hex, Some("hex")).is_empty());
}

#[tokio::test]
async fn each_answer_runs_once_grants_the_chat_declines_or_saves_a_rule() {
    let b = bed("asks");
    // once: memory in a plain chat asks (Chats' memory is at Ask), then runs
    let t = spawn_call(&b.ctx, &b.plain, "memory_add", json!({"text": "I like tea"}));
    let n = pending_ask(&b.ctx, &b.plain).await;
    assert_eq!((n["title"].clone(), n["preview"].clone(), n["key"].clone(), n["tool"].clone(), n["bot"].clone()), (json!("Save this to memory?"), json!("I like tea"), json!("memory"), json!("memory_add"), json!("")));
    assert!(n["id"].as_str().unwrap().starts_with("a_") && n.get("external").is_none());
    let turn_text = b.ctx.store.read(|s| s.thread(&b.plain).unwrap().turns.iter().rev().find(|u| u.notice.as_ref().is_some_and(|x| x["id"] == n["id"])).and_then(|u| u.text.clone()));
    assert_eq!(turn_text.as_deref(), Some("Save this to memory?"));
    let a = answer_card(&b.ctx, &b.plain, &n["id"], "once").unwrap();
    assert_eq!((a["notice"]["state"].clone(), a["notice"]["answer"].clone()), (json!("allowed"), json!("once")));
    assert!(crate::js::string_of(&t.await.unwrap().content).starts_with("remembered"));
    assert_eq!(chats_memories(&b.ctx), ["I like tea"]);
    // only once
    assert_eq!(answer_card(&b.ctx, &b.plain, &n["id"], "once").unwrap_err().code, "stale");
    // chat: runs, and later writes in this chat run without a card
    let t = spawn_call(&b.ctx, &b.plain, "memory_add", json!({"text": "I like coffee"}));
    let n = pending_ask(&b.ctx, &b.plain).await;
    answer_card(&b.ctx, &b.plain, &n["id"], "chat").unwrap();
    t.await.unwrap();
    let before = asks(&b.ctx, &b.plain).len();
    let r = exec_tool(&run_in(&b.ctx, &b.plain), &json!({"name": "memory_add", "input": {"text": "I like cocoa"}})).await;
    assert!(crate::js::string_of(&r.content).starts_with("remembered"));
    assert_eq!(asks(&b.ctx, &b.plain).len(), before, "no card after Allow in this chat");
    assert_eq!(chats_memories(&b.ctx), ["I like tea", "I like coffee", "I like cocoa"]);
    // only in this chat: another plain chat asks again
    let other = b.ctx.store.write(|s| {
        let t = s.fresh_thread("other");
        let id = t.id.clone();
        s.list.push(t);
        id
    }).unwrap();
    let t = spawn_call(&b.ctx, &other, "memory_add", json!({"text": "I like milk"}));
    let n = pending_ask(&b.ctx, &other).await;
    // no: it doesn't run, and the model is told not to retry
    let a = answer_card(&b.ctx, &other, &n["id"], "no").unwrap();
    assert_eq!((a["notice"]["state"].clone(), a["notice"]["answer"].clone()), (json!("declined"), json!("no")));
    let r = t.await.unwrap();
    assert_eq!(r, Out::err(ask::NOT_NOW.into()));
    assert_eq!(chats_memories(&b.ctx).len(), 3);
    // rule: the shell (at Ask in Chats) saves the allow, then runs
    let t = spawn_call(&b.ctx, &b.plain, "shell_run", json!({"command": "echo hi", "cwd": "/"}));
    let n = pending_ask(&b.ctx, &b.plain).await;
    assert_eq!((n["title"].clone(), n["prefix"].clone(), n["preview"].clone()), (json!("Run `echo hi` on this Mac?"), json!("echo"), json!("echo hi")));
    assert_eq!(answer_card(&b.ctx, &b.plain, &n["id"], "chat_nope").unwrap_err().code, "invalid_args");
    answer_card(&b.ctx, &b.plain, &n["id"], "rule").unwrap();
    assert_eq!(t.await.unwrap(), Out { content: json!("hi"), is_error: Some(false) });
    assert_eq!(rules(&b.ctx, "allow").iter().map(|r| r.pattern.clone()).collect::<Vec<_>>(), ["echo"]);
    // now `echo` runs at Ask without a card; another command asks
    let before = asks(&b.ctx, &b.plain).len();
    assert_eq!(exec_tool(&run_in(&b.ctx, &b.plain), &json!({"name": "shell_run", "input": {"command": "echo again", "cwd": "/"}})).await.content, json!("again"));
    assert_eq!(asks(&b.ctx, &b.plain).len(), before);
    // a block still refuses without asking
    host_rules(&b.ctx, &json!({"op": "add", "kind": "block", "pattern": "rm"})).unwrap();
    assert_eq!(exec_tool(&run_in(&b.ctx, &b.plain), &json!({"name": "shell_run", "input": {"command": "rm -rf /tmp/x", "cwd": "/"}})).await, Out::err(gate::BLOCKED.into()));
    assert_eq!(asks(&b.ctx, &b.plain).len(), before);
    // a rule only for a shell card
    let t = spawn_call(&b.ctx, &other, "memory_add", json!({"text": "x"}));
    let n = pending_ask(&b.ctx, &other).await;
    assert_eq!(answer_card(&b.ctx, &other, &n["id"], "rule").unwrap_err().code, "invalid_args");
    answer_card(&b.ctx, &other, &n["id"], "no").unwrap();
    t.await.unwrap();
    // Activity keeps each answer
    let outcomes: Vec<String> = activity(&b.ctx, None).iter().filter(|r| r.get("tool").is_some()).map(|r| crate::js::string_of(&r["outcome"])).collect();
    assert_eq!(outcomes, ["declined", "allowed", "declined", "allowed_chat", "allowed"]);
}

#[tokio::test]
async fn an_unanswered_card_expires_and_a_restart_expires_what_was_left() {
    let b = bed_waiting("expire", std::time::Duration::from_millis(150));
    let r = exec_tool(&run_in(&b.ctx, &b.plain), &json!({"name": "memory_add", "input": {"text": "tea"}})).await;
    assert_eq!(r, Out::err(ask::NO_ANSWER.into()));
    assert!(chats_memories(&b.ctx).is_empty());
    let n = asks(&b.ctx, &b.plain).pop().unwrap();
    assert_eq!(n["state"], "expired");
    // a late click: no answer is taken
    let e = answer_card(&b.ctx, &b.plain, &n["id"], "once").unwrap_err();
    assert_eq!((e.code.as_str(), e.message.as_str()), ("stale", ask::NO_ANSWER));
    assert_eq!(activity(&b.ctx, None)[0]["outcome"], "expired");
    // a card a previous run left pending: expired on start, and a click is stale
    let left = json!({"kind": "tool_ask", "id": "a_left", "bot": "", "key": "memory", "tool": "memory_add", "title": "Save this to memory?", "state": "pending", "at": 1});
    crate::hub::tools::miniapps::post_in(&b.ctx, &b.plain, &left);
    assert_eq!(answer_card(&b.ctx, &b.plain, &json!("a_left"), "once").unwrap_err().code, "stale", "nothing waits on it");
    let left2 = json!({"kind": "tool_ask", "id": "a_left2", "bot": "", "key": "memory", "tool": "memory_add", "title": "Save this to memory?", "state": "pending", "at": 1});
    crate::hub::tools::miniapps::post_in(&b.ctx, &b.plain, &left2);
    ask::expire_pending(&b.ctx);
    assert!(asks(&b.ctx, &b.plain).iter().all(|n| n["state"] == "expired"));
}

#[tokio::test]
async fn reads_never_ask_and_off_still_refuses() {
    let b = bed_waiting("reads", std::time::Duration::from_millis(100));
    let plain = run_in(&b.ctx, &b.plain);
    let r = exec_tool(&plain, &json!({"name": "memory_list", "input": {}})).await;
    assert_eq!(crate::js::string_of(&r.content), "no memories yet");
    exec_tool(&plain, &json!({"name": "fetch_url", "input": {"url": "http://127.0.0.1:9/"}})).await;
    exec_tool(&plain, &json!({"name": "routine_list", "input": {}})).await;
    assert!(asks(&b.ctx, &b.plain).is_empty(), "no card for a read");
    for name in ["memory_list", "routine_list", "agents_list", "agent_read", "web_search", "fetch_url", "query_source", "launch_app", "view_code", "app_list", "office_open", "office_read_text"] {
        assert!(keys::is_read_name(name), "{name}");
    }
    for name in ["memory_add", "memory_remove", "routine_create", "routine_run", "agent_send", "agent_create", "app_install", "app_enable", "app_details_update", "set_version", "promote_app", "office_create", "office_edit", "office_export", "mcp_x_y", "who_knows"] {
        assert!(!keys::is_read_name(name), "{name}");
    }
    assert!(keys::declared_read(&json!({"annotations": {"readOnlyHint": true}})) && !keys::declared_read(&json!({"annotations": {"readOnlyHint": false}})) && !keys::declared_read(&json!({})));
    // Web off in Chats: refused; at Ask a search still runs (a read)
    host_set(&b.ctx, &json!({"bot": "chats", "key": "web", "state": "off"})).unwrap();
    let r = exec_tool(&plain, &json!({"name": "fetch_url", "input": {"url": "http://127.0.0.1:9/"}})).await;
    assert_eq!(crate::js::string_of(&r.content), "web is off for plain chats. Only the user can turn it on; tools_set proposes it.");
    assert!(!names(&tool_list(&plain).await).contains(&"fetch_url".to_owned()));
    host_set(&b.ctx, &json!({"bot": "chats", "key": "web", "state": "ask"})).unwrap();
    let r = exec_tool(&plain, &json!({"name": "fetch_url", "input": {"url": "http://127.0.0.1:9/"}})).await;
    assert!(!crate::js::string_of(&r.content).contains("is off"));
    assert!(asks(&b.ctx, &b.plain).is_empty());
}

#[tokio::test]
async fn outside_content_makes_every_guarded_write_ask_even_on_and_with_a_grant() {
    let b = bed("guard");
    // a Bot, its memory on: after a page, a write asks, and says why
    let run = Arc::new(run_in(&b.ctx, &b.hex));
    run.state().external = true;
    let r2 = run.clone();
    let t = tokio::spawn(async move { exec_tool(&r2, &json!({"name": "memory_add", "input": {"text": "tea"}})).await });
    let n = pending_ask(&b.ctx, &b.hex).await;
    assert_eq!((n["external"].clone(), n["bot"].clone(), n["botName"].clone()), (json!(true), json!("hex"), json!("Hex")));
    answer_card(&b.ctx, &b.hex, &n["id"], "chat").unwrap();
    assert!(crate::js::string_of(&t.await.unwrap().content).starts_with("remembered"));
    // the grant doesn't lift the guard
    let r2 = run.clone();
    let t = tokio::spawn(async move { exec_tool(&r2, &json!({"name": "memory_add", "input": {"text": "coffee"}})).await });
    let n = pending_ask(&b.ctx, &b.hex).await;
    answer_card(&b.ctx, &b.hex, &n["id"], "no").unwrap();
    assert_eq!(t.await.unwrap(), Out::err(ask::NOT_NOW.into()));
    // nor does the shell on: a command that would run asks
    let r2 = run.clone();
    let t = tokio::spawn(async move { exec_tool(&r2, &json!({"name": "shell_run", "input": {"command": "ls", "cwd": "/"}})).await });
    let n = pending_ask(&b.ctx, &b.hex).await;
    assert_eq!((n["title"].clone(), n["external"].clone()), (json!("Run `ls` on this Mac?"), json!(true)));
    answer_card(&b.ctx, &b.hex, &n["id"], "once").unwrap();
    assert_eq!(t.await.unwrap().is_error, Some(false));
    // an unguarded key (office) and a read don't ask; off still refuses without a card
    let before = asks(&b.ctx, &b.hex).len();
    exec_tool(&run, &json!({"name": "memory_list", "input": {}})).await;
    host_set(&b.ctx, &json!({"bot": "hex", "key": "memory", "state": "off"})).unwrap();
    assert!(crate::js::string_of(&exec_tool(&run, &json!({"name": "memory_add", "input": {"text": "x"}})).await.content).contains("is off for this Bot"));
    assert_eq!(asks(&b.ctx, &b.hex).len(), before);
    // without outside content, a Bot's on switch never asks
    let quiet = run_in(&b.ctx, &b.hex);
    host_set(&b.ctx, &json!({"bot": "hex", "key": "memory", "state": "on"})).unwrap();
    assert!(crate::js::string_of(&exec_tool(&quiet, &json!({"name": "memory_add", "input": {"text": "milk"}})).await.content).starts_with("remembered"));
    assert_eq!(asks(&b.ctx, &b.hex).len(), before);
}

#[tokio::test]
async fn a_routine_takes_ask_as_on_but_no_one_answers_the_guard_or_an_agents_request() {
    let b = bed("nobody");
    let turn_of = |t: TurnCtx| {
        let job = b.ctx.jobs.create(&b.plain, Value::Null, None, false);
        Run::new(b.ctx.clone(), t, job, Arc::new(Settings::from(|_| None)))
    };
    let routine = || TurnCtx { thread: b.plain.clone(), routine: Some("r1".into()), ..TurnCtx::default() };
    let request = || TurnCtx { thread: b.plain.clone(), acting_bot: Some("hex".into()), ..TurnCtx::default() };
    // a routine's run in a plain chat: memory and the shell at Ask run, no card
    let run = turn_of(routine());
    assert!(crate::js::string_of(&exec_tool(&run, &json!({"name": "memory_add", "input": {"text": "x"}})).await.content).starts_with("remembered"));
    assert_eq!(exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi", "cwd": "/"}})).await.content, json!("hi"));
    assert!(asks(&b.ctx, &b.plain).is_empty());
    // off still refuses
    host_set(&b.ctx, &json!({"bot": "chats", "key": "memory", "state": "off"})).unwrap();
    assert!(crate::js::string_of(&exec_tool(&run, &json!({"name": "memory_add", "input": {"text": "y"}})).await.content).contains("is off for plain chats"));
    // after outside content the guard asks, and no one is there: refused
    run.state().external = true;
    assert_eq!(exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi", "cwd": "/"}})).await, Out::err(ask::NOBODY.into()));
    // an agent's request keeps refusing Ask, and the guard
    host_set(&b.ctx, &json!({"bot": "hex", "key": "memory", "state": "ask"})).unwrap();
    let run = turn_of(request());
    assert_eq!(exec_tool(&run, &json!({"name": "memory_add", "input": {"text": "x"}})).await, Out::err(ask::NOBODY.into()));
    run.state().external = true;
    assert_eq!(exec_tool(&run, &json!({"name": "shell_run", "input": {"command": "echo hi", "cwd": "/"}})).await, Out::err(ask::NOBODY.into()));
    assert!(asks(&b.ctx, &b.plain).is_empty() && asks(&b.ctx, &b.hex).is_empty(), "no card");
    let outcomes: Vec<Value> = activity(&b.ctx, None).iter().filter(|r| r.get("tool").is_some()).map(|r| r["outcome"].clone()).collect();
    assert_eq!(outcomes, [json!("needs_ok"), json!("needs_ok"), json!("needs_ok")]);
}

#[test]
fn the_shell_card_offers_the_programs_rule() {
    let b = bed("prefix");
    let p = |l: &str| ask::prefix_of(&b.ctx, l);
    assert_eq!((p("git push origin main"), p("ls -la"), p("npm run build"), p("git --version")), (Some("git push".into()), Some("ls".into()), Some("npm run".into()), Some("git".into())));
    assert_eq!((p("ls && rm x"), p("sudo ls"), p("bash -c x")), (None, None, None));
}

#[tokio::test]
async fn an_mcp_tool_marked_read_only_runs_at_ask_and_the_rest_ask() {
    let Some(node) = std::env::var("PATH").ok().and_then(|p| p.split(':').map(|d| std::path::Path::new(d).join("node")).find(|p| p.exists())) else { return };
    let b0 = bed_waiting("mcp-ro", std::time::Duration::from_millis(150));
    let dir = b0.ctx.data.clone();
    drop(b0);
    // the apps fixture with one more tool, marked read-only
    let src = std::fs::read_to_string(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server/test/fixtures/mcp-apps-server.mjs")).unwrap();
    let marked = src.replacen("const TOOLS = [", "const TOOLS = [\n  { name: \"peek\", description: \"Read only\", inputSchema: { type: \"object\" }, annotations: { readOnlyHint: true } },", 1);
    assert_ne!(src, marked);
    let script = dir.join("ro-server.mjs");
    std::fs::write(&script, marked).unwrap();
    let mut ctx = HubCtx::new(dir.clone(), Clock::manual());
    ctx.ext.insert(ask::AskWait(std::time::Duration::from_millis(150)));
    ctx.ext.insert(RealChatsDefaults);
    ctx.store.load().unwrap();
    ctx.mcp = Some(crate::helper::mcp_servers::McpHost::new(&dir.join("generated").join("app.db"), 8798).unwrap());
    ctx.db
        .with(|c| c.execute("INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at) VALUES('ro','ro',?,?,'{}',1,1)", rusqlite::params![node.display().to_string(), json!([script.display().to_string()]).to_string()]))
        .unwrap();
    let ctx = Arc::new(ctx);
    let chat = ctx.store.write(|s| {
        let t = s.fresh_thread("plain");
        let id = t.id.clone();
        s.list.push(t);
        id
    }).unwrap();
    let run = run_in(&ctx, &chat);
    let r = exec_tool(&run, &json!({"name": "mcp_ro_peek", "input": {"x": 1}})).await;
    assert!(crate::js::string_of(&r.content).contains("\"tool\":\"peek\""), "{r:?}");
    assert!(asks(&ctx, &chat).is_empty(), "a read-only tool runs at Ask");
    let r = exec_tool(&run, &json!({"name": "mcp_ro_echo", "input": {"x": 1}})).await;
    assert_eq!(r, Out::err(ask::NO_ANSWER.into()), "the rest ask (and no answer came)");
    assert_eq!(asks(&ctx, &chat)[0]["key"], "mcp:ro");
}

#[tokio::test]
async fn the_chats_view_lists_plain_chats_memories_and_a_click_forgets_one() {
    let b = bed("memories");
    host_set(&b.ctx, &json!({"bot": "chats", "key": "memory", "state": "on"})).unwrap();
    let plain = run_in(&b.ctx, &b.plain);
    for t in ["I like tea", "I like coffee"] {
        exec_tool(&plain, &json!({"name": "memory_add", "input": {"text": t}})).await;
    }
    let v = host_view(&b.ctx, None).unwrap();
    let ms = v["memories"].as_array().unwrap().clone();
    assert_eq!(ms.iter().map(|m| m["text"].clone()).collect::<Vec<_>>(), [json!("I like tea"), json!("I like coffee")]);
    assert!(host_view(&b.ctx, Some("hex")).unwrap().get("memories").is_none(), "a Bot's view has none");
    assert_eq!(host_memory_remove(&b.ctx, &json!({"id": ms[0]["id"]})).unwrap()["ok"], true);
    assert_eq!(chats_memories(&b.ctx), ["I like coffee"]);
    assert_eq!(host_memory_remove(&b.ctx, &json!({"id": ms[0]["id"]})).unwrap_err().code, "not_found");
    assert_eq!(host_memory_remove(&b.ctx, &json!({})).unwrap_err().code, "invalid_args");
    // the next call's system prompt has only what is left
    let section = crate::hub::library::botctx::chats_section(&b.ctx);
    assert!(section.contains("- I like coffee") && !section.contains("tea"), "{section}");
}
