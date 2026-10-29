//! What Node does to values on their way through, so a route answered here
//! reads byte for byte as Node's: `JSON.stringify` (numbers as JavaScript
//! prints them, integer keys first), `String(v)`, `trim`, `decodeURIComponent`
//! and `URLSearchParams.get`.

use serde_json::Value;

/// `JSON.stringify(v)`.
pub fn stringify(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v, None, 0);
    out
}

/// `JSON.stringify(v, null, 2)`.
pub fn stringify_pretty(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v, Some(2), 0);
    out
}

fn write(out: &mut String, v: &Value, indent: Option<usize>, depth: usize) {
    let newline = |out: &mut String, depth: usize| {
        if let Some(n) = indent {
            out.push('\n');
            out.push_str(&" ".repeat(n * depth));
        }
    };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => out.push_str(&quote(s)),
        Value::Array(items) => {
            if items.is_empty() {
                return out.push_str("[]");
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write(out, item, indent, depth + 1);
            }
            newline(out, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                return out.push_str("{}");
            }
            out.push('{');
            for (i, (k, item)) in ordered(map).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                out.push_str(&quote(k));
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write(out, item, indent, depth + 1);
            }
            newline(out, depth);
            out.push('}');
        }
    }
}

/// A JavaScript object's key order: array-index keys ascending, then the
/// rest as inserted.
pub fn ordered(map: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut index: Vec<(u32, (&String, &Value))> = map.iter().filter_map(|(k, v)| array_index(k).map(|n| (n, (k, v)))).collect();
    index.sort_by_key(|(n, _)| *n);
    index.into_iter().map(|(_, kv)| kv).chain(map.iter().filter(|(k, _)| array_index(k).is_none())).collect()
}

fn array_index(k: &str) -> Option<u32> {
    if k != "0" && (k.starts_with('0') || k.is_empty() || k.len() > 10 || !k.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    k.parse::<u64>().ok().filter(|n| *n < u32::MAX as u64).map(|n| n as u32)
}

/// A string as `JSON.stringify` quotes it (serde_json escapes the same set).
pub fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned())
}

/// `Number.prototype.toString()`, and `null` where JSON has no number.
pub fn number(f: f64) -> String {
    if !f.is_finite() {
        return "null".to_owned();
    }
    if f == 0.0 {
        return "0".to_owned();
    }
    // shortest round-trip digits, as JavaScript picks them
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
    let s = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        let head = if k == 1 { digits.clone() } else { format!("{}.{}", &digits[..1], &digits[1..]) };
        format!("{head}e{sign}{}", e.abs())
    };
    if f < 0.0 {
        format!("-{s}")
    } else {
        s
    }
}

