//! A plugin's badge on its pins (spec 16.4, `_meta["buwiz/badge"]`): a dot in
//! one of three colours and up to 8 characters, set by the plugin on a tool
//! result and kept by the broker. The broker lists each plugin's badges
//! with the plugin (`badges`) and says each change as a `badge` event; this is
//! what the app reads from both, and what a pin says for one.
//!
//! The broker has judged a badge before it says it. What is read here is
//! read again with the same rules, so a pin never shows more than the
//! protocol allows, whatever arrives.

use std::collections::HashMap;

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

/// One badge: for the pins of `app`, or only those with `ticket`. `rev` is
/// the broker's number for the change (it only goes up), which orders an
/// event against a list; a cleared badge has dot `none`, no text and its own `rev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub app: String,
    pub ticket: Option<String>,
    pub dot: Dot,
    pub text: String,
    pub rev: i64,
}

impl Badge {
    /// A badge as the broker's list or an event carries it, `None` when it breaks
    /// the protocol's rules: an unknown dot, no `rev`, text over [`TEXT_MAX`]
    /// characters, with a hidden character (spec 4.2) or a space at either end.
    /// The text is read as it is; "" is no text.
    ///
    /// This reads what the broker sends, not what a plugin sends, so it may be looser
    /// than the broker's own rule on purpose: it has no manifest to judge the app by,
    /// a listed badge always has its dot and text spelled out, and "none" with no text
    /// is a clear here. What it must never do is draw a text the rule refuses, whoever
    /// sent it (a bidi override would show a status reversed), so the text's rules
    /// are the broker's, from the same shared definition (`buwiz_protocol::text`).
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
        if text.chars().count() > TEXT_MAX || text.chars().any(buwiz_protocol::text::hidden) || text.starts_with(' ') || text.ends_with(' ') {
            return None;
        }
        Some(Badge { app, ticket, dot, text: text.to_owned(), rev: v.get("rev")?.as_i64()? })
    }

    /// Shows nothing: no dot and no text, which is what a cleared badge is.
    pub fn is_empty(&self) -> bool {
        self.dot == Dot::None && self.text.is_empty()
    }
}

/// A plugin's `badges` as the list has them; one that breaks the rules is left out.
pub fn from_list(v: Option<&Value>) -> Vec<Badge> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Badge::from_json).collect()
}

/// A `badge` event: the plugin's id and its badge (empty when cleared).
pub fn from_event(v: &Value) -> Option<(String, Badge)> {
    Some((v.get("plugin")?.as_str().filter(|p| !p.is_empty())?.to_owned(), Badge::from_json(v)?))
}

type Key = (String, String, Option<String>);

/// What the app holds of every plugin's badges, with the `rev` of each change,
/// the clears included, so that an older plugin list can never bring back a
/// badge an event changed or cleared, and an older event cannot undo a list.
#[derive(Debug, Default)]
pub struct Book {
    held: HashMap<Key, Badge>,
}

impl Book {
    /// A change `b` of `plugin`'s: taken when it is newer than what is held for its
    /// app and ticket. Whether it was taken.
    pub fn event(&mut self, plugin: &str, b: Badge) -> bool {
        let key = (plugin.to_owned(), b.app.clone(), b.ticket.clone());
        if self.held.get(&key).is_some_and(|h| h.rev >= b.rev) {
            return false;
        }
        self.held.insert(key, b);
        true
    }

    /// `plugin`'s list, as of `at` (the highest `rev` the broker had issued when it made it):
    /// each listed badge is taken when it is newer than what is held, and a badge held
    /// from `at` or before that the list does not have is gone. What is held from after
    /// `at` stays: the list is older than that.
    pub fn list(&mut self, plugin: &str, at: i64, listed: Vec<Badge>) {
        let mut seen: Vec<Key> = Vec::new();
        for b in listed {
            seen.push((plugin.to_owned(), b.app.clone(), b.ticket.clone()));
            self.event(plugin, b);
        }
        self.held.retain(|k, h| k.0 != plugin || seen.contains(k) || h.rev > at);
    }

    /// The badges `plugin` has now, in the order of their app and ticket.
    pub fn badges_of(&self, plugin: &str) -> Vec<Badge> {
        let mut out: Vec<Badge> = self.held.iter().filter(|(k, b)| k.0 == plugin && !b.is_empty()).map(|(_, b)| b.clone()).collect();
        out.sort_by(|a, b| (&a.app, &a.ticket).cmp(&(&b.app, &b.ticket)));
        out
    }

