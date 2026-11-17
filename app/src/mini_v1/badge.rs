//! A plugin's badge on its pins (spec 16.4, `_meta["buwiz/badge"]`): a dot in
//! one of three colours and up to 8 characters, set by the plugin with any
//! tool result and kept by the broker. The broker lists each plugin's badges
//! with the plugin (`badges`) and says each change as a `badge` event; this is
//! what the app reads from both, and what a pin says for one.
//!
//! The broker has judged a badge before it says it. What is read here is
//! read again with the same rules, so a pin never shows more than the
//! protocol allows, whatever arrives.

use serde_json::Value;

/// The most characters a badge's text has.
pub const TEXT_MAX: usize = 8;

/// A badge's dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    Red,
    Amber,
    Green,
    None,
}

impl Dot {
    pub fn parse(word: &str) -> Option<Dot> {
        Some(match word {
            "red" => Dot::Red,
            "amber" => Dot::Amber,
            "green" => Dot::Green,
            "none" => Dot::None,
            _ => return None,
        })
    }

    /// The word the protocol and a screen reader use.
    pub fn word(self) -> &'static str {
        match self {
            Dot::Red => "red",
            Dot::Amber => "amber",
            Dot::Green => "green",
            Dot::None => "none",
        }
    }
}

/// One badge: for the pins of `app`, or only those with `ticket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub app: String,
    pub ticket: Option<String>,
    pub dot: Dot,
    pub text: String,
}

impl Badge {
    /// A badge as the list or an event carries it, `None` when it breaks
    /// the protocol's rules: an unknown dot, text over [`TEXT_MAX`]
    /// characters or not on one line.
    pub fn from_json(v: &Value) -> Option<Badge> {
        let app = v.get("app")?.as_str().filter(|a| !a.is_empty())?.to_owned();
        let ticket = match v.get("ticket") {
            None | Some(Value::Null) => None,
            Some(t) => Some(t.as_str().filter(|t| !t.is_empty())?.to_owned()),
        };
        let dot = Dot::parse(v.get("dot")?.as_str()?)?;
        let text = match v.get("text") {
            None | Some(Value::Null) => "",
            Some(t) => t.as_str()?,
        };
        if text.chars().count() > TEXT_MAX || text.chars().any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}') {
            return None;
        }
        Some(Badge { app, ticket, dot, text: text.trim().to_owned() })
    }

    /// Shows nothing: no dot and no text, which is what a cleared badge is.
    pub fn is_empty(&self) -> bool {
        self.dot == Dot::None && self.text.is_empty()
    }
}

/// A plugin's `badges` as the list has them; one that breaks the rules is left out.
pub fn from_list(v: Option<&Value>) -> Vec<Badge> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Badge::from_json).filter(|b| !b.is_empty()).collect()
}

/// A `badge` event: the plugin's id and its badge (empty when cleared).
pub fn from_event(v: &Value) -> Option<(String, Badge)> {
    Some((v.get("plugin")?.as_str().filter(|p| !p.is_empty())?.to_owned(), Badge::from_json(v)?))
}

/// Puts `b` in `badges`, in place of the one for the same app and ticket; an
/// empty one takes it out. Whether anything changed.
pub fn apply(badges: &mut Vec<Badge>, b: Badge) -> bool {
    let at = badges.iter().position(|x| x.app == b.app && x.ticket == b.ticket);
    match (at, b.is_empty()) {
        (Some(i), true) => {
            badges.remove(i);
            true
        }
        (Some(i), false) if badges[i] != b => {
            badges[i] = b;
            true
        }
        (None, false) => {
            badges.push(b);
            true
        }
        _ => false,
    }
}

/// The badge a pin of `app` with `ticket` shows: the one for its ticket,
/// else the one for every pin of the app.
pub fn for_pin<'a>(badges: &'a [Badge], app: &str, ticket: Option<&str>) -> Option<&'a Badge> {
    let of = |t: Option<&str>| badges.iter().find(|b| b.app == app && b.ticket.as_deref() == t);
    ticket.and_then(|t| of(Some(t))).or_else(|| of(None))
}

