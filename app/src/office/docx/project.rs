//! The engine's stories as the package's model, for the save: a port of the
//! web editor's `yrsToDocument` (`@betteroffice/docx` 0.4.3,
//! `chunk-ML6E23W2.mjs`), which projects the session onto the document it
//! was opened from.
//!
//! The model is JSON throughout, as the web's is: what JavaScript leaves
//! `undefined` is an absent key here, its `null` a `Value::Null`, and the
//! web's truthiness and loose equality are kept where its output depends on
//! them. Each paragraph is built again from its segments, its source
//! paragraph supplying what the engine doesn't hold (its text ID, raw XML);
//! pictures, fields and note marks come from their own embeds, so nothing is
//! matched by order. Page breaks the engine holds as embeds, which the web
//! drops, are kept (see [`Projector::story`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

pub(crate) type Obj = Map<String, Value>;

/// The key a rebuilt paragraph carries its engine paragraph in (the web
/// keeps it in a WeakMap); taken out before the model is written.
pub(crate) const SESSION_KEY: &str = "\u{0}session";

// ---- JavaScript's view of JSON values ----

fn obj(v: &Value) -> Option<&Obj> {
    v.as_object()
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}

fn st(v: &Value) -> Option<&str> {
    v.as_str()
}

/// `v` in a JavaScript condition.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn opt_truthy(v: Option<&Value>) -> bool {
    v.is_some_and(truthy)
}

/// `o[k] || undefined`.
fn or_undef<'a>(o: &'a Obj, k: &str) -> Option<&'a Value> {
    o.get(k).filter(|v| truthy(v))
}

/// `o[k] ?? undefined`.
fn nn_undef<'a>(o: &'a Obj, k: &str) -> Option<&'a Value> {
    o.get(k).filter(|v| !v.is_null())
}

/// `o[k] != null`.
fn present(o: &Obj, k: &str) -> bool {
    o.get(k).is_some_and(|v| !v.is_null())
}

/// `a !== b`: primitives by value, objects by reference (two different
/// objects always differ).
fn strict_ne(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => false,
        (Some(x), Some(y)) => match (x, y) {
            (Value::Null, Value::Null) => false,
            (Value::Bool(p), Value::Bool(q)) => p != q,
            (Value::Number(p), Value::Number(q)) => p.as_f64() != q.as_f64(),
            (Value::String(p), Value::String(q)) => p != q,
            _ => true,
        },
        _ => true,
    }
}

/// `JSON.stringify(a) === JSON.stringify(b)` (`undefined` stringifies to
/// nothing). The engine's objects come with sorted keys, so sorted keys
/// compare as the web's do.
fn same_json(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => canon(x) == canon(y),
        _ => false,
    }
}

/// `a === b || JSON.stringify(a) === JSON.stringify(b)`.
fn re(a: Option<&Value>, b: Option<&Value>) -> bool {
    !strict_ne(a, b) || same_json(a, b)
}

/// Sets `k` to `v`, or deletes it when `v` is `undefined`.
fn set(o: &mut Obj, k: &str, v: Option<Value>) {
    match v {
        Some(v) => {
            o.insert(k.to_owned(), v);
        }
        None => {
            o.remove(k);
        }
    }
}

/// A number as JSON, whole numbers as integers (as `JSON.stringify` writes them).
pub(crate) fn jnum(n: f64) -> Value {
    if !n.is_finite() {
        return Value::Null;
    }
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        return json!(n as i64);
    }
    json!(n)
}

/// `Math.round`: halves up.
fn js_round(n: f64) -> f64 {
    (n + 0.5).floor()
}

/// `Number(s)` for a string.
fn js_number_str(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).map(|n| n as f64).unwrap_or(f64::NAN);
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if !t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// `Number(v)`.
fn js_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_number_str(s),
        Some(_) => f64::NAN,
    }
}

/// Pixels to EMU (`pixelsToEmu`).
fn emu(px: f64) -> Value {
    jnum(js_round(px / 96.0 * 914_400.0))
}

/// A stable string for a value: keys sorted, `undefined` members dropped.
pub(crate) fn canon(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(canon).collect::<Vec<_>>().join(",")),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let items: Vec<String> = keys.into_iter().map(|k| format!("{}:{}", Value::String(k.clone()), canon(&o[k]))).collect();
            format!("{{{}}}", items.join(","))
        }
        Value::Number(n) => n.as_f64().map(jnum).unwrap_or(Value::Null).to_string(),
        v => v.to_string(),
    }
}

