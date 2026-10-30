//! The agent tools (agents-tools.mjs, drafts/agent-messaging.md §4): in an
//! agent's chat and in its delivery turns, never in a routine's run. The
//! caller is the bot the turn answers as (`turn.acting_bot`, Node's
//! `job.agentBot`), else the thread's own; `turn.agent_hop` counts relays
//! (§3). Their schemas (`AGENT_TOOLS`, `tools.json`, dumped from Node) are
//! the tool loop's to offer, with [`offered`] saying when.

use std::sync::{Arc, OnceLock};

use serde_json::Value;

use super::{AgentError, AgentsHub, Party};
use crate::hub::ctx::HubCtx;
use crate::hub::store::State;
use crate::hub::types::{ToolResult, TurnCtx};
use crate::js;

pub const NAMES: [&str; 6] = ["agents_list", "agent_send", "agent_conversations", "agent_read", "agent_create", "agent_remove"];

/// `AGENT_TOOLS`: the six tools' definitions, as Node sends them.
pub fn defs() -> &'static Value {
    static DEFS: OnceLock<Value> = OnceLock::new();
    DEFS.get_or_init(|| serde_json::from_str(include_str!("tools.json")).expect("the agent tools' definitions"))
}

/// `agentToolsFor(thread, job)`: an agent's turns only, never a routine's.
pub fn offered(s: &State, turn: &TurnCtx) -> bool {
    turn.routine.is_none() && turn.surface.is_none() && (turn.acting_bot.is_some() || s.thread_bot(&turn.thread).is_some())
}

fn result(content: String, is_error: bool) -> ToolResult {
    ToolResult { content: Value::String(content), is_error }
}

/// `toolAgents(thread, name, input, job)`: one agent tool for the turn's agent.
pub fn call(hub: &Arc<AgentsHub>, ctx: &Arc<HubCtx>, turn: &TurnCtx, name: &str, input: &Value) -> ToolResult {
    let caller = ctx.store.read(|s| match &turn.acting_bot {
        Some(id) => s.bot(id).map(Party::of),
        None => s.thread_bot(&turn.thread).map(Party::of),
    });
    let Some(caller) = caller.filter(|_| turn.routine.is_none() && turn.surface.is_none()) else {
        return result(format!("{name} works only in an agent's own turns"), true);
    };
    let hop = turn.agent_hop.map_or(0, |h| h + 1);
    let a = &hub.agents;
    let out: Result<Value, AgentError> = match name {
        "agents_list" => Ok(ctx.store.read(|s| a.list(s, &caller))),
        "agent_send" => hub.write(ctx, |s, out| a.send(s, &caller, input, hop, None, out)),
        "agent_conversations" => Ok(ctx.store.read(|s| a.conversations(s, &caller.id, input))),
        "agent_read" => hub.write(ctx, |s, _| a.read(s, &caller.id, input)),
        // another agent's request does not make or end the user's agents
        _ if turn.acting_bot.is_some() => return result(format!("{name} is not available while answering another agent"), true),
        "agent_create" => hub.write(ctx, |s, out| a.create(s, &caller.id, input, hop, out)),
        "agent_remove" => hub.write(ctx, |s, _| a.remove(s, &caller.id, input)),
        _ => return result(format!("unknown tool {name}"), true),
    };
    match out {
        Ok(v) => result(js::stringify(&v), false),
        Err(e) => result(format!("{}: {}", e.code, e.message), true),
    }
}

/// Registers the six tools by name.
pub fn register(ctx: &mut HubCtx, hub: Arc<AgentsHub>) {
    for name in NAMES {
        let hub = hub.clone();
        ctx.tools.register(name, move |c: Arc<HubCtx>, turn: TurnCtx, input: Value| {
            let hub = hub.clone();
            async move { call(&hub, &c, &turn, name, &input) }
        });
    }
}
