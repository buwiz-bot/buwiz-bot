//! The bot controls' model tools (Rust's own, not in `defs.json`): `tools_list`,
//! `tools_set`, `shell_rules`, `shell_find_program`, `shell_rule_add`, `shell_rule_remove`
//! and `shell_mode_set`; the ceiling `tool_list` and `exec_tool` apply; the gate in front of
//! `shell_run`; and `skill_enable` / `mcp_enable`, which propose (F3).
//!
//! A Bot acts only on its own switches (the acting Bot, `botctx::bot_of`). A tighten
//! applies at once and posts its receipt card; a widen only ever posts a pending card
//! (`policy::propose`) that a person answers. No input applies a widen.

use std::collections::HashSet;

use serde_json::{json, Value};

use super::{builtin, Out, Run};
use crate::hub::library::botctx;
use crate::hub::policy::gate::{self, Mode, Risk, Verdict};
use crate::hub::policy::{self, keys, sandbox, Ask, Change};
use crate::js;

pub const NAMES: [&str; 7] = ["tools_list", "tools_set", "shell_rules", "shell_find_program", "shell_rule_add", "shell_rule_remove", "shell_mode_set"];

/// The tools' schemas, as the model sees them.
pub fn defs() -> Vec<Value> {
    let kind = json!({"type": "string", "enum": ["allow", "block"], "description": "allow: a command that may run in mode only; block: one that never runs"});
    vec![
        json!({
            "name": "tools_list",
            "description": "List this Bot's tool switches (shell, web, apps, office, routines, memory, agents, skill:<id>, mcp:<server>, v1:<plugin>), on or off and who changed each, and this Mac's shell mode with its rule counts.",
            "input_schema": {"type": "object", "properties": {}},
        }),
        json!({
            "name": "tools_set",
            "description": "Switch one of this Bot's tools. Off applies at once (the user can undo it). On never applies: a card in this chat asks the user, and it stays off until they confirm. You can change only this Bot's own tools.",
            "input_schema": {"type": "object", "properties": {
                "key": {"type": "string", "description": "a key tools_list shows, e.g. shell, web, skill:<id>, mcp:<server>"},
                "enabled": {"type": "boolean"},
            }, "required": ["key", "enabled"]},
        }),
        json!({
            "name": "shell_rules",
            "description": "This Mac's shell mode (only: commands run only with an allow rule; bypass: every command runs) and its rules: always allowed (with each rule's risk and whether a block overrides it) and blocked. A query lists the rules that contain it; it never removes anything.",
            "input_schema": {"type": "object", "properties": {"kind": kind.clone(), "query": {"type": "string"}}},
        }),
        json!({
            "name": "shell_find_program",
            "description": "Find a command name on this Mac's PATH (e.g. claude for Claude Code), with where each hit resolves to. It never runs the program. Use it before proposing an allow rule for a program.",
            "input_schema": {"type": "object", "properties": {"name": {"type": "string", "description": "a bare command name"}}, "required": ["name"]},
        }),
        json!({
            "name": "shell_rule_add",
            "description": "Add a shell rule for every Bot on this Mac. A rule matches the start of a command, word by word (`git status` covers `git status --short`); `*` is not a wildcard. A block applies at once. An allow never applies: a card asks the user, and it is not in effect until they confirm. sudo, shells, compound commands and anything naming the broker's files can never be allowed.",
            "input_schema": {"type": "object", "properties": {
                "kind": kind.clone(),
                "pattern": {"type": "string", "description": "one plain command's first words, e.g. git status"},
                "program_path": {"type": "string", "description": "where shell_find_program found the program, shown to the user"},
            }, "required": ["kind", "pattern"]},
        }),
        json!({
            "name": "shell_rule_remove",
            "description": "Remove a shell rule by its exact text (as shell_rules shows it). Removing an allow applies at once. Removing a block never applies: a card asks the user.",
            "input_schema": {"type": "object", "properties": {"kind": kind, "pattern": {"type": "string"}}, "required": ["kind", "pattern"]},
        }),
        json!({
            "name": "shell_mode_set",
            "description": "Set this Mac's shell mode. only (stricter: commands without an allow rule are refused) applies at once. bypass (every command runs; blocks still apply) never applies: a card asks the user.",
            "input_schema": {"type": "object", "properties": {"mode": {"type": "string", "enum": ["only", "bypass"]}}, "required": ["mode"]},
        }),
    ]
}