    /// Forgets every plugin not in `listed`.
    pub fn keep(&mut self, listed: &[String]) {
        self.held.retain(|k, _| listed.contains(&k.0));
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

    fn badge(app: &str, ticket: Option<&str>, dot: Dot, text: &str, rev: i64) -> Badge {
        Badge { app: app.into(), ticket: ticket.map(Into::into), dot, text: text.into(), rev }
    }

    /// The vectors every side shares (`vectors/badge.json` of the SDK), as the broker would list a
    /// well formed one, and a text it refuses as it might arrive anyway.
    #[test]
    fn the_shared_vectors_read_as_the_broker_lists_them() {
        let v: Value = serde_json::from_str(include_str!("../../../broker/buwiz-protocol/src/badge-vectors.json")).unwrap();
        let (mut read, mut refused) = (0, 0);
        for c in v["cases"].as_array().unwrap() {
            let b = &c["badge"];
            let Some(o) = b.as_object().filter(|o| !o.contains_key("clear") && o.get("text").is_none_or(Value::is_string)) else { continue };
            let listed = json!({"app": "board", "ticket": null, "dot": o.get("dot").cloned().unwrap_or(json!("none")), "text": o.get("text").cloned().unwrap_or(json!("")), "rev": 1});
            let got = Badge::from_json(&listed);
            if c["ok"] == true {
                let got = got.unwrap_or_else(|| panic!("{b} is well formed"));
                assert_eq!(got.dot.word(), o.get("dot").and_then(Value::as_str).unwrap_or("none"), "{b}");
                assert_eq!(Some(got.text.as_str()), o.get("text").and_then(Value::as_str).or(Some("")), "{b}");
                read += 1;
            } else if o.get("dot").is_none_or(|d| d.as_str().is_some_and(|d| Dot::parse(d).is_some())) && o.contains_key("text") && o["text"] != "" {
                // a text the rule refuses: never read, so never drawn
                assert_eq!(got, None, "{b}");
                refused += 1;
            }
        }
        assert!(read > 5 && refused > 25, "{read} {refused}");
        assert_eq!(Badge::from_json(&json!({"app": "board", "dot": "red", "text": "\u{202e}2 due", "rev": 1})), None, "a bidi override is not drawn, even from the broker");
    }

    #[test]
    fn a_clear_missed_while_the_stream_was_down_is_applied_by_the_reload() {
        let mut book = Book::default();
        book.list("p", 10, vec![badge("forms", None, Dot::Red, "2", 8), badge("card", None, Dot::Green, "", 9)]);
        // the stream drops; the plugin clears forms (rev 12) and changes card (rev 14); the app hears neither
        // the reload that follows the stream's return has them
        book.list("p", 20, vec![badge("card", None, Dot::Amber, "x", 14)]);
        assert_eq!(texts(&book, "p"), ["card:None:x"]);
    }

    #[test]
    fn the_list_and_an_event_parse_alike() {
        let listed = from_list(Some(&json!([
            {"app": "forms", "ticket": null, "dot": "red", "text": "2 due", "rev": 5},
            {"app": "forms", "ticket": "tkt_ABC", "dot": "none", "text": "18h", "rev": 6},
            {"app": "forms", "ticket": "tkt_DEF", "dot": "green", "text": "", "rev": 7},
        ])));
        assert_eq!(listed, vec![badge("forms", None, Dot::Red, "2 due", 5), badge("forms", Some("tkt_ABC"), Dot::None, "18h", 6), badge("forms", Some("tkt_DEF"), Dot::Green, "", 7)]);
        let event = json!({"plugin": "app.buwiz.ebirforms", "app": "forms", "ticket": null, "dot": "amber", "text": "18h", "rev": 9});
        assert_eq!(from_event(&event), Some(("app.buwiz.ebirforms".into(), badge("forms", None, Dot::Amber, "18h", 9))));
        let cleared = json!({"plugin": "p", "app": "forms", "ticket": null, "dot": "none", "text": "", "rev": 10});
        assert!(from_event(&cleared).unwrap().1.is_empty());
        assert!(from_list(None).is_empty());
    }

    #[test]
    fn what_breaks_the_rules_is_not_read() {
        for bad in [
            json!({"app": "forms", "dot": "blue", "text": "", "rev": 1}),
            json!({"app": "forms", "dot": "red", "text": "nine char", "rev": 1}),
            json!({"app": "forms", "dot": "red", "text": "a\nb", "rev": 1}),
            json!({"app": "forms", "dot": "red", "text": "a\u{2028}b", "rev": 1}),
            json!({"app": "forms", "dot": "red", "text": " ok", "rev": 1}),
            json!({"app": "forms", "dot": "red", "text": "ok ", "rev": 1}),
            json!({"app": "forms", "text": "x", "rev": 1}),
            json!({"dot": "red", "text": "x", "rev": 1}),
            json!({"app": "forms", "dot": "red", "text": 8, "rev": 1}),
            json!({"app": "forms", "ticket": 4, "dot": "red", "rev": 1}),
            json!({"app": "forms", "dot": "red"}),
            json!("red"),
        ] {
            assert_eq!(Badge::from_json(&bad), None, "{bad}");
        }
        assert_eq!(Badge::from_json(&json!({"app": "forms", "dot": "red", "text": "8 chars!", "rev": 1})).unwrap().text, "8 chars!", "8 is the limit");
        assert_eq!(Badge::from_json(&json!({"app": "forms", "dot": "red", "text": "ÅÅÅÅÅÅÅÅ", "rev": 1})).unwrap().text.chars().count(), 8, "characters, not bytes");
    }

    fn texts(book: &Book, plugin: &str) -> Vec<String> {
        book.badges_of(plugin).iter().map(|b| format!("{}:{:?}:{}", b.app, b.ticket, b.text)).collect()
    }

    #[test]
    fn an_event_then_an_older_list_keeps_the_event() {
        let mut book = Book::default();
        book.list("p", 10, vec![badge("forms", None, Dot::Red, "2", 8)]);
        // the list asked for before the change comes back after the event
        assert!(book.event("p", badge("forms", None, Dot::Green, "3", 12)));
        book.list("p", 10, vec![badge("forms", None, Dot::Red, "2", 8)]);
        assert_eq!(texts(&book, "p"), ["forms:None:3"], "the older list did not undo the event");
        assert!(!book.event("p", badge("forms", None, Dot::Red, "2", 8)), "an old event is no change");
    }

    #[test]
    fn a_list_then_a_newer_event_takes_the_event_and_a_newer_list_the_list() {
        let mut book = Book::default();
        book.list("p", 10, vec![badge("forms", None, Dot::Red, "2", 8)]);
        assert!(book.event("p", badge("forms", Some("tkt_A"), Dot::Amber, "1h", 11)));
        assert_eq!(texts(&book, "p"), ["forms:None:2", "forms:Some(\"tkt_A\"):1h"]);
        // a newer list that has changed one and lost the other
        book.list("p", 20, vec![badge("forms", None, Dot::Red, "5", 15)]);
        assert_eq!(texts(&book, "p"), ["forms:None:5"], "what the newer list lacks is gone");
        // but a badge newer than the list stays when the list lacks it
        book.event("p", badge("card", None, Dot::Green, "", 30));
        book.list("p", 25, vec![badge("forms", None, Dot::Red, "5", 15)]);
        assert_eq!(texts(&book, "p"), ["card:None:", "forms:None:5"]);
    }

    #[test]
    fn a_clear_then_an_older_list_stays_cleared() {
        let mut book = Book::default();
        book.list("p", 10, vec![badge("forms", None, Dot::Red, "2", 8)]);
        assert!(book.event("p", badge("forms", None, Dot::None, "", 12)), "a clear");
        assert!(book.badges_of("p").is_empty());
        book.list("p", 10, vec![badge("forms", None, Dot::Red, "2", 8)]);
        assert!(book.badges_of("p").is_empty(), "an older list does not bring it back");
        // a newer set after the clear shows again, and a later list without it ends it
        assert!(book.event("p", badge("forms", None, Dot::Amber, "9", 14)));
        assert_eq!(texts(&book, "p"), ["forms:None:9"]);
        book.list("p", 20, vec![]);
        assert!(book.badges_of("p").is_empty());
        book.keep(&[]);
        assert!(book.held.is_empty());
    }

    #[test]
    fn a_pin_shows_its_ticket_badge_else_the_apps() {
        let list = vec![badge("forms", None, Dot::Green, "ok", 1), badge("forms", Some("tkt_A"), Dot::Red, "late", 2), badge("card", None, Dot::Amber, "", 3)];
        assert_eq!(for_pin(&list, "forms", Some("tkt_A")).map(|b| b.text.as_str()), Some("late"));
        assert_eq!(for_pin(&list, "forms", Some("tkt_B")).map(|b| b.text.as_str()), Some("ok"), "every pin of the app");
        assert_eq!(for_pin(&list, "forms", None).map(|b| b.text.as_str()), Some("ok"), "a pin without a ticket has the app's, never a ticket's");
        assert_eq!(for_pin(&list, "card", None).map(|b| b.dot), Some(Dot::Amber));
        assert_eq!(for_pin(&list, "other", None), None);
    }

    #[test]
    fn a_screen_reader_hears_the_state_in_words() {
        let say = |dot, text| spoken("Tax profiles", Some(&badge("forms", None, dot, text, 1)));
        assert_eq!(say(Dot::Red, "2 due"), "Tax profiles, status red, 2 due");
        assert_eq!(say(Dot::Amber, ""), "Tax profiles, status amber");
        assert_eq!(say(Dot::Green, "ok"), "Tax profiles, status green, ok");
        assert_eq!(say(Dot::None, "18h"), "Tax profiles, 18h");
        assert_eq!(say(Dot::None, ""), "Tax profiles");
        assert_eq!(spoken("Tax profiles", None), "Tax profiles");
    }
}