fn u16len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.slice(0, n)` and `s.slice(n)` in UTF-16 units (a pair isn't split).
fn u16split(s: &str, n: usize) -> (String, String) {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= n {
            return (s[..i].to_owned(), s[i..].to_owned());
        }
        units += c.len_utf16();
    }
    (s.to_owned(), String::new())
}

/// A yrs value as JSON (numbers as `jnum` writes them).
pub(crate) fn any_value(v: &yrs::Any) -> Value {
    use yrs::Any;
    match v {
        Any::Null | Any::Undefined => Value::Null,
        Any::Bool(b) => Value::Bool(*b),
        Any::Number(n) => jnum(*n),
        Any::BigInt(n) => json!(n),
        Any::String(s) => Value::String(s.to_string()),
        Any::Buffer(_) => Value::Null,
        Any::Array(a) => Value::Array(a.iter().map(any_value).collect()),
        Any::Map(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), any_value(v))).collect()),
    }
}

pub(crate) fn attrs_obj(a: &std::collections::BTreeMap<String, yrs::Any>) -> Obj {
    a.iter().map(|(k, v)| (k.clone(), any_value(v))).collect()
}

/// `q`: `null` members dropped, all the way down.
fn compact(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(compact).collect()),
        Value::Object(o) => Value::Object(o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), compact(v))).collect()),
        v => v.clone(),
    }
}

/// `H`: a unit's attributes without the hyperlink and revisions.
fn plain(a: &Obj) -> Obj {
    let mut n = a.clone();
    n.remove("hyperlink");
    n.remove("ins");
    n.remove("del");
    n
}

fn ty(v: &Value) -> &str {
    v["type"].as_str().unwrap_or("")
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

// ---- units: a story's segments with tabs split out ----

#[derive(Clone, Debug)]
pub(crate) enum Unit {
    Text { text: String, attrs: Obj },
    Embed { kind: String, payload: Obj, attrs: Obj },
}

impl Unit {
    fn attrs(&self) -> &Obj {
        match self {
            Unit::Text { attrs, .. } | Unit::Embed { attrs, .. } => attrs,
        }
    }
    fn attrs_mut(&mut self) -> &mut Obj {
        match self {
            Unit::Text { attrs, .. } | Unit::Embed { attrs, .. } => attrs,
        }
    }
    fn embed(&self, k: &str) -> bool {
        matches!(self, Unit::Embed { kind, .. } if kind == k)
    }
}

/// `{kind, text?, embedKind?, payload?, attrs}` units of an inline content
/// control's payload.
fn units_of(items: &[Value]) -> Vec<Unit> {
    let mut out = Vec::new();
    for l in items {
        let Some(s) = obj(l) else { continue };
        let Some(kind) = s.get("kind").and_then(st).filter(|k| !k.is_empty()) else { continue };
        let attrs = s.get("attrs").and_then(obj).cloned().unwrap_or_default();
        if kind == "text" {
            if let Some(t) = s.get("text").and_then(st) {
                out.push(Unit::Text { text: t.to_owned(), attrs });
            }
            continue;
        }
        out.push(Unit::Embed { kind: kind.to_owned(), payload: s.get("payload").and_then(obj).cloned().unwrap_or_default(), attrs });
    }
    out
}

// ---- runs ----

const FLAGS: [&str; 12] = ["bold", "italic", "superscript", "subscript", "allCaps", "smallCaps", "emboss", "imprint", "textShadow", "textOutline", "hidden", "rtl"];

/// `X`: a unit's marks as the model's run formatting.
fn run_formatting(e: &Obj) -> Obj {
    let mut n = Obj::new();
    let on = |k: &str| opt_truthy(e.get(k));
    if on("bold") {
        n.insert("bold".into(), json!(true));
    }
    if on("italic") {
        n.insert("italic".into(), json!(true));
    }
    let cs = |k: &str, base: &str| match e.get(k) {
        Some(v) if !v.is_null() => truthy(v),
        _ => on(base),
    };
    if cs("boldCs", "bold") {
        n.insert("boldCs".into(), json!(true));
    }
    if cs("italicCs", "italic") {
        n.insert("italicCs".into(), json!(true));
    }
    if let Some(t) = e.get("underline").and_then(obj)
        && t.get("inheritedHyperlink") != Some(&Value::Bool(true))
    {
        let style = t.get("style").and_then(st).filter(|s| !s.is_empty()).unwrap_or("single");
        n.insert("underline".into(), json!({ "style": style, "color": t.get("color").filter(|c| c.is_object()).cloned().unwrap_or(Value::Null) }));
    }
    if let Some(r) = e.get("strike").and_then(obj) {
        n.insert(if opt_truthy(r.get("double")) { "doubleStrike" } else { "strike" }.into(), json!(true));
    }
    if let Some(o) = e.get("textColor").and_then(obj)
        && o.get("inheritedHyperlink") != Some(&Value::Bool(true))
    {
        let s = |k: &str| o.get(k).and_then(st).map(|s| json!(s)).unwrap_or(Value::Null);
        n.insert("color".into(), json!({ "rgb": s("rgb"), "themeColor": nn_undef(o, "themeColor").cloned().unwrap_or(Value::Null), "themeTint": s("themeTint"), "themeShade": s("themeShade") }));
    }
    // the engine holds a highlight as `{color}` (the web reads only a
    // string, and drops it)
    if let Some(h) = e.get("highlight").and_then(|h| h.as_str().or_else(|| h.get("color")?.as_str())) {
        n.insert("highlight".into(), json!(h));
    }
    if let Some(i) = e.get("fontSize").and_then(obj) {
        if let Some(p) = i.get("size").and_then(num) {
            n.insert("fontSize".into(), jnum(p));
        }
        if let Some(c) = i.get("sizeCs").and_then(num) {
            n.insert("fontSizeCs".into(), jnum(c));
        }
    }
    if let Some(l) = e.get("fontFamily").and_then(obj) {
        let s = |k: &str| l.get(k).and_then(st).map(|s| json!(s));
        let mut f = Obj::new();
        f.insert("ascii".into(), s("ascii").unwrap_or(Value::Null));
        f.insert("hAnsi".into(), s("hAnsi").unwrap_or(Value::Null));
        set(&mut f, "eastAsia", s("eastAsia"));
        set(&mut f, "cs", s("cs"));
        f.insert("asciiTheme".into(), nn_undef(l, "asciiTheme").cloned().unwrap_or(Value::Null));
        set(&mut f, "hAnsiTheme", s("hAnsiTheme"));
        set(&mut f, "eastAsiaTheme", s("eastAsiaTheme"));
        set(&mut f, "csTheme", s("csTheme"));
        n.insert("fontFamily".into(), Value::Object(f));
    }
    if on("superscript") {
        n.insert("vertAlign".into(), json!("superscript"));
    }
    if on("subscript") {
        n.insert("vertAlign".into(), json!("subscript"));
    }
    if on("allCaps") {
        n.insert("allCaps".into(), json!(true));
    }
    if on("smallCaps") {
        n.insert("smallCaps".into(), json!(true));
    }
    if let Some(s) = e.get("characterSpacing").and_then(obj) {
        for k in ["spacing", "position", "scale", "kerning"] {
            if let Some(v) = s.get(k).and_then(num) {
                n.insert(k.into(), jnum(v));
            }
        }
    }
    if on("emboss") {
        n.insert("emboss".into(), json!(true));
    }
    if on("imprint") {
        n.insert("imprint".into(), json!(true));
    }
    if on("textShadow") {
        n.insert("shadow".into(), json!(true));
    }
    if let Some(a) = e.get("emphasisMark").and_then(obj) {
        n.insert("emphasisMark".into(), json!(a.get("type").and_then(st).filter(|s| !s.is_empty()).unwrap_or("dot")));
    }
    if on("textOutline") {
        n.insert("outline".into(), json!(true));
    }
    if on("hidden") {
        n.insert("hidden".into(), json!(true));
    }
    if on("rtl") {
        n.insert("rtl".into(), json!(true));
    }
    if let Some(u) = e.get("textEffect").and_then(obj) {
        n.insert("effect".into(), json!(u.get("effect").and_then(st).filter(|s| !s.is_empty()).unwrap_or("blinkBackground")));
    }
    if let Some(d) = e.get("modernTextEffects").and_then(obj)
        && opt_truthy(d.get("effects"))
    {
        n.insert("modernEffects".into(), d["effects"].clone());
    }
    if let Some(g) = e.get("runStyle").and_then(obj).and_then(|f| f.get("styleId")).and_then(st).filter(|s| !s.is_empty()) {
        n.insert("styleId".into(), json!(g));
    }
    n
}

/// `J`: text as run content, soft and non-breaking hyphens and symbol-font
/// characters as their own items.
fn text_content(text: &str, fmt: &Obj) -> Vec<Value> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let fonts: Vec<&str> = fmt
        .get("fontFamily")
        .and_then(obj)
        .map(|f| ["ascii", "hAnsi", "eastAsia", "cs"].iter().filter_map(|k| f.get(*k).and_then(st).filter(|s| !s.is_empty())).collect())
        .unwrap_or_default();
    let font = (!fonts.is_empty() && fonts.iter().all(|f| *f == fonts[0])).then(|| fonts[0]);
    let flush = |buf: &mut String, out: &mut Vec<Value>| {
        if !buf.is_empty() {
            out.push(json!({ "type": "text", "text": std::mem::take(buf) }));
        }
    };
    for c in text.chars() {
        let cp = c as u32;
        if c == '\u{AD}' {
            flush(&mut buf, &mut out);
            out.push(json!({ "type": "softHyphen" }));
        } else if c == '\u{2011}' {
            flush(&mut buf, &mut out);
            out.push(json!({ "type": "noBreakHyphen" }));
        } else if let Some(f) = font.filter(|_| (0xF000..=0xF8FF).contains(&cp)) {
            flush(&mut buf, &mut out);
            out.push(json!({ "type": "symbol", "font": f, "char": format!("{cp:04X}") }));
        } else {
            buf.push(c);
        }
    }
    flush(&mut buf, &mut out);
    out
}

fn with_formatting(mut run: Value, f: Obj) -> Value {
    if !f.is_empty() {
        run["formatting"] = Value::Object(f);
    }
    run
}

/// `Q`: a text unit's run.
fn text_run(text: &str, attrs: &Obj) -> Value {
    let f = run_formatting(&plain(attrs));
    let content = text_content(text, &f);
    with_formatting(json!({ "type": "run", "content": content }), f)
}

/// `sn`: `m`'s content onto `o`, adjacent texts joined.
fn join_run(o: &mut Value, m: Value) {
    let content = o["content"].as_array_mut().expect("a run's content");
    for t in m["content"].as_array().cloned().unwrap_or_default() {
        if let Some(last) = content.last_mut()
            && last["type"] == "text"
            && t["type"] == "text"
        {
            let joined = format!("{}{}", last["text"].as_str().unwrap_or(""), t["text"].as_str().unwrap_or(""));
            last["text"] = json!(joined);
        } else {
            content.push(t);
        }
    }
}

/// `an`: FNV-1a, to 53 bits.
fn fnv53(s: &str) -> f64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (h & ((1u64 << 53) - 1)) as f64
}

/// `de`: a revision or note id as a number.
fn numeric_id(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) if n.as_f64().is_some_and(f64::is_finite) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => {
            let n = js_number_str(s);
            if n.is_finite() { n } else { fnv53(s) }
        }
        _ => 0.0,
    }
}

/// `K`: a revision's info.
fn revision_info(v: Option<&Value>, id: Option<f64>) -> Option<Value> {
    let r = obj(v?)?;
    let author = r.get("author").and_then(st).filter(|s| !s.is_empty()).unwrap_or("Unknown");
    let id = id.unwrap_or_else(|| numeric_id(nn_undef(r, "revisionId").or_else(|| r.get("id"))));
    let mut info = json!({ "id": jnum(id), "author": author });
    if let Some(d) = r.get("date").and_then(st).filter(|s| !s.is_empty()) {
        info["date"] = json!(d);
    }
    Some(info)
}

/// `un`: the hyperlink a unit is in.
fn hyperlink_of(a: &Obj) -> Option<Value> {
    let n = a.get("hyperlink").and_then(obj)?;
    let href = n.get("href").and_then(st).unwrap_or("");
    let tooltip = n.get("tooltip").and_then(st).filter(|s| !s.is_empty());
    let mut h = Obj::new();
    h.insert("type".into(), json!("hyperlink"));
    if let Some(anchor) = href.strip_prefix('#') {
        h.insert("anchor".into(), json!(anchor));
        set(&mut h, "tooltip", tooltip.map(|t| json!(t)));
    } else {
        h.insert("href".into(), json!(href));
        set(&mut h, "tooltip", tooltip.map(|t| json!(t)));
        set(&mut h, "rId", n.get("rId").and_then(st).filter(|s| !s.is_empty()).map(|s| json!(s)));
    }
    h.insert("children".into(), json!([]));
    Some(Value::Object(h))
}

/// `cn`: a unit's hyperlink target, `None` outside a hyperlink.
fn href_of(a: &Obj) -> Option<String> {
    let n = a.get("hyperlink").and_then(obj)?;
    Some(n.get("href").and_then(st).unwrap_or("").to_owned())
}

fn is_revision(v: &Value) -> bool {
    matches!(ty(v), "insertion" | "deletion" | "moveFrom" | "moveTo")
}

const INLINE: [&str; 6] = ["run", "hyperlink", "simpleField", "complexField", "inlineSdt", "mathEquation"];

/// `ue`: the items an inline content control can hold.
fn sdt_items(v: Vec<Value>) -> Vec<Value> {
    v.into_iter().filter(|n| INLINE.contains(&ty(n)) || is_revision(n)).collect()
}

/// `pn`.
fn inline_items(v: Vec<Value>) -> Vec<Value> {
    v.into_iter().filter(|n| INLINE.contains(&ty(n))).collect()
}

/// `b$1`: whether an inline item holds a revision inside a content control.
fn has_sdt_revision(t: &Value) -> bool {
    fn walk(t: &Value, mut in_sdt: bool, mut in_rev: bool, found: &mut bool) {
        if ty(t) == "inlineSdt" {
            in_sdt = true;
        }
        if is_revision(t) {
            in_rev = true;
        }
        if in_sdt && in_rev {
            *found = true;
        }
        let kids: Vec<&Value> = match ty(t) {
            "inlineSdt" | "insertion" | "deletion" | "moveFrom" | "moveTo" => arr(&t["content"]).iter().collect(),
            "hyperlink" => arr(t.get("structuredChildren").unwrap_or(&t["children"])).iter().collect(),
            "simpleField" => arr(t.pointer("/structuredResult/inline").unwrap_or(&t["content"])).iter().collect(),
            "complexField" => {
                for p in arr(t.pointer("/structuredCode/inline").unwrap_or(&t["fieldCode"])) {
                    if !(ty(p) == "run" && !(in_sdt && in_rev)) {
                        walk(p, in_sdt, in_rev, found);
                    }
                }
                arr(t.pointer("/structuredResult/inline").unwrap_or(&t["fieldResult"])).iter().collect()
            }
            _ => return,
        };
        for p in kids {
            if !(ty(p) == "run" && !(in_sdt && in_rev)) {
                walk(p, in_sdt, in_rev, found);
            }
        }
    }
    let mut found = false;
    walk(t, false, false, &mut found);
    found
}

/// The projection of one session onto the document it was opened from.
pub(crate) struct Projector<'a> {
    doc: &'a docx_edit::EditingDoc,
    story_ids: HashSet<String>,
    base_stories: HashMap<String, Vec<Value>>,
    base_paragraphs: HashMap<String, Value>,
    comments: HashMap<String, Vec<(f64, u32, u32)>>,
    styles: Value,
    synthetic_defaults: bool,
    inherited: RefCell<HashMap<String, Option<Obj>>>,
    /// The first error the web throws on (a malformed horizontal rule).
    error: RefCell<Option<String>>,
}

impl<'a> Projector<'a> {
    /// `package` is the parsed package (the web's `document.package`) with
    /// the comments to save; `session_ids` the engine's ids of comments the
    /// model numbers differently.
    pub(crate) fn new(doc: &'a docx_edit::EditingDoc, package: &Value, session_ids: &HashMap<i64, String>) -> Projector<'a> {
        let story_ids = doc.list_stories().stories.into_iter().map(|s| s.story).collect();
        let base_stories = base_stories(package);
        let base_paragraphs = base_paragraphs(&base_stories);
        let mut comments: HashMap<String, Vec<(f64, u32, u32)>> = HashMap::new();
        for c in arr(&package["document"]["comments"]) {
            let Some(id) = c["id"].as_f64() else { continue };
            let sid = session_ids.get(&(id as i64)).cloned().unwrap_or_else(|| jnum(id).to_string());
            let Ok(ranges) = doc.resolve_comment(&sid) else { continue };
            let mut by: Vec<(String, u32, u32)> = Vec::new();
            for r in ranges {
                match by.iter_mut().find(|(s, _, _)| *s == r.story) {
                    Some(e) => {
                        e.1 = e.1.min(r.start);
                        e.2 = e.2.max(r.end);
                    }
                    None => by.push((r.story.clone(), r.start, r.end)),
                }
            }
            for (story, start, end) in by {
                comments.entry(story).or_default().push((id, start, end));
            }
        }
        let styles = package.get("styles").cloned().unwrap_or(Value::Null);
        let synthetic_defaults = default_style(&styles).is_none() && styles.get("docDefaults").is_none_or(Value::is_null);
        Projector { doc, story_ids, base_stories, base_paragraphs, comments, styles, synthetic_defaults, inherited: Default::default(), error: RefCell::new(None) }
    }

    /// `_n`: the package with every story the session holds projected.
    pub(crate) fn package(&self, package: &Value) -> Result<Value, String> {
        let mut out = package.clone();
        if self.story_ids.contains("body") {
            out["document"]["content"] = Value::Array(self.story("body"));
        }
        for key in ["headerEntries", "footerEntries"] {
            if let Some(entries) = out[key].as_array_mut() {
                for e in entries {
                    let Some(id) = e[0].as_str().map(str::to_owned) else { continue };
                    let story = format!("hf:{id}");
                    if self.story_ids.contains(&story) {
                        e[1]["content"] = Value::Array(self.story(&story));
                    }
                }
            }
        }
        for (key, prefix) in [("footnotes", "fn:"), ("endnotes", "en:")] {
            if let Some(notes) = out[key].as_array_mut() {
                for m in notes {
                    let story = format!("{prefix}{}", m["id"].as_f64().map(jnum).unwrap_or(Value::Null));
                    if self.story_ids.contains(&story) {
                        let blocks = with_note_marks(self.story(&story), arr(&m["content"]));
                        m["content"] = Value::Array(blocks);
                        if let Some(o) = m.as_object_mut() {
                            o.remove("verbatimXml");
                        }
                    }
                }
            }
        }
        match self.error.borrow_mut().take() {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    fn fail(&self, e: &str) {
        self.error.borrow_mut().get_or_insert_with(|| e.to_owned());
    }

    fn media(&self, src: &str) -> String {
        if src.starts_with("media:")
            && let Some(url) = self.doc.media_table().and_then(|t| t.resolve(src))
        {
            return url;
        }
        src.to_owned()
    }

    /// `_`: every `src` naming a media token as its data URL.
    fn resolve_media(&self, v: &mut Value) {
        match v {
            Value::Array(a) => a.iter_mut().for_each(|v| self.resolve_media(v)),
            Value::Object(o) => {
                for (k, v) in o.iter_mut() {
                    match v {
                        Value::String(s) if k == "src" => *s = self.media(s),
                        v => self.resolve_media(v),
                    }
                }
            }
            _ => {}
        }
    }

    fn parse_json(&self, s: Option<&str>, limit: usize) -> Option<Value> {
        let s = s.filter(|s| !s.is_empty() && s.len() <= limit)?;
        let mut v: Value = serde_json::from_str(s).ok()?;
        if s.contains("\"media:") {
            self.resolve_media(&mut v);
        }
        Some(v)
    }

    /// `he`: a field from its embed.
    fn field(&self, p: &Obj, attrs: &Obj) -> Value {
        if let Some(mut d) = self.parse_json(p.get("fieldData").and_then(st), 2_000_000) {
            let kind = ty(&d).to_owned();
            let f = if kind == "simpleField" { &d["content"] } else { &d["fieldResult"] };
            if (kind == "simpleField" || kind == "complexField") && d["instruction"].is_string() && f.is_array() {
                if let Some(g) = p.get("displayMode").and_then(st).filter(|s| !s.is_empty())
                    && opt_truthy(d.get("fieldTree"))
                {
                    d["fieldTree"]["displayMode"] = json!(g);
                }
                return d;
            }
        }
        let r = run_formatting(&plain(attrs));
        let o = with_formatting(json!({ "type": "run", "content": [{ "type": "text", "text": p.get("displayText").and_then(st).unwrap_or("") }] }), r);
        let s = |k: &str, d: &str| p.get(k).and_then(st).filter(|s| !s.is_empty()).unwrap_or(d).to_owned();
        let mut f = Obj::new();
        let complex = p.get("fieldKind").and_then(st) == Some("complex");
        f.insert("type".into(), json!(if complex { "complexField" } else { "simpleField" }));
        f.insert("instruction".into(), json!(s("instruction", "")));
        f.insert("fieldType".into(), json!(s("fieldType", "UNKNOWN")));
        if complex {
            f.insert("fieldCode".into(), json!([]));
            f.insert("fieldResult".into(), json!([o]));
        } else {
            f.insert("content".into(), json!([o]));
        }
        set(&mut f, "fldLock", (p.get("fldLock") == Some(&Value::Bool(true))).then(|| json!(true)));
        set(&mut f, "dirty", (p.get("dirty") == Some(&Value::Bool(true))).then(|| json!(true)));
        f.insert("fieldTree".into(), json!({ "version": 1, "displayMode": s("displayMode", "result") }));
        Value::Object(f)
    }

    /// `_e`: a field standing for a comment's reference mark.
    fn comment_reference(p: &Obj) -> Option<Value> {
        if p.get("modelKind").and_then(st) != Some("commentReference") {
            return None;
        }
        let mut r = json!({ "type": "commentReference" });
        if let Some(n) = p.get("commentId").and_then(num) {
            r["id"] = jnum(n);
        }
        Some(json!({ "type": "run", "content": [r] }))
    }

    fn math(p: &Obj) -> Value {
        let s = |k: &str| p.get(k).and_then(st).filter(|s| !s.is_empty());
        let mut m = json!({ "type": "mathEquation", "display": s("display").unwrap_or("inline"), "ommlXml": s("ommlXml").unwrap_or("") });
        if let Some(t) = s("plainText") {
            m["plainText"] = json!(t);
        }
        m
    }

    /// `ye`: a horizontal rule.
    fn rule(&self, p: &Obj, attrs: &Obj) -> Option<Value> {
        let t = p.get("rule").and_then(obj);
        let ok = t.is_some_and(|t| {
            t.get("height").and_then(num).is_some()
                && (!present(t, "width") || t.get("width").and_then(num).is_some())
                && (!present(t, "widthPercent") || t.get("widthPercent").and_then(num).is_some())
                && t.get("alignment").is_some_and(Value::is_string)
                && t.get("noShade").is_some_and(Value::is_boolean)
                && t.get("color").is_some_and(Value::is_string)
                && t.get("xml").is_some_and(Value::is_string)
        });
        let Some(t) = t.filter(|_| ok) else {
            self.fail("Malformed horizontalRule embed payload");
            return None;
        };
        let n = |k: &str| t.get(k).and_then(num).map(jnum).unwrap_or(Value::Null);
        let o = json!({ "width": n("width"), "widthPercent": n("widthPercent"), "height": n("height"), "alignment": t["alignment"], "noShade": t["noShade"], "color": t["color"], "xml": t["xml"] });
        Some(with_formatting(json!({ "type": "run", "content": [{ "type": "horizontalRule", "rule": o }] }), run_formatting(&plain(attrs))))
    }

    /// `Le`: a picture, inline or floating, from its embed's geometry.
    fn image(&self, n: &Obj) -> Value {
        let s = |k: &str| n.get(k).and_then(st).filter(|s| !s.is_empty());
        let mut wrap = Obj::new();
        wrap.insert("type".into(), json!(s("wrapType").unwrap_or("inline")));
        for (k, w) in [("distTop", "distT"), ("distBottom", "distB"), ("distLeft", "distL"), ("distRight", "distR")] {
            if n.contains_key(k) {
                wrap.insert(w.into(), emu(js_number(n.get(k))));
            }
        }
        if opt_truthy(n.get("wrapText")) {
            wrap.insert("wrapText".into(), n["wrapText"].clone());
        }
        let size = |k: &str| {
            let v = js_number(n.get(k));
            emu(if v.is_nan() || v == 0.0 { 0.0 } else { v })
        };
        let mut r = Obj::new();
        r.insert("type".into(), json!("image"));
        r.insert("rId".into(), json!(s("rId").unwrap_or("")));
        r.insert("src".into(), json!(self.media(n.get("src").and_then(st).unwrap_or(""))));
        for k in ["alt", "title", "shapeType"] {
            set(&mut r, k, s(k).map(|v| json!(v)));
        }
        r.insert("size".into(), json!({ "width": size("width"), "height": size("height") }));
        r.insert("wrap".into(), Value::Object(wrap));
        if let Some(t) = s("transform") {
            let mut l = Obj::new();
            if let Some(deg) = rotation(t).filter(|d| *d != 0.0) {
                l.insert("rotation".into(), jnum(deg));
            }
            if t.contains("scaleX(-1)") {
                l.insert("flipH".into(), json!(true));
            }
            if t.contains("scaleY(-1)") {
                l.insert("flipV".into(), json!(true));
            }
            if !l.is_empty() {
                r.insert("transform".into(), Value::Object(l));
            }
        }
        if let Some(pos) = n.get("position").and_then(obj)
            && opt_truthy(pos.get("horizontal"))
            && opt_truthy(pos.get("vertical"))
        {
            let axis = |a: &Value, d: &str| {
                let mut o = Obj::new();
                o.insert("relativeTo".into(), a.get("relativeTo").filter(|v| truthy(v)).cloned().unwrap_or(json!(d)));
                set(&mut o, "alignment", a.get("align").cloned());
                set(&mut o, "posOffset", a.get("posOffset").cloned());
                Value::Object(o)
            };
            let mut p = Obj::new();
            set(&mut p, "relativeHeight", pos.get("relativeHeight").cloned());
            p.insert("horizontal".into(), axis(&pos["horizontal"], "column"));
            p.insert("vertical".into(), axis(&pos["vertical"], "paragraph"));
            r.insert("position".into(), Value::Object(p));
        }
        if let Some(w) = n.get("borderWidth").and_then(Value::as_f64).filter(|w| *w > 0.0) {
            let style = match s("borderStyle") {
                Some("dotted") => "dot",
                Some("dashed") => "dash",
                _ => "solid",
            };
            let mut o = json!({ "width": emu(w), "style": style });
            if let Some(c) = s("borderColor") {
                o["color"] = json!({ "rgb": c.replacen('#', "", 1) });
            }
            r.insert("outline".into(), o);
        }
        if opt_truthy(n.get("hlinkHref")) {
            r.insert("hlinkHref".into(), n["hlinkHref"].clone());
        }
        let mut crop = Obj::new();
        for (k, c) in [("cropTop", "top"), ("cropRight", "right"), ("cropBottom", "bottom"), ("cropLeft", "left")] {
            if present(n, k) {
                crop.insert(c.into(), n[k].clone());
            }
        }
        if !crop.is_empty() {
            r.insert("crop".into(), Value::Object(crop));
        }
        if let Some(o) = n.get("opacity").and_then(Value::as_f64).filter(|o| *o < 1.0) {
            r.insert("opacity".into(), jnum(o));
        }
        for k in ["layoutInCell", "allowOverlap"] {
            if present(n, k) {
                r.insert(k.into(), n[k].clone());
            }
        }
        let mut pad = Obj::new();
        for (k, c) in [("effectExtentTop", "top"), ("effectExtentBottom", "bottom"), ("effectExtentLeft", "left"), ("effectExtentRight", "right")] {
            if present(n, k) {
                pad.insert(c.into(), emu(js_number(n.get(k))));
            }
        }
        if !pad.is_empty() {
            r.insert("padding".into(), Value::Object(pad));
        }
        json!({ "type": "run", "content": [{ "type": "drawing", "image": r }] })
    }

    /// `$e`: a shape.
    fn shape(&self, e: &Obj) -> Value {
        let mut n = self
            .parse_json(e.get("shapeJson").and_then(st), 2_000_000)
            .filter(|t| t["type"] == "shape" && t["shapeType"].is_string())
            .map(|mut t| {
                if !t["size"].is_object() {
                    t["size"] = json!({ "width": 0, "height": 0 });
                }
                t
            })
            .unwrap_or_else(|| json!({ "type": "shape", "shapeType": "rect", "size": { "width": 0, "height": 0 } }));
        let s = |k: &str| e.get(k).and_then(st).filter(|s| !s.is_empty());
        if let Some(t) = s("shapeType") {
            n["shapeType"] = json!(t);
        }
        if let Some(r) = s("shapeId") {
            n["id"] = json!(r);
        }
        if opt_truthy(e.get("width")) {
            n["size"]["width"] = emu(js_number(e.get("width")));
        }
        if opt_truthy(e.get("height")) {
            n["size"]["height"] = emu(js_number(e.get("height")));
        }
        if let Some(g) = e.get("geometryPath").and_then(Value::as_array).filter(|g| !g.is_empty()) {
            n["geometryPath"] = Value::Array(g.clone());
        }
        let hex = |c: &str| c.replacen('#', "", 1);
        if s("fillType") == Some("gradient")
            && let Some(stops) = e.get("gradientStops").and_then(st)
        {
            match serde_json::from_str::<Vec<Value>>(stops) {
                Ok(stops) if stops.iter().all(|l| l["color"].is_string()) => {
                    let mut g = json!({ "type": s("gradientType").unwrap_or("linear"), "stops": stops.iter().map(|l| json!({ "position": l["position"], "color": { "rgb": hex(l["color"].as_str().unwrap_or("")) } })).collect::<Vec<_>>() });
                    if let Some(a) = e.get("gradientAngle").and_then(num).filter(|a| *a != 0.0) {
                        g["angle"] = jnum(a);
                    }
                    n["fill"] = json!({ "type": "gradient", "gradient": g });
                }
                _ => n["fill"] = json!({ "type": "solid", "color": { "rgb": hex(s("fillColor").unwrap_or("000000")) } }),
            }
        } else if let Some(c) = e.get("fillColor").and_then(st) {
            n["fill"] = json!({ "type": s("fillType").unwrap_or("solid"), "color": { "rgb": hex(c) } });
        } else if s("fillType") == Some("none") {
            n["fill"] = json!({ "type": "none" });
        }
        if let Some(w) = e.get("outlineWidth").and_then(Value::as_f64).filter(|w| *w > 0.0) {
            let style = match s("outlineStyle") {
                Some("dotted") => "dot",
                Some("dashed") => "dash",
                _ => "solid",
            };
            let mut o = json!({ "width": emu(w), "style": style });
            if let Some(c) = e.get("outlineColor").and_then(st) {
                o["color"] = json!({ "rgb": hex(c) });
            }
            n["outline"] = o;
        }
        let mut o = Obj::new();
        let transform = e.get("transform").and_then(st).unwrap_or("");
        if let Some(r) = e.get("rotation").and_then(Value::as_f64) {
            o.insert("rotation".into(), jnum(r));
        } else if let Some(r) = rotation(transform) {
            o.insert("rotation".into(), jnum(r));
        }
        if opt_truthy(e.get("flipH")) || transform.contains("scaleX(-1)") {
            o.insert("flipH".into(), json!(true));
        }
        if opt_truthy(e.get("flipV")) || transform.contains("scaleY(-1)") {
            o.insert("flipV".into(), json!(true));
        }
        if o.values().any(truthy) {
            n["transform"] = Value::Object(o);
        }
        json!({ "type": "run", "content": [{ "type": "shape", "shape": n }] })
    }

    /// `je`: a chart.
    fn chart(&self, e: &Obj) -> Option<Value> {
        let t = self.parse_json(e.get("chartJson").and_then(st), usize::MAX)?;
        (t["type"] == "chart" && t["chartType"].is_string()).then(|| json!({ "type": "run", "content": [{ "type": "chart", "chart": t }] }))
    }

    /// `be`: an inline content control.
    fn inline_sdt(&self, e: &Obj) -> Value {
        let mut n = sdt_properties(e);
        if let Some(l) = self.parse_json(e.get("propertiesJson").and_then(st), 1_000_000)
            && l["sdtType"].is_string()
        {
            n = l;
        }
        let units = units_of(arr(e.get("content").unwrap_or(&Value::Null)));
        let mut o = sdt_items(self.inline(units, true));
        if let Some(v) = control_value(e.get("value"))
            && let Ok((props, content)) = apply_control_value(&n, &v)
        {
            n = props;
            o = sdt_items(content.first().filter(|s| ty(s) == "paragraph").map(|s| arr(&s["content"]).to_vec()).unwrap_or_default());
        }
        json!({ "type": "inlineSdt", "properties": n, "content": o })
    }

    /// `ce`: a unit as a model item.
    fn item(&self, u: &Unit) -> Option<Value> {
        let (kind, p, attrs) = match u {
            Unit::Text { text, attrs } => return Some(text_run(text, attrs)),
            Unit::Embed { kind, payload, attrs } => (kind.as_str(), payload, attrs),
        };
        match kind {
            "break" => Some(json!({ "type": "run", "content": [{ "type": "break", "breakType": "textWrapping" }] })),
            "tab" => Some(tab_run(attrs)),
            "image" => Some(self.image(p)),
            "horizontalRule" => self.rule(p, attrs),
            "shape" => Some(self.shape(p)),
            "chart" => self.chart(p),
            "field" => Some(Self::comment_reference(p).unwrap_or_else(|| self.field(p, attrs))),
            "math" => Some(Self::math(p)),
            "sdt" => Some(self.inline_sdt(p)),
            "noteRef" => {
                let mark = match p.get("footnoteRefId") {
                    Some(n) => json!({ "type": "footnoteRef", "id": jnum(numeric_id(Some(n))) }),
                    None => json!({ "type": "endnoteRef", "id": jnum(numeric_id(p.get("endnoteRefId"))) }),
                };
                Some(json!({ "type": "run", "content": [mark] }))
            }
            _ => None,
        }
    }

    /// `gn`: a unit inside the revision `info`.
    fn revised(&self, u: &Unit, info: Value, in_sdt: bool) -> Value {
        let r = if in_sdt {
            self.item(u).and_then(|l| sdt_items(vec![l]).into_iter().next()).unwrap_or_else(|| json!({ "type": "run", "content": [] }))
        } else {
            match u {
                Unit::Embed { kind, payload, .. } if kind == "sdt" => self.inline_sdt(payload),
                Unit::Embed { kind, payload, .. } if kind == "image" => self.image(payload),
                Unit::Embed { kind, attrs, .. } if kind == "tab" => tab_run(attrs),
                Unit::Embed { kind, payload, attrs } if kind == "horizontalRule" => self.rule(payload, attrs).unwrap_or_else(|| json!({ "type": "run", "content": [] })),
                Unit::Embed { kind, payload, .. } if kind == "shape" => self.shape(payload),
                Unit::Embed { kind, payload, .. } if kind == "chart" => self.chart(payload).unwrap_or_else(|| json!({ "type": "run", "content": [] })),
                Unit::Text { text, attrs } => text_run(text, attrs),
                _ => json!({ "type": "run", "content": [] }),
            }
        };
        let a = u.attrs();
        let mv = a.get("ins").and_then(obj).or_else(|| a.get("del").and_then(obj)).and_then(|o| o.get("isMovePair")) == Some(&Value::Bool(true));
        let kind = match (opt_truthy(a.get("ins")), mv) {
            (true, true) => "moveTo",
            (true, false) => "insertion",
            (false, true) => "moveFrom",
            (false, false) => "deletion",
        };
        json!({ "type": kind, "info": info, "content": [r] })
    }

    /// `mn`: a unit into the hyperlink `h`.
    fn link_child(&self, h: &mut Value, u: &Unit) {
        let t = match u {
            Unit::Text { text, attrs } => Some(text_run(text, attrs)),
            Unit::Embed { kind, payload, attrs } => match kind.as_str() {
                "break" => Some(json!({ "type": "run", "content": [{ "type": "break", "breakType": "textWrapping" }] })),
                "tab" => Some(json!({ "type": "run", "content": [{ "type": "tab" }] })),
                "horizontalRule" => self.rule(payload, attrs),
                "field" => Some(Self::comment_reference(payload).unwrap_or_else(|| self.field(payload, attrs))),
                "math" => Some(Self::math(payload)),
                "sdt" => {
                    let r = self.inline_sdt(payload);
                    (opt_truthy(attrs.get("ins")) || opt_truthy(attrs.get("del")) || has_sdt_revision(&r)).then_some(r)
                }
                _ => None,
            },
        };
        let Some(t) = t else { return };
        let is_run = ty(&t) == "run";
        if is_run {
            h["children"].as_array_mut().expect("children").push(t.clone());
        }
        if !is_run || h.get("structuredChildren").is_some() {
            if h.get("structuredChildren").is_none() {
                h["structuredChildren"] = h["children"].clone();
            }
            h["structuredChildren"].as_array_mut().expect("structured").push(t);
        }
    }

    /// `ee`: a paragraph's units as its inline items.
    fn inline(&self, units: Vec<Unit>, in_sdt: bool) -> Vec<Value> {
        let units = self.field_results(units);
        let mut r: Vec<Value> = Vec::new();
        let mut run: Option<(Value, String)> = None;
        let mut link: Option<Value> = None;
        let flush_run = |run: &mut Option<(Value, String)>, r: &mut Vec<Value>| {
            if let Some((o, _)) = run.take() {
                r.push(o);
            }
        };
        let flush_link = |link: &mut Option<Value>, r: &mut Vec<Value>| {
            if let Some(l) = link.take() {
                r.push(l);
            }
        };
        for u in &units {
            if u.embed("noteRef") {
                flush_run(&mut run, &mut r);
                flush_link(&mut link, &mut r);
                if let Some(c) = self.item(u) {
                    r.push(c);
                }
                continue;
            }
            let a = u.attrs();
            let rev = nn_undef(a, "ins").or_else(|| a.get("del"));
            if let Some(f) = revision_info(rev, None) {
                flush_run(&mut run, &mut r);
                flush_link(&mut link, &mut r);
                let c = self.revised(u, f, in_sdt);
                match r.last_mut() {
                    Some(m) if is_revision(m) && ty(m) == ty(&c) && m["info"]["id"] == c["info"]["id"] && m["info"]["author"] == c["info"]["author"] && m["info"].get("date") == c["info"].get("date") => {
                        let more = c["content"].as_array().cloned().unwrap_or_default();
                        m["content"].as_array_mut().expect("content").extend(more);
                    }
                    _ => r.push(c),
                }
                continue;
            }
            if let Some(g) = href_of(a) {
                flush_run(&mut run, &mut r);
                let current = link.as_ref().map(|l| match l["href"].as_str() {
                    Some(h) if !h.is_empty() => h.to_owned(),
                    _ => l["anchor"].as_str().map(|a| format!("#{a}")).unwrap_or_default(),
                });
                if current.as_deref() != Some(g.as_str()) {
                    flush_link(&mut link, &mut r);
                    link = hyperlink_of(a);
                }
                if let Some(l) = link.as_mut() {
                    self.link_child(l, u);
                }
                continue;
            }
            flush_link(&mut link, &mut r);
            if let Unit::Text { text, attrs } = u {
                let key = canon(&Value::Object(plain(attrs)));
                let m = text_run(text, attrs);
                match run.as_mut() {
                    Some((o, k)) if *k == key => join_run(o, m),
                    _ => {
                        flush_run(&mut run, &mut r);
                        run = Some((m, key));
                    }
                }
                continue;
            }
            flush_run(&mut run, &mut r);
            if let Some(p) = self.item(u) {
                r.push(p);
            }
        }
        flush_run(&mut run, &mut r);
        flush_link(&mut link, &mut r);
        r
    }

    /// `hn`: units marked as a complex field's result folded into that
    /// field, where they differ from what it held.
    fn field_results(&self, mut e: Vec<Unit>) -> Vec<Unit> {
        let owners: Vec<(f64, usize)> = e
            .iter()
            .enumerate()
            .filter_map(|(o, u)| match u {
                Unit::Embed { kind, payload, .. } if kind == "field" => payload.get("resultProjection").and_then(|r| r["id"].as_f64()).map(|id| (id, o)),
                _ => None,
            })
            .collect();
        if owners.is_empty() {
            return e;
        }
        // results by owner position, by result index
        let mut taken: HashMap<usize, Vec<(i64, Vec<Unit>)>> = HashMap::new();
        let mut kept = Vec::new();
        for (i, mut o) in std::mem::take(&mut e).into_iter().enumerate() {
            let l = o.attrs().get("fieldResult").and_then(obj).cloned();
            let s = l.as_ref().and_then(|l| l.get("id")).and_then(num);
            let a = l.as_ref().and_then(|l| l.get("index")).and_then(num);
            let owner = owners.iter().find(|(id, at)| Some(*id) == s && *at > i).map(|(_, at)| *at);
            match (a, owner) {
                (Some(a), Some(owner)) => {
                    o.attrs_mut().remove("fieldResult");
                    let list = taken.entry(owner).or_default();
                    match list.iter_mut().find(|(k, _)| *k == a as i64) {
                        Some((_, v)) => v.push(o),
                        None => list.push((a as i64, vec![o])),
                    }
                }
                _ => kept.push((i, o)),
            }
        }
        let mut out = Vec::new();
        for (i, mut o) in kept {
            if let Unit::Embed { kind, payload, attrs } = &mut o
                && kind == "field"
                && owners.iter().any(|(_, at)| *at == i)
            {
                self.fold_field(payload, attrs, taken.remove(&i).unwrap_or_default());
            }
            out.push(o);
        }
        out
    }

    fn fold_field(&self, payload: &mut Obj, attrs: &Obj, results: Vec<(i64, Vec<Unit>)>) {
        let mut i = self.field(payload, attrs);
        if ty(&i) != "complexField" {
            return;
        }
        let children = payload.get("resultProjection").map(|l| arr(&l["children"]).to_vec()).unwrap_or_default();
        let mut replaced: HashMap<i64, Vec<Value>> = HashMap::new();
        for f in &children {
            let Some(p) = f.get("index").and_then(num) else { continue };
            let Some(items) = f.get("items").and_then(Value::as_array) else { continue };
            let p = p as i64;
            let c2: Vec<Unit> = results.iter().find(|(k, _)| *k == p).map(|(_, v)| v.clone()).unwrap_or_default();
            if items_key(&c2) == items_key(&units_of(items)) {
                continue;
            }
            let mut m = inline_items(self.inline(c2, false));
            let s = if p < 0 { i.pointer(&format!("/structuredCode/inline/{}", -p - 1)) } else { i.pointer(&format!("/structuredResult/inline/{p}")) };
            if let Some(s) = s.filter(|s| ty(s) == "hyperlink")
                && m.len() == 1
                && ty(&m[0]) == "hyperlink"
            {
                let mut merged = s.as_object().cloned().unwrap_or_default();
                for (k, v) in m[0].as_object().cloned().unwrap_or_default() {
                    merged.insert(k, v);
                }
                set(&mut merged, "structuredChildren", m[0].get("structuredChildren").cloned());
                m[0] = Value::Object(merged);
            }
            replaced.insert(p, m);
        }
        if replaced.is_empty() {
            return;
        }
        let result: Vec<Value> = arr(i.pointer("/structuredResult/inline").unwrap_or(&Value::Null)).iter().enumerate().flat_map(|(g, f)| replaced.get(&(g as i64)).cloned().unwrap_or_else(|| vec![f.clone()])).collect();
        let code: Option<Vec<Value>> = i.pointer("/structuredCode/inline").and_then(Value::as_array).map(|c| c.iter().enumerate().flat_map(|(g, f)| replaced.get(&(-(g as i64) - 1)).cloned().unwrap_or_else(|| vec![f.clone()])).collect());
        let merge = |v: &mut Value, inline: Vec<Value>| {
            if !v.is_object() {
                *v = json!({});
            }
            v["inline"] = Value::Array(inline);
        };
        merge(&mut i["structuredResult"], result.clone());
        if let Some(c) = &code {
            merge(&mut i["structuredCode"], c.clone());
        }
        if opt_truthy(i.get("fieldTree")) {
            merge(&mut i["fieldTree"]["result"], result.clone());
            if let Some(c) = &code {
                merge(&mut i["fieldTree"]["code"], c.clone());
            }
        }
        i["fieldResult"] = Value::Array(
            result
                .iter()
                .flat_map(|f| match ty(f) {
                    "run" => vec![f.clone()],
                    "hyperlink" => arr(&f["children"]).iter().filter(|g| ty(g) == "run").cloned().collect(),
                    _ => vec![],
                })
                .collect(),
        );
        payload.insert("fieldData".into(), json!(i.to_string()));
    }

    /// `wn`: a paragraph whose text and marks are as it was parsed takes
    /// its parsed runs back (their boundaries, breaks and note marks).
    fn original_runs(&self, e: Vec<Value>, units: &[Unit], bounds: Option<&Vec<Value>>) -> Vec<Value> {
        let Some(t) = bounds.filter(|t| !t.is_empty()) else { return e };
        if !e.iter().all(|s| ty(s) == "run" && arr(&s["content"]).iter().all(|a| ty(a) == "text"))
            || units.iter().any(|s| !matches!(s, Unit::Text { .. }) || s.attrs().contains_key("hyperlink"))
            || t.iter().any(|s| !arr(&s["noteMarks"]).is_empty() && !s["text"].as_str().unwrap_or("").is_empty())
            || units.iter().map(|s| if let Unit::Text { text, .. } = s { text.as_str() } else { "" }).collect::<String>() != t.iter().map(|s| s["text"].as_str().unwrap_or("")).collect::<String>()
        {
            return e;
        }
        let (mut o, mut i) = (0usize, 0usize);
        let mut marks = Vec::new();
        for s in t {
            let Some(a) = marks_of(s["marksKey"].as_str()) else { return e };
            let key = canon(&Value::Object(a.clone()));
            marks.push(a);
            let mut u = u16len(s["text"].as_str().unwrap_or(""));
            while u > 0 {
                let Some(Unit::Text { text, attrs }) = units.get(o) else { return e };
                if canon(&Value::Object(plain(attrs))) != key {
                    return e;
                }
                let f = u16len(text) - i;
                let g = u.min(f);
                u -= g;
                i += g;
                if i == u16len(text) {
                    o += 1;
                    i = 0;
                }
            }
        }
        if o != units.len() || i != 0 {
            return e;
        }
        t.iter()
            .zip(marks)
            .map(|(s, m)| {
                let text = s["text"].as_str().unwrap_or("");
                let u: Option<Obj> = if text.is_empty() { s.get("formatting").and_then(obj).cloned() } else { Some(run_formatting(&m)) };
                let mut d = json!({ "type": "run", "content": boundary_content(s, u.as_ref().unwrap_or(&Obj::new())) });
                if let Some(u) = u.filter(|u| !u.is_empty()) {
                    d["formatting"] = Value::Object(u);
                }
                if !arr(&s["propertyChanges"]).is_empty() {
                    d["propertyChanges"] = s["propertyChanges"].clone();
                }
                d
            })
            .collect()
    }

    /// `In`: a paragraph from its pilcrow and units.
    #[allow(clippy::too_many_arguments)]
    fn paragraph(&self, para_id: &str, props: &Obj, units: Vec<Unit>, marks: Vec<Mark>, base: Option<&Value>, inherited: Option<Obj>) -> Value {
        let s = paragraph_attrs(props);
        let mut a = self.inline(units.clone(), false);
        a = self.original_runs(a, &units, s.get("_originalRunBoundaries").and_then(Value::as_array));
        if let Some(o) = base {
            a = raw_items(a, o);
            copy_ordinals(&mut a, &o["content"]);
        }
        a = place_marks(a, marks, &comment_mark);
        let total = || a.iter().map(item_len).sum::<usize>();
        let bookmarks: Vec<Mark> = bookmarks(props).into_iter().map(|mut p| {
            if p.offset == usize::MAX {
                p.offset = total();
            }
            p
        })
        .collect();
        if !bookmarks.is_empty() {
            a = place_marks(a, bookmarks, &bookmark_mark);
        }
        let mut d = Obj::new();
        d.insert("type".into(), json!("paragraph"));
        set(&mut d, "paraId", (!para_id.is_empty()).then(|| json!(para_id)));
        set(&mut d, "textId", base.and_then(|o| o.get("textId")).cloned());
        if let Some(x) = base.and_then(|o| o.get("extraAttributes")).filter(|v| truthy(v)) {
            d.insert("extraAttributes".into(), x.clone());
        }
        if let Some(x) = base.and_then(|o| o.get("paraIdAttribute")).filter(|v| truthy(v)) {
            d.insert("paraIdAttribute".into(), x.clone());
        }
        set(&mut d, "formatting", paragraph_formatting(&s, inherited.as_ref()).map(Value::Object));
        d.insert("content".into(), Value::Array(a));
        if base.is_some_and(|o| opt_truthy(o.get("renderedPageBreakBefore"))) {
            d.insert("renderedPageBreakBefore".into(), json!(true));
        }
        if let Some(f) = revision_info(props.get("pPrIns"), None) {
            d.insert("pPrIns".into(), f);
        }
        if let Some(g) = revision_info(props.get("pPrDel"), None) {
            d.insert("pPrDel".into(), g);
        }
        if !arr(props.get("pPrChange").unwrap_or(&Value::Null)).is_empty() {
            d.insert("propertyChanges".into(), props["pPrChange"].clone());
        }
        if opt_truthy(props.get("sectPr")) {
            d.insert("sectionProperties".into(), props["sectPr"].clone());
        } else if opt_truthy(props.get("sectionBreakType")) {
            d.insert("sectionProperties".into(), json!({ "sectionStart": props["sectionBreakType"] }));
        }
        Value::Object(d)
    }

    /// `inheritedFormatting`: what a body paragraph's style gives it, which
    /// a new paragraph doesn't repeat.
    fn inherited(&self, story: &str, props: &Obj) -> Option<Obj> {
        if nested_story(story) || self.synthetic_defaults || present(props, "numPr") {
            return None;
        }
        let r = props.get("pStyle").and_then(st).unwrap_or("").to_owned();
        let mut cache = self.inherited.borrow_mut();
        cache
            .entry(r.clone())
            .or_insert_with(|| {
                let o = resolve_paragraph_style(&self.styles, (!r.is_empty()).then_some(r.as_str()))?;
                if present(&o, "numPr") {
                    return None;
                }
                let mut i = o;
                i.remove("indentLeft");
                i.remove("indentRight");
                Some(i)
            })
            .clone()
    }

    fn cell_blocks(&self, story: Option<&str>) -> Vec<Value> {
        match story {
            Some(s) if self.story_ids.contains(s) => self.story(s),
            _ => Vec::new(),
        }
    }

    /// `Fn`: a table cell.
    fn cell(&self, n: &Value) -> Value {
        let mut t = cell_defaults();
        if let Some(p) = n["tcPr"].as_object() {
            t.extend(p.clone());
        }
        let r = self.cell_blocks(n["story"].as_str());
        let mut o = json!({ "type": "tableCell", "content": r });
        if let Some(f) = cell_formatting(&t) {
            o["formatting"] = Value::Object(f);
        }
        if let Some(i) = t.get("cellMarker").and_then(obj).filter(|_| opt_truthy(t.get("cellMarker"))) {
            let info = revision_info(i.get("info"), None).unwrap_or_else(|| json!({ "id": 0, "author": "Unknown" }));
            o["structuralChange"] = match i.get("kind").and_then(st) {
                Some("ins") => json!({ "type": "tableCellInsertion", "info": info }),
                Some("del") => json!({ "type": "tableCellDeletion", "info": info }),
                _ => {
                    let mut m = json!({ "type": "tableCellMerge", "info": info });
                    for k in ["vMerge", "vMergeOrig"] {
                        if opt_truthy(i.get(k)) {
                            m[k] = i[k].clone();
                        }
                    }
                    m
                }
            };
        }
        if !arr(t.get("tcPrChange").unwrap_or(&Value::Null)).is_empty() {
            o["propertyChanges"] = t["tcPrChange"].clone();
        }
        o
    }

    /// `vn`: a table from its embed, cells from their stories, the grid's
    /// spans as Word writes them.
    fn table(&self, n: &Obj) -> Value {
        let rows = arr(n.get("rows").unwrap_or(&Value::Null));
        struct Placed {
            row: usize,
            col: usize,
            rowspan: usize,
            colspan: usize,
            cell: Value,
        }
        let mut taken: Vec<Vec<bool>> = Vec::new();
        let mut placed: Vec<Placed> = Vec::new();
        let mut width = 0;
        for (c2, p) in rows.iter().enumerate() {
            let mut m = 0;
            for s in arr(&p["cells"]) {
                while taken.get(c2).and_then(|r| r.get(m)).copied().unwrap_or(false) {
                    m += 1;
                }
                let b = &s["tcPr"];
                let span = |k: &str| b[k].as_f64().filter(|v| v.is_finite() && *v != 0.0).map(|v| v as usize).unwrap_or(1);
                let (o, d) = (span("rowspan"), span("colspan"));
                placed.push(Placed { row: c2, col: m, rowspan: o, colspan: d, cell: self.cell(s) });
                for x in c2..c2 + o {
                    if taken.len() <= x {
                        taken.resize(x + 1, Vec::new());
                    }
                    if taken[x].len() < m + d {
                        taken[x].resize(m + d, false);
                    }
                    for h in m..m + d {
                        taken[x][h] = true;
                    }
                }
                m += d;
                width = width.max(m);
            }
        }
        let at: HashMap<(usize, usize), usize> = placed.iter().enumerate().map(|(i, p)| ((p.row, p.col), i)).collect();
        let mut covered: HashMap<(usize, usize), usize> = HashMap::new();
        for (i, p) in placed.iter().enumerate() {
            for c in p.row..p.row + p.rowspan {
                for m in p.col..p.col + p.colspan {
                    covered.insert((c, m), i);
                }
            }
        }
        let mut out_rows: Vec<Value> = rows
            .iter()
            .enumerate()
            .map(|(c2, p)| {
                let mut m = Vec::new();
                let mut x = 0;
                while x < width {
                    if let Some(&i) = at.get(&(c2, x)) {
                        let h = &placed[i];
                        let mut c = h.cell["formatting"].as_object().cloned().unwrap_or_default();
                        set(&mut c, "gridSpan", (h.colspan > 1).then(|| json!(h.colspan)));
                        if h.rowspan > 1 {
                            c.insert("vMerge".into(), json!("restart"));
                        } else if !matches!(c.get("vMerge").and_then(st), Some("restart" | "continue")) {
                            c.remove("vMerge");
                        }
                        let mut cell = h.cell.clone();
                        set(cell.as_object_mut().expect("cell"), "formatting", (!c.is_empty()).then_some(Value::Object(c)));
                        m.push(cell);
                        x += h.colspan;
                        continue;
                    }
                    let Some(&i) = covered.get(&(c2, x)) else {
                        x += 1;
                        continue;
                    };
                    let b = &placed[i];
                    let mut k = b.cell["formatting"].as_object().cloned().unwrap_or_default();
                    set(&mut k, "gridSpan", (b.colspan > 1).then(|| json!(b.colspan)));
                    k.insert("vMerge".into(), json!("continue"));
                    let mut cell = b.cell.clone();
                    cell["content"] = json!([]);
                    cell["formatting"] = Value::Object(k);
                    m.push(cell);
                    x += b.colspan;
                }
                let mut s = row_defaults();
                if let Some(t) = p["trPr"].as_object() {
                    s.extend(t.clone());
                }
                let mut b = json!({ "type": "tableRow", "cells": m });
                if let Some(f) = row_formatting(&s) {
                    b["formatting"] = Value::Object(f);
                }
                if let Some(o) = revision_info(s.get("trIns"), None) {
                    b["structuralChange"] = json!({ "type": "tableRowInsertion", "info": o });
                } else if let Some(d) = revision_info(s.get("trDel"), None) {
                    b["structuralChange"] = json!({ "type": "tableRowDeletion", "info": d });
                }
                if !arr(s.get("trPrChange").unwrap_or(&Value::Null)).is_empty() {
                    b["propertyChanges"] = s["trPrChange"].clone();
                }
                b
            })
            .collect();
        clean_vmerge(&mut out_rows);
        let grid: Vec<Value> = arr(n.get("grid").unwrap_or(&Value::Null)).iter().filter(|p| p.is_number()).cloned().collect();
        let mut d2 = table_defaults();
        if let Some(t) = n.get("tblPr").and_then(obj) {
            d2.extend(t.clone());
        }
        set(&mut d2, "columnWidths", (!grid.is_empty()).then(|| Value::Array(grid)));
        let mut f = table_formatting(&d2);
        if !f.as_ref().is_some_and(|f| opt_truthy(f.get("borders")))
            && let Some(p) = outer_borders(&out_rows)
        {
            f.get_or_insert_with(Obj::new).insert("borders".into(), p);
        }
        let mut g = Obj::new();
        g.insert("type".into(), json!("table"));
        set(&mut g, "columnWidths", or_undef(&d2, "columnWidths").cloned());
        set(&mut g, "formatting", f.map(Value::Object));
        g.insert("rows".into(), Value::Array(out_rows));
        if !arr(d2.get("tblPrChange").unwrap_or(&Value::Null)).is_empty() {
            g.insert("propertyChanges".into(), d2["tblPrChange"].clone());
        }
        Value::Object(g)
    }

    /// `storyToBlocks`: a story's blocks.
    ///
    /// A page or column break the engine holds as an embed at a
    /// paragraph's start is the twin of one its previous paragraph was
    /// parsed with (the seed lays it out so); one the previous paragraph
    /// doesn't hold was added in the editor, and saves as a paragraph of
    /// its own holding the break (as the web saves an opaque page break).
    /// The web drops both kinds.
    pub(crate) fn story(&self, n: &str) -> Vec<Value> {
        let t = self.base_stories.get(n);
        let r: &[(f64, u32, u32)] = self.comments.get(n).map(Vec::as_slice).unwrap_or(&[]);
        let a = self.doc.story_paragraph_ids(n).unwrap_or_default();
        let Ok(segments) = self.doc.story_segments(n) else { return Vec::new() };
        let mut s: Vec<Value> = Vec::new();
        let mut d2: Vec<Unit> = Vec::new();
        let (mut f, mut g, mut p, mut c2) = (0u32, 0u32, 0usize, 0u32);
        // the breaks the previous paragraph holds, which their twins stand for
        let mut held: Vec<String> = Vec::new();
        let marks_until = |h: u32, f: u32, g: u32| -> Vec<Mark> {
            let mut b2 = Vec::new();
            for &(id, start, end) in r {
                if start >= f && start <= h {
                    b2.push(Mark { id, kind: MarkKind::Start, offset: start.saturating_sub(g) as usize, name: None, col_first: None, col_last: None });
                }
                if end >= f && end <= h {
                    b2.push(Mark { id, kind: MarkKind::End, offset: end.saturating_sub(g) as usize, name: None, col_first: None, col_last: None });
                }
            }
            b2
        };
        for h in segments {
            let attrs = attrs_obj(&h.attributes);
            match h.content {
                docx_edit::SegmentContent::Text(text) => {
                    c2 += u16len(&text) as u32;
                    let parts: Vec<&str> = text.split('\t').collect();
                    for (i, part) in parts.iter().enumerate() {
                        if !part.is_empty() {
                            d2.push(Unit::Text { text: (*part).to_owned(), attrs: attrs.clone() });
                        }
                        if i + 1 < parts.len() {
                            d2.push(Unit::Embed { kind: "tab".into(), payload: Obj::new(), attrs: attrs.clone() });
                        }
                    }
                    continue;
                }
                docx_edit::SegmentContent::Pilcrow(props) => {
                    let props_obj = attrs_obj(&props.values);
                    // breaks added in the editor, before the paragraph
                    for u in d2.iter().take_while(|u| u.embed("pageBreak") || u.embed("columnBreak")) {
                        let kind = if u.embed("pageBreak") { "page" } else { "column" };
                        match held.iter().position(|b| b == kind) {
                            Some(i) => {
                                held.remove(i);
                            }
                            None => s.push(break_paragraph(kind)),
                        }
                    }
                    let b2 = a.get(p).cloned().flatten().unwrap_or_default();
                    let k = self.base_paragraphs.get(&props.para_id);
                    let c = marks_until(c2, f, g);
                    let units = std::mem::take(&mut d2);
                    let mut para = self.paragraph(&b2, &props_obj, units, c, k, self.inherited(n, &props_obj));
                    held = breaks_in(&para);
                    para[SESSION_KEY] = json!(props.para_id);
                    s.push(para);
                    p += 1;
                    c2 += 1;
                    f = c2;
                    g = c2;
                    continue;
                }
                docx_edit::SegmentContent::OtherEmbed { kind, payload } => {
                    let payload = attrs_obj(&payload);
                    match kind.as_str() {
                        "table" => {
                            s.push(self.table(&payload));
                            held.clear();
                        }
                        "blockSdt" => {
                            let b2 = payload.get("story").and_then(st);
                            let k = self.cell_blocks(b2);
                            let mut props = sdt_properties(&payload);
                            let mut content = k.clone();
                            if let Some(m) = control_value(payload.get("value"))
                                && let Ok((pr, c)) = apply_control_value(&props, &m)
                            {
                                props = pr;
                                content = with_para_ids(c, &k);
                            }
                            s.push(json!({ "type": "blockSdt", "properties": props, "content": content }));
                            held.clear();
                        }
                        "opaque" => {
                            let blob = payload.get("blob").and_then(obj);
                            if blob.is_some_and(|b| b.get("type").and_then(st) == Some("pageBreak")) {
                                s.push(break_paragraph("page"));
                            } else if let Some(k) = t.and_then(|t| t.get(s.len()))
                                && blob.is_some_and(|b| b.get("type").and_then(st) == Some("blockSdt"))
                                && ty(k) == "blockSdt"
                            {
                                s.push(k.clone());
                            }
                        }
                        _ => d2.push(Unit::Embed { kind, payload, attrs }),
                    }
                }
            }
            c2 += 1;
            if d2.iter().all(|u| u.embed("pageBreak") || u.embed("columnBreak")) {
                g = c2;
            }
        }
        if !d2.is_empty() {
            let h = self.inline(d2, false);
            if !h.is_empty() {
                s.push(json!({ "type": "paragraph", "content": h }));
            }
        }
        reinsert_raw(s, t.map(Vec::as_slice).unwrap_or(&[]))
    }
}

fn rotation(t: &str) -> Option<f64> {
    let at = t.find("rotate(")? + 7;
    let rest = &t[at..];
    let end = rest.find("deg)")?;
    let n = &rest[..end];
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit() || b == b'-' || b == b'.')).then(|| n.parse().ok()).flatten()
}

fn tab_run(attrs: &Obj) -> Value {
    with_formatting(json!({ "type": "run", "content": [{ "type": "tab" }] }), run_formatting(&plain(attrs)))
}

/// `Bn`: a paragraph holding only a break.
fn break_paragraph(kind: &str) -> Value {
    json!({ "type": "paragraph", "content": [{ "type": "run", "content": [{ "type": "break", "breakType": kind }] }] })
}

/// The page and column breaks a paragraph holds.
fn breaks_in(p: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if o.get("type").and_then(st) == Some("break")
                    && let Some(k @ ("page" | "column")) = o.get("breakType").and_then(st)
                {
                    out.push(k.to_owned());
                }
                o.values().for_each(|v| walk(v, out));
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    walk(&p["content"], &mut out);
    out
}

/// `U`: a table cell's or block content control's story.
fn nested_story(s: &str) -> bool {
    let tail = |s: &str| -> bool {
        // `:t<n>:r<n>c<n>` (a cell id may carry a `~client` suffix) or `:sdt<n>`
        if let Some(at) = s.rfind(":sdt") {
            let n = &s[at + 4..];
            if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
                return true;
            }
        }
        crate::office::docx::index::cell_of(s).is_some()
    };
    tail(s)
}

/// `En`: the parsed blocks of every story, cells and content controls
/// included, by story id.
fn base_stories(package: &Value) -> HashMap<String, Vec<Value>> {
    fn add(out: &mut HashMap<String, Vec<Value>>, r: &str, o: &[Value]) {
        out.insert(r.to_owned(), o.to_vec());
        let (mut i, mut l) = (0, 0);
        for s in o {
            match ty(s) {
                "blockSdt" => {
                    add(out, &format!("{r}:sdt{l}"), arr(&s["content"]));
                    l += 1;
                }
                "table" => {
                    for (d, u) in arr(&s["rows"]).iter().enumerate() {
                        for (g, f) in arr(&u["cells"]).iter().enumerate() {
                            add(out, &format!("{r}:t{i}:r{d}c{g}"), arr(&f["content"]));
                        }
                    }
                    i += 1;
                }
                _ => {}
            }
        }
    }
    let mut out = HashMap::new();
    add(&mut out, "body", arr(&package["document"]["content"]));
    for key in ["headerEntries", "footerEntries"] {
        for e in arr(&package[key]) {
            if let Some(id) = e[0].as_str() {
                add(&mut out, &format!("hf:{id}"), arr(&e[1]["content"]));
            }
        }
    }
    for (key, prefix) in [("footnotes", "fn"), ("endnotes", "en")] {
        for m in arr(&package[key]) {
            add(&mut out, &format!("{prefix}:{}", m["id"].as_f64().map(jnum).unwrap_or(Value::Null)), arr(&m["content"]));
        }
    }
    out
}

/// `Dn`: the parsed paragraphs by the engine paragraph each was seeded as.
fn base_paragraphs(stories: &HashMap<String, Vec<Value>>) -> HashMap<String, Value> {
    let mut n = HashMap::new();
    for (t, r) in stories {
        let mut o = 0;
        for i in r.iter().filter(|i| ty(i) == "paragraph") {
            let key = match i["paraId"].as_str() {
                Some(id) if !opt_truthy(i.get("repeatedParaId")) && !id.is_empty() => id.to_owned(),
                _ => format!("{t}:p{o}"),
            };
            o += 1;
            n.entry(key).or_insert_with(|| i.clone());
        }
    }
    n
}

/// `Ln`: the parsed story's raw XML blocks back in their places.
fn reinsert_raw(mut e: Vec<Value>, n: &[Value]) -> Vec<Value> {
    let mut t = 0usize;
    for (r, o) in n.iter().enumerate() {
        if ty(o) != "rawXml" {
            continue;
        }
        let next = n[r + 1..].iter().find(|a| ty(a) == "paragraph" && a["paraId"].as_str().is_some_and(|id| !id.is_empty() && e.iter().any(|u| ty(u) == "paragraph" && u["paraId"].as_str() == Some(id))));
        let l = next.and_then(|i| e.iter().position(|a| ty(a) == "paragraph" && a["paraId"] == i["paraId"]));
        let s = l.unwrap_or((r + t).min(e.len()));
        e.insert(s, o.clone());
        t = s.saturating_sub(r);
    }
    e
}

/// `Nn`: a note's leading reference marks, which the engine doesn't hold,
/// put back on its first paragraph.
fn with_note_marks(e: Vec<Value>, original: &[Value]) -> Vec<Value> {
    fn mark(a: &Value) -> bool {
        if is_revision(a) {
            return !arr(&a["content"]).is_empty() && arr(&a["content"]).iter().all(mark);
        }
        ty(a) == "run" && !arr(&a["content"]).is_empty() && arr(&a["content"]).iter().all(|n| matches!(ty(n), "footnoteRefMark" | "endnoteRefMark"))
    }
    if e.iter().any(|n| ty(n) == "paragraph" && arr(&n["content"]).iter().any(mark)) {
        return e;
    }
    let t = original.iter().find(|a| ty(a) == "paragraph").map(|p| arr(&p["content"])).unwrap_or(&[]);
    let o: Vec<Value> = t.iter().take_while(|a| mark(a)).cloned().collect();
    if o.is_empty() {
        return e;
    }
    let Some(i) = e.iter().position(|a| ty(a) == "paragraph") else { return e };
    let mut s = e;
    let mut content = o;
    content.extend(arr(&s[i]["content"]).iter().cloned());
    s[i]["content"] = Value::Array(content);
    s
}

/// `jn`: a content control's new blocks with its old paragraphs' ids.
fn with_para_ids(e: Vec<Value>, n: &[Value]) -> Vec<Value> {
    let mut t = n.iter().filter(|o| ty(o) == "paragraph");
    e.into_iter()
        .map(|mut o| {
            if ty(&o) != "paragraph" {
                return o;
            }
            if let Some(i) = t.next()
                && let Some(id) = i["paraId"].as_str().filter(|s| !s.is_empty())
            {
                o["paraId"] = json!(id);
                if let Some(k) = i.get(SESSION_KEY) {
                    o[SESSION_KEY] = k.clone();
                }
            }
            o
        })
        .collect()
}

// ---- marks placed at offsets: comment ranges, bookmarks, raw XML ----

#[derive(Clone, Copy, Debug, PartialEq)]
enum MarkKind {
    Start,
    End,
}

#[derive(Clone, Debug)]
struct Mark {
    id: f64,
    kind: MarkKind,
    offset: usize,
    name: Option<String>,
    col_first: Option<f64>,
    col_last: Option<f64>,
}

fn comment_mark(m: &Mark) -> Value {
    json!({ "type": if m.kind == MarkKind::Start { "commentRangeStart" } else { "commentRangeEnd" }, "id": jnum(m.id) })
}

fn bookmark_mark(m: &Mark) -> Value {
    if m.kind == MarkKind::Start {
        let mut b = json!({ "type": "bookmarkStart", "id": jnum(m.id), "name": m.name.clone().unwrap_or_default() });
        if let Some(c) = m.col_first {
            b["colFirst"] = jnum(c);
        }
        if let Some(c) = m.col_last {
            b["colLast"] = jnum(c);
        }
        b["position"] = json!({ "offset": m.offset });
        b
    } else {
        json!({ "type": "bookmarkEnd", "id": jnum(m.id), "position": { "offset": m.offset } })
    }
}

/// `Tn`: a paragraph's bookmarks.
fn bookmarks(props: &Obj) -> Vec<Mark> {
    let mut n = Vec::new();
    for t in arr(props.get("bookmarks").unwrap_or(&Value::Null)) {
        let Some(r) = obj(t) else { continue };
        let Some(o) = r.get("id").and_then(num) else { continue };
        let i = r.get("offset").and_then(num).unwrap_or(0.0).max(0.0) as usize;
        let name = r.get("name").and_then(st).map(str::to_owned);
        let (cf, cl) = (r.get("colFirst").and_then(num), r.get("colLast").and_then(num));
        let m = |kind, offset| Mark { id: o, kind, offset, name: name.clone(), col_first: cf, col_last: cl };
        match r.get("kind").and_then(st) {
            Some("start") => n.push(m(MarkKind::Start, i)),
            Some("end") => n.push(m(MarkKind::End, i)),
            _ => {
                n.push(m(MarkKind::Start, 0));
                n.push(m(MarkKind::End, usize::MAX));
            }
        }
    }
    n
}

/// `ze`: a run item's length in the engine's units.
fn content_len(e: &Value) -> usize {
    match ty(e) {
        "text" | "instrText" => u16len(e["text"].as_str().unwrap_or("")),
        "symbol" => u16len(e["char"].as_str().unwrap_or("")),
        "tab" | "softHyphen" | "noBreakHyphen" | "footnoteRef" | "endnoteRef" | "horizontalRule" | "commentReference" | "drawing" | "shape" | "chart" => 1,
        "break" if matches!(e.get("breakType").and_then(st), None | Some("textWrapping")) => 1,
        _ => 0,
    }
}

fn run_len(e: &Value) -> usize {
    arr(&e["content"]).iter().map(content_len).sum()
}

/// `We`.
fn child_len(e: &Value) -> usize {
    match ty(e) {
        "run" => run_len(e),
        "simpleField" | "complexField" | "inlineSdt" | "mathEquation" => 1,
        _ => 0,
    }
}

/// `$`: an inline item's length.
fn item_len(e: &Value) -> usize {
    match ty(e) {
        "run" => run_len(e),
        "hyperlink" => arr(e.get("structuredChildren").unwrap_or(&e["children"])).iter().map(child_len).sum(),
        "complexField" => arr(e.pointer("/structuredCode/inline").unwrap_or(&Value::Null)).iter().chain(arr(e.pointer("/structuredResult/inline").unwrap_or(&Value::Null))).filter(|n| matches!(ty(n), "hyperlink" | "simpleField")).map(item_len).sum::<usize>() + 1,
        "simpleField" | "inlineSdt" | "mathEquation" => 1,
        "insertion" | "deletion" | "moveFrom" | "moveTo" => arr(&e["content"]).iter().map(item_len).sum(),
        _ => 0,
    }
}

/// `se`: `e` split at `n` units.
fn split_list(e: &[Value], n: usize, len: &dyn Fn(&Value) -> usize, split: &dyn Fn(&Value, usize) -> Option<(Option<Value>, Option<Value>)>) -> Option<(Vec<Value>, Vec<Value>)> {
    let (mut o, mut i) = (Vec::new(), Vec::new());
    let mut l = 0;
    for s in e {
        let a = len(s);
        if l >= n {
            i.push(s.clone());
        } else if l + a <= n {
            o.push(s.clone());
        } else {
            let (x, y) = split(s, n - l)?;
            o.extend(x);
            i.extend(y);
        }
        l += a;
    }
    Some((o, i))
}

/// `Cn`.
fn split_text(e: &Value, n: usize) -> Option<(Option<Value>, Option<Value>)> {
    if !matches!(ty(e), "text" | "instrText") {
        return None;
    }
    let (a, b) = u16split(e["text"].as_str().unwrap_or(""), n);
    let (mut x, mut y) = (e.clone(), e.clone());
    x["text"] = json!(a);
    y["text"] = json!(b);
    Some((Some(x), Some(y)))
}

/// `Ne`: a hyperlink half holding `n`.
fn link_half(e: &Value, n: Vec<Value>) -> Option<Value> {
    if n.is_empty() {
        return None;
    }
    let t: Vec<Value> = n.iter().filter(|r| matches!(ty(r), "run" | "bookmarkStart" | "bookmarkEnd")).cloned().collect();
    let mut h = e.clone();
    h["children"] = Value::Array(t);
    if e.get("structuredChildren").is_some() {
        h["structuredChildren"] = Value::Array(n);
    }
    Some(h)
}

/// `pe`: an inline item split at `n` units.
fn split_item(e: &Value, n: usize) -> Option<(Option<Value>, Option<Value>)> {
    let half = |e: &Value, k: Vec<Value>| -> Option<Value> {
        (!k.is_empty()).then(|| {
            let mut h = e.clone();
            h["content"] = Value::Array(k);
            h
        })
    };
    match ty(e) {
        "run" => {
            let (a, b) = split_list(arr(&e["content"]), n, &content_len, &split_text)?;
            Some((half(e, a), half(e, b)))
        }
        "hyperlink" => {
            let t = arr(e.get("structuredChildren").unwrap_or(&e["children"]));
            let (a, b) = split_list(t, n, &child_len, &|o, i| if ty(o) == "run" { split_item(o, i) } else { None })?;
            Some((link_half(e, a), link_half(e, b)))
        }
        _ if is_revision(e) => {
            let (a, b) = split_list(arr(&e["content"]), n, &item_len, &split_item)?;
            Some((half(e, a), half(e, b)))
        }
        _ => None,
    }
}

/// `ge`: marks placed among the items at their offsets, items split
/// where a mark falls inside one.
fn place_marks(e: Vec<Value>, n: Vec<Mark>, make: &dyn Fn(&Mark) -> Value) -> Vec<Value> {
    if n.is_empty() {
        return e;
    }
    let paired: HashSet<(usize, i64)> = n.iter().filter(|d| d.kind == MarkKind::End && n.iter().any(|f| f.kind == MarkKind::Start && f.id == d.id && f.offset == d.offset)).map(|d| (d.offset, d.id as i64)).collect();
    let rank = |d: &Mark| if paired.contains(&(d.offset, d.id as i64)) { 1 } else if d.kind == MarkKind::End { 0 } else { 2 };
    let mut i = n.clone();
    i.sort_by(|d, f| {
        d.offset
            .cmp(&f.offset)
            .then(rank(d).cmp(&rank(f)))
            .then(d.id.partial_cmp(&f.id).unwrap_or(std::cmp::Ordering::Equal))
            .then(if d.kind == f.kind {
                std::cmp::Ordering::Equal
            } else if d.kind == MarkKind::Start {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            })
    });
    let mut l = Vec::new();
    let (mut s, mut a) = (0usize, 0usize);
    let up_to = |d: usize, a: &mut usize, l: &mut Vec<Value>| {
        while *a < i.len() && i[*a].offset <= d {
            l.push(make(&i[*a]));
            *a += 1;
        }
    };
    up_to(0, &mut a, &mut l);
    for d in e {
        let f = s + item_len(&d);
        let mut g = Some(d);
        while let Some(cur) = g.as_ref() {
            if !(a < i.len() && i[a].offset < f) {
                break;
            }
            let c = i[a].offset;
            let Some((x, y)) = split_item(cur, c - s) else { break };
            if let Some(x) = x {
                l.push(x);
            }
            s = c;
            up_to(s, &mut a, &mut l);
            g = y;
        }
        let mut p = Vec::new();
        while a < i.len() && i[a].offset < f {
            p.push(i[a].clone());
            a += 1;
        }
        for c in p.iter().filter(|c| c.kind == MarkKind::Start) {
            l.push(make(c));
        }
        if let Some(g) = g {
            l.push(g);
        }
        for c in p.iter().filter(|c| c.kind == MarkKind::End) {
            l.push(make(c));
        }
        s = f;
        up_to(s, &mut a, &mut l);
    }
    up_to(usize::MAX, &mut a, &mut l);
    l
}

/// `xn`: the parsed paragraph's raw XML items back at their offsets.
fn raw_items(e: Vec<Value>, base: &Value) -> Vec<Value> {
    let t: usize = e.iter().map(item_len).sum();
    let content = arr(&base["content"]);
    let mut r = Vec::new();
    let mut o = 0;
    for (i, l) in content.iter().enumerate() {
        if ty(l) == "rawXml" {
            r.push(Mark { id: i as f64, kind: MarkKind::Start, offset: o.min(t), name: None, col_first: None, col_last: None });
        }
        o += item_len(l);
    }
    place_marks(e, r, &|m| content[m.id as usize].clone())
}

/// `Sn`: nested paragraphs take their parsed source ordinals.
fn copy_ordinals(e: &mut [Value], n: &Value) {
    fn ordinals(v: &Value, out: &mut Vec<Option<Value>>) {
        match v {
            Value::Array(a) => a.iter().for_each(|v| ordinals(v, out)),
            Value::Object(o) => {
                if o.get("type").and_then(st) == Some("paragraph") {
                    out.push(o.get("sourceOrdinal").filter(|v| v.is_number()).cloned());
                }
                o.values().for_each(|v| ordinals(v, out));
            }
            _ => {}
        }
    }
    fn count(v: &Value) -> usize {
        match v {
            Value::Array(a) => a.iter().map(count).sum(),
            Value::Object(o) => usize::from(o.get("type").and_then(st) == Some("paragraph")) + o.values().map(count).sum::<usize>(),
            _ => 0,
        }
    }
    fn write(v: &mut Value, t: &[Option<Value>], at: &mut usize) {
        match v {
            Value::Array(a) => a.iter_mut().for_each(|v| write(v, t, at)),
            Value::Object(o) => {
                if o.get("type").and_then(st) == Some("paragraph") {
                    if let Some(Some(n)) = t.get(*at) {
                        o.insert("sourceOrdinal".into(), n.clone());
                    }
                    *at += 1;
                }
                o.values_mut().for_each(|v| write(v, t, at));
            }
            _ => {}
        }
    }
    let mut t = Vec::new();
    ordinals(n, &mut t);
    if !t.iter().any(Option::is_some) || e.iter().map(count).sum::<usize>() != t.len() {
        return;
    }
    let mut at = 0;
    e.iter_mut().for_each(|v| write(v, &t, &mut at));
}

// ---- original runs ----

/// `yn`: a run's marks from its key.
fn marks_of(e: Option<&str>) -> Option<Obj> {
    let Some(e) = e.filter(|e| !e.is_empty()) else { return Some(Obj::new()) };
    let mut n = Obj::new();
    for t in e.split('|') {
        let r = t.find(':').filter(|r| *r > 0)?;
        let o = &t[..r];
        let i: Value = serde_json::from_str(&t[r + 1..]).ok()?;
        match o {
            "comment" | "footnoteRef" => {}
            o if FLAGS.contains(&o) => {
                n.insert(o.into(), json!(true));
            }
            "highlight" => {
                set(&mut n, "highlight", i.get("color").cloned());
            }
            "insertion" | "deletion" => {
                let v = json!({ "id": i["revisionId"], "author": i["author"], "date": i["date"], "isMovePair": i["isMovePair"] });
                n.insert(if o == "insertion" { "ins" } else { "del" }.into(), compact(&v));
            }
            o => {
                n.insert(o.into(), compact(&i));
            }
        }
    }
    Some(n)
}

/// `kn`: a parsed run's content: its note marks, or its text with breaks.
fn boundary_content(e: &Value, fmt: &Obj) -> Vec<Value> {
    if let Some(m) = e["noteMarks"].as_array().filter(|m| !m.is_empty()) {
        return m.iter().map(|n| json!({ "type": if n == "endnote" { "endnoteRefMark" } else { "footnoteRefMark" } })).collect();
    }
    let text = e["text"].as_str().unwrap_or("");
    let breaks = arr(&e["breaks"]);
    if breaks.is_empty() {
        return text_content(text, fmt);
    }
    let len = u16len(text);
    let mut r = Vec::new();
    let mut o = 0;
    for i in breaks {
        let l = (i["offset"].as_f64().unwrap_or(0.0).max(0.0) as usize).max(o).min(len);
        if l > o {
            let (_, rest) = u16split(text, o);
            let (piece, _) = u16split(&rest, l - o);
            r.extend(text_content(&piece, fmt));
        }
        r.push(json!({ "type": "break", "breakType": i["type"] }));
        o = l;
    }
    if o < len {
        r.extend(text_content(&u16split(text, o).1, fmt));
    }
    r
}

/// `Me`: field result items compared without their result marks, tabs as
/// text and adjacent texts joined.
fn items_key(e: &[Unit]) -> String {
    let mut n: Vec<Value> = Vec::new();
    for t in e {
        let mut r = t.attrs().clone();
        r.remove("fieldResult");
        let o = match t {
            Unit::Embed { kind, .. } if kind == "tab" => json!({ "kind": "text", "text": "\t", "attributes": r }),
            Unit::Text { text, .. } => json!({ "kind": "text", "text": text, "attributes": r }),
            Unit::Embed { kind, payload, .. } => json!({ "kind": "embed", "embedKind": kind, "payload": payload, "attributes": r }),
        };
        if o["kind"] == "text"
            && let Some(i) = n.last_mut()
            && i["kind"] == "text"
            && canon(&i["attributes"]) == canon(&o["attributes"])
        {
            let joined = format!("{}{}", i["text"].as_str().unwrap_or(""), o["text"].as_str().unwrap_or(""));
            i["text"] = json!(joined);
        } else {
            n.push(o);
        }
    }
    canon(&Value::Array(n))
}

// ---- paragraph properties ----

const PARAGRAPH_DEFAULTS: [&str; 51] = [
    "paraId", "textId", "alignment", "spaceBefore", "spaceAfter", "lineSpacing", "lineSpacingRule", "spacingExplicit", "indentLeft", "indentRight", "indentFirstLine", "hangingIndent", "numPr", "numPrFromStyle", "listNumFmt", "listIsBullet", "listMarker", "listMarkerHidden", "listMarkerFontFamily",
    "listMarkerFontSize", "listMarkerSuffix", "listLevelNumFmts", "listAbstractNumId", "listStartOverride", "styleId", "borders", "shading", "tabs", "pageBreakBefore", "renderedPageBreakBefore", "keepNext", "keepLines", "widowControl", "contextualSpacing", "snapToGrid", "autoSpaceDE", "autoSpaceDN",
    "defaultTextFormatting", "sectionBreakType", "bidi", "outlineLevel", "bookmarks", "_originalFormatting", "_originalRunBoundaries", "_sectionProperties", "pPrIns", "pPrDel", "pPrChange", "", "", "",
];

/// `Pn`: a pilcrow's properties over the paragraph defaults.
fn paragraph_attrs(e: &Obj) -> Obj {
    let mut n = Obj::new();
    for k in PARAGRAPH_DEFAULTS.iter().filter(|k| !k.is_empty()) {
        n.insert((*k).into(), if *k == "hangingIndent" { json!(false) } else { Value::Null });
    }
    n.extend(e.clone());
    n.insert("styleId".into(), nn_undef(e, "pStyle").cloned().unwrap_or(Value::Null));
    n.insert("_sectionProperties".into(), nn_undef(e, "sectPr").cloned().unwrap_or(Value::Null));
    n.remove("pStyle");
    n.remove("sectPr");
    n
}

/// `xe`: the list is the style's.
fn list_from_style(e: &Obj) -> bool {
    present(e, "numPrFromStyle") && present(e, "numPr") && same_json(e.get("numPr"), e.get("numPrFromStyle"))
}

/// `en`: what the style gives dropped.
fn without_inherited(e: Obj, n: &Obj) -> Option<Obj> {
    let mut t = e;
    let keys: Vec<String> = t.keys().cloned().collect();
    for o in keys {
        if matches!(o.as_str(), "styleId" | "numPr" | "lineSpacing" | "lineSpacingRule" | "indentFirstLine" | "hangingIndent") {
            continue;
        }
        if n.contains_key(&o) && re(t.get(&o), n.get(&o)) {
            t.remove(&o);
        }
    }
    let rule = |o: &Obj| o.get("lineSpacingRule").filter(|v| !v.is_null()).cloned().unwrap_or(json!("auto"));
    if n.contains_key("lineSpacing") && !strict_ne(t.get("lineSpacing"), n.get("lineSpacing")) && rule(&t) == rule(n) {
        t.remove("lineSpacing");
        t.remove("lineSpacingRule");
    }
    (!t.is_empty()).then_some(t)
}

/// `Ae`: a paragraph's formatting: its source's, with what the editor
/// changed; a new paragraph's from its properties, without what its style
/// gives.
fn paragraph_formatting(e: &Obj, n: Option<&Obj>) -> Option<Obj> {
    if let Some(o) = e.get("_originalFormatting").and_then(obj).filter(|_| opt_truthy(e.get("_originalFormatting"))) {
        let mut i = o.clone();
        let inherited = |k: &str| !o.contains_key(k) && n.is_some_and(|n| n.contains_key(k) && re(e.get(k), n.get(k)));
        if !inherited("alignment") && strict_ne(e.get("alignment"), or_undef(o, "alignment")) {
            set(&mut i, "alignment", or_undef(e, "alignment").cloned());
        }
        if list_from_style(e) {
            i.remove("numPr");
            i.remove("numPrFromStyle");
        } else if strict_ne(e.get("numPr"), o.get("numPr")) && !same_json(e.get("numPr"), o.get("numPr")) {
            set(&mut i, "numPr", or_undef(e, "numPr").cloned());
            i.remove("numPrFromStyle");
        }
        if strict_ne(e.get("styleId"), or_undef(o, "styleId")) {
            set(&mut i, "styleId", or_undef(e, "styleId").cloned());
        }
        if !inherited("pageBreakBefore") && strict_ne(e.get("pageBreakBefore"), or_undef(o, "pageBreakBefore")) {
            set(&mut i, "pageBreakBefore", or_undef(e, "pageBreakBefore").cloned());
        }
        for k in ["widowControl", "autoSpaceDE", "autoSpaceDN"] {
            if !inherited(k) && strict_ne(e.get(k), nn_undef(o, k)) {
                set(&mut i, k, nn_undef(e, k).cloned());
            }
        }
        if !inherited("bidi") && strict_ne(e.get("bidi"), or_undef(o, "bidi")) {
            set(&mut i, "bidi", or_undef(e, "bidi").cloned());
        }
        return Some(i);
    }
    let any = ["alignment", "lineSpacing", "indentLeft", "indentRight", "indentFirstLine", "numPr", "styleId", "borders", "shading", "tabs", "contextualSpacing", "pageBreakBefore", "bidi"].iter().any(|k| opt_truthy(e.get(*k)))
        || ["spaceBefore", "spaceAfter", "spaceBeforeLines", "spaceAfterLines", "beforeAutospacing", "afterAutospacing", "outlineLevel", "widowControl", "autoSpaceDE", "autoSpaceDN"].iter().any(|k| present(e, k));
    if !any {
        return None;
    }
    let mut r = Obj::new();
    let truthy_keys = ["alignment"];
    for k in truthy_keys {
        set(&mut r, k, or_undef(e, k).cloned());
    }
    for k in ["spaceBefore", "spaceAfter", "spaceBeforeLines", "spaceAfterLines", "beforeAutospacing", "afterAutospacing"] {
        set(&mut r, k, nn_undef(e, k).cloned());
    }
    for k in ["lineSpacing", "lineSpacingRule", "indentLeft", "indentRight", "indentFirstLine", "hangingIndent"] {
        set(&mut r, k, or_undef(e, k).cloned());
    }
    set(&mut r, "numPr", if list_from_style(e) { None } else { or_undef(e, "numPr").cloned() });
    for k in ["styleId", "borders", "shading", "tabs"] {
        set(&mut r, k, or_undef(e, k).cloned());
    }
    set(&mut r, "outlineLevel", nn_undef(e, "outlineLevel").cloned());
    for k in ["contextualSpacing", "pageBreakBefore"] {
        set(&mut r, k, or_undef(e, k).cloned());
    }
    for k in ["widowControl", "autoSpaceDE", "autoSpaceDN"] {
        set(&mut r, k, nn_undef(e, k).cloned());
    }
    set(&mut r, "bidi", or_undef(e, "bidi").cloned());
    match n {
        Some(n) => without_inherited(r, n),
        None => Some(r),
    }
}

// ---- styles (`chunk-47Q2FMFF.mjs`) ----

fn styles_list(s: &Value) -> &[Value] {
    arr(&s["styles"])
}

/// The default paragraph style: the one marked default, or `Normal`.
fn default_style(s: &Value) -> Option<&Value> {
    let list = styles_list(s);
    list.iter().find(|e| e["type"] == "paragraph" && opt_truthy(e.get("default"))).or_else(|| list.iter().find(|e| e["styleId"] == "Normal"))
}

/// `g`: paragraph formatting merged.
fn merge_paragraph(i: Option<Obj>, t: &Obj) -> Obj {
    let mut e = i.unwrap_or_default();
    for (r, n) in t {
        if r == "runProperties" {
            continue;
        }
        if matches!(r.as_str(), "borders" | "numPr" | "frame") {
            let mut a = e.get(r).and_then(obj).cloned().unwrap_or_default();
            if let Some(o) = n.as_object() {
                a.extend(o.clone());
            }
            e.insert(r.clone(), Value::Object(a));
        } else {
            e.insert(r.clone(), n.clone());
        }
    }
    e
}

/// `resolveParagraphStyle(id).paragraphFormatting`.
fn resolve_paragraph_style(s: &Value, id: Option<&str>) -> Option<Obj> {
    let mut r: Option<Obj> = s.pointer("/docDefaults/pPr").and_then(obj).cloned();
    let style = id.and_then(|id| styles_list(s).iter().find(|e| e["styleId"] == id)).or_else(|| default_style(s));
    if let Some(p) = style.and_then(|n| n["pPr"].as_object()) {
        r = Some(merge_paragraph(r, p));
    }
    r
}

// ---- tables ----

fn table_defaults() -> Obj {
    ["styleId", "width", "widthType", "justification", "columnWidths", "tableLayout", "floating", "cellMargins", "look", "bidi", "_originalFormatting", "tblPrChange"].iter().map(|k| ((*k).to_owned(), Value::Null)).collect()
}

fn row_defaults() -> Obj {
    let mut o: Obj = ["height", "heightRule", "_originalFormatting", "trIns", "trDel", "trPrChange"].iter().map(|k| ((*k).to_owned(), Value::Null)).collect();
    o.insert("isHeader".into(), json!(false));
    o
}

fn cell_defaults() -> Obj {
    let mut o: Obj = ["colwidth", "width", "widthType", "verticalAlign", "backgroundColor", "borders", "margins", "textDirection", "_originalFormatting", "_originalResolvedFill", "cellMarker", "tcPrChange"].iter().map(|k| ((*k).to_owned(), Value::Null)).collect();
    o.insert("colspan".into(), json!(1));
    o.insert("rowspan".into(), json!(1));
    o.insert("noWrap".into(), json!(false));
    o
}

fn dxa(v: &Value) -> Value {
    json!({ "value": v, "type": "dxa" })
}

/// `Pe`.
fn table_margins(e: &Obj) -> Value {
    let mut o = Obj::new();
    for k in ["top", "bottom", "left", "right"] {
        set(&mut o, k, nn_undef(e, k).map(dxa));
    }
    Value::Object(o)
}

/// `Ie`.
fn cell_margins(e: &Obj) -> Value {
    table_margins(e)
}

fn width_of(e: &Obj) -> Option<Value> {
    (present(e, "width") || opt_truthy(e.get("widthType"))).then(|| json!({ "value": nn_undef(e, "width").cloned().unwrap_or(json!(0)), "type": or_undef(e, "widthType").cloned().unwrap_or(json!("dxa")) }))
}

/// `Fe`: a table's properties.
fn table_formatting(e: &Obj) -> Option<Obj> {
    if let Some(t) = e.get("_originalFormatting").and_then(obj) {
        let mut r = t.clone();
        for (k, m) in [("styleId", "styleId"), ("justification", "justification"), ("floating", "floating"), ("tableLayout", "layout"), ("look", "look"), ("bidi", "bidi")] {
            if strict_ne(e.get(k), or_undef(t, m)) {
                set(&mut r, m, or_undef(e, k).cloned());
            }
        }
        let tw = t.get("width").and_then(obj);
        if strict_ne(e.get("width"), tw.and_then(|w| w.get("value"))) || strict_ne(e.get("widthType"), tw.and_then(|w| w.get("type"))) {
            set(&mut r, "width", width_of(e));
        }
        if let Some(m) = e.get("cellMargins").and_then(obj) {
            r.insert("cellMargins".into(), table_margins(m));
        }
        return Some(r);
    }
    let any = opt_truthy(e.get("styleId")) || present(e, "width") || ["widthType", "justification", "tableLayout", "floating", "cellMargins", "look", "bidi"].iter().any(|k| opt_truthy(e.get(*k)));
    if !any {
        return None;
    }
    let mut r = Obj::new();
    set(&mut r, "styleId", or_undef(e, "styleId").cloned());
    set(&mut r, "width", width_of(e));
    set(&mut r, "justification", or_undef(e, "justification").cloned());
    set(&mut r, "layout", or_undef(e, "tableLayout").cloned());
    set(&mut r, "floating", or_undef(e, "floating").cloned());
    set(&mut r, "cellMargins", e.get("cellMargins").and_then(obj).map(table_margins));
    set(&mut r, "look", or_undef(e, "look").cloned());
    set(&mut r, "bidi", or_undef(e, "bidi").cloned());
    Some(r)
}

/// `ve`: a row's properties.
fn row_formatting(e: &Obj) -> Option<Obj> {
    let height = |e: &Obj| or_undef(e, "height").map(dxa);
    if let Some(n) = e.get("_originalFormatting").and_then(obj) {
        let mut t = n.clone();
        if strict_ne(e.get("height"), n.get("height").and_then(obj).and_then(|h| h.get("value")).filter(|v| truthy(v))) {
            set(&mut t, "height", height(e));
        }
        if strict_ne(e.get("heightRule"), or_undef(n, "heightRule")) {
            set(&mut t, "heightRule", or_undef(e, "heightRule").cloned());
        }
        if strict_ne(e.get("isHeader"), or_undef(n, "header")) {
            set(&mut t, "header", or_undef(e, "isHeader").cloned());
        }
        return Some(t);
    }
    if !opt_truthy(e.get("height")) && !opt_truthy(e.get("isHeader")) {
        return None;
    }
    let mut r = Obj::new();
    set(&mut r, "height", height(e));
    set(&mut r, "heightRule", or_undef(e, "heightRule").cloned());
    set(&mut r, "header", or_undef(e, "isHeader").cloned());
    Some(r)
}

/// `Be`: a cell's properties.
fn cell_formatting(e: &Obj) -> Option<Obj> {
    let colspan = e.get("colspan").and_then(Value::as_f64).unwrap_or(1.0);
    let rowspan = e.get("rowspan").and_then(Value::as_f64).unwrap_or(1.0);
    let width = || present(e, "width").then(|| json!({ "value": e["width"], "type": or_undef(e, "widthType").cloned().unwrap_or(json!("dxa")) }));
    if let Some(t) = e.get("_originalFormatting").and_then(obj) {
        let mut r = t.clone();
        if colspan > 1.0 {
            r.insert("gridSpan".into(), jnum(colspan));
        }
        if let Some(w) = width() {
            r.insert("width".into(), w);
        }
        if strict_ne(e.get("verticalAlign"), or_undef(t, "verticalAlign")) {
            set(&mut r, "verticalAlign", or_undef(e, "verticalAlign").cloned());
        }
        if opt_truthy(e.get("backgroundColor")) {
            let keep = !strict_ne(e.get("_originalResolvedFill"), e.get("backgroundColor")) && opt_truthy(t.get("shading"));
            r.insert("shading".into(), if keep { t["shading"].clone() } else { json!({ "fill": { "rgb": e["backgroundColor"] } }) });
        } else if opt_truthy(t.get("shading")) {
            r.remove("shading");
        }
        if opt_truthy(e.get("borders")) {
            r.insert("borders".into(), e["borders"].clone());
        }
        if let Some(m) = e.get("margins").and_then(obj).filter(|_| opt_truthy(e.get("margins"))) {
            r.insert("margins".into(), cell_margins(m));
        }
        if strict_ne(e.get("textDirection"), or_undef(t, "textDirection")) {
            set(&mut r, "textDirection", or_undef(e, "textDirection").cloned());
        }
        return Some(r);
    }
    let any = colspan > 1.0 || rowspan > 1.0 || present(e, "width") || ["verticalAlign", "backgroundColor", "borders", "margins", "textDirection"].iter().any(|k| opt_truthy(e.get(*k)));
    if !any {
        return None;
    }
    let mut r = Obj::new();
    set(&mut r, "gridSpan", (colspan > 1.0).then(|| jnum(colspan)));
    set(&mut r, "width", width());
    set(&mut r, "verticalAlign", or_undef(e, "verticalAlign").cloned());
    set(&mut r, "textDirection", or_undef(e, "textDirection").cloned());
    set(&mut r, "shading", or_undef(e, "backgroundColor").map(|c| json!({ "fill": { "rgb": c } })));
    set(&mut r, "borders", e.get("borders").filter(|v| !v.is_null()).cloned());
    set(&mut r, "margins", e.get("margins").and_then(obj).filter(|_| opt_truthy(e.get("margins"))).map(cell_margins));
    Some(r)
}

/// `Rn`: a table's borders from its corner cells', when it has none.
fn outer_borders(e: &[Value]) -> Option<Value> {
    let n = e.first().map(|r| arr(&r["cells"]))?;
    let t = e.last().map(|r| arr(&r["cells"]))?;
    let r = n.first().and_then(|c| c.pointer("/formatting/borders")).filter(|b| truthy(b))?;
    let mut o = Obj::new();
    set(&mut o, "top", r.get("top").cloned());
    set(&mut o, "left", r.get("left").cloned());
    set(&mut o, "bottom", t.first().and_then(|c| c.pointer("/formatting/borders/bottom")).cloned());
    set(&mut o, "right", n.last().and_then(|c| c.pointer("/formatting/borders/right")).cloned());
    set(&mut o, "insideH", if e.len() > 1 { r.get("bottom").cloned() } else { None });
    set(&mut o, "insideV", if n.len() > 1 { r.get("right").cloned() } else { None });
    (!o.is_empty()).then_some(Value::Object(o))
}

/// `An`: a vertical merge of one cell is no merge.
fn clean_vmerge(rows: &mut [Value]) {
    let mut cols: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
    for (o, r) in rows.iter().enumerate() {
        let mut i = 0;
        for (ci, l) in arr(&r["cells"]).iter().enumerate() {
            let s = i;
            i += l.pointer("/formatting/gridSpan").and_then(Value::as_u64).unwrap_or(1) as usize;
            if l.pointer("/formatting/vMerge").is_some_and(truthy) {
                match cols.iter_mut().find(|(c, _)| *c == s) {
                    Some((_, v)) => v.push((o, ci)),
                    None => cols.push((s, vec![(o, ci)])),
                }
            }
        }
    }
    let clear = |rows: &mut [Value], (r, c): (usize, usize)| {
        let cell = &mut rows[r]["cells"][c];
        if let Some(f) = cell["formatting"].as_object_mut() {
            f.remove("vMerge");
            if f.is_empty() {
                cell.as_object_mut().expect("cell").remove("formatting");
            }
        }
    };
    for (_, list) in cols {
        let mut open: Option<(usize, usize)> = None;
        let (mut count, mut last) = (0, 0usize);
        for (r, c) in list {
            let v = rows[r]["cells"][c].pointer("/formatting/vMerge").and_then(Value::as_str).map(str::to_owned);
            match v.as_deref() {
                Some("restart") => {
                    if let Some(o) = open.take().filter(|_| count < 2) {
                        clear(rows, o);
                    }
                    open = Some((r, c));
                    count = 1;
                    last = r;
                }
                Some("continue") => {
                    if open.is_some() && r == last + 1 {
                        count += 1;
                        last = r;
                    } else {
                        if let Some(o) = open.take().filter(|_| count < 2) {
                            clear(rows, o);
                        }
                        count = 0;
                        clear(rows, (r, c));
                    }
                }
                _ => {}
            }
        }
        if let Some(o) = open.filter(|_| count < 2) {
            clear(rows, o);
        }
    }
}

// ---- content controls ----

/// `le`: a content control's properties from its embed.
fn sdt_properties(e: &Obj) -> Value {
    let mut n = Obj::new();
    n.insert("sdtType".into(), json!(e.get("sdtType").and_then(st).unwrap_or("richText")));
    if let Some(id) = e.get("id").filter(|v| v.is_number()) {
        n.insert("id".into(), id.clone());
    }
    for k in ["alias", "tag", "placeholder", "dateFormat"] {
        if present(e, k) {
            n.insert(k.into(), json!(e[k].as_str().map(str::to_owned).unwrap_or_else(|| e[k].to_string())));
        }
    }
    if present(e, "lock") {
        n.insert("lock".into(), e["lock"].clone());
    }
    if opt_truthy(e.get("showingPlaceholder")) {
        n.insert("showingPlaceholder".into(), json!(true));
    }
    if let Some(l) = e.get("listItems").and_then(st).filter(|s| !s.is_empty())
        && let Ok(v) = serde_json::from_str::<Value>(l)
    {
        n.insert("listItems".into(), v);
    }
    if present(e, "checked") {
        n.insert("checked".into(), e["checked"].clone());
    }
    if let Some(l) = e.get("dataBinding").and_then(st).filter(|s| !s.is_empty())
        && let Ok(v) = serde_json::from_str::<Value>(l)
    {
        n.insert("dataBinding".into(), v);
    }
    if let Some(b) = e.get("multiLine").filter(|v| v.is_boolean()) {
        n.insert("multiLine".into(), b.clone());
    }
    for k in ["rawPropertiesXml", "rawEndPropertiesXml"] {
        if present(e, k) {
            n.insert(k.into(), json!(e[k].as_str().map(str::to_owned).unwrap_or_else(|| e[k].to_string())));
        }
    }
    Value::Object(n)
}

/// `Ke`: a content control's value set in the editor.
fn control_value(e: Option<&Value>) -> Option<Value> {
    let n = e?.as_object()?;
    match n.get("kind").and_then(st)? {
        "checkbox" if n.get("checked").is_some_and(Value::is_boolean) => Some(json!({ "kind": "checkbox", "checked": n["checked"] })),
        "dropdown" if n.get("value").is_some_and(Value::is_string) => Some(json!({ "kind": "dropdown", "value": n["value"] })),
        "date" if n.get("date").is_some_and(Value::is_string) => Some(json!({ "kind": "date", "date": n["date"] })),
        _ => None,
    }
}

/// `V`: an attribute of the first `tag` in `xml`.
fn xml_attr(xml: &str, tag: &str, name: &str) -> Option<String> {
    let at = xml.find(&format!("<{tag}")).filter(|at| xml[at + tag.len() + 1..].starts_with([' ', '>', '/', '\t', '\n', '\r']))?;
    let end = xml[at..].find('>')? + at;
    let t = &xml[at..=end];
    let k = t.find(&format!("{name}=\""))? + name.len() + 2;
    Some(t[k..].split('"').next()?.to_owned())
}

/// `te`: `xml` with an attribute of its first `tag` set.
fn set_xml_attr(xml: &str, tag: &str, name: &str, value: &str) -> String {
    let Some(at) = xml.find(&format!("<{tag}")).filter(|at| xml[at + tag.len() + 1..].starts_with([' ', '>', '/', '\t', '\n', '\r'])) else { return xml.to_owned() };
    let Some(end) = xml[at..].find('>').map(|e| e + at) else { return xml.to_owned() };
    let l = &xml[at..=end];
    let closed = l.ends_with("/>");
    let a = &l[1..l.len() - if closed { 2 } else { 1 }];
    let key = format!("{name}=\"");
    let d = match a.find(&key) {
        Some(k) => {
            let rest = &a[k + key.len()..];
            let close = rest.find('"').unwrap_or(rest.len());
            format!("{}{name}=\"{value}\"{}", &a[..k], &rest[(close + 1).min(rest.len())..])
        }
        None => format!("{a} {name}=\"{value}\""),
    };
    format!("{}<{d}{}{}", &xml[..at], if closed { "/>" } else { ">" }, &xml[end + 1..])
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `re`: a control's one paragraph of text.
fn control_paragraph(text: &str, font: Option<&str>) -> Value {
    if text.is_empty() {
        return json!({ "type": "paragraph", "content": [] });
    }
    let mut run = json!({ "type": "run", "content": [{ "type": "text", "text": text }] });
    if let Some(f) = font.filter(|f| !f.is_empty()) {
        run["formatting"] = json!({ "fontFamily": { "ascii": f, "hAnsi": f, "eastAsia": f, "cs": f } });
    }
    json!({ "type": "paragraph", "content": [run] })
}

/// `oe`: properties no longer showing the placeholder.
fn shown(e: &Value, xml: &str) -> Obj {
    let mut o = e.as_object().cloned().unwrap_or_default();
    o.insert("showingPlaceholder".into(), json!(false));
    let mut t = xml.to_owned();
    while let Some(a) = t.find("<w:showingPlcHdr") {
        let Some(end) = t[a..].find('>').map(|e| e + a) else { break };
        let cut = if t[..=end].ends_with("/>") {
            end + 1
        } else {
            t[a..].find("</w:showingPlcHdr>").map(|c| c + a + "</w:showingPlcHdr>".len()).unwrap_or(end + 1)
        };
        t.replace_range(a..cut, "");
    }
    set(&mut o, "rawPropertiesXml", (!t.is_empty()).then(|| json!(t)));
    o
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn valid_date(y: i32, m: u32, d: u32) -> bool {
    y != 0 && (1..=12).contains(&m) && d >= 1 && d <= [31, if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m as usize - 1]
}

/// `Ge`: a date in a control's format (month names in English).
fn format_date(iso: &str, format: Option<&str>) -> String {
    let parts: Vec<i32> = iso.get(..10).unwrap_or(iso).split('-').map(|p| p.parse().unwrap_or(0)).collect();
    let (y, m, d) = (parts.first().copied().unwrap_or(0), parts.get(1).copied().unwrap_or(0) as u32, parts.get(2).copied().unwrap_or(0) as u32);
    if !valid_date(y, m, d) {
        return iso.to_owned();
    }
    let l = format.filter(|f| !f.trim().is_empty()).unwrap_or("M/d/yyyy");
    let month = MONTHS[m as usize - 1];
    let mut out = String::new();
    let mut rest = l;
    while !rest.is_empty() {
        let (tok, val): (&str, String) = if rest.starts_with("yyyy") {
            ("yyyy", y.to_string())
        } else if rest.starts_with("yy") {
            ("yy", format!("{:02}", y.rem_euclid(100)))
        } else if rest.starts_with("MMMM") {
            ("MMMM", month.to_owned())
        } else if rest.starts_with("MMM") {
            ("MMM", month[..3].to_owned())
        } else if rest.starts_with("MM") {
            ("MM", format!("{m:02}"))
        } else if rest.starts_with('M') {
            ("M", m.to_string())
        } else if rest.starts_with("dd") {
            ("dd", format!("{d:02}"))
        } else if rest.starts_with('d') {
            ("d", d.to_string())
        } else {
            let c = rest.chars().next().expect("non-empty");
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        };
        out += &val;
        rest = &rest[tok.len()..];
    }
    out
}

/// `ie`: a content control with a value set in the editor; `Err` where the
/// web refuses it (the control is then saved as it is).
fn apply_control_value(e: &Value, n: &Value) -> Result<(Value, Vec<Value>), String> {
    let t = e["rawPropertiesXml"].as_str().unwrap_or("").to_owned();
    let sdt = e["sdtType"].as_str().unwrap_or("");
    let state = |extra: Value| {
        let mut s = e["controlState"].as_object().cloned().unwrap_or_default();
        s.extend(extra.as_object().cloned().unwrap_or_default());
        Value::Object(s)
    };
    match n["kind"].as_str() {
        Some("dropdown") => {
            if sdt != "dropDownList" && sdt != "comboBox" {
                return Err(format!("Control is '{sdt}', not a dropdown/combo box."));
            }
            let value = n["value"].as_str().unwrap_or("");
            let items = arr(&e["listItems"]);
            let r = items.iter().position(|u| u["value"] == value || u["displayText"] == value);
            if r.is_none() && sdt == "dropDownList" {
                return Err(format!("'{value}' is not one of the control's list items."));
            }
            let o = r.and_then(|i| items[i]["value"].as_str()).unwrap_or(value).to_owned();
            let i = r.and_then(|i| items[i]["displayText"].as_str()).unwrap_or(value).to_owned();
            let a = set_xml_attr(&t, if sdt == "comboBox" { "w:comboBox" } else { "w:dropDownList" }, "w:lastValue", &xml_escape(&o));
            let mut p = shown(e, &a);
            p.insert("listLastValue".into(), json!(o));
            let mut extra = json!({ "value": i, "selectedValue": o, "placeholder": false });
            if let Some(l) = r {
                extra["selectedIndex"] = json!(l);
            }
            p.insert("controlState".into(), state(extra));
            Ok((Value::Object(p), vec![control_paragraph(&i, None)]))
        }
        Some("checkbox") => {
            if sdt != "checkbox" {
                return Err(format!("Control is '{sdt}', not a checkbox."));
            }
            if xml_attr(&t, "w14:checked", "w14:val").is_none() {
                return Err("Checkbox control has no <w14:checked> state to update (not a Word checkbox).".into());
            }
            let checked = n["checked"].as_bool().unwrap_or(false);
            let r = if checked { "w14:checkedState" } else { "w14:uncheckedState" };
            let glyph = xml_attr(&t, r, "w14:val").and_then(|h| u32::from_str_radix(&h, 16).ok()).filter(|c| *c <= 0x10FFFF && !(0xD800..=0xDFFF).contains(c)).and_then(char::from_u32).map(String::from).unwrap_or_else(|| if checked { "\u{2612}" } else { "\u{2610}" }.into());
            let font = xml_attr(&t, r, "w14:font");
            let l = set_xml_attr(&t, "w14:checked", "w14:val", if checked { "1" } else { "0" });
            let mut p = shown(e, &l);
            p.insert("checked".into(), json!(checked));
            p.insert("controlState".into(), state(json!({ "checked": checked, "value": glyph, "placeholder": false })));
            Ok((Value::Object(p), vec![control_paragraph(&glyph, font.as_deref())]))
        }
        Some("date") => {
            if sdt != "date" {
                return Err(format!("Control is '{sdt}', not a date control."));
            }
            let raw = n["date"].as_str().unwrap_or("");
            let r = raw.get(..10).unwrap_or(raw);
            let ymd: Vec<&str> = r.split('-').collect();
            let ok = r.len() == 10 && ymd.len() == 3 && ymd.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())) && valid_date(ymd[0].parse().unwrap_or(0), ymd[1].parse().unwrap_or(0), ymd[2].parse().unwrap_or(0));
            if !ok {
                return Err(format!("Date must be ISO yyyy-mm-dd, got '{raw}'."));
            }
            let s = format!("{r}T00:00:00");
            let a = set_xml_attr(&t, "w:date", "w:fullDate", &s);
            let u = e.pointer("/dateState/format").and_then(st).map(str::to_owned).or_else(|| e["dateFormat"].as_str().map(str::to_owned)).or_else(|| xml_attr(&t, "w:dateFormat", "w:val"));
            let f = format_date(r, u.as_deref());
            let mut p = shown(e, &a);
            set(&mut p, "dateFormat", u.clone().map(|u| json!(u)));
            let mut ds = e["dateState"].as_object().cloned().unwrap_or_default();
            ds.insert("fullDate".into(), json!(s));
            set(&mut ds, "format", u.map(|u| json!(u)));
            p.insert("dateState".into(), Value::Object(ds));
            p.insert("controlState".into(), state(json!({ "value": r, "placeholder": false })));
            Ok((Value::Object(p), vec![control_paragraph(&f, None)]))
        }
        _ => Err("unknown value".into()),
    }
}

/// Every paragraph's session key out of `v`, as `(key, paraId)`; the keys
/// are taken out.
pub(crate) fn take_session_keys(v: &mut Value, out: &mut Vec<(String, Option<String>)>) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|v| take_session_keys(v, out)),
        Value::Object(o) => {
            if let Some(Value::String(k)) = o.remove(SESSION_KEY) {
                out.push((k, o.get("paraId").and_then(Value::as_str).map(str::to_owned)));
            }
            o.values_mut().for_each(|v| take_session_keys(v, out));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_numbers_and_rounding() {
        assert_eq!(jnum(3.0), json!(3));
        assert_eq!(jnum(2.5), json!(2.5));
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(emu(96.0), json!(914_400));
        assert_eq!(numeric_id(Some(&json!("12"))), 12.0);
        assert_eq!(numeric_id(Some(&json!("7:12"))), fnv53("7:12"));
    }

    #[test]
    fn marks_split_runs_at_their_offsets() {
        let runs = vec![json!({ "type": "run", "content": [{ "type": "text", "text": "Hello world" }] })];
        let marks = vec![Mark { id: 3.0, kind: MarkKind::Start, offset: 6, name: None, col_first: None, col_last: None }, Mark { id: 3.0, kind: MarkKind::End, offset: 11, name: None, col_first: None, col_last: None }];
        let out = place_marks(runs, marks, &comment_mark);
        let kinds: Vec<&str> = out.iter().map(ty).collect();
        assert_eq!(kinds, ["run", "commentRangeStart", "run", "commentRangeEnd"]);
        assert_eq!(out[0]["content"][0]["text"], "Hello ");
        assert_eq!(out[2]["content"][0]["text"], "world");
    }

    #[test]
    fn run_keys_parse_back_to_marks() {
        let m = marks_of(Some(r#"bold:{}|fontSize:{"size":22,"sizeCs":null}|highlight:{"color":"yellow"}"#)).unwrap();
        assert_eq!(Value::Object(m), json!({ "bold": true, "fontSize": { "size": 22 }, "highlight": "yellow" }));
        assert_eq!(marks_of(Some("")), Some(Obj::new()));
        assert_eq!(marks_of(Some("nocolon")), None);
    }

    #[test]
    fn content_control_dates_format_as_word_does() {
        assert_eq!(format_date("2026-03-05", Some("dd MMMM yyyy")), "05 March 2026");
        assert_eq!(format_date("2026-03-05", None), "3/5/2026");
        assert_eq!(set_xml_attr(r#"<w:sdtPr><w:date w:fullDate="x"/></w:sdtPr>"#, "w:date", "w:fullDate", "y"), r#"<w:sdtPr><w:date w:fullDate="y"/></w:sdtPr>"#);
    }
}
