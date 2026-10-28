//! v1 app cards in a chat (spec 6.5, 9.6, Q9).
//!
//! A tool result's `_meta["rox/open"]` reaches Rox as a card on the reply
//! (the broker adds it to the turn's `links`, provider `rox-app`, with the
//! checked open under `card`). The card has the title and **Open** (full)
//! or **Show** (inline); nothing opens or mounts until it is pressed
//! (INV-08, 9.7). An inline app is sized from its manifest's `inline`,
//! clamped to 240–960 × 96–800 and to the chat column, under a title bar
//! with Open (full screen). A user's own command, token or pin mounts an
//! inline app at once (Q9); a tool result or link waits for Show.

use serde_json::{Map, Value};

use super::{V1Key, perms};

/// Inline width and height ranges (5.5, Appendix A).
pub const WIDTH: (f32, f32) = (240.0, 960.0);
pub const HEIGHT: (f32, f32) = (96.0, 800.0);

/// How an open is placed (6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Full,
    Inline,
}

impl Placement {
    pub fn parse(s: &str) -> Option<Placement> {
        match s {
            "full" => Some(Placement::Full),
            "inline" => Some(Placement::Inline),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Placement::Full => "full",
            Placement::Inline => "inline",
        }
    }
}

/// A tool result's open, as its card shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCard {
    pub plugin: String,
    pub app: String,
    pub title: String,
    pub placement: Placement,
    pub ticket: Option<String>,
    pub props: Map<String, Value>,
}

impl OpenCard {
    /// The card a turn's link carries (`provider: "rox-app"`, its checked
    /// open under `card`); `None` for any other link, or a card that
    /// breaks the grammars (dropped, as the broker drops a bad one).
    pub fn from_link(link: &Value) -> Option<OpenCard> {
        if link.get("provider").and_then(Value::as_str) != Some("rox-app") {
            return None;
        }
        let c = link.get("card")?;
        let text = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_owned);
        let (plugin, app) = (text("plugin")?, text("app")?);
        if !perms::plugin_id_ok(&plugin) || !perms::app_id_ok(&app) {
            return None;
        }
        let ticket = match c.get("ticket") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) if perms::ticket_ok(t) => Some(t.clone()),
            Some(_) => return None,
        };
        Some(OpenCard {
            title: text("title").filter(|t| !t.is_empty()).unwrap_or_else(|| app.clone()).chars().take(60).collect(),
            placement: text("placement").and_then(|p| Placement::parse(&p)).unwrap_or(Placement::Full),
            props: c.get("props").and_then(Value::as_object).cloned().unwrap_or_default(),
            plugin,
            app,
            ticket,
        })
    }

    /// The module key it opens.
    pub fn key(&self) -> V1Key {
        V1Key { plugin: self.plugin.clone(), app: self.app.clone(), ticket: self.ticket.clone(), props: self.props.clone() }
    }

    /// Its button: Open for a full app, Show for an inline one (9.6).
    pub fn button(&self) -> &'static str {
        match self.placement {
            Placement::Full => "Open",
            Placement::Inline => "Show",
        }
    }
}

/// Who asked for an open: what decides whether an inline app mounts
/// before a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener {
    /// A command or token the person sent.
    User,
    /// A pin (library or dock).
    Pin,
    /// A tool result's card.
    ToolResult,
    /// A `rox://app/` link.
    Link,
}

/// Whether an inline open from `opener` mounts without a click (6.5, Q9).
pub fn mounts_at_once(opener: Opener) -> bool {
    matches!(opener, Opener::User | Opener::Pin)
}

/// The placement an app declares first (5.5: `placement[0]`, full when
/// absent).
pub fn declared(app: &Value) -> Placement {
    app.get("placement")
        .and_then(Value::as_array)
        .and_then(|p| p.first())
        .and_then(Value::as_str)
        .and_then(Placement::parse)
        .unwrap_or(Placement::Full)
}

/// An inline app's card size: its manifest's `inline.width`/`height`
/// clamped to the spec's ranges and then to `column` (the chat column's
/// width; the column wins over the minimum).
pub fn inline_size(app: &Value, column: f32) -> (f32, f32) {
    let inline = app.get("inline");
    let dim = |k: &str, (lo, hi): (f32, f32)| {
        inline.and_then(|i| i.get(k)).and_then(Value::as_f64).map_or(lo, |v| (v as f32).clamp(lo, hi))
    };
    let (w, h) = (dim("width", WIDTH), dim("height", HEIGHT));
    (if column > 0.0 { w.min(column) } else { w }, h)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const T: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

    fn link(card: Value) -> Value {
        json!({
            "provider": "rox-app", "id": "rox://app/dev.goldcoders.bir/forms", "name": "BIR Forms",
            "link": "rox://app/dev.goldcoders.bir/forms", "action": "open", "kind": "app", "type": "Mini app", "card": card,
        })
    }

    #[test]
    fn inv_08_a_tool_result_card_offers_open_or_show_and_never_opens_by_itself() {
        let full = OpenCard::from_link(&link(json!({
            "plugin": "dev.goldcoders.bir", "app": "forms", "props": {"form": "1601C"}, "ticket": T, "title": "BIR Forms", "placement": "full",
        })))
        .unwrap();
        assert_eq!(full.button(), "Open");
        assert_eq!(full.key().to_key(), format!("v1:dev.goldcoders.bir/forms?ticket={T}&props=%7B%22form%22%3A%221601C%22%7D"));
        let inline = OpenCard::from_link(&link(json!({"plugin": "dev.goldcoders.bir", "app": "calendar", "title": "Calendar", "placement": "inline"}))).unwrap();
        assert_eq!(inline.button(), "Show");
        // tool results and links wait for the press; the person's own
        // commands and pins do not
        assert!(!mounts_at_once(Opener::ToolResult));
        assert!(!mounts_at_once(Opener::Link));
        assert!(mounts_at_once(Opener::User));
        assert!(mounts_at_once(Opener::Pin));
        // other links are no cards; bad cards are dropped
        assert_eq!(OpenCard::from_link(&json!({"provider": "gdrive", "card": {"plugin": "dev.goldcoders.bir", "app": "forms"}})), None);
        assert_eq!(OpenCard::from_link(&link(json!({"plugin": "bir", "app": "forms"}))), None);
        assert_eq!(OpenCard::from_link(&link(json!({"plugin": "dev.goldcoders.bir", "app": "forms", "ticket": "123"}))), None);
    }

    #[test]
    fn inline_cards_take_the_declared_size_within_the_range_and_the_column() {
        let app = |w: f64, h: f64| json!({"placement": ["inline"], "inline": {"width": w, "height": h}});
        assert_eq!(inline_size(&app(480.0, 320.0), 700.0), (480.0, 320.0));
        assert_eq!(inline_size(&app(100.0, 20.0), 700.0), (240.0, 96.0));
        assert_eq!(inline_size(&app(2000.0, 2000.0), 2000.0), (960.0, 800.0));
        assert_eq!(inline_size(&app(960.0, 300.0), 560.0), (560.0, 300.0));
        assert_eq!(inline_size(&json!({}), 0.0), (240.0, 96.0));
        assert_eq!(declared(&app(1.0, 1.0)), Placement::Inline);
        assert_eq!(declared(&json!({"placement": ["full", "inline"]})), Placement::Full);
        assert_eq!(declared(&json!({})), Placement::Full);
    }
}
