//! A routine's `params` schema (spec 19.2; `v1/params.mjs`): the settings
//! one instance of a routine runs with, as a small JSON Schema. The
//! manifest check needs [`schema_problem`]; instances come with routines.

use serde_json::Value;

use super::get;
use crate::js;

pub const PARAMS_MAX_BYTES: usize = 4096;

const SCALARS: &[&str] = &["string", "integer", "number", "boolean"];
const KEYS: &[&str] = &["type", "description", "enum", "maxLength", "items", "maxItems"];

/// `^[a-z][a-z0-9_]{0,39}$`.
fn name_ok(n: &str) -> bool {
    let mut cs = n.chars();
    n.len() <= 40 && cs.next().is_some_and(|c| c.is_ascii_lowercase()) && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `Number.isInteger(v)`.
fn integer(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0)
}

/// Why `schema` is no params schema (`""` when it is one).
pub fn schema_problem(schema: &Value) -> String {
    let props = get(schema, "properties").and_then(Value::as_object);
    let (Some(o), Some(props)) = (schema.as_object().filter(|_| get(schema, "type") == Some(&Value::from("object"))), props) else {
        return "must be {type: \"object\", properties: {...}}".to_owned();
    };
    for (k, _) in js::ordered(o) {
        if !["type", "properties", "required", "additionalProperties", "description"].contains(&k.as_str()) {
            return format!("has unknown key {k}");
        }
    }
    if o.get("additionalProperties").is_some_and(|a| *a != Value::Bool(false)) {
        return "additionalProperties may only be false".to_owned();
    }
    let names: Vec<&String> = js::ordered(props).into_iter().map(|(k, _)| k).collect();
    if names.is_empty() || names.len() > 16 {
        return "declares 1 to 16 properties".to_owned();
    }
    for n in &names {
        if !name_ok(n) {
            return format!("property {} is not a lower snake case name", js::slice_utf16(n, 40));
        }
        let why = prop_problem(Some(&props[*n]), true);
        if !why.is_empty() {
            return format!("property {n} {why}");
        }
    }
    if let Some(required) = o.get("required") {
        let ok = required.as_array().is_some_and(|r| r.iter().all(|r| r.as_str().is_some_and(|r| names.iter().any(|n| *n == r))));
        if !ok {
            return "required names only declared properties".to_owned();
        }
    }
    if js::stringify(schema).encode_utf16().count() > PARAMS_MAX_BYTES {
        return format!("is over {PARAMS_MAX_BYTES} bytes");
    }
    String::new()
}

fn prop_problem(p: Option<&Value>, arrays: bool) -> String {
    let Some(o) = p.and_then(Value::as_object) else { return "must be an object".to_owned() };
    for (k, _) in js::ordered(o) {
        if !KEYS.contains(&k.as_str()) {
            return format!("has unknown key {k}");
        }
    }
    if o.get("description").is_some_and(|d| !d.is_string()) {
        return "description must be a string".to_owned();
    }
    let ty = o.get("type").and_then(Value::as_str);
    if ty == Some("array") {
        if !arrays {
            return "may not nest arrays".to_owned();
        }
        if o.contains_key("maxItems") && !integer(o.get("maxItems")).is_some_and(|n| n > 0.0) {
            return "maxItems must be a positive integer".to_owned();
        }
        let why = prop_problem(o.get("items"), false);
        return if why.is_empty() { why } else { format!("items {why}") };
    }
    let Some(ty) = ty.filter(|t| SCALARS.contains(t)) else { return "type must be string, integer, number, boolean or array".to_owned() };
    if o.contains_key("items") || o.contains_key("maxItems") {
        return "has items but is no array".to_owned();
    }
    if let Some(e) = o.get("enum") {
        let ok = e.as_array().is_some_and(|e| !e.is_empty() && e.iter().all(|v| scalar_ok(ty, v)));
        if !ok {
            return format!("enum must list {ty} values");
        }
    }
    if o.contains_key("maxLength") && !(ty == "string" && integer(o.get("maxLength")).is_some_and(|n| n > 0.0)) {
        return "maxLength is for strings, a positive integer".to_owned();
    }
    String::new()
}

/// `valueProblem({type}, v) === ""` for a scalar type.
fn scalar_ok(ty: &str, v: &Value) -> bool {
    match ty {
        "string" => v.is_string(),
        "integer" => integer(Some(v)).is_some(),
        "number" => v.as_f64().is_some_and(f64::is_finite),
        "boolean" => v.is_boolean(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::schema_problem;
    use serde_json::json;

    #[test]
    fn a_params_schema_is_a_flat_object_of_named_scalars_and_lists() {
        let ok = json!({"type": "object", "properties": {"taxpayers": {"type": "array", "items": {"type": "string"}, "maxItems": 20}}, "required": ["taxpayers"]});
        assert_eq!(schema_problem(&ok), "");
        for (schema, why) in [
            (json!({"type": "array"}), "must be {type: \"object\", properties: {...}}"),
            (json!({"type": "object", "properties": {}}), "declares 1 to 16 properties"),
            (json!({"type": "object", "properties": {"T": {"type": "string"}}}), "property T is not a lower snake case name"),
            (json!({"type": "object", "properties": {"t": {"type": "object"}}}), "property t type must be string, integer, number, boolean or array"),
            (json!({"type": "object", "properties": {"t": {"type": "array", "items": {"type": "array"}}}}), "property t items may not nest arrays"),
            (json!({"type": "object", "properties": {"t": {"type": "string", "enum": [1]}}}), "property t enum must list string values"),
            (json!({"type": "object", "properties": {"t": {"type": "string"}}, "required": ["u"]}), "required names only declared properties"),
            (json!({"type": "object", "properties": {"t": {"type": "string"}}, "x": 1}), "has unknown key x"),
        ] {
            assert_eq!(schema_problem(&schema), why);
        }
    }
}
