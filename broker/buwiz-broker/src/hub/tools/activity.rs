//! What `/status` says a turn is doing (tools.mjs `PHRASES`, `phraseFor`,
//! `TOOL_ACTIVITY`, `toolActivity`): a phase (Rox's avatar animation) and a
//! short phrase, one drawn per phase and kept while that phase lasts.

use serde_json::{json, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::jobs::JobId;
use crate::js;

pub const THINKING: [&str; 3] = ["Thinking it through", "Mulling it over", "Putting it together"];
pub const WORKING: [&str; 5] = ["Getting it done", "Making progress", "Working on it", "Handling it", "Chipping away"];

/// The pool `PHRASES[key] || PHRASES.working`.
pub fn pool(key: &str) -> &'static [&'static str] {
    if key == "thinking" {
        &THINKING
    } else {
        &WORKING
    }
}

/// `phraseFor(job, key, pool)`: a phrase drawn at random when the key
/// changes, the same one while it stays (kept on the job as Node keeps
/// `_phraseKey` and `_phrase`).
pub fn phrase_for(ctx: &HubCtx, job: JobId, key: &str, pool: &[&str]) -> String {
    ctx.jobs
        .with(job, |j| {
            if j.fields.get("_phraseKey").and_then(Value::as_str) != Some(key) {
                let pick = pool[(buwiz_protocol::keys::random::<1>()[0] as usize) % pool.len()];
                j.fields.insert("_phraseKey".into(), json!(key));
                j.fields.insert("_phrase".into(), json!(pick));
            }
            j.fields.get("_phrase").map(js::string_of).unwrap_or_default()
        })
        .unwrap_or_else(|| pool[0].to_owned())
}

/// `TOOL_ACTIVITY[name]`.
fn fixed(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "web_search" => ("searching", "Digging around"),
        "fetch_url" => ("browsing", "Doing some reading"),
        "query_source" => ("connecting", "Working in a connected app"),
        "generate_app" | "revise_app" => ("coding", "Writing some code"),
        "view_code" => ("reading", "Reading the file"),
        "generate_image" | "app_icon" => ("generating", "Sketching something up"),
        "app_details_update" => ("writing", "Jotting it down"),
        "mcp_install" => ("connecting", "Setting up a connected app"),
        "mcp_uninstall" | "mcp_enable" | "mcp_disable" | "mcp_list" => ("connecting", "Working in a connected app"),
        "shell_run" => ("running-commands", "Running a few commands"),
        "office_create" | "office_export" => ("writing", "Putting the file together"),
        "skill_create" | "skill_update" | "memory_add" | "memory_remove" | "bot_profile_update" => ("writing", "Jotting it down"),
        _ => return None,
    })
}

/// `titleCase(s)`: trimmed, runs of `-`/`_` as a space, each word's first
/// letter (after a `\b`) upper-cased.
pub fn title_case(s: &str) -> String {
    let s = s.trim_matches(js::is_space);
    let mut spaced = String::new();
    let mut gap = false;
    for c in s.chars() {
        if c == '-' || c == '_' {
            if !gap {
                spaced.push(' ');
            }
            gap = true;
        } else {
            gap = false;
            spaced.push(c);
        }
    }
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut prev = false;
    for c in spaced.chars() {
        if word(c) && !prev {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        prev = word(c);
    }
    out
}

/// A connected app's display name (`appName`): its row's name, else its id.
fn app_name(ctx: &HubCtx, server: &str) -> String {
    ctx.db
        .with(|c| {
            use rusqlite::OptionalExtension;
            c.query_row("SELECT name FROM mcp_servers WHERE id=?", [server], |r| r.get::<_, Option<String>>(0)).optional()
        })
        .ok()
        .flatten()
        .flatten()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| server.to_owned())
}

/// `new URL(u).hostname`, for an absolute URL with a host.
fn hostname(u: &str) -> Option<String> {
    let url = url::Url::parse(u).ok()?;
    Some(url.host_str().unwrap_or("").to_owned())
}

/// `toolActivity(name, input)`: what a search or read is about, a connected
/// app's name, else the fixed table, else a working phrase.
pub fn tool_activity(ctx: &HubCtx, name: &str, input: &Value) -> (String, String) {
    let clip = |t: &str| if t.encode_utf16().count() > 40 { format!("{}…", js::slice_utf16(t, 39)) } else { t.to_owned() };
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    if name == "web_search" {
        if let Some(q) = s("query").map(|q| q.trim_matches(js::is_space)).filter(|q| !q.is_empty()) {
            return ("searching".into(), format!("Searching for {}", clip(q)));
        }
    }
    if name == "fetch_url" {
        if let Some(h) = s("url").and_then(hostname) {
            return ("browsing".into(), format!("Reading {}", h.strip_prefix("www.").unwrap_or(&h)));
        }
    }
    if name == "query_source" {
        if let Some(src) = s("source").filter(|x| !x.trim_matches(js::is_space).is_empty()) {
            return ("connecting".into(), format!("Working in {}", title_case(src)));
        }
    }
    if name == "app_tools_load" {
        if let Some(app) = input.get("app").filter(|a| js::truthy(a)) {
            return ("connecting".into(), format!("Working in {}", title_case(&app_name(ctx, &js::string_of(app)))));
        }
    }
    if let Some((server, _)) = super::mcp::parse_mcp_name(name) {
        return ("connecting".into(), format!("Working in {}", title_case(&app_name(ctx, &server))));
    }
    if name.starts_with("office_") {
        return ("reading".into(), "Reading the file".into());
    }
    match fixed(name) {
        Some((p, d)) => (p.into(), d.into()),
        None => ("working".into(), WORKING[0].into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_titles_and_clips_as_tools_mjs_does() {
        assert_eq!(title_case(" google-workspace_x "), "Google Workspace X");
        assert_eq!(title_case("many"), "Many");
        assert_eq!(title_case("it's"), "It'S");
        let ctx = HubCtx::new(std::env::temp_dir().join("rox-activity-none"), crate::hub::traits::Clock::manual());
        assert_eq!(tool_activity(&ctx, "react", &json!({})), ("working".into(), "Getting it done".into()));
        assert_eq!(tool_activity(&ctx, "web_search", &json!({"query": "  bbtree "})), ("searching".into(), "Searching for bbtree".into()));
        let long = "x".repeat(41);
        assert_eq!(tool_activity(&ctx, "web_search", &json!({"query": long})).1, format!("Searching for {}…", "x".repeat(39)));
        assert_eq!(tool_activity(&ctx, "fetch_url", &json!({"url": "https://www.example.com/a"})).1, "Reading example.com");
        assert_eq!(tool_activity(&ctx, "fetch_url", &json!({"url": "nope"})), ("browsing".into(), "Doing some reading".into()));
        assert_eq!(tool_activity(&ctx, "query_source", &json!({"source": "timeclock"})).1, "Working in Timeclock");
        assert_eq!(tool_activity(&ctx, "office_open", &json!({})), ("reading".into(), "Reading the file".into()));
        assert_eq!(tool_activity(&ctx, "generate_app", &json!({})), ("coding".into(), "Writing some code".into()));
    }
}
