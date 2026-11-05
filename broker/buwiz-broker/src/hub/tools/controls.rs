//! The bot controls' model tools (Rust's own, not in `defs.json`): `tools_list`,
//! `tools_set`, `shell_rules`, `shell_find_program`, `shell_rule_add`, `shell_rule_remove`
//! and `shell_mode_set`; the ceiling `tool_list` and `exec_tool` apply; the gate in front of
//! `shell_run`; and `skill_enable` / `mcp_enable` / `app_enable`, which propose (F3).
//!
//! A Bot acts only on its own switches (the acting Bot, `botctx::bot_of`); a plain chat on
//! the Chats profile every plain chat shares (`policy::CHATS`). A tighten applies at once
//! and posts its receipt card; a widen only ever posts a pending card (`policy::propose`)
//! that a person answers. No input applies a widen.
//!
//! [`ask_first`] (and [`shell_run`] for the shell) parks a call that changes something or
//! reaches someone while its switch is at Ask, or after the turn read outside content, until
//! the user answers its `tool_ask` card (`policy::ask`).

use std::collections::HashSet;

use serde_json::{json, Value};

use super::{builtin, ui, Out, Run};
use crate::hub::library::botctx;
use crate::hub::policy::ask::{self as asking, Asking};
use crate::hub::policy::gate::{self, Mode, Risk, Verdict};
use crate::hub::policy::{self, keys, sandbox, Ask, Change, Pos, CHATS};
use crate::js;

pub const NAMES: [&str; 7] = ["tools_list", "tools_set", "shell_rules", "shell_find_program", "shell_rule_add", "shell_rule_remove", "shell_mode_set"];

