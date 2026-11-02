//! What v1 plugins put in front of the model (v1/agent.mjs, spec 16, 18):
//! their tools under their own names (`<short>_<tool>`, `[short] `
//! descriptions capped at 1024), server instructions and scope hints, only
//! from enabled, running, approved plugins; a routine's turn sees its own
//! surface (19.1); a tool result's `buwiz/open` becomes a card on the reply
//! and its `buwiz/suggest` the reply's next steps, never shown to the model.
//! A package tool that answers `{code: "account_required", account}` for an
//! account its package declares puts a sign-in card (`account_required`)
//! in the chat.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{LinkTurn, Out, Run};
use crate::helper::plugins::Row;
use crate::hub::ctx::HubCtx;
use crate::hub::routines::plugins::{disabled_of, NoPlugins, PluginView};
use crate::hub::routines::text::approved_text;
use crate::js;

/// The plugin host the tool loop reads: a test's (`PluginsSeam` in
/// `ctx.ext`), else the hub's, else none.
#[derive(Clone)]
pub struct PluginsSeam(pub Arc<dyn PluginView>);

pub fn view(ctx: &HubCtx) -> Arc<dyn PluginView> {
    if let Some(s) = ctx.ext.get::<PluginsSeam>() {
        return s.0.clone();
    }
    match &ctx.plugins {
        Some(h) => Arc::new(h.clone()),
        None => Arc::new(NoPlugins),
    }
}

/// Enabled, running and approved as installed (5.1).
pub fn contributing(p: &dyn PluginView) -> Vec<Row> {
    let trust = p.trust();
    p.rows().into_iter().filter(|r| r.enabled && p.runtime(&r.id).state == "running" && trust.is_approved(&r.id, &r.publisher_key_id)).collect()
}

/// `name` starts like a built-in's (`mcp_`, `office_`, `skill_`, …).
fn builtin_prefix(name: &str) -> bool {
    ["mcp_", "office_", "skill_", "routine_", "app_", "bot_"].iter().any(|p| name.starts_with(p))
}

/// A routine's turn sees its plugin's tools and the `reads` it names.
fn in_surface(surface: Option<&Value>, plugin: &str, name: &str) -> bool {
    let Some(s) = surface else { return true };
    if s.get("plugin").and_then(Value::as_str) == Some(plugin) {
        return true;
    }
    s.get("reads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|x| x.get("plugin").and_then(Value::as_str) == Some(plugin) && x.get("tools").and_then(Value::as_array).is_some_and(|t| t.iter().any(|n| n.as_str() == Some(name))))
}

/// 16.4: a tool whose `buwiz/audience` leaves out "agent" serves apps only.
fn for_agent(t: &Value) -> bool {
    match t.pointer("/_meta/buwiz~1audience") {
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some("agent")),
        _ => true,
    }
}

/// `v1Tools(plugins, taken, surface)`: (plugin id, the model's def).
pub fn v1_tools(p: &dyn PluginView, taken: &[String], surface: Option<&Value>) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for row in contributing(p) {
        let off = disabled_of(&row);
        for t in p.runtime(&row.id).tools {
            let name = t.get("name").map(js::string_of).unwrap_or_default();
            if crate::v1::is_v1("reservedTool", t.get("name")) || off.contains(&name) || taken.contains(&name) || builtin_prefix(&name) || !name.starts_with(&format!("{}_", row.short)) {
                continue;
            }
            if !in_surface(surface, &row.id, &name) || !for_agent(&t) {
                continue;
            }
            let desc = t.get("description").filter(|d| js::truthy(d)).map(js::string_of).unwrap_or_else(|| name.clone());
            let schema = t.get("inputSchema").filter(|s| js::truthy(s)).cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            out.push((row.id.clone(), json!({"name": name, "description": approved_text(&format!("[{}] {desc}", row.short), 1024), "input_schema": schema})));
        }
    }
    out
}

/// `v1ToolPlugin(plugins, name, surface)`: the plugin serving `name`.
pub fn tool_plugin(p: &dyn PluginView, name: &str, surface: Option<&Value>) -> Option<String> {
    v1_tools(p, &[], surface).into_iter().find(|(_, t)| t["name"] == name).map(|(p, _)| p)
}

