//! A `/chat` or `/retry` job to its end (agent.mjs `generate(job, prompt)`)
//! and a turn nobody typed (`backgroundTurn`): the slash commands, the
//! tool loop, the chat reply, and the legacy one-shot generator (a card
//! from `askModel`, checked by nsh, fixed up to three times, else the
//! canned module).

use std::time::Duration;

use serde_json::{json, Value};

use super::slash_mock::{mock, slash, upload};
use super::turn::agent_turn;
use super::user_content::{artifact_blocks, user_content};
use crate::hub::model::{self, Ask};
use crate::hub::pipeline::{self, versions, Dirs};
use crate::hub::store::json::js_slice;
use crate::hub::store::Turn;
use crate::hub::tools::artifact::mark_revising;
use crate::hub::tools::{activity, Run};
use crate::hub::traits::TurnDone;
use crate::js;

/// The thread's turns as `chatReply` reads them (`role`, `text`, `name`).
fn history(run: &Run) -> Vec<Value> {
    run.ctx.store.read(|s| {
        s.thread(&run.turn.thread)
            .map(|t| {
                t.turns
                    .iter()
                    .map(|u| {
                        let mut m = serde_json::Map::new();
                        m.insert("role".into(), json!(u.role));
                        if let Some(text) = &u.text {
                            m.insert("text".into(), json!(text));
                        }
                        match (&u.name, u.null_name) {
                            (Some(n), _) => {
                                m.insert("name".into(), json!(n));
                            }
                            (None, true) => {
                                m.insert("name".into(), Value::Null);
                            }
                            _ => {}
                        }
                        Value::Object(m)
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// `chatReply(prompt, thread, job)`.
async fn chat_reply(run: &Run, prompt: &str, extra: &Value, ctx_blocks: &[Value]) -> String {
    let content = user_content(&run.ctx, prompt, extra, ctx_blocks).await;
    let Some(api) = run.ctx.model.clone() else { return "_(model error — no model client)_".into() };
    model::chat_reply(&run.settings, api.as_ref(), &run.turn, &history(run), content).await
}

/// The reply's turn with the links the tools left (`linkFields(job)`).
fn reply_turn(run: &Run, text: &str) -> Option<Turn> {
    let (links, suggest) = run.link_fields();
    if text.is_empty() && links.is_none() && suggest.is_none() {
        return None;
    }
    let mut t = Turn::ai(text, &run.label());
    t.links = links;
    t.suggest = suggest;
    Some(t)
}

/// Whether the tool loop runs (`!MOCK && PROVIDER === "anthropic"`).
fn tools_on(run: &Run) -> bool {
    !run.settings.mock && run.settings.anthropic()
}

/// `backgroundTurn(job, thread, shown, prompt, extra)`: `shown` in the
/// chat, `prompt` to the model; the reply with what the tools made, even
/// after a crash. → the shown turn's place.
pub async fn background_turn(run: &Run, shown: &str, prompt: &str, extra: &Value) -> Result<TurnDone, String> {
    let ctx = &run.ctx;
    let done = ctx.store.append_turn(&run.turn.thread, Turn::user(shown).with_extra(extra))?.ok_or_else(|| format!("unknown thread {}", run.turn.thread))?;
    ctx.store.write_index();
    let text = if tools_on(run) { agent_turn(run, prompt, &json!({}), &[]).await } else { Ok(chat_reply(run, prompt, &json!({}), &[]).await) };
    let t = text.as_deref().unwrap_or("");
    if let Some(turn) = reply_turn(run, t) {
        ctx.store.append_turn(&run.turn.thread, turn)?;
    }
    ctx.store.write_index();
    run.set_job("done", "replied");
    ctx.jobs.with(run.job, |j| {
        j.done = true;
        j.result = Some("reply".into());
    });
    text.map(|_| done)
}

/// An agent's request (agents-wire.mjs `runRequest`): `agentTurn` on the
/// run's stand-in thread when the tools are on, else `chatReply`.
pub async fn request_reply(run: &Run, prompt: &str) -> Result<String, String> {
    if tools_on(run) {
        agent_turn(run, prompt, &json!({}), &[]).await
    } else {
        Ok(chat_reply(run, prompt, &json!({}), &[]).await)
    }
}

/// Pushes the user's turn (its title on an empty thread).
fn push_user(run: &Run, prompt: &str, extra: &Value) -> Result<(), String> {
    run.ctx.store.write(|s| {
        if let Some(t) = s.thread_mut(&run.turn.thread) {
            if t.turns.is_empty() {
                t.title = js_slice(prompt, 42);
            }
            t.turns.push(Turn::user(prompt).with_extra(extra));
        }
    })
}

/// `/^module:\/\/(R\d+)(?:@v(\d+))?$/` of the first attachment that is one.
fn attached_card(extra: &Value) -> Option<(String, Option<i64>)> {
    extra.get("attachments").and_then(Value::as_array).into_iter().flatten().find_map(|a| {
        let uri = a.get("uri").filter(|u| js::truthy(u)).map(js::string_of).unwrap_or_default();
        let rest = uri.strip_prefix("module://R")?;
        let (d, v) = match rest.split_once("@v") {
            Some((d, v)) => (d, Some(v)),
            None => (rest, None),
        };
        let num = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        (num(d) && v.is_none_or(num)).then(|| (format!("R{d}"), v.and_then(|v| v.parse::<i64>().ok()).filter(|v| *v != 0)))
    })
}

/// `generate(job, prompt)`. `retry` is `/retry`'s card name.
pub async fn generate(run: &Run, prompt: &str, extra: &Value, retry: Option<&str>) -> Result<(), String> {
    let ctx = &run.ctx;
    let p = prompt.trim_matches(js::is_space);
    if retry.is_none() && slash(p, "mock") {
        return mock(run, prompt, extra).await;
    }
    if retry.is_none() && slash(p, "upload") || retry.is_none() && slash(p, "uploadx") {
        return upload(run, prompt, extra).await;
    }
    // a reply on an artifact: its files are read into the model's context
    let ctx_blocks = if retry.is_none() { artifact_blocks(ctx, extra.get("reply").and_then(|r| r.get("files"))).await } else { Vec::new() };
    // a module:// attachment = iterate on that card: a new version of it
    let revise = attached_card(extra);
    if revise.is_some() {
        run.state().revise = revise.clone();
    }
    if retry.is_none() && tools_on(run) {
        push_user(run, prompt, extra)?;
        let text = match agent_turn(run, prompt, extra, &ctx_blocks).await {
            Ok(t) => t,
            Err(e) => {
                // what the tools made rides its own turn, even after a crash
                if let Some(turn) = reply_turn(run, "") {
                    ctx.store.append_turn(&run.turn.thread, turn)?;
                    ctx.store.write_index();
                }
                return Err(e);
            }
        };
        if let Some(turn) = reply_turn(run, &text) {
            ctx.store.append_turn(&run.turn.thread, turn)?;
        }
        ctx.store.write_index();
        let cards = run.state().cards;
        println!("✓ turn via {} ({}) — {cards} card(s)", run.label(), js::slice_utf16(prompt, 60));
        ctx.jobs.finish(&ctx.store, run.job, &if cards > 0 { format!("{cards} card(s)") } else { "replied".into() }, Some(if cards > 0 { "cards" } else { "reply" }));
        return Ok(());
    }
    if retry.is_none() {
        // a turn carrying files is a question about them; a bare module
        // attach still wants generation (a revise)
        let non_module = extra.get("attachments").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| !x.get("uri").and_then(Value::as_str).is_some_and(|u| u.starts_with("module://"))));
        let has_files = !ctx_blocks.is_empty() || non_module;
        let intent = if revise.is_some() {
            "ui"
        } else if !has_files {
            match &ctx.model {
                Some(api) => model::classify_intent(&run.settings, api.as_ref(), &run.turn, prompt).await,
                None => "ui",
            }
        } else {
            "chat"
        };
        if intent == "chat" {
            let phrase = activity::phrase_for(ctx, run.job, "thinking", &activity::THINKING);
            run.set_job("thinking", &phrase);
            ctx.store.write(|s| {
                if let Some(t) = s.thread_mut(&run.turn.thread) {
                    if t.turns.is_empty() {
                        t.title = js_slice(prompt, 42);
                    }
                }
            })?;
            let text = chat_reply(run, prompt, extra, &ctx_blocks).await;
            ctx.store.write(|s| {
                if let Some(t) = s.thread_mut(&run.turn.thread) {
                    t.turns.push(Turn::user(prompt).with_extra(extra));
                    t.turns.push(Turn::ai(&text, &run.label()));
                }
            })?;
            ctx.store.write_index();
            println!("✓ reply ({})", js::slice_utf16(prompt, 60));
            ctx.jobs.finish(&ctx.store, run.job, "replied", Some("reply"));
            return Ok(());
        }
    }
    legacy(run, prompt, extra, retry, revise).await
}

/// `tryCode(code)`: checked in staging (nothing lands in generated/ until
/// it compiles); `err` is nsh's verdict.
async fn try_code(run: &Run, dirs: &Dirs, nsh: &std::sync::Arc<dyn crate::hub::traits::Nsh>, name: &str, code: &str, last_code: &mut Option<String>, err: &mut Option<String>) -> Result<bool, String> {
    let code = pipeline::imports::fix_imports(code);
    *last_code = Some(code.clone());
    std::fs::write(dirs.stage.join(format!("{name}.tsx")), &code).map_err(|e| e.to_string())?;
    run.set_job("checking", "Checking the code");
    *err = pipeline::nsh::check_candidate(nsh.as_ref(), &dirs.stage, name).await.err();
    Ok(err.is_none())
}

/// The legacy generator: a card from the model, checked, fixed, else mock.
async fn legacy(run: &Run, prompt: &str, extra: &Value, retry: Option<&str>, revise: Option<(String, Option<i64>)>) -> Result<(), String> {
    let ctx = &run.ctx;
    let label = run.label();
    let (name, base_v, ai_id, pushed) = match (retry, &revise) {
        (Some(_), _) | (None, Some(_)) => {
            let name = retry.map(str::to_owned).unwrap_or_else(|| revise.as_ref().map(|r| r.0.clone()).unwrap_or_default());
            let prior = ctx.store.read(|s| {
                s.thread(&run.turn.thread).and_then(|t| {
                    let is = |u: &&Turn| u.role == "ai" && u.name.as_deref() == Some(name.as_str());
                    t.turns.iter().find(|u| is(u) && u.widget).or_else(|| t.turns.iter().find(is)).cloned()
                })
            });
            let head = versions::head_of(ctx, &name)?;
            let base_v = match retry {
                Some(_) => prior.as_ref().and_then(|p| p.v).or(head),
                None => revise.as_ref().and_then(|r| r.1).or(head).or(prior.as_ref().and_then(|p| p.v)),
            };
            if retry.is_none() {
                push_user(run, prompt, extra)?;
            }
            let mut ai = Turn::ai("", &label);
            ai.text = None;
            ai.name = Some(name.clone());
            ai.retries = Some(prior.as_ref().and_then(|p| p.retries).unwrap_or(0) + 1);
            if let Some(art) = prior.as_ref().and_then(|p| p.art.clone()).filter(js::truthy) {
                ai.art = Some(art);
            }
            if retry.is_none() {
                mark_revising(run, &name, base_v);
            }
            (name, base_v, ai, false)
        }
        _ => {
            let name = format!("R{}", ctx.store.write(|s| s.next_module_seq())?);
            push_user(run, prompt, extra)?;
            let mut ai = Turn::ai("", &label);
            ai.text = None;
            ai.name = Some(name.clone());
            let id = ai.id.clone().unwrap_or_default();
            ctx.store.append_turn(&run.turn.thread, ai)?;
            let mut held = Turn::default();
            held.id = Some(id);
            (name, None, held, true)
        }
    };
    let use_llm = !run.settings.mock && (run.settings.anthropic() || run.settings.openrouter_key.is_some());
    let dirs = Dirs::of(&ctx.data);
    let _ = std::fs::create_dir_all(&dirs.stage);
    let nsh = ctx.nsh.clone().unwrap_or_else(|| std::sync::Arc::new(pipeline::nsh::NshCli::from_env()));
    let mut last_code: Option<String> = None;
    let mut last_caption = String::new();
    let mut err: Option<String> = None;
    let mut ok = false;
    let mut used = if use_llm { run.settings.model.clone() } else { "mock".into() };
    if use_llm {
        let linear = crate::hub::routes::leftovers::linear_key(ctx).await.is_some();
        for attempt in 0..3 {
            if ok {
                break;
            }
            if attempt > 0 {
                run.set_job("retrying", "Fixing the code");
            } else {
                run.set_job("coding", "Writing some code");
            }
            let asked = match &ctx.model {
                Some(api) => {
                    let ask = Ask { prompt, prev_error: err.as_deref(), prev_code: last_code.as_deref(), name: &name, linear };
                    model::ask_model(&run.settings, api.as_ref(), &run.turn, &ask, pipeline::mock_module, || run.set_job("coding", "Writing some code")).await
                }
                None => Err(crate::hub::model::anthropic::llm(None, "no model client")),
            };
            match asked {
                Ok(m) => {
                    last_caption = m.caption;
                    ok = try_code(run, &dirs, &nsh, &name, &m.code, &mut last_code, &mut err).await?;
                    if !ok {
                        eprintln!("  attempt {} rejected by nsh check: {}", attempt + 1, err.as_deref().unwrap_or("").split('\n').next().unwrap_or(""));
                    }
                }
                Err(e) => {
                    eprintln!("  llm call failed: {}", e.split('\n').next().unwrap_or(""));
                    let dead = e.starts_with("llm-api");
                    err = Some(e);
                    if dead {
                        break;
                    }
                }
            }
        }
    }
    if !ok {
        used = if use_llm { "mock-fallback".into() } else { "mock".into() };
        run.set_job("fallback", "Trying another way");
        ok = try_code(run, &dirs, &nsh, &name, &pipeline::mock_module(prompt), &mut last_code, &mut err).await.unwrap_or(false);
    }
    let caption = if ok && used == run.settings.model { last_caption } else { String::new() };
    if !ok {
        eprintln!("✗ {name}: generation failed\n{}", err.as_deref().unwrap_or("null"));
    }
    run.set_job("publishing", "Putting it in place");
    let code = if ok { last_code.clone().unwrap_or_default() } else { pipeline::error_card(err.as_deref().filter(|e| !e.is_empty()).unwrap_or("unknown")) };
    let (v, _chunks) = versions::write_version(ctx, &name, &code, base_v, Some(prompt), None)?;
    let id = ai_id.id.clone().unwrap_or_default();
    ctx.store.write(|s| {
        let Some(t) = s.thread_mut(&run.turn.thread) else { return };
        match t.turns.iter_mut().find(|u| pushed && u.id.as_deref() == Some(id.as_str())) {
            Some(u) => {
                u.label = Some(used.clone());
                u.caption = Some(caption.clone());
                u.v = Some(v);
            }
            None => {
                let mut u = ai_id.clone();
                u.label = Some(used.clone());
                u.caption = Some(caption.clone());
                u.v = Some(v);
                t.turns.push(u);
            }
        }
    })?;
    // the watcher sees the module before index.tsx imports it
    tokio::time::sleep(Duration::from_millis(500)).await;
    ctx.store.write_index();
    let line = format!("{name}.tsx via {used}");
    println!("{} {line} ({})", if ok { "✓" } else { "✗" }, js::slice_utf16(prompt, 60));
    ctx.jobs.finish(&ctx.store, run.job, &format!("{} — {line}", if ok { "rendered" } else { "failed" }), Some(&line));
    Ok(())
}