fn out(v: Value) -> Out {
    Out::ok(js::stringify(&v))
}

fn refused(why: impl Into<String>) -> Out {
    Out::err(js::stringify(&json!({"status": "refused", "why": why.into()})))
}

/// The acting Bot: (id, name); `None` in a chat without one.
fn acting(run: &Run) -> Option<(String, String)> {
    run.ctx.store.read(|s| botctx::bot_of(s, &run.turn).map(|b| (b.id.clone(), b.name.clone())))
}

fn actor(bot: &Option<(String, String)>) -> String {
    format!("bot:{}", bot.as_ref().map(|b| b.0.as_str()).unwrap_or(""))
}

/// The user's last message in this turn (or the thread's last).
fn last_user(run: &Run) -> String {
    if let Some(p) = run.state().prompt.clone() {
        return p;
    }
    run.ctx.store.read(|s| s.thread(&run.turn.thread).and_then(|t| t.turns.iter().rev().find(|u| u.role == "user").and_then(|u| u.text.clone()))).unwrap_or_default()
}

/// The keys off for this turn's Bot; `None` with no acting Bot (no ceiling).
pub fn off(run: &Run) -> Option<HashSet<String>> {
    let (bot, _) = acting(run)?;
    Some(policy::off_keys(&run.ctx, &bot))
}

/// `tools` without those whose key is off for the acting Bot.
pub fn ceiling(run: &Run, tools: Vec<Value>) -> Vec<Value> {
    let Some(off) = off(run).filter(|o| !o.is_empty()) else { return tools };
    tools
        .into_iter()
        .filter(|t| {
            let name = t.get("name").map(js::string_of).unwrap_or_default();
            keys::key_of(&run.ctx, &name, run.turn.surface.as_ref()).is_none_or(|k| !off.contains(&k))
        })
        .collect()
}

/// The deferred apps whose key is on for the acting Bot.
pub fn ceiling_deferred(run: &Run, deferred: Vec<Value>) -> Vec<Value> {
    let Some(off) = off(run).filter(|o| !o.is_empty()) else { return deferred };
    deferred.into_iter().filter(|d| keys::key_of_app(&run.ctx, &js::string_of(&d["id"])).is_none_or(|k| !off.contains(&k))).collect()
}

/// The refusal for a call to a tool switched off for the acting Bot.
pub fn refusal(run: &Run, name: &str, input: &Value) -> Option<Out> {
    let off = off(run).filter(|o| !o.is_empty())?;
    let key = if name == "app_tools_load" { keys::key_of_app(&run.ctx, &input.get("app").map(js::string_of).unwrap_or_default()) } else { keys::key_of(&run.ctx, name, run.turn.surface.as_ref()) }?;
    off.contains(&key).then(|| Out::err(format!("{key} is off for this Bot. Only the user can turn it on; tools_set proposes it.")))
}

/// Whether a tool that reads outside content (a page, a search, an app) ran this turn.
pub fn note_external(run: &Run, name: &str) {
    let ext = matches!(name, "fetch_url" | "web_search") || keys::key_of(&run.ctx, name, run.turn.surface.as_ref()).is_some_and(|k| k.starts_with("mcp:") || k.starts_with("v1:"));
    if ext {
        run.state().external = true;
    }
}

/// Proposes `change` from this turn; → the model's answer.
fn propose(run: &Run, bot: &Option<(String, String)>, change: &Change, named: impl Fn(&str) -> bool, extra: Value) -> Out {
    let text = last_user(run);
    let named = named(&text);
    let unrequested = run.state().external && !named;
    let chat = run.thread_id();
    let ask = Ask { chat: &chat, bot: bot.as_ref().map(|b| b.0.as_str()).unwrap_or(""), unrequested, named };
    match policy::propose(&run.ctx, &ask, change) {
        Ok(n) => {
            let mut v = json!({"status": "proposed", "proposal": n["id"], "note": policy::NOT_IN_EFFECT});
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            out(v)
        }
        Err(e) => refused(e),
    }
}

