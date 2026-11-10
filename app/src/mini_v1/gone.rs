//! A card whose app its plugin no longer has (G3-1): a newer version of
//! the plugin dropped that card type, so nothing can draw it again.
//!
//! The card then shows what it last showed as plain text, read-only. That
//! text comes from the plugin's own store, through its `<short>_card_text`
//! tool, and is held only while the card is on screen: nothing of it goes
//! in the chat's record or in Rox's. Without it (no such tool, nothing
//! stored, any refusal) the card shows its title and "Open in plugin".
//!
//! The decisions are pure; [`read`] is the one call out.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    RoxError,
    attribution::Actor,
    bridge,
    cards::OpenCard,
    connect::Listed,
    host_api,
    load::PluginEntry,
    session::Session,
};

/// The most lines a card's text shows, and the longest a line is.
pub const LINES_MAX: usize = 12;
const LINE_MAX: usize = 200;
const TIMEOUT: Duration = Duration::from_secs(10);
const RESULT_MAX: usize = 64 * 1024;

/// Whether `plugin`'s manifest says the card's `app` is gone: the plugin
/// is installed and running, and has no such app. A plugin that is down,
/// restarting, failed or not listed says nothing about its apps: its
/// cards keep what they do today.
pub fn gone(entry: Option<&PluginEntry>, listed: Option<&Listed>, app: &str) -> bool {
    match (entry, listed) {
        (Some(e), Some(l)) => l.state == "running" && l.enabled && e.app(app).is_none(),
        _ => false,
    }
}

/// What the note says: "The new version of Google Workspace can't redraw
/// this card."
pub fn note(plugin_title: &str) -> String {
    format!("The new version of {plugin_title} can't redraw this card.")
}

fn is_ticket(s: &str) -> bool {
    s.strip_prefix("tkt_").is_some_and(|rest| rest.len() == 26 && rest.bytes().all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b)))
}

/// The card's ticket: its own, else the one prop that holds a ticket (a
/// Mail card's `search`, a draft's `draft`). `None` with none, or with
/// two props that could each be it.
pub fn ticket(card: &OpenCard) -> Option<String> {
    if let Some(t) = card.ticket.as_deref().filter(|t| is_ticket(t)) {
        return Some(t.to_owned());
    }
    let mut held = card.props.values().filter_map(Value::as_str).filter(|v| is_ticket(v));
    let first = held.next()?;
    held.next().is_none().then(|| first.to_owned())
}

/// The plugin's tool that says a card's text, when it lists one the
/// person left on: `<short>_card_text`.
pub fn text_tool(listed: &Listed) -> Option<String> {
    let name = format!("{}_card_text", listed.short);
    (!listed.short.is_empty() && listed.tools.iter().any(|t| t.name == name && !t.disabled)).then_some(name)
}

fn places(app: &Value, placement: &str) -> bool {
    app.get("placement").and_then(Value::as_array).is_some_and(|p| p.iter().any(|x| x.as_str() == Some(placement)))
}

/// The plugin's first full-screen app, `(id, title)`: where "Open in
/// plugin" goes.
pub fn full_app(entry: &PluginEntry) -> Option<(String, String)> {
    let app = entry.apps.iter().find(|a| places(a, "full"))?;
    let id = app.get("id")?.as_str()?.to_owned();
    let title = app.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or(&entry.title).to_owned();
    Some((id, title))
}

/// The app whose session reads the text: an app-only tool takes a call
/// from any app of its plugin, and the gone app has no session to give.
/// A full-screen one first: the person can open it themselves.
fn session_app(entry: &PluginEntry) -> Option<(String, Option<Value>)> {
    let app = entry.apps.iter().find(|a| places(a, "full")).or_else(|| entry.apps.first())?;
    Some((app.get("id")?.as_str()?.to_owned(), app.get("permissions").cloned()))
}

/// A card's last content, as the plugin's store has it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Text {
    pub lines: Vec<String>,
    /// When the store last had it from the service (ms), 0 unknown.
    pub at: i64,
}

