//! The agent's routine tools (routines.mjs `toolRoutine`): routine_create,
//! routine_list, routine_run, routine_update, routine_delete, over v0
//! routines and the plugin routine instances of the chats the agent speaks
//! for. Their schemas are the tool loop's (`ROUTINE_TOOLS`); a routine's
//! own turn gets none, and is refused here too.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::text::tpl;
use super::v0::V0;
use super::Routines;
use crate::hub::ctx::HubCtx;
use crate::hub::types::{ToolResult, TurnCtx};
use crate::js;

pub const NAMES: [&str; 5] = ["routine_create", "routine_list", "routine_run", "routine_update", "routine_delete"];

fn ok(content: String) -> ToolResult {
    ToolResult { content: json!(content), is_error: false }
}

fn err(content: String) -> ToolResult {
    ToolResult { content: json!(content), is_error: true }
}

/// `every(s)`: 1d, 2h, 5m or 90s.
fn every(s: f64) -> String {
    let n = js::number;
    if s % 86400.0 == 0.0 {
        format!("{}d", n(s / 86400.0))
    } else if s % 3600.0 == 0.0 {
        format!("{}h", n(s / 3600.0))
    } else if s % 60.0 == 0.0 {
        format!("{}m", n(s / 60.0))
    } else {
        format!("{}s", n(s))
    }
}

/// A v0 routine's line (`line`).
fn line(r: &Value, thread: Option<&str>) -> String {
    let f = |k: &str| r.get(k).map(js::string_of).unwrap_or_default();
    let chat = if Some(f("thread").as_str()) == thread { "this one".to_owned() } else { format!("\"{}\"", f("threadTitle")) };
    let check = match r.get("check").filter(|c| js::truthy(c)) {
        Some(c) => format!(" · check {}/{} ({} seen)", tpl(c.get("server")), tpl(c.get("tool")), f("seen_count")),
        None => " · no check".to_owned(),
    };
    let last = r.get("last_run").and_then(Value::as_f64).filter(|v| *v != 0.0).map(|v| format!(" · last run {}", js::iso(v))).unwrap_or_default();
    let error = r.get("last_error").filter(|e| js::truthy(e)).map(|e| format!(" · error: {}", js::string_of(e))).unwrap_or_default();
    let on = if r.get("enabled") == Some(&Value::Bool(true)) { "on" } else { "off" };
    format!("{} — {} · every {} · {on} · chat {chat}{check}{last}{error}", f("id"), f("name"), every(r["every_s"].as_f64().unwrap_or(f64::NAN)))
}

/// A plugin routine instance's line (`v1Line`).
fn v1_line(x: &Value) -> String {
    let f = |k: &str| x.get(k).map(js::string_of).unwrap_or_default();
    let t = |k: &str| x.get(k).is_some_and(js::truthy);
    let summary = if t("summary") { format!(" ({})", f("summary")) } else { String::new() };
    let when =
        if f("trigger") == "webhook" { format!("on its webhook{}", if t("hook") { "" } else { " (no URL yet)" }) } else { format!("every {}", every(x["every_s"].as_f64().unwrap_or(f64::NAN))) };
    let state = if t("enabled") {
        "on"
    } else if t("pending") {
        "waiting for the user's Enable"
    } else {
        "paused"
    };
    let writes: Vec<String> = x.get("writes").and_then(Value::as_array).into_iter().flatten().map(js::string_of).collect();
    let may = if writes.is_empty() { String::new() } else { format!(" · may change data: {}", writes.join(", ")) };
    let running = if t("running") { " · running now" } else { "" };
    let cant = if t("available") || !t("enabled") { String::new() } else { format!(" · can't run: {}", f("reason")) };
    let last = match x.get("last_run").and_then(Value::as_f64).filter(|v| *v != 0.0) {
        Some(at) => {
            let how = if t("last_error") {
                format!(" failed: {}", f("last_error"))
            } else if x.get("last_items").is_some_and(|v| !v.is_null()) {
                format!(" ({} new)", f("last_items"))
            } else {
                String::new()
            };
            format!(" · last run {}{how}", js::iso(at))
        }
        None => " · not run yet".to_owned(),
    };
    let home = if t("home") { format!(" · thread {}", f("home")) } else { String::new() };
    format!("{} — {}/{} {}{summary} · plugin {} · {when} · {state}{may}{running}{cant}{last}{home}", f("instance"), f("plugin"), f("id"), f("name"), f("title"))
}

fn is_v1_ref(id: Option<&Value>) -> bool {
    let s = id.filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    s.contains('/') || s.starts_with("ri_")
}