/// The tools' schemas, as the model sees them.
pub fn defs() -> Vec<Value> {
    let kind = json!({"type": "string", "enum": ["allow", "block"], "description": "allow: a command that may run in mode only; block: one that never runs"});
    vec![
        json!({
            "name": "tools_list",
            "description": "List the tool switches of this Bot (in a plain chat: of Plain chats, the switches every plain chat shares): shell, web, apps, office, routines, memory, agents, screens, screen_actions, skill:<id>, mcp:<server>, v1:<plugin>. Each is on, ask (the user OKs each call that changes something or reaches someone; reads still run) or off, with who changed it; and this Mac's shell mode with its rule counts.",
            "input_schema": {"type": "object", "properties": {}},
        }),
        json!({
            "name": "tools_set",
            "description": "Set one of this Bot's tools (in a plain chat: Plain chats') to on, ask or off. Toward off (on to ask, on or ask to off) applies at once (the user can undo it). Toward on never applies: a card in this chat asks the user, and it stays as it is until they confirm. You can change only this Bot's own tools.",
            "input_schema": {"type": "object", "properties": {
                "key": {"type": "string", "description": "a key tools_list shows, e.g. shell, web, skill:<id>, mcp:<server>"},
                "state": {"type": "string", "enum": ["on", "ask", "off"]},
            }, "required": ["key", "state"]},
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
    // a plain chat's model acts as "the assistant", whichever profile it changes
    format!("bot:{}", bot.as_ref().map(|b| b.0.as_str()).filter(|b| *b != CHATS).unwrap_or(""))
}

/// The user's last message in this turn (or the thread's last).
fn last_user(run: &Run) -> String {
    if let Some(p) = run.state().prompt.clone() {
        return p;
    }
    run.ctx.store.read(|s| s.thread(&run.turn.thread).and_then(|t| t.turns.iter().rev().find(|u| u.role == "user").and_then(|u| u.text.clone()))).unwrap_or_default()
}

/// The profile this turn's switches are: the acting Bot's, else the Chats profile.
pub fn profile(run: &Run) -> String {
    acting(run).map(|b| b.0).unwrap_or_else(|| CHATS.to_owned())
}

/// The keys off for this turn's profile; `None` when none is.
pub fn off(run: &Run) -> Option<HashSet<String>> {
    Some(policy::off_keys(&run.ctx, &profile(run))).filter(|o| !o.is_empty())
}

/// `tools` without those whose key is off for the turn's profile.
pub fn ceiling(run: &Run, tools: Vec<Value>) -> Vec<Value> {
    let Some(off) = off(run) else { return tools };
    tools
        .into_iter()
        .filter(|t| {
            let name = t.get("name").map(js::string_of).unwrap_or_default();
            keys::key_of(&run.ctx, &name, run.turn.surface.as_ref()).is_none_or(|k| !off.contains(&k)) && !needs_screens(&name, &off)
        })
        .collect()
}

/// Whether `name` is `ui_act` and the look it depends on is off: its answer is a look.
fn needs_screens(name: &str, off: &HashSet<String>) -> bool {
    name == ui::ACT && off.contains("screens")
}

/// The deferred apps whose key is on for the acting Bot.
pub fn ceiling_deferred(run: &Run, deferred: Vec<Value>) -> Vec<Value> {
    let Some(off) = off(run) else { return deferred };
    deferred.into_iter().filter(|d| keys::key_of_app(&run.ctx, &js::string_of(&d["id"])).is_none_or(|k| !off.contains(&k))).collect()
}

fn key_for(run: &Run, name: &str, input: &Value) -> Option<String> {
    if name == "app_tools_load" {
        keys::key_of_app(&run.ctx, &input.get("app").map(js::string_of).unwrap_or_default())
    } else {
        keys::key_of(&run.ctx, name, run.turn.surface.as_ref())
    }
}

/// The refusal for a call to a tool switched off for the turn's profile.
pub fn refusal(run: &Run, name: &str, input: &Value) -> Option<Out> {
    let off = off(run)?;
    let key = key_for(run, name, input)?;
    let whose = if acting(run).is_some() { "this Bot" } else { "plain chats" };
    let key = if needs_screens(name, &off) { "screens".to_owned() } else { key };
    off.contains(&key).then(|| Out::err(format!("{key} is off for {whose}. Only the user can turn it on; tools_set proposes it.")))
}

/// `pos` as this turn reads it: a routine's run takes Ask as On (the user allowed the
/// routine when it was made); an agent's request does not.
fn effective(run: &Run, pos: Pos) -> Pos {
    let routine = run.turn.routine.is_some() || run.turn.surface.is_some();
    if pos == Pos::Ask && routine && run.turn.acting_bot.is_none() {
        Pos::On
    } else {
        pos
    }
}

/// Whether the call `name` only reads: by its name, else as its MCP server or v1 plugin
/// declares it (`readOnlyHint`). Everything else is a write.
async fn reads(run: &Run, name: &str, key: &str) -> bool {
    if keys::is_read_name(name) {
        return true;
    }
    if key.starts_with("mcp:") {
        return super::mcp::tool_def(&run.ctx, name).await.is_some_and(|d| keys::declared_read(&d));
    }
    if let Some(plugin) = key.strip_prefix("v1:") {
        return super::v1::tool_def(&run.ctx, plugin, name).is_some_and(|d| keys::declared_read(&d));
    }
    false
}

/// Before any call but the shell's (its own gate asks, [`shell_run`]): a write on a switch
/// at Ask, without a grant for this chat, or any write in a guarded key after outside
/// content, waits for the user's OK. → the model's answer when it doesn't run.
pub async fn ask_first(run: &Run, name: &str, input: &Value) -> Option<Out> {
    if name == "shell_run" || name == "app_tools_load" {
        return None;
    }
    let key = key_for(run, name, input)?;
    let press = key == ui::ACT_KEY;
    let outside = if press { run.state().beyond_screens } else { run.state().external };
    ask_with(run, name, input, key, outside, press).await
}

/// [`ask_first`] for a write of plugin tool `name` that a press on a screen led to: the look the
/// press needed doesn't make it ask, or every press would. A press stands alone (O2): no grant
/// in this chat covers it, and its card leaves none.
pub async fn ask_press(run: &Run, name: &str) -> Option<Out> {
    let input = json!({});
    let key = key_for(run, name, &input)?;
    let outside = run.state().beyond_screens;
    ask_with(run, name, &input, key, outside, true).await
}

async fn ask_with(run: &Run, name: &str, input: &Value, key: String, outside: bool, press: bool) -> Option<Out> {
    let ctx = &run.ctx;
    let pos = effective(run, policy::tool_state(ctx, &profile(run), &key));
    if pos == Pos::Off || reads(run, name, &key).await {
        return None;
    }
    let external = outside && keys::guarded(&key);
    if !external && (pos == Pos::On || (!press && asking::granted(ctx, &run.thread_id(), &key))) {
        return None;
    }
    let (title, lines, preview) = describe(run, name, &key, input);
    let a = Asking { bot: acting(run).map(|b| b.0).unwrap_or_default(), key, tool: name.to_owned(), title, lines, preview, external, prefix: None, once: press };
    asking::ask(run, a).await.err().map(Out::err)
}

fn clip(s: &str, n: usize) -> String {
    js::slice_utf16(s, n).to_owned()
}

/// A tool card's title (what will happen), its lines and its payload.
fn describe(run: &Run, name: &str, key: &str, input: &Value) -> (String, Vec<String>, String) {
    let ctx = &run.ctx;
    let s = |k: &str| input.get(k).filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    let quoted = |v: String| if v.is_empty() { String::new() } else { format!(" \"{}\"", clip(&v, 80)) };
    let payload = || {
        let p = js::stringify_pretty(input);
        if p == "{}" {
            String::new()
        } else {
            clip(&p, 2000)
        }
    };
    let routine_name = |id: &str| -> String {
        let n = ctx.db.with(|c| c.query_row("SELECT name FROM routines WHERE id=?", [id], |r| r.get::<_, Option<String>>(0))).ok().flatten();
        n.filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_owned())
    };
    match name {
        "agent_send" => {
            let to: Vec<String> = match input.get("to") {
                Some(Value::Array(a)) => a.iter().map(js::string_of).collect(),
                Some(Value::String(t)) if t == "*" => vec!["all your agents".to_owned()],
                Some(v) if !v.is_null() => vec![js::string_of(v)],
                _ => vec!["the agents in this conversation".to_owned()],
            };
            let to: Vec<String> = to.into_iter().map(|t| ctx.store.read(|st| st.bot(&t).map(|b| b.name.clone())).unwrap_or(t)).collect();
            let mut lines = Vec::new();
            if !s("subject").is_empty() {
                lines.push(format!("Subject: {}", clip(&s("subject"), 120)));
            }
            (format!("Send a message to {}?", to.join(", ")), lines, clip(&s("text"), 4000))
        }
        "agent_create" => (format!("Make the agent{}?", quoted(s("name"))), vec![s("purpose")].into_iter().filter(|l| !l.is_empty()).collect(), clip(&s("first_message"), 4000)),
        "agent_remove" => (format!("End the helper{}?", quoted(s("id"))), Vec::new(), String::new()),
        "memory_add" => ("Save this to memory?".into(), Vec::new(), clip(&s("text"), 1000)),
        "memory_remove" => ("Remove this from memory?".into(), Vec::new(), s("id")),
        "routine_create" => {
            let n = [s("name"), s("label"), s("routine")].into_iter().find(|n| !n.is_empty()).unwrap_or_default();
            let mut lines = Vec::new();
            if let Some(every) = input.get("every_s").and_then(Value::as_f64).filter(|e| *e > 0.0) {
                lines.push(if every >= 3600.0 { format!("Every {} h", js::number((every / 3600.0 * 10.0).round() / 10.0)) } else { format!("Every {} min", js::number((every / 60.0).round())) });
            }
            let preview = if s("prompt").is_empty() { payload() } else { clip(&s("prompt"), 4000) };
            (if n.is_empty() { "Make a routine?".into() } else { format!("Make the routine{}?", quoted(n)) }, lines, preview)
        }
        "routine_update" => (format!("Change the routine{}?", quoted(routine_name(&s("id")))), Vec::new(), payload()),
        "routine_delete" => (format!("Delete the routine{}?", quoted(routine_name(&s("id")))), Vec::new(), String::new()),
        "routine_run" => (format!("Run the routine{} now?", quoted(routine_name(&s("id")))), Vec::new(), String::new()),
        "app_install" => ("Install a mini app?".into(), Vec::new(), payload()),
        "app_enable" => (format!("Switch on the mini app{}?", quoted(s("id"))), Vec::new(), String::new()),
        "app_disable" => (format!("Switch off the mini app{}?", quoted(s("id"))), Vec::new(), String::new()),
        "app_uninstall" => (format!("Uninstall the mini app{}?", quoted(s("id"))), Vec::new(), String::new()),
        "office_create" => {
            let file = s("path").rsplit('/').next().unwrap_or("").to_owned();
            (format!("Make the document{}?", quoted(file)), Vec::new(), String::new())
        }
        ui::ACT => {
            let what = if s("action") == "click" { "Press" } else { "Type in" };
            (format!("{what}{} on the screen?", quoted(s("node"))), Vec::new(), s("text"))
        }
        _ if name.starts_with("office_") => ("Change a document?".into(), vec![format!("Tool: {name}")], payload()),
        _ if key == "apps" => (format!("Change the card{}?", quoted(s("name"))), vec![format!("Tool: {name}")], payload()),
        _ => {
            let label = keys::label_of(ctx, key).map(|l| l.0).unwrap_or_else(|| key.to_owned());
            (format!("Let it use {label}?"), vec![format!("Tool: {name}")], payload())
        }
    }
}

/// Whether a tool that reads outside content (a page, a search, an app) ran this turn.
pub fn note_external(run: &Run, name: &str) {
    let screen = matches!(name, ui::NAME | ui::ACT);
    let ext = matches!(name, "fetch_url" | "web_search" | "query_source") || keys::key_of(&run.ctx, name, run.turn.surface.as_ref()).is_some_and(|k| k.starts_with("mcp:") || k.starts_with("v1:"));
    if ext || screen {
        run.state().external = true;
    }
    if ext {
        run.state().beyond_screens = true;
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
    match policy::commit(&run.ctx, change, &actor(bot), Some(&run.thread_id())) {
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
            let (id, bname) = bot.clone().unwrap_or_else(|| (CHATS.to_owned(), String::new()));
            let me = actor(&bot);
            let rows = policy::tool_rows(ctx, &id);
            let tools: Vec<Value> = keys::all_keys(ctx, &id)
                .into_iter()
                .map(|k| {
                    let row = rows.iter().find(|r| r.0 == k);
                    let mut v = json!({"key": k, "state": policy::tool_state(ctx, &id, &k).as_str()});
                    if let Some(r) = row {
                        v["changedBy"] = json!(if r.2 == "human" { "the user".to_owned() } else if r.2 == me { "you".to_owned() } else { r.2.clone() });
                    }
                    v
                })
                .collect();
            if bot.is_none() {
                return Ok(out(json!({"profile": "chats", "note": "This chat has no Bot: these are Plain chats' switches, shared by every plain chat.", "tools": tools, "mac": counts})));
            }
            Ok(out(json!({"profile": "bot", "bot": bname, "tools": tools, "mac": counts})))
        }
        "tools_set" => {
            let key = s("key");
            let state = match (input.get("state").and_then(Value::as_str), input.get("enabled").and_then(Value::as_bool)) {
                (Some(st), _) => Pos::parse(st),
                (None, Some(e)) => Some(if e { Pos::On } else { Pos::Off }),
                _ => None,
            };
            let Some(state) = state else { return Ok(refused("send state: on, ask or off")) };
            let owner = profile(run);
            if let Some(b) = &bot {
                if input.get("bot").is_some_and(|x| js::string_of(x) != b.0) {
                    return Ok(refused(format!("{} can't change another Bot's tools from here.", b.1)));
                }
            } else if input.get("bot").is_some_and(|x| !x.is_null() && js::string_of(x) != "chats") {
                return Ok(refused("A plain chat can change only the Plain chats switches."));
            }
            if !keys::known(ctx, &key) {
                return Ok(refused(format!("no tool switch {} (tools_list shows the keys)", js::slice_utf16(&key, 80))));
            }
            let change = Change::tool(ctx, &owner, &key, state);
            if change.noop(ctx) {
                return Ok(out(json!({"status": state.as_str(), "key": key, "note": "Already so."})));
            }
            // a card names the profile it changes: the Bot, or Chats
            let whose = Some(bot.clone().unwrap_or_else(|| (CHATS.to_owned(), policy::CHATS_NAME.to_owned())));
            if change.widens() {
                return Ok(propose(run, &whose, &change, |t| keys::names_key(ctx, &key, t), json!({})));
            }
            Ok(tighten(run, &whose, &change, json!({"status": state.as_str(), "key": key})))
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

/// `shell_run` through the gate: refused by a block (any mode) or, with the switch on and
/// in mode `only`, for a part without an allow; else run in the sandbox. With the switch at
/// Ask a line every part of which has an allow runs, and any other asks (unless this chat
/// was allowed the shell); after outside content every line that would run asks. Every
/// call goes into Activity.
pub async fn shell_run(run: &Run, input: &Value) -> Out {
    let ctx = &run.ctx;
    let command = input.get("command").filter(|c| js::truthy(c)).map(js::string_of).unwrap_or_default();
    let bot = acting(run).map(|b| b.0);
    let line = command.trim_matches(js::is_space);
    if line.is_empty() {
        return builtin::shell_run(&ctx.data, input).await;
    }
    // as a proposal's `unrequested`: after outside content, and the user did not name it
    let external = run.state().external;
    let unasked = external && !words_name(&last_user(run), line);
    let pos = effective(run, policy::tool_state(ctx, &profile(run), "shell"));
    let mut verdict = if pos == Pos::Ask { policy::judge_in(ctx, Mode::Only, line) } else { policy::judge(ctx, line) };
    // at Ask, a line without an allow asks (or runs, where this chat was allowed the shell)
    let mut ask = external && verdict == Verdict::Run;
    if pos == Pos::Ask && matches!(verdict, Verdict::NotAllowed { .. }) {
        ask = external || !asking::granted(ctx, &run.thread_id(), "shell");
        verdict = Verdict::Run;
    }
    if ask {
        let shown = gate::mask_and_clip(line);
        let mut lines = Vec::new();
        if let Some(cwd) = input.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
            lines.push(format!("In {cwd}"));
        }
        let a = Asking {
            bot: bot.clone().unwrap_or_default(),
            key: "shell".into(),
            tool: "shell_run".into(),
            title: format!("Run `{}` on this Mac?", gate::clip(&shown, 80)),
            lines,
            preview: gate::clip(line, 2000),
            external,
            prefix: asking::prefix_of(ctx, line),
            once: false,
        };
        if let Err(e) = asking::ask(run, a).await {
            return Out::err(e);
        }
    }
    let (outcome, refusal) = match verdict {
        Verdict::Run => ("ran", None),
        Verdict::Blocked { rule } => ("refused_block", Some((gate::BLOCKED.to_owned(), format!("blocked by `{rule}`")))),
        Verdict::NotAllowed { part } => {
            let shown = gate::clip(&gate::mask_and_clip(&part), 80);
            ("refused_mode", Some((format!("`{shown}` is not allowed on this Mac. Ask the user, or propose a rule with shell_rule_add."), "not allowed".to_owned())))
        }
        Verdict::TooLong => ("refused_mode", Some((format!("This command is {} bytes; the gate reads at most {} and refuses a longer one. Split it into shorter commands.", line.len(), gate::MAX_JUDGED_BYTES), "too long".to_owned()))),
    };
    if let Some((why, result)) = refusal {
        policy::log_activity(ctx, bot.as_deref(), line, &result, outcome, unasked);
        return Out::err(why);
    }
    let r = builtin::shell_run(&ctx.data, input).await;
    let text = js::string_of(&r.content);
    let result = match text.rfind("(exit ") {
        Some(i) if r.is_error == Some(true) => text[i..].trim_matches(['(', ')']).to_owned(),
        _ if r.is_error == Some(true) => "failed".to_owned(),
        _ => "exit 0".to_owned(),
    };
    // it ran but did not succeed (a sandbox denial is an exit 1 like any other)
    let outcome = if r.is_error == Some(true) { "failed" } else { outcome };
    policy::log_activity(ctx, bot.as_deref(), line, &result, outcome, unasked);
    r
}

/// `skill_enable` (F3): a skill switched off for every Bot, or for this Bot, becomes a
/// `tool_on` proposal; one already on says so. `None` leaves the call to the skill tools.
pub fn skill_enable(run: &Run, input: &Value) -> Option<Out> {
    let ctx = &run.ctx;
    let id = crate::hub::market::skills::find_id(ctx, input)?;
    let bot = acting(run);
    let key = format!("skill:{id}");
    let change = Change::tool(ctx, bot.as_ref().map(|b| b.0.as_str()).unwrap_or(""), &key, Pos::On);
    if change.noop(ctx) {
        return Some(Out::ok(format!("skill \"{id}\" enabled — instructions apply")));
    }
    Some(propose(run, &bot, &change, |t| keys::names_key(ctx, &key, t), json!({"key": key})))
}

/// `app_enable` (F3): a mini app off for every Bot, or for this Bot, becomes a `tool_on`
/// proposal for `v1:<id>`. `None` leaves the call to the mini app tools (one not installed,
/// one on already, which they start again).
pub fn app_enable(run: &Run, input: &Value) -> Option<Out> {
    let ctx = &run.ctx;
    let id = input.get("id").filter(|v| !v.is_null()).map(js::string_of)?;
    super::v1::view(ctx).row(&id)?;
    let bot = acting(run);
    let key = format!("v1:{id}");
    let change = Change::tool(ctx, bot.as_ref().map(|b| b.0.as_str()).unwrap_or(""), &key, Pos::On);
    if change.noop(ctx) {
        return None;
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
    let change = Change::tool(ctx, bot.as_ref().map(|b| b.0.as_str()).unwrap_or(""), &key, Pos::On);
    if change.noop(ctx) {
        return None;
    }
    Some(propose(run, &bot, &change, |t| keys::names_key(ctx, &key, t), json!({"key": key})))
}
