//! The tools' input checks (mcp.ts's `z.object(shape).strict().parse`),
//! driven by the JSON Schema zod made of each shape (tools.json), with
//! zod's issue messages, so an invalid call answers as Node's does.

use serde_json::{json, Map, Value};

const DATETIME: &str = r"^(?:(?:\d\d[2468][048]|\d\d[13579][26]|\d\d0[48]|[02468][048]00|[13579][26]00)-02-29|\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\d|30)|(?:02)-(?:0[1-9]|1\d|2[0-8])))T(?:(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z))$";

/// zod's `z.string().datetime()` (UTC, any precision).
pub fn iso_datetime(s: &str) -> bool {
    regex_lite::Regex::new(DATETIME).is_ok_and(|re| re.is_match(s))
}

fn kind(v: Option<&Value>) -> &'static str {
    match v {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn num(s: &Value, k: &str) -> Option<f64> {
    s.get(k).and_then(Value::as_f64)
}

fn issue(issues: &mut Vec<Value>, path: &[Value], message: String) {
    issues.push(json!({ "path": path, "message": message }));
}

/// Checks `v` against `schema` at `path`; returns the parsed value (with
/// defaults filled) and adds zod's issues.
fn check(schema: &Value, v: Option<&Value>, path: &mut Vec<Value>, issues: &mut Vec<Value>) -> Option<Value> {
    let ty = schema.get("type").and_then(Value::as_str).unwrap_or("");
    let v = match v {
        None if schema.get("default").is_some() => return Some(schema["default"].clone()),
        v => v,
    };
    let expected = if ty == "integer" { "number" } else { ty };
    let ok = match ty {
        "integer" => v.is_some_and(Value::is_number),
        "string" => v.is_some_and(Value::is_string),
        "boolean" => v.is_some_and(Value::is_boolean),
        "array" => v.is_some_and(Value::is_array),
        "object" => v.is_some_and(Value::is_object),
        _ => true,
    };
    if !ok {
        if let (Some(options), true) = (schema.get("enum").and_then(Value::as_array), v.is_some()) {
            let listed: Vec<String> = options.iter().map(crate::js::stringify).collect();
            issue(issues, path, format!("Invalid option: expected one of {}", listed.join("|")));
        } else {
            issue(issues, path, format!("Invalid input: expected {expected}, received {}", kind(v)));
        }
        return None;
    }
    let v = v.unwrap();
    match ty {
        "string" => {
            let s = v.as_str().unwrap();
            if let Some(options) = schema.get("enum").and_then(Value::as_array) {
                if !options.iter().any(|o| o == v) {
                    let listed: Vec<String> = options.iter().map(crate::js::stringify).collect();
                    issue(issues, path, format!("Invalid option: expected one of {}", listed.join("|")));
                }
                return Some(v.clone());
            }
            let len = s.encode_utf16().count() as f64;
            if let Some(min) = num(schema, "minLength").filter(|m| len < *m) {
                issue(issues, path, format!("Too small: expected string to have >={} characters", crate::js::number(min)));
            }
            if let Some(max) = num(schema, "maxLength").filter(|m| len > *m) {
                issue(issues, path, format!("Too big: expected string to have <={} characters", crate::js::number(max)));
            }
            if schema.get("format").and_then(Value::as_str) == Some("date-time") && !iso_datetime(s) {
                issue(issues, path, "Invalid ISO datetime".into());
            }
        }
        "integer" => {
            let n = v.as_f64().unwrap();
            if n.fract() != 0.0 {
                issue(issues, path, "Invalid input: expected int, received number".into());
                return None;
            }
            if let Some(min) = num(schema, "minimum").filter(|m| n < *m) {
                issue(issues, path, format!("Too small: expected number to be >={}", crate::js::number(min)));
            }
            if let Some(max) = num(schema, "maximum").filter(|m| n > *m) {
                issue(issues, path, format!("Too big: expected number to be <={}", crate::js::number(max)));
            }
        }
        "array" => {
            let items = v.as_array().unwrap();
            let mut out = Vec::new();
            for (i, item) in items.iter().enumerate() {
                path.push(json!(i));
                if let Some(x) = check(&schema["items"], Some(item), path, issues) {
                    out.push(x);
                }
                path.pop();
            }
            let len = items.len() as f64;
            if let Some(min) = num(schema, "minItems").filter(|m| len < *m) {
                issue(issues, path, format!("Too small: expected array to have >={} items", crate::js::number(min)));
            }
            if let Some(max) = num(schema, "maxItems").filter(|m| len > *m) {
                issue(issues, path, format!("Too big: expected array to have <={} items", crate::js::number(max)));
            }
            return Some(Value::Array(out));
        }
        "object" => {
            let given = v.as_object().unwrap();
            let required: Vec<&str> = schema.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let mut out = Map::new();
            for (key, sub) in schema.get("properties").and_then(Value::as_object).into_iter().flatten() {
                let present = given.get(key);
                if present.is_none() && !required.contains(&key.as_str()) && sub.get("default").is_none() {
                    continue;
                }
                path.push(json!(key));
                if let Some(x) = check(sub, present, path, issues) {
                    out.insert(key.clone(), x);
                }
                path.pop();
            }
            let props = schema.get("properties").and_then(Value::as_object);
            let unknown: Vec<&String> = given.keys().filter(|k| !props.is_some_and(|p| p.contains_key(*k))).collect();
            if !unknown.is_empty() {
                let quoted: Vec<String> = unknown.iter().map(|k| format!("\"{k}\"")).collect();
                issue(issues, path, format!("Unrecognized key{}: {}", if unknown.len() > 1 { "s" } else { "" }, quoted.join(", ")));
            }
            return Some(Value::Object(out));
        }
        _ => {}
    }
    Some(v.clone())
}

/// `schema.parse(args)`: the parsed arguments, or zod's issues.
pub fn parse(schema: &Value, args: &Value) -> Result<Value, Vec<Value>> {
    let mut issues = Vec::new();
    let out = check(schema, Some(args), &mut Vec::new(), &mut issues);
    if issues.is_empty() {
        Ok(out.unwrap_or(Value::Null))
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({"type":"object","properties":{"document":{"type":"string","minLength":1,"maxLength":128},"page":{"default":1,"type":"integer","minimum":1,"maximum":100000},"field":{"type":"string","enum":["displayText","formula"]},"date":{"type":"string","format":"date-time"},"edits":{"minItems":1,"type":"array","items":{"type":"object","properties":{"match":{"type":"string"}},"required":["match"],"additionalProperties":false}}},"required":["document","page"],"additionalProperties":false})
    }

    #[test]
    fn office_args_answer_as_zod_does() {
        assert_eq!(parse(&schema(), &json!({"document": "doc1"})).unwrap(), json!({"document": "doc1", "page": 1}));
        let issues = parse(&schema(), &json!({"page": 0, "x": 1})).unwrap_err();
        assert_eq!(
            issues,
            vec![
                json!({"path": ["document"], "message": "Invalid input: expected string, received undefined"}),
                json!({"path": ["page"], "message": "Too small: expected number to be >=1"}),
                json!({"path": [], "message": "Unrecognized key: \"x\""}),
            ]
        );
        let issues = parse(&schema(), &json!({"document": "", "page": 1.5, "field": "x", "date": "yesterday", "edits": [{"match": 1, "y": 2}]})).unwrap_err();
        assert_eq!(
            issues.iter().map(|i| i["message"].as_str().unwrap()).collect::<Vec<_>>(),
            vec![
                "Too small: expected string to have >=1 characters",
                "Invalid input: expected int, received number",
                "Invalid option: expected one of \"displayText\"|\"formula\"",
                "Invalid ISO datetime",
                "Invalid input: expected string, received number",
                "Unrecognized key: \"y\"",
            ]
        );
        assert_eq!(issues[4]["path"], json!(["edits", 0, "match"]));
        assert!(iso_datetime("2026-10-30T00:00:00Z") && iso_datetime("2026-10-30T00:00:00.123Z") && !iso_datetime("2026-10-30"));
    }
}