/// v1 content folded for the model.
fn fold(r: &Value) -> String {
    let parts: Vec<String> = r
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| match c.get("type").and_then(Value::as_str) {
            Some("text") => c.get("text").map(js::string_of).unwrap_or_else(|| "undefined".into()),
            Some("image") => format!("[image: {}]", c.get("mimeType").filter(|m| js::truthy(m)).map(js::string_of).unwrap_or_else(|| "image/png".into())),
            _ => js::stringify(c),
        })
        .collect();
    let s = parts.join("\n");
    if s.is_empty() {
        "(empty)".into()
    } else {
        s
    }
}

pub const SUGGEST_MAX: usize = 3;
const SUGGEST_LABEL: usize = 40;
const SUGGEST_PROMPT: usize = 200;

fn cap_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 16.3 `noteSuggest`: at most 3 next steps, labels 40 and prompts 200
/// characters, the rest of each dropped.
pub fn note_suggest(lt: &mut LinkTurn, suggest: &Value) {
    let Some(list) = suggest.as_array() else { return };
    let mine = lt.suggest.get_or_insert_with(Vec::new);
    for s in list {
        if mine.len() >= SUGGEST_MAX {
            break;
        }
        let (Some(label), Some(prompt)) = (s.get("label").and_then(Value::as_str), s.get("prompt").and_then(Value::as_str)) else { continue };
        let (label, prompt) = (label.trim_matches(js::is_space), prompt.trim_matches(js::is_space));
        if !s.is_object() || label.is_empty() || prompt.is_empty() {
            continue;
        }
        mine.push(json!({"label": cap_chars(label, SUGGEST_LABEL), "prompt": cap_chars(prompt, SUGGEST_PROMPT)}));
    }
}

/// 16.2 `noteCard`: a card for the reply, never an open; a bad card is
/// dropped and logged.
fn note_card(run: &Run, row: &Row, open: &Value) {
    let card = match crate::helper::sessions::check_open(&row.manifest(), open) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  v1 {}: buwiz/open card dropped: {}", row.id, js::slice_utf16(&e.message, 200));
            return;
        }
    };
    let query = card.get("ticket").filter(|t| js::truthy(t)).map(|t| format!("?ticket={}", js::string_of(t))).unwrap_or_default();
    let link = format!("buwiz://app/{}/{}{query}", js::string_of(&card["plugin"]), js::string_of(&card["app"]));
    let action = if card.get("placement").and_then(Value::as_str) == Some("inline") { "show" } else { "open" };
    let mut st = run.state();
    st.v1_cards.push(card.clone());
    st.link_turn.get_or_insert_with(Default::default).links.push(json!({
        "provider": "buwiz-app", "id": link, "name": card["title"], "link": link, "action": action, "kind": "app", "file_type": "Mini app", "card": card,
    }));
}

/// `v1AgentCall(plugins, job, name, input)`: a model call to a v1 tool
/// (origin agent, 16.1); in a routine's turn a call to its own plugin names
/// the routine, and a tool of it that changes data runs only when the
/// routine lists it in `writes` (19.1).
pub async fn call(run: &Run, name: &str, input: &Value) -> Out {
    let p = view(&run.ctx);
    let surface = run.turn.surface.as_ref();
    let plugin = tool_plugin(p.as_ref(), name, surface);
    let Some(row) = plugin.as_deref().and_then(|id| p.row(id)) else {
        return Out { content: json!(format!("{name} is not available: its plugin is off, failed, or the tool is switched off")), is_error: Some(true) };
    };
    let routine = surface.and_then(|s| s.get("routine")).filter(|r| js::truthy(r));
    let own = routine.is_some() && surface.and_then(|s| s.get("plugin")).and_then(Value::as_str) == Some(row.id.as_str());
    let writes = surface.and_then(|s| s.get("writes")).and_then(Value::as_array).is_some_and(|w| w.iter().any(|x| x.as_str() == Some(name)));
    let read_only = p.runtime(&row.id).tools.iter().any(|t| t.get("name").and_then(Value::as_str) == Some(name) && t.pointer("/annotations/readOnlyHint") == Some(&Value::Bool(true)));
    if own && !writes && !read_only {
        return Out {
            content: json!(format!("{name} changes data, and routine {} may change data only with the tools its manifest lists in writes", js::string_of(routine.unwrap_or(&Value::Null)))),
            is_error: Some(true),
        };
    }
    let mut caller = serde_json::Map::new();
    caller.insert("origin".into(), json!("agent"));
    caller.insert("plugin".into(), json!(row.id));
    if own {
        let s = surface.expect("own implies a surface");
        caller.insert("routine".into(), s["routine"].clone());
        if s.get("instance").is_some_and(js::truthy) {
            caller.insert("instance".into(), s["instance"].clone());
            caller.insert("params".into(), s.get("params").filter(|x| js::truthy(x)).cloned().unwrap_or(json!({})));
        }
    }
    caller.insert("ts".into(), json!(crate::hub::store::now_ms()));
    let args = if input.is_object() { input.clone() } else { json!({}) };
    let r = match p.call_tool(&row.id, name, args, Some(json!({"buwiz/caller": Value::Object(caller)}))).await {
        Ok(r) => r,
        Err(e) => return Out { content: json!(format!("{name}: {}: {}", if e.code.is_empty() { "error" } else { &e.code }, js::slice_utf16(&e.message, 300))), is_error: Some(true) },
    };
    let failed = r.get("isError").is_some_and(js::truthy);
    if failed {
        if let Some(out) = account_required(run, p.as_ref(), &row, &r) {
            return out;
        }
    }
    if let Some(open) = r.pointer("/_meta/buwiz~1open").filter(|o| js::truthy(o)) {
        if !failed {
            note_card(run, &row, open);
        }
    }
    if let Some(suggest) = r.pointer("/_meta/buwiz~1suggest").filter(|o| js::truthy(o)) {
        if !failed {
            note_suggest(run.state().link_turn.get_or_insert_with(Default::default), suggest);
        }
    }
    Out { content: json!(fold(&r)), is_error: Some(failed) }
}

