//! The text helpers routines share with Node: `approvedText` (v1/agent.mjs),
//! `paramsProblem` and `paramsSummary` (v1/params.mjs), JavaScript's
//! `String(v)`, `Number(v)` and template-literal forms, `isEmail`, and the
//! v0 tool name (`mcpName`). Lengths and cuts count UTF-16 units where
//! Node's do.

use serde_json::Value;

use crate::js;

pub const TRUNCATED: &str = "…[truncated]";

/// `s.length`.
pub fn len16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `String(v)` of a value that may be missing (`undefined`).
pub fn tpl(v: Option<&Value>) -> String {
    v.map_or_else(|| "undefined".to_owned(), js::string_of)
}

/// JavaScript's `trim()`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(js::is_space)
}

/// `isString(v) && v` (a non-empty string), else `None`.
pub fn nonempty(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// `isObject(v)` (jcs.mjs): a plain object.
pub fn is_obj(v: Option<&Value>) -> bool {
    v.is_some_and(Value::is_object)
}

/// Plugin text for the model: heading markers removed, at most `cap`
/// characters (code points), cut at a word boundary with the marker
/// (`approvedText`, 18.3).
pub fn approved_text(text: &str, cap: usize) -> String {
    let clean = strip_headings(text);
    let clean = trim(&clean);
    let cs: Vec<char> = clean.chars().collect();
    if cs.len() <= cap {
        return clean.to_owned();
    }
    let room: Vec<char> = cs[..cap.saturating_sub(TRUNCATED.chars().count())].to_vec();
    // /\s\S*$/: the last space, everything after it not space
    let cut = room.iter().rposition(|c| js::is_space(*c));
    let head: String = match cut {
        Some(i) if i > 0 => room[..i].iter().collect(),
        _ => room.iter().collect(),
    };
    format!("{}{TRUNCATED}", head.trim_end_matches(js::is_space))
}

/// `.replace(/^[ \t]*#{1,6}[ \t]+/gm, "")`.
fn strip_headings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        // one line, its terminator kept
        let end = rest.find(['\n', '\r', '\u{2028}', '\u{2029}']).map(|i| i + rest[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(rest.len());
        let (line, next) = rest.split_at(end);
        let body = line.trim_start_matches([' ', '\t']);
        let hashes = body.len() - body.trim_start_matches('#').len();
        let after = &body[hashes..];
        let spaced = after.len() - after.trim_start_matches([' ', '\t']).len();
        // a run of seven or more `#` is no heading marker
        if (1..=6).contains(&hashes) && spaced > 0 {
            out.push_str(&after[spaced..]);
        } else {
            out.push_str(line);
        }
        if next.is_empty() {
            break;
        }
        rest = next;
    }
    out
}

/// `Number(v)` for a JSON value.
pub fn number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => {
            let t = trim(s);
            if t.is_empty() {
                return 0.0;
            }
            let hex = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X"));
            if let Some(h) = hex {
                return u64::from_str_radix(h, 16).map_or(f64::NAN, |n| n as f64);
            }
            match t {
                "Infinity" | "+Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                _ if t.bytes().all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) => t.parse().unwrap_or(f64::NAN),
                _ => f64::NAN,
            }
        }
        Some(Value::Array(a)) => match a.as_slice() {
            [] => 0.0,
            [one] => number(Some(&Value::String(js::string_of(one)))),
            _ => f64::NAN,
        },
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// A number as JSON carries it: an integral one as an integer (so values
/// compare as Node's do), else a float.
pub fn num(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        Value::from(x as i64)
    } else {
        Value::from(x)
    }
}

/// `num` of a value that may be missing (`null`).
pub fn num_or_null(x: Option<f64>) -> Value {
    x.map_or(Value::Null, num)
}

