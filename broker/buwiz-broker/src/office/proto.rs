//! What the three formats share (agents/prototype.ts and types.ts):
//! `DocumentToolError`, the integer and text-window checks, the literal
//! search the agents run with a JavaScript `RegExp` (`gu`/`giu`), and the
//! proposals and cursors of `PrototypeDocument` (xlsx, pptx).

use serde_json::{json, Map, Value};

/// `DocumentToolError`, or a Node system error (`code` like `ENOENT`).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

impl ToolError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into(), details: None }
    }

    pub fn with(code: &str, message: impl Into<String>, details: Value) -> Self {
        Self { code: code.to_owned(), message: message.into(), details: Some(details) }
    }

    /// A Node `fs` error as `failure()` reports it: no details.
    pub fn system(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into(), details: None }
    }

    /// `failure(error)`'s body.
    pub fn body(&self) -> Value {
        let mut out = Map::new();
        out.insert("code".into(), json!(self.code));
        let message = if self.code == "EEXIST" { "Output already exists. Choose a new export path.".to_owned() } else { self.message.clone() };
        out.insert("message".into(), json!(message));
        if let Some(d) = &self.details {
            out.insert("details".into(), d.clone());
        }
        Value::Object(out)
    }
}

pub type Res<T> = Result<T, ToolError>;

/// `integer(value, min, max, name)`.
pub fn integer(value: f64, min: f64, max: f64, name: &str) -> Res<i64> {
    if value.fract() != 0.0 || !value.is_finite() || value.abs() > 9007199254740991.0 || value < min || value > max {
        return Err(ToolError::new("INVALID_ARGUMENT", format!("{name} must be an integer from {} to {}.", crate::js::number(min), crate::js::number(max))));
    }
    Ok(value as i64)
}

/// An optional number argument, `default` when absent.
pub fn opt_num(args: &Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(Value::as_f64).unwrap_or(default)
}

pub fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// UTF-16 units of `s`.
pub fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn string(u: &[u16]) -> String {
    String::from_utf16_lossy(u)
}

fn high(u: u16) -> bool {
    (0xd800..=0xdbff).contains(&u)
}

fn low(u: u16) -> bool {
    (0xdc00..=0xdfff).contains(&u)
}

/// `text.slice(a, b)` on UTF-16 units, clamped as JavaScript clamps.
pub fn slice(u: &[u16], a: usize, b: usize) -> String {
    let b = b.min(u.len());
    let a = a.min(b);
    string(&u[a..b])
}

/// `textWindow(text, {start, length})`.
pub fn text_window(text: &[u16], start: Option<f64>, length: Option<f64>) -> Res<Value> {
    let mut start = integer(start.unwrap_or(0.0), 0.0, text.len() as f64, "start")? as usize;
    let length = integer(length.unwrap_or(4000.0), 1.0, 16000.0, "length")? as usize;
    let split = |i: usize| i >= 1 && i < text.len() + 1 && high(text.get(i - 1).copied().unwrap_or(0)) && text.get(i).is_some_and(|u| low(*u));
    if split(start) {
        start -= 1;
    }
    let mut end = text.len().min(start + length);
    if split(end) {
        end -= 1;
    }
    if end == start && end < text.len() {
        end += 2;
    }
    Ok(json!({ "text": slice(text, start, end), "start": start, "end": end, "length": text.len(), "nextStart": if end < text.len() { json!(end) } else { Value::Null } }))
}

/// `boundary(text, offset)` of document.ts.
pub fn boundary(text: &[u16], offset: usize) -> bool {
    offset == 0 || offset == text.len() || !(text.get(offset - 1).is_some_and(|u| high(*u)) && text.get(offset).is_some_and(|u| low(*u)))
}

/// An unpaired surrogate in UTF-16 units.
pub fn lone_surrogate(u: &[u16]) -> bool {
    let mut i = 0;
    while i < u.len() {
        if high(u[i]) {
            if u.get(i + 1).is_some_and(|n| low(*n)) {
                i += 2;
                continue;
            }
            return true;
        }
        if low(u[i]) {
            return true;
        }
        i += 1;
    }
    false
}

/// A JSON string as JavaScript holds it: Rust strings are always valid
/// UTF-16 once decoded, so a lone surrogate never reaches here; JSON input
/// with one decodes to U+FFFD (serde_json) — the checks below see that.
pub fn xml_control(s: &str) -> bool {
    s.chars().any(|c| matches!(c as u32, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f))
}

/// `plainText(value)`.
pub fn plain_text(value: &str) -> Res<()> {
    if xml_control(value) || lone_surrogate(&units(value)) {
        return Err(ToolError::new("INVALID_ARGUMENT", "Text must contain complete Unicode characters and no XML control characters."));
    }
    Ok(())
}