/// The chats whose plugin routines the agent sees: in a routine's thread
/// the chat it reports to, the bot's own chat, and this one (`chatsOf`).
fn chats_of(rs: &Routines, thread: Option<&super::host::Thread>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let reports = thread.and_then(|t| t.routine.as_ref()?.get("thread")?.as_str().map(str::to_owned));
    let bot = thread.and_then(|t| rs.host.bot_chat(&t.id));
    for c in [reports, bot, thread.map(|t| t.id.clone())].into_iter().flatten() {
        if !c.is_empty() && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// `toolRoutine(thread, name, input, job)`.
pub async fn call(rs: Arc<Routines>, ctx: Arc<HubCtx>, turn: TurnCtx, name: &str, input: Value) -> ToolResult {
    if turn.routine.is_some() {
        return err(format!("{name} isn't available in a routine's run"));
    }
    let thread = rs.host.thread(&turn.thread);
    let here = thread.as_ref().map(|t| t.id.clone());
    let chats = chats_of(&rs, thread.as_ref());
    let id = input.get("id");
    let id_s = id.filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    let patch = || {
        let mut p = input.clone();
        if let Some(o) = p.as_object_mut() {
            o.remove("id");
        }
        p
    };
    match name {
        "routine_run" if is_v1_ref(id) => rs.v1.agent_run(&ctx, &id_s, &chats, Duration::from_secs(60)).await,
        "routine_run" => {
            let key = id.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
            let Some(r) = V0::row_of(&ctx.db, &key) else { return err(format!("no routine {}", tpl(id))) };
            if rs.v0.is_running(&r.id) {
                return err(format!("{} is already running", r.name));
            }
            // one that posts in this chat runs once this reply is done, so
            // its turn doesn't land in the middle of it
            if Some(&r.thread) == here.as_ref() {
                if let Some(job) = turn.job {
                    let (rs, ctx, rid) = (rs.clone(), ctx.clone(), r.id.clone());
                    tokio::spawn(async move {
                        let until = std::time::Instant::now() + Duration::from_secs(600);
                        while !rs.host.job_done(job) && std::time::Instant::now() < until {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                        let _ = rs.v0.run(&ctx, Some(&json!(rid))).await;
                    });
                    return ok(format!("{} runs as soon as this reply is done; its turn posts in this chat", r.name));
                }
            }
            let (v0, c, rid) = (rs.v0.clone(), ctx.clone(), r.id.clone());
            let done = tokio::spawn(async move { v0.run(&c, Some(&json!(rid))).await });
            if tokio::time::timeout(Duration::from_secs(60), done).await.is_err() {
                return ok(format!("{} is running; its turn posts in its chat", r.name));
            }
            let after = V0::row_of(&ctx.db, &r.id).map(|x| rs.v0.json(&x)).unwrap_or(Value::Null);
            match after.get("last_error").filter(|e| js::truthy(e)) {
                Some(e) => err(format!("{} ran and failed: {}", r.name, js::string_of(e))),
                None => ok(format!("{} ran", r.name)),
            }
        }
        "routine_update" if is_v1_ref(id) => rs.v1.agent_update(&ctx, &id_s, &patch(), &chats),
        "routine_delete" if is_v1_ref(id) => rs.v1.agent_delete(&ctx, &id_s, &chats),
        "routine_create" if input.get("plugin").is_some() => rs.v1.agent_create(&ctx, &input, &chats),
        "routine_create" => {
            if input.get("every_s").is_none() {
                return err("a routine needs every_s (or plugin and routine for a plugin routine)".to_owned());
            }
            let mut with = input.clone();
            with["thread"] = here.clone().map(Value::from).unwrap_or(Value::Null);
            let enabled = with.get("enabled").map_or(true, js::truthy);
            let template = with.get("template").and_then(Value::as_str).map(str::to_owned);
            match rs.v0.create(&ctx, &with, enabled, template.as_deref()).await {
                Ok(r) => {
                    let next = r.get("next_run").and_then(Value::as_f64).filter(|v| *v != 0.0);
                    let first = match next {
                        Some(n) if n <= ctx.clock.now() => "now".to_owned(),
                        n => format!("at {}", js::iso(n.unwrap_or(f64::NAN))),
                    };
                    ok(format!("created {}; first tick {first}", line(&r, here.as_deref())))
                }
                Err(e) => err(e.error),
            }
        }
        "routine_list" => {
            let mut out: Vec<String> = rs.v0.list(&ctx.db).iter().map(|r| line(r, here.as_deref())).collect();
            let v1s: Vec<String> = rs.v1.of_chats(&ctx.db, &chats).iter().map(v1_line).collect();
            if !v1s.is_empty() {
                out.push("Plugin routines of this chat:".to_owned());
                out.extend(v1s);
            }
            ok(if out.is_empty() { "no routines yet".to_owned() } else { out.join("\n") })
        }
        "routine_update" => {
            let mut j = patch();
            j["id"] = id.cloned().unwrap_or(Value::Null);
            match rs.v0.update(&ctx, &j) {
                Ok(r) => ok(format!("updated {}", line(&r, here.as_deref()))),
                Err(e) => err(e.error),
            }
        }
        "routine_delete" => match rs.v0.delete(&ctx, id) {
            Ok(()) => ok(format!("deleted {}", tpl(id))),
            Err(e) => err(e.error),
        },
        _ => err(format!("unknown routine tool {name}")),
    }
}
