//! A mini app setup's checks, the broker's (the save route, the sign-in
//! guard) and the app's (as values are typed) alike. A field carries its
//! own (the broker's `google_fields`): `pattern` its trimmed value must
//! match, else `pattern_hint`; `not_pattern` it must not, else
//! `not_pattern_hint`; `swap_with` the field a value of the other's shape
//! was meant for, `swap_hint` what a swap is told. No message holds a
//! value.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use regex_lite::Regex;
use serde_json::{Map, Value};

/// Whether `v` matches `pattern`, each pattern compiled once (a bad one
/// never matches).
pub fn matches(pattern: &str, v: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Regex>>>> = OnceLock::new();
    let mut c = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    c.entry(pattern.to_owned()).or_insert_with(|| Regex::new(pattern).ok()).as_ref().is_some_and(|r| r.is_match(v))
}

/// Why `v` (trimmed, not empty) fails field `f`'s shape check, if it does.
pub fn shape_error(f: &Value, v: &str) -> Option<String> {
    let s = |k: &str| f.get(k).and_then(Value::as_str).filter(|x| !x.is_empty());
    if s("pattern").is_some_and(|p| !matches(p, v)) {
        return Some(s("pattern_hint").unwrap_or("This value has the wrong shape").to_owned());
    }
    if s("not_pattern").is_some_and(|p| matches(p, v)) {
        return Some(s("not_pattern_hint").unwrap_or("This value has the wrong shape").to_owned());
    }
    None
}

/// `values` (key → value as it would be stored) entered the wrong way
/// round: a field with `swap_with` whose value has the other field's shape
/// while the other's fails it. → its `swap_hint`.
pub fn swapped(fields: &[Value], values: &Map<String, Value>) -> Option<String> {
    let val = |k: &str| values.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    for f in fields {
        let Some(other) = f.get("swap_with").and_then(Value::as_str).filter(|o| !o.is_empty()) else { continue };
        let Some(o) = fields.iter().find(|x| x.get("key").and_then(Value::as_str) == Some(other)) else { continue };
        let (Some(mine), Some(theirs)) = (f.get("key").and_then(Value::as_str).and_then(val), val(other)) else { continue };
        let Some(p) = o.get("pattern").and_then(Value::as_str).filter(|p| !p.is_empty()) else { continue };
        if matches(p, mine) && !matches(p, theirs) {
            return Some(f.get("swap_hint").and_then(Value::as_str).unwrap_or("These two values look swapped").to_owned());
        }
    }
    None
}

/// What a provider's setup is called ("Google client").
pub fn setup_name(provider: &str) -> String {
    if provider == "google" { "Google client".to_owned() } else { format!("{provider} setup") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_field_checks_its_shape_and_a_pair_its_swap() {
        let id = json!({"key": "ID", "pattern": "^[0-9]+-x$", "pattern_hint": "An ID ends with -x"});
        let secret = json!({"key": "S", "not_pattern": "-x$", "not_pattern_hint": "Looks like an ID", "swap_with": "ID", "swap_hint": "Swapped"});
        assert_eq!(shape_error(&id, "12-x"), None);
        assert_eq!(shape_error(&id, "S-1").as_deref(), Some("An ID ends with -x"));
        assert_eq!(shape_error(&secret, "S-1"), None);
        assert_eq!(shape_error(&secret, "12-x").as_deref(), Some("Looks like an ID"));
        assert_eq!(shape_error(&json!({"key": "K", "pattern": "("}), "x").as_deref(), Some("This value has the wrong shape"), "a bad pattern never matches");
        let fields = [id, secret];
        let vals = |i: &str, s: &str| json!({"ID": i, "S": s}).as_object().unwrap().clone();
        assert_eq!(swapped(&fields, &vals("S-1", "12-x")).as_deref(), Some("Swapped"));
        assert_eq!(swapped(&fields, &vals("12-x", "S-1")), None);
        assert_eq!(swapped(&fields, &vals("12-x", "13-x")), None);
        assert_eq!(swapped(&fields, &vals("S-1", "")), None);
        // compiled once: the same answer again from the cache
        assert!(matches("^[0-9]+-x$", "12-x") && matches("^[0-9]+-x$", "12-x"));
        assert_eq!((setup_name("google"), setup_name("oauth:x")), ("Google client".to_owned(), "oauth:x setup".to_owned()));
    }
}