/// `String(v)` of a JSON value.
pub fn string_of(v: &Value) -> String {
    match v {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(|i| if i.is_null() { String::new() } else { string_of(i) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// JavaScript truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// What `\s` and `trim()` count as space.
pub fn is_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `s.replace(/\s+/g, " ").trim()`.
pub fn squeeze(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if is_space(c) {
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

/// `s.slice(0, n)`: n UTF-16 units. A pair cut in half leaves its first
/// half, which SQLite then stores as U+FFFD (Node writes it so).
pub fn slice_utf16(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        let len = c.len_utf16();
        if units + len > n {
            if units < n {
                out.push('\u{fffd}');
            }
            break;
        }
        units += len;
        out.push(c);
    }
    out
}

/// `decodeURIComponent(s)`; `Err` where it throws ("URI malformed").
pub fn decode_uri_component(s: &str) -> Result<String, ()> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
            out.push(hex.ok_or(())?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

/// `new URL(url, base).searchParams.get(name)`: the first, `+` as a space.
pub fn search_param(path_and_query: &str, name: &str) -> Option<String> {
    let query = path_and_query.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or("");
    query.split('&').map(|pair| pair.split_once('=').unwrap_or((pair, ""))).find_map(|(k, v)| (form_decode(k) == name).then(|| form_decode(v)))
}

fn form_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => match b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(byte) => {
                    out.push(byte);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `new URL(url, base).pathname`, as far as a name in a path needs it: no
/// query or fragment, `\` as `/`, and `.` and `..` segments resolved.
pub fn pathname(path_and_query: &str) -> String {
    let path = path_and_query.split(['?', '#']).next().unwrap_or("").replace('\\', "/");
    let dot = |s: &str| s == "." || s.eq_ignore_ascii_case("%2e");
    let dotdot = |s: &str| matches!(s.to_ascii_lowercase().as_str(), ".." | ".%2e" | "%2e." | "%2e%2e");
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        if dotdot(seg) {
            out.pop();
            if last {
                out.push("");
            }
        } else if dot(seg) {
            if last {
                out.push("");
            }
        } else {
            out.push(seg);
        }
    }
    format!("/{}", out.join("/"))
}

/// `new Date(ms).toISOString()`
pub fn iso(ms: f64) -> String {
    let ms = ms as i64;
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // civil_from_days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_print_as_javascript_prints_them() {
        for (f, s) in [
            (1.0, "1"),
            (-0.0, "0"),
            (0.1, "0.1"),
            (123.456, "123.456"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (-1.5e-9, "-1.5e-9"),
            (12345678901234567890.0, "12345678901234567000"),
            (2.5e25, "2.5e+25"),
        ] {
            assert_eq!(number(f), s, "{f}");
        }
    }

    #[test]
    fn objects_put_array_index_keys_first() {
        let v: Value = serde_json::from_str(r#"{"b":1,"10":2,"2":3,"01":4,"4294967295":5,"a":[]}"#).unwrap();
        assert_eq!(stringify(&v), r#"{"2":3,"10":2,"b":1,"01":4,"4294967295":5,"a":[]}"#);
    }

    #[test]
    fn pretty_matches_two_space_json_stringify() {
        assert_eq!(stringify_pretty(&json!({"a": ["x"], "b": [], "c": {}})), "{\n  \"a\": [\n    \"x\"\n  ],\n  \"b\": [],\n  \"c\": {}\n}");
    }

    #[test]
    fn string_of_follows_javascript() {
        assert_eq!(string_of(&json!(null)), "null");
        assert_eq!(string_of(&json!([1, null, "a", [2, 3]])), "1,,a,2,3");
        assert_eq!(string_of(&json!({"a": 1})), "[object Object]");
        assert_eq!(string_of(&json!(5.0)), "5");
    }

    #[test]
    fn text_helpers() {
        assert_eq!(squeeze("  My\n \u{a0} chats \u{feff}"), "My chats");
        assert_eq!(squeeze("a\u{85}b"), "a\u{85}b");
        assert_eq!(slice_utf16("ab😀c", 3), "ab\u{fffd}");
        assert_eq!(slice_utf16("ab😀c", 4), "ab😀");
        assert_eq!(decode_uri_component("leaf%2Dd%C3%A9"), Ok("leaf-dé".to_owned()));
        assert!(decode_uri_component("%zz").is_err());
        assert!(decode_uri_component("%C3").is_err());
        assert_eq!(search_param("/state?x=1&name=a+b%21&name=c", "name").as_deref(), Some("a b!"));
        assert_eq!(search_param("/state?name=%zz", "name").as_deref(), Some("%zz"));
        assert_eq!(pathname("/state/./x?q"), "/state/x");
        assert_eq!(pathname("/state/.."), "/");
        assert_eq!(pathname("/state/."), "/state/");
        assert_eq!(pathname("/state\\x"), "/state/x");
    }

    #[test]
    fn truthiness() {
        for v in [json!(null), json!(false), json!(0), json!(""), json!(-0.0)] {
            assert!(!truthy(&v), "{v}");
        }
        for v in [json!("0"), json!([]), json!({}), json!(1)] {
            assert!(truthy(&v), "{v}");
        }
    }
}