/// Simple case folding of one character, as a `u` + `i` RegExp compares.
fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Every match of `query` (a literal) in `text`, as `[start, end)` UTF-16
/// offsets, the way `new RegExp(escaped, 'gu' | 'giu')` with `exec` finds
/// them from `from` on: left to right, never overlapping.
pub fn find_all(text: &[u16], query: &str, case_sensitive: bool, from: usize) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = {
        let mut out = Vec::new();
        let mut at = 0;
        for c in char::decode_utf16(text.iter().copied()) {
            let c = c.unwrap_or('\u{fffd}');
            out.push((at, c));
            at += c.len_utf16();
        }
        out
    };
    let q: Vec<char> = query.chars().map(|c| if case_sensitive { c } else { fold(c) }).collect();
    let mut hits = Vec::new();
    if q.is_empty() {
        return hits;
    }
    let mut i = chars.iter().position(|(at, _)| *at >= from).unwrap_or(chars.len());
    while i + q.len() <= chars.len() {
        let matched = q.iter().enumerate().all(|(k, qc)| {
            let c = chars[i + k].1;
            (if case_sensitive { c } else { fold(c) }) == *qc
        });
        if matched {
            let start = chars[i].0;
            let last = chars[i + q.len() - 1];
            hits.push((start, last.0 + last.1.len_utf16()));
            i += q.len();
        } else {
            i += 1;
        }
    }
    hits
}

/// JavaScript's `JSON.stringify(value).length`.
pub fn json_len(v: &Value) -> usize {
    crate::js::stringify(v).encode_utf16().count()
}

/// An insertion-ordered map with JavaScript `Map`'s semantics for what the
/// agents keep (ids, matches, cursors, proposals).
#[derive(Debug)]
pub struct Ordered<V> {
    items: Vec<(String, V)>,
}

impl<V> Default for Ordered<V> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<V> Ordered<V> {
    pub fn get(&self, k: &str) -> Option<&V> {
        self.items.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, k: &str) -> Option<&mut V> {
        self.items.iter_mut().find(|(key, _)| key == k).map(|(_, v)| v)
    }
    pub fn has(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
    /// `map.set(k, v)`: replaces in place, else appends.
    pub fn set(&mut self, k: String, v: V) {
        match self.items.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) => slot.1 = v,
            None => self.items.push((k, v)),
        }
    }
    pub fn delete(&mut self, k: &str) -> Option<V> {
        let i = self.items.iter().position(|(key, _)| key == k)?;
        Some(self.items.remove(i).1)
    }
    /// `map.delete(map.keys().next().value)`.
    pub fn delete_first(&mut self) {
        if !self.items.is_empty() {
            self.items.remove(0);
        }
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
    pub fn iter(&self) -> impl Iterator<Item = &(String, V)> {
        self.items.iter()
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.items.iter().map(|(_, v)| v)
    }
    pub fn retain(&mut self, mut f: impl FnMut(&str, &V) -> bool) {
        self.items.retain(|(k, v)| f(k, v));
    }
}

/// A random UUID (v4), as `crypto.randomUUID()` gives the prototypes'
/// handle prefix.
pub fn uuid() -> String {
    let b: [u8; 16] = buwiz_protocol::keys::random::<16>();
    let mut b = b;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// A JSON value's object keys sorted, recursively: what the engines' JSON
/// looks like when their `serde_json` builds maps as `BTreeMap`s (the wasm
/// builds); this crate turns `preserve_order` on, which keeps struct order.
pub fn sorted(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = m.into_iter().map(|(k, v)| (k, sorted(v))).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        v => v,
    }
}

/// An engine's `{ok, failure}` outcome, as `unwrap()` reads it.
pub fn unwrap(v: Value) -> Res<Value> {
    if v.get("ok") == Some(&Value::Bool(true)) {
        return Ok(v);
    }
    let failure = v.get("failure").cloned().unwrap_or(Value::Null);
    let code = failure.get("code").and_then(Value::as_str).unwrap_or("ENGINE_ERROR").to_owned();
    let message = failure.get("message").map(crate::js::string_of).unwrap_or_default();
    Err(ToolError::with(&code, message, failure))
}

/// An engine error as a thrown `Error` (`failure()` gives it ENGINE_ERROR).
pub fn engine<E: std::fmt::Display>(e: E) -> ToolError {
    ToolError::new("ENGINE_ERROR", e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_find_all_is_literal_and_folds_case_like_giu() {
        let t = units("Engine engine ENGINE");
        assert_eq!(find_all(&t, "engine", false, 0), vec![(0, 6), (7, 13), (14, 20)]);
        assert_eq!(find_all(&t, "engine", true, 0), vec![(7, 13)]);
        assert_eq!(find_all(&t, "engine", false, 7), vec![(7, 13), (14, 20)]);
        assert_eq!(find_all(&units("a.b a+b"), "a+b", true, 0), vec![(4, 7)]);
        assert_eq!(find_all(&units("aaa"), "aa", true, 0), vec![(0, 2)]);
    }

    #[test]
    fn office_text_window_keeps_surrogate_pairs_whole() {
        let t = units("ab😀cd");
        assert_eq!(text_window(&t, Some(3.0), Some(1.0)).unwrap()["start"], 2);
        assert_eq!(text_window(&t, Some(0.0), Some(3.0)).unwrap()["end"], 2);
        assert_eq!(text_window(&t, None, None).unwrap()["nextStart"], Value::Null);
        assert_eq!(text_window(&t, Some(9.0), None).unwrap_err().message, "start must be an integer from 0 to 6.");
    }
}
