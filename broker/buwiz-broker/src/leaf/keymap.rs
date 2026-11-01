//! `/keymap`: the user's keyboard shortcuts in `<data>/keymap.json`, a map
//! of command id → keystrokes (server/src/routes/apps.mjs). The file can be
//! edited by hand, so a GET reads it fresh.

use serde_json::{Map, Value};

use super::{Answer, Leaf};
use crate::js;

pub fn get(leaf: &Leaf) -> Answer {
    let map = std::fs::read_to_string(leaf.data.join("keymap.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    Answer::json(200, format!("{{\"keymap\":{}}}", js::stringify(&map)))
}

pub fn post(leaf: &Leaf, body: &str) -> Answer {
    match clean(body) {
        Ok(map) => match std::fs::write(leaf.data.join("keymap.json"), js::stringify_pretty(&Value::Object(map)) + "\n") {
            Ok(()) => Answer::json(200, "{\"ok\":true}"),
            Err(e) => Answer::refused(400, true, &e.to_string()),
        },
        Err(e) => Answer::refused(400, true, &e),
    }
}

/// The keymap a body holds, cleaned: ids checked, each key trimmed and
/// lowercased, blanks dropped, at most 4.
fn clean(body: &str) -> Result<Map<String, Value>, String> {
    let parsed = super::parse_body(body)?;
    let map = match &parsed {
        Value::Null => return Err("Cannot read properties of null (reading 'keymap')".to_owned()),
        Value::Object(m) => m.get("keymap"),
        _ => None,
    };
    let Some(Value::Object(map)) = map else {
        return Err("keymap must be an object".to_owned());
    };
    let mut out = Map::new();
    for (id, keys) in js::ordered(map) {
        let id_ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        let Some(keys) = keys.as_array().filter(|_| id_ok) else {
            return Err(format!("bad entry {id}"));
        };
        let keys: Vec<Value> = keys
            .iter()
            .map(|k| js::string_of(k).trim_matches(js::is_space).to_lowercase())
            .filter(|k| !k.is_empty())
            .take(4)
            .map(Value::String)
            .collect();
        out.insert(id.clone(), Value::Array(keys));
    }
    Ok(out)
}