/// `<short>_card_text`'s answer (`{title, lines, at}`) as the card shows
/// it: plain lines, a few, each cut to one line's length. `None` when it
/// holds no line.
pub fn text_of(answer: &Value) -> Option<Text> {
    let lines: Vec<String> = answer
        .get("lines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        // one line each, whatever a subject carries
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .map(|l| if l.chars().count() > LINE_MAX { format!("{}…", l.chars().take(LINE_MAX).collect::<String>()) } else { l })
        .take(LINES_MAX)
        .collect();
    (!lines.is_empty()).then(|| Text { lines, at: answer.get("at").and_then(Value::as_i64).unwrap_or(0) })
}

/// Reads the card's text from its plugin's store: a session of an app the
/// plugin still has, one call of its text tool as the person, and the
/// session closed. Blocks: call it off the main thread.
pub fn read(broker: Arc<dyn host_api::Broker>, entry: &PluginEntry, listed: &Listed, card: &OpenCard) -> Result<Text, RoxError> {
    let none = |why: &str| RoxError::new("not_found", why);
    let tool = text_tool(listed).ok_or_else(|| none("the plugin has no card text"))?;
    let ticket = ticket(card).ok_or_else(|| none("the card has no ticket"))?;
    let (app, permissions) = session_app(entry).ok_or_else(|| none("the plugin has no app"))?;
    let session = Session::open(&*broker, &entry.id, &app, permissions.as_ref(), entry.generation)?;
    let answer = session
        .sign_call(&tool, &json!({"ticket": ticket}), Actor::Person, None)
        .and_then(|(envelope, sig)| host_api::call(&*broker, &envelope, &sig, TIMEOUT))
        .and_then(|r| bridge::tool_result(r, RESULT_MAX));
    session.close(broker);
    text_of(&answer?).ok_or_else(|| none("the plugin's store has nothing for this card"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const T: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

    fn plugin(apps: Value, state: &str, enabled: bool, tools: Value) -> (PluginEntry, Listed) {
        let v = json!({"id": "app.buwiz.google-workspace", "short": "gws", "title": "Google Workspace", "version": "1.5.0", "state": state, "enabled": enabled, "apps": apps, "tools": tools});
        (PluginEntry::from_json(&v).unwrap(), Listed::of(&v))
    }

    fn card(link: Value) -> OpenCard {
        OpenCard::from_link(&json!({"provider": "buwiz-app", "card": link})).unwrap()
    }

    #[test]
    fn a_card_is_gone_only_when_a_running_plugins_manifest_lacks_its_app() {
        let apps = json!([{"id": "mail", "title": "Mail", "placement": ["full", "inline"]}]);
        let (e, l) = plugin(apps.clone(), "running", true, json!([]));
        assert!(!gone(Some(&e), Some(&l), "mail"), "it has the app");
        assert!(gone(Some(&e), Some(&l), "digest"), "the new version dropped it");
        // a plugin that isn't running says nothing about its apps
        for state in ["starting", "restarting", "stopped", "failed", "update_available"] {
            let (e, l) = plugin(apps.clone(), state, true, json!([]));
            assert!(!gone(Some(&e), Some(&l), "digest"), "{state}");
        }
        let (e, l) = plugin(apps, "running", false, json!([]));
        assert!(!gone(Some(&e), Some(&l), "digest"), "disabled");
        assert!(!gone(None, Some(&l), "digest") && !gone(Some(&e), None, "digest") && !gone(None, None, "digest"), "not listed: uninstalled, or the broker is down");
        assert_eq!(note("Google Workspace"), "The new version of Google Workspace can't redraw this card.");
    }

    #[test]
    fn the_cards_ticket_is_its_own_or_its_one_ticket_prop() {
        let of = |c: Value| ticket(&card(c));
        assert_eq!(of(json!({"plugin": "app.buwiz.google-workspace", "app": "mail", "props": {"search": T}, "placement": "inline"})).as_deref(), Some(T));
        assert_eq!(of(json!({"plugin": "app.buwiz.google-workspace", "app": "mail", "props": {"view": "inbox"}, "placement": "inline"})), None);
        assert_eq!(of(json!({"plugin": "app.buwiz.google-workspace", "app": "mail", "placement": "inline"})), None);
        let other = "tkt_ZZZZZZZZZZZZZZZZZZZZZZZZZZ";
        assert_eq!(of(json!({"plugin": "app.buwiz.google-workspace", "app": "mail", "props": {"a": T, "b": other}, "placement": "inline"})), None, "two could be it: neither is guessed");
        assert!(!is_ticket("tkt_short") && !is_ticket("tkt_0123456789ABCDEFGHJKMNPQRI") && !is_ticket(""));
    }

    #[test]
    fn the_text_tool_is_the_plugins_own_and_only_while_it_is_on() {
        let tools = |disabled: bool| json!([{"name": "gws_card_text", "description": "", "disabled": disabled}, {"name": "gws_search_mail", "description": ""}]);
        let apps = json!([{"id": "card", "title": "Card", "placement": ["inline"]}, {"id": "mail", "title": "Mail", "placement": ["full", "inline"], "permissions": {"call": ["gws_mail_list"]}}]);
        let (e, l) = plugin(apps, "running", true, tools(false));
        assert_eq!(text_tool(&l).as_deref(), Some("gws_card_text"));
        assert_eq!(text_tool(&plugin(json!([]), "running", true, tools(true)).1), None, "switched off by the person");
        assert_eq!(text_tool(&plugin(json!([]), "running", true, json!([{"name": "gws_search_mail"}])).1), None);
        // Open in plugin, and the session, go to its first full-screen app
        assert_eq!(full_app(&e), Some(("mail".to_owned(), "Mail".to_owned())));
        assert_eq!(session_app(&e), Some(("mail".to_owned(), Some(json!({"call": ["gws_mail_list"]})))));
        let (inline_only, _) = plugin(json!([{"id": "card", "title": "Card", "placement": ["inline"]}]), "running", true, json!([]));
        assert_eq!(full_app(&inline_only), None);
        assert_eq!(session_app(&inline_only).map(|a| a.0).as_deref(), Some("card"));
        assert_eq!(session_app(&plugin(json!([]), "running", true, json!([])).0), None);
    }

    #[test]
    fn a_cards_text_is_a_few_plain_lines() {
        let t = text_of(&json!({"title": "Mail", "lines": ["Ada Lovelace — Lunch\non  Friday", "", 7, " Grace Hopper — Notes "], "at": 1_794_268_800_000i64})).unwrap();
        assert_eq!(t.lines, ["Ada Lovelace — Lunch on Friday", "Grace Hopper — Notes"]);
        assert_eq!(t.at, 1_794_268_800_000);
        let many: Vec<String> = (0..40).map(|n| format!("line {n}")).collect();
        assert_eq!(text_of(&json!({"lines": many})).unwrap().lines.len(), LINES_MAX);
        let long = text_of(&json!({"lines": ["x".repeat(500)]})).unwrap();
        assert_eq!(long.lines[0].chars().count(), LINE_MAX + 1);
        assert_eq!(text_of(&json!({"lines": []})), None);
        assert_eq!(text_of(&json!({"title": "Mail"})), None);
        assert_eq!(text_of(&json!("Mail")), None);
    }
}
