//! `/prefs`: theme, model, effort, fast and the sidebar's two section
//! names, kept row by row in `meta` (server/src/routes/apps.mjs).

use rusqlite::OptionalExtension;
use serde_json::{Map, Value};

use super::{Answer, Leaf};
use crate::js;

fn meta_get(leaf: &Leaf, k: &str) -> Result<String, String> {
    leaf.db(|db| db.query_row("SELECT CAST(v AS TEXT) FROM meta WHERE k=?", [k], |r| r.get::<_, Option<String>>(0)).optional())
        .map(|v| v.flatten().unwrap_or_default())
}

fn meta_set(leaf: &Leaf, k: &str, v: &str) -> Result<(), String> {
    leaf.db(|db| db.execute("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)", [k, v])).map(|_| ())
}

pub fn get(leaf: &Leaf) -> Answer {
    let read = || -> Result<Map<String, Value>, String> {
        let or = |k: &str, default: &str| -> Result<Value, String> {
            let v = meta_get(leaf, k)?;
            Ok(Value::String(if v.is_empty() { default.to_owned() } else { v }))
        };
        let mut m = Map::new();
        m.insert("theme".into(), or("theme", "dark")?);
        m.insert("model".into(), or("model", "")?);
        m.insert("effort".into(), or("effort", "")?);
        m.insert("fast".into(), Value::Bool(meta_get(leaf, "fast")? == "1"));
        m.insert("unassigned_name".into(), or("unassigned_name", "Unassigned")?);
        m.insert("chats_name".into(), or("chats_name", "Chats")?);
        Ok(m)
    };
    match read() {
        Ok(m) => Answer::json(200, js::stringify(&Value::Object(m))),
        Err(e) => Answer::thrown(&e),
    }
}

pub fn post(leaf: &Leaf, body: &str) -> Answer {
    // Node parses outside any try: a body that isn't JSON, or null, throws
    // and takes Node down (decision D8). Here it is refused instead.
    let parsed = match super::parse_body(body) {
        Ok(Value::Null) => return Answer::refused(400, true, "Cannot destructure 'JSON.parse(...)' as it is null."),
        Ok(v) => v,
        Err(e) => return Answer::refused(400, true, &e),
    };
    let field = |k: &str| parsed.as_object().and_then(|m| m.get(k));
    let run = || -> Result<Option<Answer>, String> {
        if let Some(Value::String(theme)) = field("theme") {
            if ["system", "dark", "light"].contains(&theme.as_str()) {
                meta_set(leaf, "theme", theme)?;
            }
        }
        for k in ["model", "effort"] {
            if let Some(v) = field(k) {
                meta_set(leaf, k, &js::string_of(v))?;
            }
        }
        if let Some(v) = field("fast") {
            meta_set(leaf, "fast", if js::truthy(v) { "1" } else { "0" })?;
        }
        // a section keeps a name: a blank one is refused
        for k in ["unassigned_name", "chats_name"] {
            let Some(v) = field(k) else { continue };
            let n = js::slice_utf16(&js::squeeze(&js::string_of(v)), 60);
            if n.is_empty() {
                return Ok(Some(Answer::refused(400, true, "a section needs a name")));
            }
            meta_set(leaf, k, &n)?;
        }
        Ok(None)
    };
    match run() {
        Ok(Some(refused)) => refused,
        Ok(None) => Answer::json(200, "{\"ok\":true}"),
        Err(e) => Answer::thrown(&e),
    }
}
