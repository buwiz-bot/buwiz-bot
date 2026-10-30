//! The tool loop (agent.mjs `agentTurn`, `backgroundTurn`): up to 8 model
//! calls, the last with `tool_choice: none`; `pause_turn` polls again; a
//! refused server web search turns on the client one, and a stream that
//! fails before its first event turns streaming off, both for the process.

use serde_json::{json, Value};

use super::placeholders::{drop_pending, Stream};
use super::user_content::user_content;
use crate::hub::model::anthropic::{llm, parse_llm};
use crate::hub::model::{agent_stream, set_agent_stream, sse};
use crate::hub::store::{Thread, Turn};
use crate::hub::tools::{activity, defs, exec_tool, server_search, set_server_search, tool_list, Run};
use crate::js;

/// The thread's last ten turns as the model reads them (notices left out).
fn history(t: Option<&Thread>) -> Vec<Value> {
    let Some(t) = t else { return Vec::new() };
    let kept: Vec<&Turn> = t.turns.iter().filter(|u| u.role != "notice").collect();
    kept[kept.len().saturating_sub(10)..]
        .iter()
        .map(|u| {
            if u.role == "user" {
                match &u.text {
                    Some(text) => json!({"role": "user", "content": text}),
                    None => json!({"role": "user"}),
                }
            } else {
                let content = match u.name.as_deref().filter(|n| !n.is_empty()) {
                    Some(n) => format!("[mounted card {n}]"),
                    None => u.text.clone().unwrap_or_default(),
                };
                json!({"role": "assistant", "content": content})
            }
        })
        .collect()
}

/// `/web_search|server_tool|tool_use/i`.
fn search_refused(m: &str) -> bool {
    let l = m.to_lowercase();
    ["web_search", "server_tool", "tool_use"].iter().any(|w| l.contains(w))
}

/// `agentTurn(job, prompt, thread)`: the reply's text ("" when the model
/// answered only with tools). `Err` is Node's thrown `llm-api …`.
pub async fn agent_turn(run: &Run, prompt: &str, extra: &Value, ctx_blocks: &[Value]) -> Result<String, String> {
    let ctx = &run.ctx;
    let mut messages = ctx.store.read(|s| history(s.thread(&run.turn.thread)));
    messages.push(json!({"role": "user", "content": user_content(ctx, prompt, extra, ctx_blocks).await}));
    // the chat's attachments and the hints of the `@` scopes in play
    let scope = crate::hub::library::attachments::scope_section(ctx, &run.turn.thread, prompt).await
        + &crate::hub::tools::v1::prompt_section(crate::hub::tools::v1::view(ctx).as_ref(), prompt, extra.get("v1Attachments"));
    let label = run.label();
    let think = || {
        let p = activity::phrase_for(ctx, run.job, "thinking", &activity::THINKING);
        run.set_job("thinking", &p);
    };
    let mut reply = String::new();
    for i in 0..8 {
        if i == 0 {
            think();
        } else {
            let p = activity::phrase_for(ctx, run.job, &format!("working{i}"), &activity::WORKING);
            run.set_job("working", &p);
        }
        let mut events = 0usize;
        let streaming = agent_stream();
        let call: Result<Value, String> = async {
            let mut params = json!({
                "model": run.settings.model,
                "max_tokens": run.settings.max_tokens,
                "system": format!("{}{scope}", defs::agent_system(ctx, &run.turn).await),
                "tools": tool_list(run).await,
                "messages": messages.clone(),
            });
            // the last round must answer: a turn still calling tools when the
            // rounds run out would otherwise end with no reply
            if i == 7 {
                params["tool_choice"] = json!({"type": "none"});
            }
            let api = ctx.model.clone().ok_or_else(|| llm(None, "no model client"))?;
            if streaming {
                let rx = api.stream(&run.turn, params).await?;
                let mut watch = Stream::default();
                sse::collect(rx, |event, _| {
                    events += 1;
                    watch.event(run, event, &label, &think);
                })
                .await
            } else {
                api.create(&run.turn, params).await
            }
        }
        .await;
        let msg = match call {
            Ok(m) => m,
            Err(e) => {
                drop_pending(run);
                let (status, m) = parse_llm(&e);
                if server_search() && search_refused(m) {
                    eprintln!("  server web_search rejected ({}) — using client search", js::slice_utf16(m, 120));
                    set_server_search(false);
                    continue;
                }
                if agent_stream() && events == 0 {
                    eprintln!("  Messages streaming unavailable ({}) — using create", js::slice_utf16(m, 160));
                    set_agent_stream(false);
                    continue;
                }
                return Err(llm(status, m));
            }
        };
        let content = msg.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
        messages.push(json!({"role": "assistant", "content": content}));
        let stop = msg.get("stop_reason").and_then(Value::as_str).unwrap_or("");
        println!(
            "  · call {i}: stop={stop} blocks={}{}",
            content.iter().map(|b| b.get("type").map(js::string_of).unwrap_or_default()).collect::<Vec<_>>().join(","),
            if streaming { "" } else { " (not streamed)" }
        );
        // pause_turn: a server-side tool (web_search) is still mid-run
        if stop == "pause_turn" {
            continue;
        }
        let uses: Vec<&Value> = content.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")).collect();
        let said = crate::hub::model::text_of(&msg).trim_matches(js::is_space).to_owned();
        if !uses.is_empty() && !said.is_empty() {
            ctx.store.append_turn(&run.turn.thread, Turn::ai(&said, &label))?;
            ctx.store.write_index();
        }
        if stop != "tool_use" || uses.is_empty() {
            reply = said;
            break;
        }
        let mut results = Vec::new();
        for u in uses {
            let name = u.get("name").map(js::string_of).unwrap_or_default();
            let (p, d) = activity::tool_activity(ctx, &name, u.get("input").unwrap_or(&json!({})));
            run.set_job(&p, &d);
            let r = exec_tool(run, u).await;
            if r.is_error == Some(true) {
                println!("  ✗ {name}: {}", js::slice_utf16(&js::squeeze(&js::string_of(&r.content)), 200));
            }
            results.push(r.result_block(u.get("id").unwrap_or(&Value::Null)));
        }
        messages.push(json!({"role": "user", "content": results}));
        drop_pending(run);
        if stop == "max_tokens" {
            break;
        }
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_history_reads_cards_and_drops_notices() {
        let mut t = Thread::default();
        t.turns.push(Turn::user("hi"));
        t.turns.push(Turn { role: "ai".into(), name: Some("R3".into()), text: Some("x".into()), ..Default::default() });
        t.turns.push(Turn::notice("n", json!({})));
        t.turns.push(Turn { role: "ai".into(), name: Some(String::new()), ..Default::default() });
        t.turns.push(Turn { role: "user".into(), ..Default::default() });
        assert_eq!(
            history(Some(&t)),
            vec![json!({"role": "user", "content": "hi"}), json!({"role": "assistant", "content": "[mounted card R3]"}), json!({"role": "assistant", "content": ""}), json!({"role": "user"})]
        );
        assert!(search_refused("400 tool type web_search_20250305 is not supported") && !search_refused("prompt is too long"));
    }
}
