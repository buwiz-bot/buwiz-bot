//! The store's JSON as Node writes it: turns and bots read from objects
//! (a loaded row, the legacy JSON, a `meta` column), `turns.meta`'s fixed
//! key order, `/threads` and `/turns`' bodies (state.mjs `turnsJson`,
//! routes/chat.mjs), drive.mjs's `driveType`, and the JavaScript string
//! and number rules they lean on.
//!
//! Lone UTF-16 halves: `s.slice(0, n)` can cut an emoji in two, and Node
//! keeps the high half in memory (`/threads` shows `"\ud83d"`) while SQLite
//! stores U+FFFD. [`js_slice`] keeps such a half as a character of
//! U+10F800..U+10FFFF (private use), [`out`] writes it back as `\udXXX`,
//! and [`sql_text`] stores it as U+FFFD.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::{Bot, State, Thread, Turn};
use crate::js;

const LONE: u32 = 0x10F800;

/// `s.slice(0, n)` in UTF-16 units, a pair cut in half kept as its lone
/// first half (see the module's docs).
pub fn js_slice(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        let len = c.len_utf16();
        if units + len > n {
            if units < n {
                let high = 0xD800 + ((c as u32 - 0x10000) >> 10);
                out.push(char::from_u32(LONE + (high - 0xD800)).unwrap_or('\u{fffd}'));
            }
            break;
        }
        units += len;
        out.push(c);
    }
    out
}

/// A string's length in UTF-16 units (a lone half counts one).
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| if is_lone(c) { 1 } else { c.len_utf16() }).sum()
}

fn is_lone(c: char) -> bool {
    (LONE..=0x10FFFF).contains(&(c as u32))
}

/// A string as SQLite stores it from Node: a lone half becomes U+FFFD.
pub fn sql_text(s: &str) -> String {
    if !s.chars().any(is_lone) {
        return s.to_owned();
    }
    s.chars().map(|c| if is_lone(c) { '\u{fffd}' } else { c }).collect()
}

/// `JSON.stringify(v)`, lone halves as `\udXXX`.
pub fn out(v: &Value) -> String {
    let s = js::stringify(v);
    if !s.chars().any(is_lone) {
        return s;
    }
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if is_lone(c) {
            o.push_str(&format!("\\u{:04x}", 0xD800 + (c as u32 - LONE)));
        } else {
            o.push(c);
        }
    }
    o
}

/// `JSON.stringify(v)` as SQLite gets it (lone halves as U+FFFD).
pub fn sql_json(v: &Value) -> String {
    sql_text(&js::stringify(v))
}

/// `Number(s)` of a string.
pub fn js_number_str(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
    if t.is_empty() {
        return 0.0;
    }
    for (p, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(d) = t.strip_prefix(p) {
            if d.is_empty() || !d.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return d.chars().fold(0.0, |n, c| n * radix as f64 + c.to_digit(radix).unwrap_or(0) as f64);
        }
    }
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    if body == "Infinity" {
        return if t.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    // digits [. digits] [e [+-] digits], at least one digit before the e
    let b = body.as_bytes();
    let mut i = 0;
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return f64::NAN;
        }
    }
    if i != b.len() {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// `Number(v)`; `None` is `undefined`.
pub fn js_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => *b as u8 as f64,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_number_str(s),
        Some(a @ Value::Array(_)) => js_number_str(&js::string_of(a)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// `` `${v}` `` with `undefined` for `None`.
pub fn template(v: Option<&Value>) -> String {
    v.map(js::string_of).unwrap_or_else(|| "undefined".into())
}

/// `x?.length` is truthy: a non-empty array or string.
pub fn has_len(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        _ => false,
    }
}

pub fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(js::truthy)
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(js::string_of(other)),
    }
}