/// §20.4: a failed call whose data is `{code: "account_required",
/// account}`, `account` one the package declares: one pending sign-in card
/// per chat and account, and a line for the model.
fn account_required(run: &Run, p: &dyn PluginView, row: &Row, r: &Value) -> Option<Out> {
    let data = crate::helper::plugins::result_data(r);
    if data.get("code").and_then(Value::as_str) != Some("account_required") {
        return None;
    }
    let account = data.get("account").and_then(Value::as_str)?;
    let a = p.declared_accounts(&row.id).into_iter().find(|a| a.id == account)?;
    let title = row.manifest().get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| row.id.clone());
    let chat = run.thread_id();
    let pending = run.ctx.store.read(|s| {
        s.thread(&chat).is_some_and(|t| t.turns.iter().filter_map(|u| u.notice.as_ref()).any(|n| n["kind"] == "account_required" && n["id"] == json!(row.id) && n["account"] == json!(a.id) && n["state"] == "pending"))
    });
    let n = json!({"kind": "account_required", "id": row.id, "plugin": row.id, "title": title, "account": a.id, "provider": a.provider, "label": a.label, "state": "pending"});
    let asked = pending || super::miniapps::post(run, &n);
    let how = if asked { ": a card in this chat asks them to" } else { "" };
    Some(Out { content: json!(format!("{title} needs the user to sign in to their {} first{how}. You cannot sign in for them; try again after they have.", a.label)), is_error: Some(true) })
}