/// Applies the tighten `change` and posts its receipt card; → `{status, …, undo}`.
fn tighten(run: &Run, bot: &Option<(String, String)>, change: &Change, status: Value) -> Out {
    match policy::commit(&run.ctx, change, &actor(bot)) {
        Ok((undo, text)) => {
            policy::post_applied(&run.ctx, &run.thread_id(), bot.as_ref().map(|b| b.0.as_str()).unwrap_or(""), change, &undo, &text);
            let mut v = status;
            v["undo"] = json!(undo);
            out(v)
        }
        Err(e) => Out::err(format!("tool error: {e}")),
    }
}

fn words_name(text: &str, pattern: &str) -> bool {
    let t = text.to_lowercase();
    let p = pattern.to_lowercase();
    t.contains(&p) || p.split_whitespace().next().is_some_and(|w| t.contains(gate_basename(w)))
}

fn gate_basename(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

/// One of [`NAMES`].
pub async fn tool(run: &Run, name: &str, input: &Value) -> Result<Out, String> {
    let ctx = &run.ctx;
    let s = |k: &str| input.get(k).filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    let bot = acting(run);
    match name {
        "tools_list" => {
            let mac = policy::mac_json(ctx);
            let counts = json!({"mode": mac["mode"], "allow": mac["allow"].as_array().map_or(0, Vec::len), "block": mac["block"].as_array().map_or(0, Vec::len)});
            let Some((id, bname)) = bot else { return Ok(out(json!({"bot": Value::Null, "tools": [], "note": "This chat has no Bot, so it has no tool switches.", "mac": counts}))) };
            let rows = policy::tool_rows(ctx, &id);
            let tools: Vec<Value> = keys::all_keys(ctx, &id)
                .into_iter()
                .map(|k| {
                    let row = rows.iter().find(|r| r.0 == k);
                    let mut v = json!({"key": k, "on": row.is_none_or(|r| r.1)});
                    if let Some(r) = row {
                        v["changedBy"] = json!(if r.2 == "human" { "the user".to_owned() } else if r.2 == format!("bot:{id}") { "you".to_owned() } else { r.2.clone() });
                    }
                    v
                })
                .collect();
            Ok(out(json!({"bot": bname, "tools": tools, "mac": counts})))
        }
        "tools_set" => {
            let Some(b) = bot.clone() else { return Ok(refused("This chat has no Bot. Open a Bot's chat to change its tools.")) };
            let key = s("key");
            let Some(enabled) = input.get("enabled").and_then(Value::as_bool) else { return Ok(refused("send enabled: true or false")) };
            if input.get("bot").is_some_and(|x| js::string_of(x) != b.0) {
                return Ok(refused(format!("{} can't change another Bot's tools from here.", b.1)));
            }
            if !keys::known(ctx, &key) {
                return Ok(refused(format!("no tool switch {} (tools_list shows the keys)", js::slice_utf16(&key, 80))));
            }
            let change = Change::Tool { bot: b.0.clone(), key: key.clone(), enabled };
            if change.noop(ctx) {
                return Ok(out(json!({"status": if enabled { "on" } else { "off" }, "key": key, "note": "Already so."})));
            }
            if enabled {
                return Ok(propose(run, &bot, &change, |t| keys::names_key(ctx, &key, t), json!({})));
            }
            Ok(tighten(run, &bot, &change, json!({"status": "off", "key": key})))
        }
        "shell_rules" => {
            let mac = policy::mac_json(ctx);
            let kind = s("kind");
            let q = s("query").to_lowercase();
            let pick = |list: &Value| -> Vec<Value> { list.as_array().cloned().unwrap_or_default().into_iter().filter(|r| q.is_empty() || js::string_of(&r["pattern"]).to_lowercase().contains(&q)).collect() };
            let mut v = json!({"mode": mac["mode"]});
            if kind != "block" {
                v["allow"] = json!(pick(&mac["allow"]));
            }
            if kind != "allow" {
                v["block"] = json!(pick(&mac["block"]));
            }
            Ok(out(v))
        }
        "shell_find_program" => {
            let n = s("name");
            match sandbox::find_program(&ctx.data, &n).await {
                Ok(found) if found.is_empty() => Ok(out(json!({"found": [], "note": format!("{n} isn't on this Mac's PATH. Install it, or give a full path.")}))),
                Ok(found) => Ok(out(json!({"found": found.into_iter().map(|(p, t)| json!({"path": p, "target": t})).collect::<Vec<_>>()}))),
                Err(e) => Ok(Out::err(e)),
            }
        }
        "shell_rule_add" => {
            let kind = match s("kind").as_str() {
                "allow" => "allow",
                "block" | "deny" => "block",
                _ => return Ok(refused("kind is allow or block")),
            };
            let (pattern, starred) = gate::normalize(&s("pattern"));
            let star = starred.then(|| gate::star_note(&pattern));
            if pattern.is_empty() {
                return Ok(refused("Not saved: the rule is empty."));
            }
            if policy::rules(ctx, kind).iter().any(|r| r.pattern == pattern) {
                return Ok(out(json!({"status": "exists", "note": if kind == "allow" { "Already always allowed." } else { "Already blocked." }})));
            }
            if kind == "block" {
                let change = Change::RuleAdd { kind: "block".into(), pattern: pattern.clone(), risk: None, path: None };
                let mut st = json!({"status": "added", "pattern": pattern});
                if let Some(n) = star {
                    st["note"] = json!(n);
                }
                return Ok(tighten(run, &bot, &change, st));
            }
            let risk = match gate::risk(&pattern, &ctx.data) {
                Risk::Never(why) => return Ok(refused(why)),
                r => r,
            };
            let path = input.get("program_path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned);
            if let Some(p) = &path {
                let program = pattern.split_whitespace().next().unwrap_or("");
                if !p.starts_with('/') || gate_basename(p) != gate_basename(program) {
                    return Ok(refused(format!("program_path must be where `{program}` is found (shell_find_program shows it).")));
                }
            }
            let change = Change::RuleAdd { kind: "allow".into(), pattern: pattern.clone(), risk: Some(risk.as_str().into()), path };
            let mut extra = json!({"risk": risk.as_str(), "pattern": pattern});
            if risk == Risk::Any {
                extra["note"] = json!("Not in effect until the user confirms, twice.");
            }
            if let Some(n) = star {
                extra["star"] = json!(n);
            }
            if let Some(i) = policy::inert_of(ctx, &pattern) {
                extra["inert"] = json!(format!("Not in effect even if allowed: {i}, and a block wins."));
            }
            let p2 = pattern.clone();
            Ok(propose(run, &bot, &change, move |t| words_name(t, &p2), extra))
        }
        "shell_rule_remove" => {
            let kind = match s("kind").as_str() {
                "allow" => "allow",
                "block" | "deny" => "block",
                _ => return Ok(refused("kind is allow or block")),
            };
            let pattern = s("pattern");
            if !policy::rules(ctx, kind).iter().any(|r| r.pattern == pattern) {
                return Ok(out(json!({"status": "missing", "note": "No rule has exactly that text; shell_rules lists them."})));
            }
            let change = Change::RuleRemove { kind: kind.into(), pattern: pattern.clone() };
            if kind == "block" {
                let p2 = pattern.clone();
                return Ok(propose(run, &bot, &change, move |t| words_name(t, &p2), json!({})));
            }
            Ok(tighten(run, &bot, &change, json!({"status": "removed", "pattern": pattern})))
        }
        _ => {
            let m = match s("mode").as_str() {
                "only" => Mode::Only,
                "bypass" => Mode::Bypass,
                other => return Ok(refused(format!("mode is only or bypass, not {}", js::slice_utf16(other, 40)))),
            };
            let change = Change::Mode(m);
            if change.noop(ctx) {
                return Ok(out(json!({"status": "unchanged", "mode": m.as_str()})));
            }
            if m == Mode::Bypass {
                return Ok(propose(run, &bot, &change, |t| t.to_lowercase().contains("bypass"), json!({})));
            }
            Ok(tighten(run, &bot, &change, json!({"status": "applied", "mode": "only"})))
        }
    }
}

/// `shell_run` through the gate: refused by a block (any mode) or, in mode `only`, for a
/// part without an allow; else run in the sandbox. Every call goes into Activity.
pub async fn shell_run(run: &Run, input: &Value) -> Out {
    let ctx = &run.ctx;
    let command = input.get("command").filter(|c| js::truthy(c)).map(js::string_of).unwrap_or_default();
    let bot = acting(run).map(|b| b.0);
    let line = command.trim_matches(js::is_space);
    if line.is_empty() {
        return builtin::shell_run(&ctx.data, input).await;
    }
    let (outcome, refusal) = match policy::judge(ctx, line) {
        Verdict::Run => ("ran", None),
        Verdict::Blocked { rule } => ("refused_block", Some((gate::BLOCKED.to_owned(), format!("blocked by `{rule}`")))),
        Verdict::NotAllowed { part } => {
            let shown = gate::clip(&gate::mask_and_clip(&part), 80);
            ("refused_mode", Some((format!("`{shown}` is not allowed on this Mac. Ask the user, or propose a rule with shell_rule_add."), "not allowed".to_owned())))
        }
        Verdict::TooLong => ("refused_mode", Some((format!("This command is {} bytes; the gate reads at most {} and refuses a longer one. Split it into shorter commands.", line.len(), gate::MAX_JUDGED_BYTES), "too long".to_owned()))),
    };
    if let Some((why, result)) = refusal {
        policy::log_activity(ctx, bot.as_deref(), line, &result, outcome);
        return Out::err(why);
    }
    let r = builtin::shell_run(&ctx.data, input).await;
    let text = js::string_of(&r.content);
    let result = match text.rfind("(exit ") {
        Some(i) if r.is_error == Some(true) => text[i..].trim_matches(['(', ')']).to_owned(),
        _ if r.is_error == Some(true) => "failed".to_owned(),
        _ => "exit 0".to_owned(),
    };
    policy::log_activity(ctx, bot.as_deref(), line, &result, outcome);
    r
}

/// `skill_enable` (F3): a skill switched off for every Bot, or for this Bot, becomes a
/// `tool_on` proposal; one already on says so. `None` leaves the call to the skill tools.
pub fn skill_enable(run: &Run, input: &Value) -> Option<Out> {
    let ctx = &run.ctx;
    let id = crate::hub::market::skills::find_id(ctx, input)?;
    let bot = acting(run);
    let key = format!("skill:{id}");
    let change = Change::Tool { bot: bot.as_ref().map(|b| b.0.clone()).unwrap_or_default(), key: key.clone(), enabled: true };
    if change.noop(ctx) {
        return Some(Out::ok(format!("skill \"{id}\" enabled — instructions apply")));
    }
    Some(propose(run, &bot, &change, |t| keys::names_key(ctx, &key, t), json!({"key": key})))
}

/// `mcp_enable` (F3) for a server the user installed: a `tool_on` proposal. `None` leaves
/// the call to `mcp::admin` (a missing server, one the Bot added, one on already).
pub fn mcp_enable(run: &Run, input: &Value) -> Option<Out> {
    let ctx = &run.ctx;
    let id = super::mcp::server_id(ctx, input)?;
    let r = super::mcp::rows(ctx, "WHERE id=?", &[&id]).into_iter().next()?;
    if js::truthy(&r["agent_pending"]) {
        return None;
    }
    let bot = acting(run);
    let key = format!("mcp:{id}");
    let change = Change::Tool { bot: bot.as_ref().map(|b| b.0.clone()).unwrap_or_default(), key: key.clone(), enabled: true };
    if change.noop(ctx) {
        return None;
    }
    Some(propose(run, &bot, &change, |t| keys::names_key(ctx, &key, t), json!({"key": key})))
}