fn int_of(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

impl Turn {
    /// Sets one property as `turn[key] = value` would.
    pub fn set_field(&mut self, key: &str, v: Value) {
        match key {
            "role" => self.role = js::string_of(&v),
            "id" => self.id = text_of(&v),
            "text" => self.text = text_of(&v),
            "name" => {
                self.null_name = v.is_null();
                self.name = text_of(&v);
            }
            "label" => self.label = text_of(&v),
            "caption" => self.caption = text_of(&v),
            "reaction" => self.reaction = text_of(&v),
            "retries" => self.retries = int_of(&v),
            "v" => self.v = int_of(&v),
            "widget" => self.widget = js::truthy(&v),
            "reply" => self.reply = Some(v),
            "attachments" => self.attachments = Some(v),
            "files" => self.files = Some(v),
            "art" => self.art = Some(v),
            "links" => self.links = Some(v),
            "suggest" => self.suggest = Some(v),
            "v1Attachments" => self.v1_attachments = Some(v),
            "routine" => self.routine = Some(v),
            "notice" => self.notice = Some(v),
            "message" => self.message = Some(v),
            "pending" => self.pending = Some(v),
            "read_by" => self.read_by = Some(v),
            "agent" => self.agent = Some(v),
            "building" => self.building = js::truthy(&v),
            "status" => self.status = text_of(&v),
            "revising" => self.revising = js::truthy(&v),
            _ => {}
        }
    }

    /// `{…turn, ...extra}`: each of `extra`'s properties set on the turn.
    pub fn with_extra(mut self, extra: &Value) -> Self {
        if let Some(o) = extra.as_object() {
            for (k, v) in o {
                self.set_field(k, v.clone());
            }
        }
        self
    }

    /// `turns.meta` as `persist()` writes it, or `None` (NULL).
    pub fn meta_json(&self) -> Option<String> {
        let t = |v: &Option<Value>| truthy(v.as_ref());
        let l = |v: &Option<Value>| has_len(v.as_ref());
        let any = t(&self.reply)
            || l(&self.attachments)
            || l(&self.files)
            || t(&self.art)
            || l(&self.links)
            || l(&self.suggest)
            || l(&self.v1_attachments)
            || t(&self.routine)
            || t(&self.notice)
            || t(&self.message)
            || t(&self.agent);
        if !any {
            return None;
        }
        let mut m = Map::new();
        for (k, v) in [("reply", &self.reply), ("attachments", &self.attachments), ("files", &self.files), ("art", &self.art), ("links", &self.links)] {
            if let Some(v) = v {
                m.insert(k.into(), v.clone());
            }
        }
        let opt = [
            ("suggest", &self.suggest, l(&self.suggest)),
            ("v1Attachments", &self.v1_attachments, l(&self.v1_attachments)),
            ("routine", &self.routine, t(&self.routine)),
            ("notice", &self.notice, t(&self.notice)),
            ("message", &self.message, t(&self.message)),
            ("pending", &self.pending, l(&self.pending)),
            ("read_by", &self.read_by, l(&self.read_by)),
            ("agent", &self.agent, t(&self.agent)),
        ];
        for (k, v, on) in opt {
            if on {
                m.insert(k.into(), v.clone().unwrap_or(Value::Null));
            }
        }
        Some(sql_json(&Value::Object(m)))
    }
}

/// A turn from an object (the legacy turns.json, a test's seed).
pub fn turn_from(v: &Value) -> Turn {
    let mut t = Turn::default();
    if let Some(o) = v.as_object() {
        for (k, x) in o {
            t.set_field(k, x.clone());
        }
    }
    t
}

/// The bots table's `extra` keys, and the rest of a bot object.
pub fn bot_set(b: &mut Bot, key: &str, v: Value) {
    let s = |v: &Value| if js::truthy(v) { js::string_of(v) } else { String::new() };
    match key {
        "id" => b.id = js::string_of(&v),
        "name" => b.name = js::string_of(&v),
        "shape" => b.shape = js::string_of(&v),
        "color" => b.color = js::string_of(&v),
        "threadId" => b.thread_id = text_of(&v),
        "pinned" => b.pinned = js::truthy(&v),
        "section" => b.section = s(&v),
        "hidden" => b.hidden = js::truthy(&v),
        "image" => b.image = s(&v),
        "crop" => b.crop = v,
        "model" => b.model = s(&v),
        "effort" => b.effort = s(&v),
        "fast" => b.fast = js::truthy(&v),
        "label" => b.label = s(&v),
        "description" => b.description = text_of(&v),
        "instructions" => b.instructions = text_of(&v),
        "memories" => b.memories = v.as_array().cloned().unwrap_or_default(),
        "integrations" => b.integrations = v.as_array().cloned().unwrap_or_default(),
        "made_by" => b.made_by = Some(v),
        "purpose" => b.purpose = Some(v),
        "helper" => b.helper = Some(v),
        "accepts" => b.accepts = Some(v),
        "updated_at" => b.updated_at = v.as_f64(),
        _ => {}
    }
}

/// A bot from an object (the legacy threads.json's `bots`).
pub fn bot_from(v: &Value) -> Bot {
    let mut b = Bot::new("", "", "", "", None);
    b.description = None;
    b.instructions = None;
    if let Some(o) = v.as_object() {
        for (k, x) in o {
            bot_set(&mut b, k, x.clone());
        }
    }
    b
}

impl Bot {
    /// state.mjs `botExtra`: `{made_by, purpose, helper, accepts}`, or
    /// `None` when all four are undefined.
    pub fn extra_json(&self) -> Option<String> {
        let mut m = Map::new();
        for (k, v) in [("made_by", &self.made_by), ("purpose", &self.purpose), ("helper", &self.helper), ("accepts", &self.accepts)] {
            if let Some(v) = v {
                m.insert(k.into(), v.clone());
            }
        }
        (!m.is_empty()).then(|| sql_json(&Value::Object(m)))
    }
}

/// drive.mjs `driveType(mime, name)`: a Drive file's kind and type name.
pub fn drive_type(mime: Option<&Value>, name: Option<&Value>) -> (&'static str, String) {
    let mime = mime.filter(|m| js::truthy(m)).map(js::string_of).unwrap_or_default();
    let has = |words: &[&str]| words.iter().any(|w| mime.contains(w));
    let by_mime: [(bool, &str, &str); 10] = [
        (mime.ends_with("google-apps.document"), "document", "Google Doc"),
        (mime.ends_with("google-apps.spreadsheet"), "spreadsheet", "Google Sheet"),
        (mime.ends_with("google-apps.presentation"), "presentation", "Google Slides"),
        (has(&["wordprocessingml", "msword", "opendocument.text", "rtf"]), "document", "Word document"),
        (has(&["spreadsheetml", "ms-excel", "opendocument.spreadsheet", "text/csv", "tab-separated"]), "spreadsheet", "Spreadsheet"),
        (has(&["presentationml", "ms-powerpoint", "opendocument.presentation"]), "presentation", "Presentation"),
        (mime == "application/pdf", "pdf", "PDF"),
        (mime.starts_with("image/"), "image", "Image"),
        (mime.starts_with("audio/") || mime.starts_with("video/"), "media", "Media"),
        (mime.starts_with("text/"), "document", "Text"),
    ];
    if let Some((_, k, t)) = by_mime.iter().find(|(hit, _, _)| *hit) {
        return (k, (*t).to_owned());
    }
    let name = name.filter(|n| js::truthy(n)).map(js::string_of).unwrap_or_default();
    let ext = name.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric())).map(str::to_ascii_lowercase).unwrap_or_default();
    if !ext.is_empty() {
        let is = |xs: &[&str]| xs.contains(&ext.as_str());
        let by_ext: [(bool, &str, &str); 9] = [
            (is(&["doc", "docx", "odt", "rtf"]), "document", "Word document"),
            (is(&["xlsx", "xlsm", "xls", "csv", "tsv", "ods"]), "spreadsheet", "Spreadsheet"),
            (is(&["ppt", "pptx", "odp"]), "presentation", "Presentation"),
            (is(&["pdf"]), "pdf", "PDF"),
            (is(&["png", "jpg", "jpeg", "gif", "webp", "heic", "svg"]), "image", "Image"),
            (is(&["mp3", "wav", "m4a", "aac", "mov", "mp4", "m4v", "webm"]), "media", "Media"),
            (is(&["txt", "md", "markdown"]), "document", "Text"),
            (false, "", ""),
            (false, "", ""),
        ];
        if let Some((_, k, t)) = by_ext.iter().find(|(hit, _, _)| *hit) {
            return (k, (*t).to_owned());
        }
    }
    ("file", if ext.is_empty() { "File".to_owned() } else { ext.to_ascii_uppercase() })
}

