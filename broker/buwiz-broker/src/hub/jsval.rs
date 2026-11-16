//! JSON values read the way JavaScript reads them: a member of what may not be an object,
//! truthiness, `trim()`, and the message `JSON.parse` fails with. Shared by the routes and
//! the data sources that answer in those shapes.

use serde_json::Value;

use crate::js;

/// `b.<name>` of a parsed body; `Err` with the TypeError Node throws when
/// the body is `null`.
pub fn field<'a>(b: &'a Value, name: &str) -> Result<Option<&'a Value>, String> {
    match b {
        Value::Null => Err(format!("Cannot read properties of null (reading '{name}')")),
        Value::Object(m) => Ok(m.get(name)),
        _ => Ok(None),
    }
}

/// `a?.b`: the member of an object, else nothing.
pub fn get<'a>(v: Option<&'a Value>, name: &str) -> Option<&'a Value> {
    v.and_then(|v| v.as_object()).and_then(|m| m.get(name))
}

pub fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(js::truthy)
}

/// `s.trim()`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(js::is_space)
}

/// V8's SyntaxError message for `JSON.parse(src)`, for the shapes of
/// error serde reports (V8 shortens a long source; this does not).
pub fn parse_error(src: &str, e: &serde_json::Error) -> String {
    if e.is_eof() {
        return "Unexpected end of JSON input".to_owned();
    }
    // serde's line and column (1-based) of the character it stopped at
    let line_start: usize = src.split_inclusive('\n').take(e.line().saturating_sub(1)).map(str::len).sum();
    let line = &src[line_start..];
    let ch = line.chars().nth(e.column().saturating_sub(1));
    let pos = src[..line_start].encode_utf16().count() + line.chars().take(e.column().saturating_sub(1)).map(char::len_utf16).sum::<usize>();
    let at = format!("in JSON at position {pos} (line {} column {})", e.line(), e.column());
    let msg = e.to_string();
    if msg.starts_with("trailing characters") {
        return format!("Unexpected non-whitespace character after JSON at position {pos} (line {} column {})", e.line(), e.column());
    }
    if msg.starts_with("key must be a string") {
        return format!("Expected property name or '}}' {at}");
    }
    match ch {
        Some(c) => format!("Unexpected token '{c}', \"{src}\" is not valid JSON"),
        None => "Unexpected end of JSON input".to_owned(),
    }
}
