//! `/state`: one JSON blob per widget or Rox store, in `kv`
//! (server/src/routes/apps.mjs). Accepts `/state/<name>`, `/state?name=`,
//! and bodies of `{name, state}` or the bare state.

use rusqlite::OptionalExtension;
use serde_json::Value;

use super::{Answer, Leaf};
use crate::js;

/// The name in the URL: `?name=` first, else the path after `/state/`,
/// decoded. `Err` where Node's decodeURIComponent throws.
fn state_name(url: &str) -> Result<String, ()> {
    if let Some(name) = js::search_param(url, "name").filter(|n| !n.is_empty()) {
        return Ok(name);
    }
    let path = js::pathname(url);
    let rest = path.strip_prefix("/state").map(|r| r.strip_prefix('/').unwrap_or(r)).unwrap_or(&path);
    js::decode_uri_component(rest)
}

/// `/^[\w.-]+$/`
fn valid(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

pub fn get(leaf: &Leaf, url: &str) -> Answer {
    let Ok(name) = state_name(url) else {
        return Answer::bare(500, "{\"error\":\"URI malformed\"}");
    };
    if !valid(&name) {
        return Answer::json(200, "{\"state\":null}");
    }
    let row = leaf.db(|db| db.query_row("SELECT CAST(data AS TEXT) FROM kv WHERE name=?", [&name], |r| r.get::<_, Option<String>>(0)).optional());
    let state = match row {
        Ok(data) => data.flatten().and_then(|d| serde_json::from_str::<Value>(&d).ok()).unwrap_or(Value::Null),
        Err(e) => return Answer::thrown(&e),
    };
    Answer::json(200, format!("{{\"state\":{}}}", js::stringify(&state)))
}

pub fn post(leaf: &Leaf, url: &str, body: &str) -> Answer {
    // errors are caught by the handler, which sets no content type on them
    let bad = |error: &str| Answer::refused(400, false, error);
    let parsed = match super::parse_body(body) {
        Ok(v) => v,
        Err(e) => return bad(&e),
    };
    let Ok(url_name) = state_name(url) else { return bad("URI malformed") };
    let name = if !url_name.is_empty() {
        Value::String(url_name)
    } else {
        match &parsed {
            Value::Null => return bad("Cannot read properties of null (reading 'name')"),
            Value::Object(m) => m.get("name").cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        }
    };
    let state = match &parsed {
        Value::Null => return bad("Cannot read properties of null (reading 'state')"),
        Value::Object(m) if m.contains_key("state") => m["state"].clone(),
        _ => parsed.clone(),
    };
    let shown = if js::truthy(&name) { js::string_of(&name) } else { String::new() };
    if !valid(&shown) {
        return bad("bad module name");
    }
    let data = if state.is_null() { "{}".to_owned() } else { js::stringify(&state) };
    // node:sqlite binds a string as text and a number as a real; nothing else
    let key: rusqlite::types::Value = match &name {
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
        _ => return bad("Provided value cannot be bound to SQLite parameter 1."),
    };
    match leaf.db(|db| db.execute("INSERT OR REPLACE INTO kv(name,data) VALUES(?,?)", rusqlite::params![key, data])) {
        Ok(_) => Answer::json(200, "{\"ok\":true}"),
        Err(e) => bad(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_comes_from_the_query_then_the_path() {
        assert_eq!(state_name("/state/rox-zoom"), Ok("rox-zoom".to_owned()));
        assert_eq!(state_name("/state?name=a&x=1"), Ok("a".to_owned()));
        assert_eq!(state_name("/state/b?name="), Ok("b".to_owned()));
        assert_eq!(state_name("/state"), Ok(String::new()));
        assert_eq!(state_name("/statefoo"), Ok("foo".to_owned()));
        assert_eq!(state_name("/state/leaf%2Dd"), Ok("leaf-d".to_owned()));
        assert_eq!(state_name("/state/.."), Ok("/".to_owned()));
        assert_eq!(state_name("/state/%E0%A4%A"), Err(()));
        assert!(valid("rox-main.bot_1"));
        assert!(!valid("no spaces") && !valid("") && !valid("é"));
    }
}