/// What `turnsJson` reads from app.db: app titles, Edit Details' titles
/// and file names, and tokens' display names.
#[derive(Default)]
pub struct Lookups {
    pub app_titles: HashMap<String, Value>,
    /// id → (title, file)
    pub edits: HashMap<String, (Value, Value)>,
    pub renamed: HashMap<String, Value>,
}

fn cell(r: &rusqlite::Row, i: usize) -> Value {
    crate::hub::store::write::to_json(r.get_ref(i).map(rusqlite::types::Value::from).unwrap_or(rusqlite::types::Value::Null).clone())
}

impl Lookups {
    pub fn read(conn: &Connection) -> rusqlite::Result<Self> {
        let mut l = Lookups::default();
        let mut st = conn.prepare("SELECT name,title FROM apps")?;
        for r in st.query_map([], |r| Ok((js::string_of(&cell(r, 0)), cell(r, 1))))? {
            let (k, v) = r?;
            l.app_titles.insert(k, v);
        }
        let mut st = conn.prepare("SELECT id,title,file FROM artifact_meta")?;
        for r in st.query_map([], |r| Ok((js::string_of(&cell(r, 0)), cell(r, 1), cell(r, 2))))? {
            let (k, t, f) = r?;
            l.edits.insert(k, (t, f));
        }
        let mut st = conn.prepare("SELECT token,title FROM attachment_names")?;
        for r in st.query_map([], |r| Ok((js::string_of(&cell(r, 0)), cell(r, 1))))? {
            let (k, v) = r?;
            l.renamed.insert(k, v);
        }
        Ok(l)
    }
}

