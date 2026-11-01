//! The tool schemas and the agent's system prompt, as Node writes them
//! (D35): `defs.json` is dumped from tools.mjs, routines.mjs, botctx.mjs
//! and agents-tools.mjs, and server/test/tool-defs.test.mjs fails when it
//! drifts. `TOOLS`, `CLIENT_SEARCH`, `ROUTINE_TOOLS`, `BOT_TOOLS`,
//! `AGENT_TOOLS`; `agentSystem` (`baseSystem() + botSection(thread)`, with
//! `privateSkillsInject()`, `pluginSkillsInject()` and `contractFor`).

use std::sync::{Arc, OnceLock};

use serde_json::Value;

use crate::hub::ctx::HubCtx;
use crate::hub::store::Ceiling;
use crate::hub::library::botctx;
use crate::hub::market::catalog::Market;
use crate::hub::types::TurnCtx;

struct Defs {
    tools: Vec<Value>,
    client_search: Value,
    routine_tools: Vec<Value>,
    bot_tools: Vec<Value>,
    agent_tools: Vec<Value>,
    system: String,
    contract: String,
}

fn defs() -> &'static Defs {
    static D: OnceLock<Defs> = OnceLock::new();
    D.get_or_init(|| {
        let v: Value = serde_json::from_str(include_str!("defs.json")).expect("defs.json is JSON");
        let list = |k: &str| v[k].as_array().cloned().expect("a list of tools");
        Defs {
            tools: list("TOOLS"),
            client_search: v["CLIENT_SEARCH"].clone(),
            routine_tools: list("ROUTINE_TOOLS"),
            bot_tools: list("BOT_TOOLS"),
            agent_tools: list("AGENT_TOOLS"),
            system: v["SYSTEM"].as_str().expect("the system prompt").to_owned(),
            contract: v["CONTRACT"].as_str().expect("the contract's lead").to_owned(),
        }
    })
}

/// `TOOLS`.
pub fn tools() -> &'static [Value] {
    &defs().tools
}

/// `CLIENT_SEARCH`: the client-side web search a refused server search
/// leaves the model.
pub fn client_search() -> &'static Value {
    &defs().client_search
}

/// `ROUTINE_TOOLS` (their handlers are routines', 5.5).
pub fn routine_tools() -> &'static [Value] {
    &defs().routine_tools
}

/// `BOT_TOOLS` (handlers in `library/botctx.rs`).
pub fn bot_tools() -> &'static [Value] {
    &defs().bot_tools
}

/// `AGENT_TOOLS` (handlers are agent messaging's, 5.6).
pub fn agent_tools() -> &'static [Value] {
    &defs().agent_tools
}

pub fn name_of(t: &Value) -> &str {
    t.get("name").and_then(Value::as_str).unwrap_or("")
}

/// Whether `name` is one of `list`'s tools.
pub fn has(list: &[Value], name: &str) -> bool {
    list.iter().any(|t| name_of(t) == name)
}

/// The names of every built-in tool (what a v1 tool may not shadow).
pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    let d = defs();
    d.tools.iter().chain(&d.routine_tools).chain(&d.bot_tools).chain(&d.agent_tools).map(name_of)
}

/// `baseSystem()`: the agent's prompt with the skill sections and the
/// widget contract (`linear`: whether a Linear key is set).
pub fn base_system(ctx: &HubCtx, linear: bool, ceiling: Option<&Ceiling>) -> String {
    let d = defs();
    let plugins = ctx.ext.get::<Arc<Market>>().map(|m| crate::hub::market::install::plugin_skills_inject(m, ctx)).unwrap_or_default();
    format!("{}{}{}{}{}", d.system, crate::hub::market::skills::private_skills_inject_under(ctx, ceiling), plugins, d.contract, crate::hub::model::prompts::contract_for("__NAME__", linear))
}

/// `agentSystem(thread)`: the base prompt plus, in a bot's thread, the
/// bot's own section.
pub async fn agent_system(ctx: &HubCtx, turn: &TurnCtx) -> String {
    let linear = crate::hub::routes::leftovers::linear_key(ctx).await.is_some();
    let (bot, ceiling) = ctx.store.read(|s| {
        let b = botctx::bot_of(s, turn);
        (botctx::bot_section(b), b.map(|b| b.tools.clone()))
    });
    format!("{}{bot}", base_system(ctx, linear, ceiling.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defs_hold_every_list_nodes_test_names() {
        assert_eq!(tools().len(), 27);
        assert_eq!(tools()[0]["name"], "generate_app");
        assert_eq!(client_search()["name"], "web_search");
        assert_eq!(routine_tools().len(), 5);
        assert_eq!(bot_tools().iter().map(name_of).collect::<Vec<_>>(), botctx::BOT_TOOLS);
        assert_eq!(agent_tools().len(), 6);
        assert!(defs().system.starts_with("You are the assistant inside a generative-UI chat app: a chat where you answer in text OR native UI, whichever fits the request.\n"));
        // Node's key order survives: the schema is sent as written
        assert!(crate::js::stringify(&tools()[0]).starts_with(r#"{"name":"generate_app","description":"Build an interactive"#));
    }
}