/// `Math.round(x)`.
pub fn round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v.trim())` of a string.
pub fn is_email(v: Option<&Value>) -> bool {
    let Some(s) = v.and_then(Value::as_str) else { return false };
    let s = trim(s);
    if s.chars().any(js::is_space) {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else { return false };
    if local.is_empty() || domain.contains('@') {
        return false;
    }
    let chars: Vec<char> = domain.chars().collect();
    chars.iter().enumerate().any(|(i, c)| *c == '.' && i > 0 && i + 1 < chars.len())
}

/// `mcpName(id, tool)` (mcp.mjs): `mcp_<id>_<tool>`, other characters
/// `_`, at most 64.
pub fn mcp_name(id: &str, tool: &str) -> String {
    let raw = format!("mcp_{id}_{tool}");
    let units: Vec<u16> = raw.encode_utf16().map(|u| if u < 128 && ((u as u8).is_ascii_alphanumeric() || u == b'_' as u16 || u == b'-' as u16) { u } else { b'_' as u16 }).take(64).collect();
    String::from_utf16_lossy(&units)
}

// ── params (v1/params.mjs) ───────────────────────────────────────────────

pub const PARAMS_MAX_BYTES: usize = 4096;

/// `Number.isInteger(v)`.
fn integer(v: &Value) -> bool {
    v.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

/// `valueProblem(p, v)`.
fn value_problem(p: &Value, v: &Value) -> String {
    let ty = p.get("type").and_then(Value::as_str);
    if ty == Some("array") {
        let Some(list) = v.as_array() else { return "must be a list".to_owned() };
        if let Some(max) = p.get("maxItems").filter(|m| js::truthy(m)) {
            if (list.len() as f64) > max.as_f64().unwrap_or(f64::NAN) {
                return format!("has more than {} items", js::string_of(max));
            }
        }
        let items = p.get("items").cloned().unwrap_or(Value::Null);
        for x in list {
            let why = value_problem(&items, x);
            if !why.is_empty() {
                return format!("item {why}");
            }
        }
        return String::new();
    }
    let ok = match ty {
        Some("string") => v.is_string(),
        Some("integer") => integer(v),
        Some("number") => v.as_f64().is_some_and(f64::is_finite),
        Some("boolean") => v.is_boolean(),
        _ => false,
    };
    if !ok {
        return format!("must be a {}", tpl(p.get("type")));
    }
    if let Some(e) = p.get("enum").filter(|e| js::truthy(e)) {
        let found = e.as_array().is_some_and(|list| list.iter().any(|x| same_value(x, v)));
        if !found {
            let shown: Vec<String> = e.as_array().into_iter().flatten().map(|x| if x.is_null() { String::new() } else { js::string_of(x) }).collect();
            return format!("must be one of {}", shown.join(", "));
        }
    }
    if let (Some(max), Some(s)) = (p.get("maxLength").filter(|m| js::truthy(m)), v.as_str()) {
        if (len16(s) as f64) > max.as_f64().unwrap_or(f64::NAN) {
            return format!("is longer than {}", js::string_of(max));
        }
    }
    String::new()
}

/// `Array.prototype.includes`'s SameValueZero on JSON values.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(_) | Value::Object(_), _) => false,
        _ => a == b,
    }
}

/// Why `params` don't fit `schema` ("" when they do). A routine without a
/// schema takes no params (`paramsProblem`).
pub fn params_problem(schema: Option<&Value>, params: Option<&Value>) -> String {
    let Some(obj) = params.and_then(Value::as_object) else { return "params must be an object".to_owned() };
    let p = params.expect("an object");
    if len16(&js::stringify(p)) > PARAMS_MAX_BYTES {
        return format!("params are over {PARAMS_MAX_BYTES} bytes");
    }
    let schema = schema.filter(|s| js::truthy(s));
    let Some(schema) = schema else { return if obj.is_empty() { String::new() } else { "this routine takes no params".to_owned() } };
    let props = schema.get("properties").and_then(Value::as_object);
    let names: Vec<&String> = props.map(|p| js::ordered(p).into_iter().map(|(k, _)| k).collect()).unwrap_or_default();
    let prop = |k: &str| props.and_then(|p| p.get(k)).filter(|v| js::truthy(v));
    for (k, _) in js::ordered(obj) {
        if prop(k).is_none() {
            return format!("params.{} is not one of {}", js::slice_utf16(k, 40), names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", "));
        }
    }
    for k in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
        let k = js::string_of(k);
        if !obj.contains_key(&k) {
            return format!("params.{k} is required");
        }
    }
    for (k, v) in js::ordered(obj) {
        let why = value_problem(prop(k).expect("checked above"), v);
        if !why.is_empty() {
            return format!("params.{k} {why}");
        }
    }
    String::new()
}

/// `clip(s, n)`.
fn clip(s: &str, n: usize) -> String {
    if len16(s) > n {
        format!("{}…", js::slice_utf16(s, n - 1))
    } else {
        s.to_owned()
    }
}

/// One line for an instance's params, in the schema's order (`paramsSummary`):
/// "taxpayers: tp_1…, tp_2 · forms: 2550Q"; "" for none.
pub fn params_summary(schema: Option<&Value>, params: &Value) -> String {
    let obj = params.as_object();
    let keys: Vec<String> = match schema.filter(|s| js::truthy(s)) {
        Some(s) => {
            s.get("properties").and_then(Value::as_object).map(|p| js::ordered(p).into_iter().map(|(k, _)| k.clone()).filter(|k| obj.is_some_and(|o| o.contains_key(k))).collect()).unwrap_or_default()
        }
        None => obj.map(|o| js::ordered(o).into_iter().map(|(k, _)| k.clone()).collect()).unwrap_or_default(),
    };
    let parts: Vec<String> = keys
        .iter()
        .map(|k| {
            let v = &params[k.as_str()];
            let shown = match v {
                Value::Array(items) => items.iter().map(|x| clip(&js::string_of(x), 24)).collect::<Vec<_>>().join(", "),
                v => clip(&js::string_of(v), 40),
            };
            format!("{k}: {shown}")
        })
        .collect();
    clip(&parts.join(" · "), 160)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn approved_text_strips_headings_and_cuts_at_a_word() {
        assert_eq!(approved_text("  ## Acme Corp  ", 60), "Acme Corp");
        assert_eq!(approved_text("# a\n  ### b\n####### c", 60), "a\nb\n####### c");
        assert_eq!(approved_text("one two three four", 15), "one…[truncated]");
        assert_eq!(approved_text("abcdefghijklmnopqrst", 15), "abc…[truncated]");
    }

    #[test]
    fn params_are_checked_and_summed_up_as_node_does() {
        let schema = json!({"type": "object", "properties": {"q": {"type": "string", "maxLength": 40}, "n": {"type": "integer", "enum": [1, 2]}}, "required": ["q"]});
        assert_eq!(params_problem(Some(&schema), Some(&json!({"zzz": 1}))), "params.zzz is not one of q, n");
        assert_eq!(params_problem(Some(&schema), Some(&json!({"q": 5}))), "params.q must be a string");
        assert_eq!(params_problem(Some(&schema), Some(&json!([]))), "params must be an object");
        assert_eq!(params_problem(Some(&schema), Some(&json!({}))), "params.q is required");
        assert_eq!(params_problem(Some(&schema), Some(&json!({"q": "x".repeat(41)}))), "params.q is longer than 40");
        assert_eq!(params_problem(Some(&schema), Some(&json!({"q": "a", "n": 3}))), "params.n must be one of 1, 2");
        assert_eq!(params_problem(None, Some(&json!({"a": 1}))), "this routine takes no params");
        assert_eq!(params_problem(None, Some(&json!({}))), "");
        assert_eq!(params_summary(Some(&schema), &json!({"n": 1, "q": "acme"})), "q: acme · n: 1");
        assert_eq!(params_summary(None, &json!({"t": ["a", "b"]})), "t: a, b");
        assert_eq!(params_summary(Some(&schema), &json!({})), "");
    }

    #[test]
    fn numbers_emails_and_tool_names_read_as_javascript() {
        assert!(number(Some(&json!("abc"))).is_nan());
        assert_eq!(number(Some(&json!(" 90.6 "))), 90.6);
        assert_eq!(number(Some(&json!(null))), 0.0);
        assert_eq!(round(90.6), 91.0);
        assert!(is_email(Some(&json!(" owner@example.com "))));
        assert!(!is_email(Some(&json!("bob"))) && !is_email(Some(&json!("a@b."))) && !is_email(Some(&json!("a b@c.d"))));
        assert_eq!(mcp_name("mailfx", "get_gmail_messages_content_batch"), "mcp_mailfx_get_gmail_messages_content_batch");
        assert_eq!(mcp_name("a.b", "é"), "mcp_a_b__");
    }
}