/// state.mjs `turnsJson(thread)`: the turns as the app renders them. It
/// may fill an old image card's `art` from its module (persisted with the
/// next write, as in Node).
pub fn turns_json(thread: &mut Thread, l: &Lookups, gen_dir: &Path) -> Value {
    // an image card made before cards carried their file
    for t in thread.turns.iter_mut() {
        let kind = t.art.as_ref().and_then(|a| a.get("kind")).is_some_and(js::truthy);
        if t.label.as_deref() == Some("image") && t.name.as_deref().is_some_and(|n| !n.is_empty()) && !kind {
            let name = t.name.clone().unwrap_or_default();
            if let Ok(code) = std::fs::read_to_string(gen_dir.join(format!("{name}.tsx"))) {
                if let Some(file) = asset_file(&code) {
                    let mut a = t.art.as_ref().and_then(Value::as_object).cloned().unwrap_or_default();
                    a.insert("kind".into(), json!("image"));
                    a.insert("file".into(), json!(file));
                    t.art = Some(Value::Object(a));
                }
            }
        }
    }
    let turns = &thread.turns;
    let art = |t: &Turn| -> Option<Value> {
        let empty = Map::new();
        let a = t.art.as_ref().filter(|a| js::truthy(a)).and_then(Value::as_object).unwrap_or(&empty);
        let get = |k: &str| a.get(k).cloned().unwrap_or(Value::Null);
        let name = t.name.as_deref().filter(|n| !n.is_empty());
        let mut title = Value::Null;
        if let Some(n) = name {
            let edit = l.edits.get(n).map(|e| e.0.clone()).filter(js::truthy);
            title = edit.or_else(|| l.app_titles.get(n).cloned()).unwrap_or(Value::Null);
        }
        if !js::truthy(&title) {
            title = get("title");
        }
        if !js::truthy(&title) {
            if let Some(n) = name {
                title = turns
                    .iter()
                    .find(|x| x.name.as_deref() == Some(n) && x.art.as_ref().and_then(|a| a.get("title")).is_some_and(js::truthy))
                    .and_then(|x| x.art.as_ref().and_then(|a| a.get("title")).cloned())
                    .unwrap_or(Value::Null);
            }
        }
        if !js::truthy(&title) {
            title = Value::Null;
        }
        let or0 = |k: &str| if js::truthy(&get(k)) { get(k) } else { json!(0) };
        if !js::truthy(&title) && !js::truthy(&get("desc")) && !js::truthy(&get("w")) && !js::truthy(&get("h")) {
            return None;
        }
        let mut o = Map::new();
        o.insert("title".into(), title);
        o.insert("desc".into(), if js::truthy(&get("desc")) { get("desc") } else { json!("") });
        o.insert("w".into(), or0("w"));
        o.insert("h".into(), or0("h"));
        if js::truthy(&get("kind")) {
            o.insert("kind".into(), get("kind"));
            o.insert("file".into(), if js::truthy(&get("file")) { get("file") } else { json!("") });
        }
        Some(Value::Object(o))
    };
    let s = |v: &Option<String>| json!(v.clone().unwrap_or_default());
    let arr = |v: &Option<Value>| v.clone().filter(js::truthy).unwrap_or(json!([]));
    let items: Vec<Value> = turns
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut o = Map::new();
            o.insert("i".into(), json!(i));
            o.insert("id".into(), json!(t.id.clone().unwrap_or_default()));
            o.insert("role".into(), json!(t.role));
            o.insert("text".into(), s(&t.text));
            o.insert("name".into(), s(&t.name));
            o.insert("label".into(), s(&t.label));
            o.insert("caption".into(), s(&t.caption));
            o.insert("reaction".into(), s(&t.reaction));
            o.insert("retries".into(), json!(t.retries.unwrap_or(0)));
            o.insert("widget".into(), json!(t.widget));
            o.insert("reply".into(), t.reply.clone().filter(js::truthy).unwrap_or(Value::Null));
            o.insert("attachments".into(), arr(&t.attachments));
            let files: Vec<Value> = arr(&t.files)
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|f| {
                    let path = f.get("path").filter(|p| js::truthy(p)).map(js::string_of);
                    let name = path.and_then(|p| l.edits.get(&format!("file:{p}"))).map(|e| e.1.clone()).filter(js::truthy);
                    match (name, f) {
                        (Some(n), Value::Object(mut m)) => {
                            m.insert("name".into(), n);
                            Value::Object(m)
                        }
                        (_, f) => f,
                    }
                })
                .collect();
            o.insert("files".into(), Value::Array(files));
            let links: Vec<Value> = arr(&t.links)
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|link| {
                    let mut m = link.as_object().cloned().unwrap_or_default();
                    m.shift_remove("account");
                    let orig_name = m.get("name").cloned();
                    if let Some(n) = m.get("token").filter(|t| js::truthy(t)).and_then(Value::as_str).and_then(|t| l.renamed.get(t)).filter(|n| js::truthy(n)).cloned() {
                        m.insert("name".into(), n);
                    }
                    if m.get("provider").and_then(Value::as_str) == Some("mcp-app") {
                        let ty = m.get("file_type").filter(|f| js::truthy(f)).cloned().unwrap_or(json!("Mini app"));
                        m.insert("kind".into(), json!("app"));
                        m.insert("type".into(), ty);
                    } else {
                        let (k, ty) = drive_type(m.get("mime"), orig_name.as_ref());
                        m.insert("kind".into(), json!(k));
                        m.insert("type".into(), json!(ty));
                    }
                    Value::Object(m)
                })
                .collect();
            o.insert("links".into(), Value::Array(links));
            o.insert("v".into(), t.v.map(|v| json!(v)).unwrap_or(Value::Null));
            o.insert("suggest".into(), arr(&t.suggest));
            if has_len(t.v1_attachments.as_ref()) {
                o.insert("v1Attachments".into(), t.v1_attachments.clone().unwrap_or(Value::Null));
            }
            for (k, v) in [("routine", &t.routine), ("notice", &t.notice), ("message", &t.message), ("agent", &t.agent)] {
                if truthy(v.as_ref()) {
                    o.insert(k.into(), v.clone().unwrap_or(Value::Null));
                }
            }
            if let Some(a) = art(t) {
                o.insert("art".into(), a);
            }
            if t.building {
                o.insert("building".into(), json!(true));
            }
            if let Some(st) = t.status.as_ref().filter(|s| !s.is_empty()) {
                o.insert("status".into(), json!(st));
            }
            if t.revising {
                o.insert("revising".into(), json!(true));
            }
            Value::Object(o)
        })
        .collect();
    Value::Array(items)
}