/// What a screen reader says for a pin: its title, then the badge in words
/// ("Tax profiles, status red, 2 due"), so the colour is not the only signal.
pub fn spoken(title: &str, badge: Option<&Badge>) -> String {
    let Some(b) = badge.filter(|b| !b.is_empty()) else { return title.to_owned() };
    let mut out = title.to_owned();
    if b.dot != Dot::None {
        out.push_str(&format!(", status {}", b.dot.word()));
    }
    if !b.text.is_empty() {
        out.push_str(&format!(", {}", b.text));
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn badge(app: &str, ticket: Option<&str>, dot: Dot, text: &str) -> Badge {
        Badge { app: app.into(), ticket: ticket.map(Into::into), dot, text: text.into() }
    }

    #[test]
    fn the_list_and_an_event_parse_alike() {
        let listed = from_list(Some(&json!([
            {"app": "forms", "ticket": null, "dot": "red", "text": "2 due"},
            {"app": "forms", "ticket": "tkt_ABC", "dot": "none", "text": "18h"},
            {"app": "forms", "ticket": "tkt_DEF", "dot": "green", "text": ""},
        ])));
        assert_eq!(listed, vec![badge("forms", None, Dot::Red, "2 due"), badge("forms", Some("tkt_ABC"), Dot::None, "18h"), badge("forms", Some("tkt_DEF"), Dot::Green, "")]);
        let event = json!({"plugin": "app.buwiz.ebirforms", "app": "forms", "ticket": null, "dot": "amber", "text": "18h"});
        assert_eq!(from_event(&event), Some(("app.buwiz.ebirforms".into(), badge("forms", None, Dot::Amber, "18h"))));
        let cleared = json!({"plugin": "p", "app": "forms", "ticket": null, "dot": "none", "text": ""});
        assert!(from_event(&cleared).unwrap().1.is_empty());
        assert!(from_list(Some(&json!([{"app": "forms", "ticket": null, "dot": "none", "text": ""}]))).is_empty(), "a cleared one is not kept");
        assert!(from_list(None).is_empty());
    }

    #[test]
    fn what_breaks_the_rules_is_not_read() {
        for bad in [
            json!({"app": "forms", "dot": "blue", "text": ""}),
            json!({"app": "forms", "dot": "red", "text": "nine char"}),
            json!({"app": "forms", "dot": "red", "text": "a\nb"}),
            json!({"app": "forms", "dot": "red", "text": "a\u{2028}b"}),
            json!({"app": "forms", "text": "x"}),
            json!({"dot": "red", "text": "x"}),
            json!({"app": "forms", "dot": "red", "text": 8}),
            json!({"app": "forms", "ticket": 4, "dot": "red"}),
            json!("red"),
        ] {
            assert_eq!(Badge::from_json(&bad), None, "{bad}");
        }
        assert_eq!(Badge::from_json(&json!({"app": "forms", "dot": "red", "text": "8 chars!"})).unwrap().text, "8 chars!", "8 is the limit");
        assert_eq!(Badge::from_json(&json!({"app": "forms", "dot": "red", "text": "ÅÅÅÅÅÅÅÅ"})).unwrap().text.chars().count(), 8, "characters, not bytes");
    }

    #[test]
    fn a_change_replaces_the_badge_of_its_app_and_ticket_and_a_clear_removes_it() {
        let mut list = vec![badge("forms", None, Dot::Red, "2")];
        assert!(!apply(&mut list, badge("forms", None, Dot::Red, "2")), "the same again is no change");
        assert!(apply(&mut list, badge("forms", None, Dot::Green, "")));
        assert!(apply(&mut list, badge("forms", Some("tkt_A"), Dot::Amber, "1h")));
        assert_eq!(list, vec![badge("forms", None, Dot::Green, ""), badge("forms", Some("tkt_A"), Dot::Amber, "1h")]);
        assert!(apply(&mut list, badge("forms", None, Dot::None, "")));
        assert_eq!(list, vec![badge("forms", Some("tkt_A"), Dot::Amber, "1h")]);
        assert!(!apply(&mut list, badge("forms", None, Dot::None, "")), "clearing nothing changes nothing");
    }

    #[test]
    fn a_pin_shows_its_ticket_badge_else_the_apps() {
        let list = vec![badge("forms", None, Dot::Green, "ok"), badge("forms", Some("tkt_A"), Dot::Red, "late"), badge("card", None, Dot::Amber, "")];
        assert_eq!(for_pin(&list, "forms", Some("tkt_A")).map(|b| b.text.as_str()), Some("late"));
        assert_eq!(for_pin(&list, "forms", Some("tkt_B")).map(|b| b.text.as_str()), Some("ok"), "every pin of the app");
        assert_eq!(for_pin(&list, "forms", None).map(|b| b.text.as_str()), Some("ok"), "a pin without a ticket has the app's, never a ticket's");
        assert_eq!(for_pin(&list, "card", None).map(|b| b.dot), Some(Dot::Amber));
        assert_eq!(for_pin(&list, "other", None), None);
    }

    #[test]
    fn a_screen_reader_hears_the_state_in_words() {
        let say = |dot, text| spoken("Tax profiles", Some(&badge("forms", None, dot, text)));
        assert_eq!(say(Dot::Red, "2 due"), "Tax profiles, status red, 2 due");
        assert_eq!(say(Dot::Amber, ""), "Tax profiles, status amber");
        assert_eq!(say(Dot::Green, "ok"), "Tax profiles, status green, ok");
        assert_eq!(say(Dot::None, "18h"), "Tax profiles, 18h");
        assert_eq!(say(Dot::None, ""), "Tax profiles");
        assert_eq!(spoken("Tax profiles", None), "Tax profiles");
    }
}
