//! The model's prompt texts, byte for byte as model.mjs writes them
//! (`systemFor`, `contractFor`, `sourcesBlock`, the classifier's and the
//! chat reply's system prompts, the generator's follow-ups). The long
//! texts sit beside this file as dumped from Node; `{{NAME}}`,
//! `{{SOURCES}}` and `{{CONTRACT}}` mark what is filled in.

const SYSTEM: &str = include_str!("prompts/system.txt");
const CONTRACT: &str = include_str!("prompts/contract.txt");
const SOURCES: &str = include_str!("prompts/sources.txt");
const LINEAR: &str = include_str!("prompts/linear.txt");

/// `sourcesBlock()`: the `/data` sources a widget may fetch; Linear's
/// only when there is a Linear key (`getLinearKey()`).
pub fn sources_block(linear: bool) -> String {
    if linear {
        format!("{SOURCES}\n{LINEAR}")
    } else {
        SOURCES.to_owned()
    }
}

/// `contractFor(name)`: the module contract; `name` is the module's
/// `/state` key (`__NAME__` for the tool loop, replaced on write).
pub fn contract_for(name: &str, linear: bool) -> String {
    CONTRACT.replace("{{SOURCES}}", &sources_block(linear)).replace("{{NAME}}", name)
}

/// `systemFor(name)`: the one-shot generator's system prompt.
pub fn system_for(name: &str, linear: bool) -> String {
    SYSTEM.replace("{{NAME}}", name).replace("{{CONTRACT}}", &contract_for(name, linear))
}

/// `classifyIntent`'s system prompt.
pub const CLASSIFY: &str = "Classify the user message for a generative-UI chat app. Reply with exactly one word: UI or CHAT.\nUI = the user wants an interface, widget, app, card, or game built/rendered — including terse requests that name a thing to build (\"pomodoro timer\", \"make a calculator\").\nCHAT = greetings, questions, discussion, feedback, or anything that only wants a text answer.";

/// `chatReply`'s system prompt (the Anthropic path only: MODEL-04).
pub const CHAT: &str = "You are the assistant inside a generative-UI chat app: when the user asks for an interface, a tool builds and renders it natively. This message is conversation — answer in GitHub-flavored markdown, concise. If a UI would obviously help, offer to build it in one line.";

/// `askModel`'s follow-up after a failed try.
pub fn follow_up(prev_error: &str) -> String {
    if prev_error.contains("truncated at the token limit") {
        "Your reply was cut off at the token limit. Return the SAME module but shorter: fewer comments, tighter code, no extras.".to_owned()
    } else if prev_error.contains("did not return a module") {
        "Your reply had no module. Reply with ONE fenced ```tsx block containing the complete module (it must include export default) and nothing else.".to_owned()
    } else {
        format!("The nsh compiler rejected it:\n{prev_error}\nReturn a corrected module only.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_prompts_fill_in_the_name_and_sources() {
        let s = system_for("R7", false);
        assert!(s.starts_with("You write Native Shell (nsh) modules: ONE TSX file, rendered as native UI (UIKit/AppKit). You are writing module R7.\nReply format:"));
        assert!(s.contains("\n\nContract:\n- Exactly one default export"));
        assert!(s.contains("fetch ONLY http://127.0.0.1:8787/state/R7 (all other hosts") && s.contains("const S = \"R7\"; fetch(`http://127.0.0.1:8787/state/${S}`)"));
        assert!(!s.contains("{{") || s.contains("style={{ width"));
        assert!(!s.contains("{{NAME}}") && !s.contains("{{SOURCES}}") && !s.contains("{{CONTRACT}}"));
        assert!(!s.contains("linear —") && contract_for("x", true).contains("\n  linear — the user's Linear issues"));
        assert!(s.ends_with("Compact code, but take the lines a working game needs (~150 is fine)."));
    }
}