/// `/asset:\/generated\/assets\/([\w.-]+)/` in a module's code.
fn asset_file(code: &str) -> Option<String> {
    let at = code.find("asset:/generated/assets/")? + "asset:/generated/assets/".len();
    let file: String = code[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
    (!file.is_empty()).then_some(file)
}

/// The `/threads` preview: the latest text, markdown marks and runs of
/// space flattened, 160 UTF-16 units.
pub fn preview(t: Option<&Thread>) -> String {
    let last = t.and_then(|t| t.turns.iter().rev().find(|x| x.text.as_deref().is_some_and(|s| !s.is_empty()) || x.caption.as_deref().is_some_and(|s| !s.is_empty())));
    let text = last.and_then(|x| x.text.clone().filter(|s| !s.is_empty()).or_else(|| x.caption.clone().filter(|s| !s.is_empty()))).unwrap_or_default();
    let stripped: String = text.chars().filter(|c| !matches!(c, '#' | '*' | '`' | '>' | '_')).collect();
    js_slice(&squeeze(&stripped), 160)
}

/// `s.replace(/\s+/g, " ").trim()` (`\s` with JavaScript's line terminators).
fn squeeze(s: &str) -> String {
    let space = |c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if space(c) {
            gap = true;
        } else {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    out
}

/// `GET /threads`'s body. `model` is `MOCK ? "mock" : MODEL`;
/// `integrations` is botctx's `integrationsOf`.
pub fn threads_json(s: &State, model: &str, integrations: impl Fn(&Bot) -> Value) -> Value {
    let active = s.active_thread();
    let list: Vec<Value> = s
        .list
        .iter()
        .map(|t| {
            let bot = s.thread_bot(&t.id);
            let mut o = Map::new();
            o.insert("id".into(), json!(t.id));
            o.insert("uid".into(), json!(t.uid.clone().unwrap_or_default()));
            o.insert("title".into(), json!(t.title));
            o.insert("count".into(), json!(t.turns.len()));
            o.insert("hidden".into(), json!(t.hidden));
            o.insert("archived".into(), json!(t.archived));
            o.insert("lastApp".into(), json!(t.turns.iter().rev().find_map(|x| x.name.clone().filter(|n| !n.is_empty())).unwrap_or_default()));
            o.insert("bot".into(), bot.map(|b| json!({"id": b.id, "name": b.name, "shape": b.shape, "color": b.color})).unwrap_or(Value::Null));
            if truthy(t.routine.as_ref()) {
                o.insert("routine".into(), t.routine.clone().unwrap_or(Value::Null));
            }
            if truthy(t.agents.as_ref()) {
                o.insert("agents".into(), t.agents.clone().unwrap_or(Value::Null));
            }
            Value::Object(o)
        })
        .collect();
    let bots: Vec<Value> = s
        .bots
        .iter()
        .map(|b| {
            let t = s.list.iter().find(|x| Some(&x.id) == b.thread_id.as_ref());
            let mut o = Map::new();
            o.insert("id".into(), json!(b.id));
            o.insert("name".into(), json!(b.name));
            o.insert("shape".into(), json!(b.shape));
            o.insert("color".into(), json!(b.color));
            o.insert("threadId".into(), b.thread_id.as_ref().map(|x| json!(x)).unwrap_or(Value::Null));
            o.insert("pinned".into(), json!(b.pinned));
            o.insert("section".into(), json!(b.section));
            o.insert("hidden".into(), json!(b.hidden));
            o.insert("image".into(), json!(b.image));
            o.insert("crop".into(), if js::truthy(&b.crop) { b.crop.clone() } else { Value::Null });
            o.insert("model".into(), json!(b.model));
            o.insert("effort".into(), json!(b.effort));
            o.insert("fast".into(), json!(b.fast));
            o.insert("label".into(), json!(b.label));
            o.insert("description".into(), json!(b.description.clone().unwrap_or_default()));
            o.insert("instructions".into(), json!(b.instructions.clone().unwrap_or_default()));
            o.insert("memories".into(), Value::Array(b.memories.clone()));
            o.insert("integrations".into(), integrations(b));
            o.insert("updated_at".into(), b.updated_at.filter(|u| *u != 0.0 && !u.is_nan()).map(|u| json!(u)).unwrap_or(json!(0)));
            if truthy(b.made_by.as_ref()) {
                o.insert("made_by".into(), b.made_by.clone().unwrap_or(Value::Null));
                o.insert("purpose".into(), b.purpose.clone().filter(js::truthy).unwrap_or(json!("")));
            }
            if truthy(b.helper.as_ref()) {
                o.insert("helper".into(), b.helper.clone().unwrap_or(Value::Null));
            }
            if truthy(b.accepts.as_ref()) {
                o.insert("accepts".into(), b.accepts.clone().unwrap_or(Value::Null));
            }
            o.insert("preview".into(), json!(preview(t)));
            Value::Object(o)
        })
        .collect();
    let mut o = Map::new();
    o.insert("active".into(), json!(active.id));
    if let Some(uid) = &active.uid {
        o.insert("activeUid".into(), json!(uid));
    }
    o.insert("model".into(), json!(model));
    o.insert("list".into(), Value::Array(list));
    o.insert("sections".into(), if js::truthy(&s.sections) { s.sections.clone() } else { json!([]) });
    o.insert("bots".into(), Value::Array(bots));
    Value::Object(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_json_lone_halves_and_numbers() {
        let cut = js_slice("ab😀c", 3);
        assert_eq!(out(&json!(cut)), "\"ab\\ud83d\"");
        assert_eq!(sql_text(&cut), "ab\u{fffd}");
        assert_eq!(utf16_len(&cut), 3);
        assert_eq!(js_slice("ab😀c", 4), "ab😀");
        for (s, n) in [("", 0.0), (" 7 ", 7.0), ("0x10", 16.0), ("1e3", 1000.0), ("1.5", 1.5), (".5", 0.5), ("-2", -2.0)] {
            assert_eq!(js_number_str(s), n, "{s}");
        }
        for s in ["abc", "inf", "nan", "1e", "0x", "--1", "1 2"] {
            assert!(js_number_str(s).is_nan(), "{s}");
        }
        assert_eq!(js_number(Some(&json!([]))), 0.0);
        assert!(js_number(None).is_nan());
    }

    #[test]
    fn store_json_drive_types() {
        let t = |m: Option<&str>, n: Option<&str>| {
            let (k, ty) = drive_type(m.map(|m| json!(m)).as_ref(), n.map(|n| json!(n)).as_ref());
            (k.to_owned(), ty)
        };
        assert_eq!(t(Some("application/vnd.google-apps.document"), None), ("document".into(), "Google Doc".into()));
        assert_eq!(t(Some("text/csv"), None), ("spreadsheet".into(), "Spreadsheet".into()));
        assert_eq!(t(None, Some("a.PDF")), ("pdf".into(), "PDF".into()));
        assert_eq!(t(None, Some("a.zip")), ("file".into(), "ZIP".into()));
        assert_eq!(t(None, Some("noext")), ("file".into(), "File".into()));
    }
}