/// `@short:scope:value` tokens a message types (grammar.mjs `typedToken`,
/// global): (short, scope) of each.
pub fn typed_tokens(text: &str) -> Vec<(String, String)> {
    let cs: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let lower = |c: char| c.is_ascii_lowercase();
    let lower_digit = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    while i < cs.len() {
        let before_ok = i == 0 || !(cs[i - 1].is_ascii_alphanumeric() || cs[i - 1] == '_' || cs[i - 1] == '@' || cs[i - 1] == '.');
        if cs[i] != '@' || !before_ok {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let take = |j: &mut usize, first: &dyn Fn(char) -> bool, rest: &dyn Fn(char) -> bool, min: usize, max: usize| -> Option<String> {
            let start = *j;
            if *j >= cs.len() || !first(cs[*j]) {
                return None;
            }
            *j += 1;
            while *j < cs.len() && rest(cs[*j]) {
                *j += 1;
            }
            let n = *j - start - 1;
            (n >= min && n <= max).then(|| cs[start..*j].iter().collect())
        };
        let short = take(&mut j, &lower, &lower_digit, 1, 15);
        let ok = short.is_some() && cs.get(j) == Some(&':');
        let scope = if ok {
            j += 1;
            take(&mut j, &lower, &|c: char| lower_digit(c) || c == '-', 0, 31)
        } else {
            None
        };
        match (short, scope) {
            (Some(sh), Some(sc)) if cs.get(j) == Some(&':') => {
                j += 1;
                while j < cs.len() && !cs[j].is_whitespace() {
                    j += 1;
                }
                out.push((sh, sc));
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

/// `v1Canonical(s)`: `@short:scope:tkt_…` → (short, scope).
pub fn canonical(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix('@')?;
    let mut parts = rest.splitn(3, ':');
    let (short, scope, ticket) = (parts.next()?, parts.next()?, parts.next()?);
    let lower = |c: char| c.is_ascii_lowercase();
    let short_ok = (2..=16).contains(&short.len()) && short.starts_with(lower) && short.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let scope_ok = (1..=32).contains(&scope.len()) && scope.starts_with(lower) && scope.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (short_ok && scope_ok && crate::v1::grammar_ok("ticket", ticket)).then(|| (short.to_owned(), scope.to_owned()))
}

/// `String(x)` of a value, `""` for undefined and null (`?? ""`).
fn text(v: Option<&Value>) -> String {
    v.filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default()
}

/// `v1PromptSection(plugins, prompt, attachments)`: the approved plugins'
/// instructions, the chat's attachments (18.3) and the scopes in play.
pub fn prompt_section(p: &dyn PluginView, prompt: &str, attachments: Option<&Value>) -> String {
    let rows = contributing(p);
    let mut parts = Vec::new();
    for row in &rows {
        let instr = p.runtime(&row.id).instructions;
        if !instr.is_empty() {
            parts.push(format!("## Instructions from {} (approved)\n{}", text(row.manifest().get("title")), approved_text(&instr, 4000)));
        }
    }
    let mut used: Vec<(String, String)> = Vec::new();
    let mut lines = Vec::new();
    for a in attachments.and_then(Value::as_array).into_iter().flatten().take(16) {
        let Some((short, scope)) = a.get("token").and_then(Value::as_str).and_then(canonical) else { continue };
        let Some(row) = rows.iter().find(|x| x.short == short && a.get("plugin").and_then(Value::as_str) == Some(x.id.as_str())) else { continue };
        let m = row.manifest();
        let Some(s) =
            m.get("scopes").and_then(Value::as_array).and_then(|ss| ss.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(scope.as_str()) && x.get("attach").is_some_and(js::truthy)))
        else {
            continue;
        };
        if !used.contains(&(short.clone(), scope.clone())) {
            used.push((short.clone(), scope.clone()));
        }
        let src = s.get("description").filter(|d| js::truthy(d)).or_else(|| s.get("name"));
        let first = text(src).trim_matches(js::is_space).split(char::is_whitespace).next().unwrap_or("").to_owned();
        let trimmed = first.trim_end_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        let noun = if trimmed.is_empty() { "item".to_owned() } else { trimmed };
        lines.push(format!("Attached: {} ({}): {} tools default to this {noun} here", text(a.get("token")), approved_text(&text(a.get("title").filter(|t| js::truthy(t))), 60), text(m.get("title"))));
    }
    if !lines.is_empty() {
        parts.push(format!("## Attached to this chat\n{}", lines.join("\n")));
    }
    for k in typed_tokens(prompt) {
        if !used.contains(&k) {
            used.push(k);
        }
    }
    for (short, scope) in used {
        let Some(row) = rows.iter().find(|x| x.short == short) else { continue };
        let m = row.manifest();
        let Some(s) = m.get("scopes").and_then(Value::as_array).and_then(|ss| ss.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(scope.as_str()))) else { continue };
        let mut body = vec![approved_text(&text(s.get("description")), 120)];
        if s.get("hint").is_some_and(js::truthy) {
            body.push(approved_text(&text(s.get("hint")), 1500));
        }
        let body: Vec<String> = body.into_iter().filter(|b| !b.is_empty()).collect();
        parts.push(format!("## Scope: {short}:{scope} (approved plugin {})\n{}", row.id, body.join("\n")));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", parts.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_tokens_parse_as_grammar_mjs_does() {
        assert_eq!(typed_tokens("check @fx:item:acme-9 today"), vec![("fx".into(), "item".into())]);
        assert_eq!(typed_tokens("a@fx:item:x @f:item:x @fx:Item:x @fx:item:"), vec![("fx".into(), "item".into())]);
        assert_eq!(typed_tokens("x.@fx:item:1 _@fx:item:2"), vec![]);
        assert_eq!(typed_tokens("@fx:item:a@fx:note:b (@fx:note:c)"), vec![("fx".into(), "item".into()), ("fx".into(), "note".into())]);
        assert_eq!(canonical("@fx:item:tkt_0123456789ABCDEFGHJKMNPQRS"), Some(("fx".into(), "item".into())));
        assert_eq!(canonical("@fx:item:123-456-789"), None);
    }
}
