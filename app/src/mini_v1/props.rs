//! `buwiz.props` checked before an app mounts (spec 6.6): the limits on any
//! props object, then the app's `apps[].props` schema in the JSON Schema
//! subset the manifest allows. Props that fail are not opened; the host
//! adds nothing to them.
//!
//! An app that declares no `props` schema takes none: only `{}` passes.
//! Properties a schema does not list are allowed, as in JSON Schema.

use serde_json::{Map, Value};

use super::RoxError;

/// 8 KiB serialized.
pub const MAX_BYTES: usize = 8 * 1024;
pub const MAX_DEPTH: usize = 8;
pub const MAX_ITEMS: usize = 100;
/// The largest `maxLength` a schema may set.
const MAX_LENGTH_CAP: u64 = 512;
/// The most properties a schema lists (Appendix A).
const MAX_PROPERTIES: usize = 16;

fn invalid(m: impl Into<String>) -> RoxError {
    RoxError::new("invalid_args", m)
}

/// Checks `props` against the limits and `schema` (the app's
/// `apps[].props`, `None` when it declares none).
pub fn validate(props: &Map<String, Value>, schema: Option<&Value>) -> Result<(), RoxError> {
    let value = Value::Object(props.clone());
    if value.to_string().len() > MAX_BYTES {
        return Err(invalid("props are over 8 KiB"));
    }
    limits(&value, 1)?;
    match schema {
        Some(schema) => {
            check_schema(schema, 0)?;
            matches(&value, schema, "props")
        }
        None if props.is_empty() => Ok(()),
        None => Err(invalid("this app takes no props")),
    }
}

/// Depth (an object or array is one level), keys and array lengths.
fn limits(v: &Value, depth: usize) -> Result<(), RoxError> {
    match v {
        Value::Object(m) => {
            if depth > MAX_DEPTH {
                return Err(invalid("props are nested deeper than 8"));
            }
            for (k, x) in m {
                if !super::perms::prop_key_ok(k) {
                    return Err(invalid(format!("bad prop key {k:?}")));
                }
                limits(x, depth + 1)?;
            }
        }
        Value::Array(items) => {
            if depth > MAX_DEPTH {
                return Err(invalid("props are nested deeper than 8"));
            }
            if items.len() > MAX_ITEMS {
                return Err(invalid("a props array has more than 100 items"));
            }
            for x in items {
                limits(x, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether `schema` uses only the subset (6.6); anything else is refused,
/// as the broker refuses it at verification.
pub fn check_schema(schema: &Value, depth: usize) -> Result<(), RoxError> {
    let bad = |m: &str| Err(invalid(format!("props schema: {m}")));
    let Some(o) = schema.as_object() else { return bad("a schema is an object") };
    if depth > MAX_DEPTH {
        return bad("nested too deep");
    }
    for (k, v) in o {
        match k.as_str() {
            "type" => {
                let ok = v.as_str().is_some_and(|t| {
                    ["object", "array", "string", "number", "integer", "boolean", "null"].contains(&t)
                });
                if !ok {
                    return bad("unknown type");
                }
            }
            "properties" => {
                let Some(ps) = v.as_object() else { return bad("properties is an object") };
                if ps.len() > MAX_PROPERTIES {
                    return bad("more than 16 properties");
                }
                for (name, s) in ps {
                    if !super::perms::prop_key_ok(name) {
                        return bad("bad property name");
                    }
                    check_schema(s, depth + 1)?;
                }
            }
            "required" => {
                if !v.as_array().is_some_and(|r| r.iter().all(Value::is_string)) {
                    return bad("required is a list of names");
                }
            }
            "items" => check_schema(v, depth + 1)?,
            "enum" => {
                if !v.is_array() {
                    return bad("enum is a list");
                }
            }
            "maxLength" => {
                if !v.as_u64().is_some_and(|n| n <= MAX_LENGTH_CAP) {
                    return bad("maxLength is at most 512");
                }
            }
            "minimum" | "maximum" => {
                if !v.is_number() {
                    return bad("minimum and maximum are numbers");
                }
            }
            other => return bad(&format!("{other} is not in the subset")),
        }
    }
    Ok(())
}

/// Whether `v` (at `at`) matches `schema`.
fn matches(v: &Value, schema: &Value, at: &str) -> Result<(), RoxError> {
    let fail = |m: &str| Err(invalid(format!("{at}: {m}")));
    if let Some(t) = schema.get("type").and_then(Value::as_str)
        && !of_type(v, t)
    {
        return fail(&format!("is not of type {t}"));
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array)
        && !choices.contains(v)
    {
        return fail("is not one of the allowed values");
    }
    if let (Some(max), Some(s)) = (schema.get("maxLength").and_then(Value::as_u64), v.as_str())
        && s.encode_utf16().count() as u64 > max
    {
        return fail(&format!("is longer than {max}"));
    }
    if let Some(n) = v.as_f64() {
        if schema.get("minimum").and_then(Value::as_f64).is_some_and(|min| n < min) {
            return fail("is under the minimum");
        }
        if schema.get("maximum").and_then(Value::as_f64).is_some_and(|max| n > max) {
            return fail("is over the maximum");
        }
    }
    if let Some(o) = v.as_object() {
        for name in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
            if !o.contains_key(name.as_str().unwrap_or_default()) {
                return fail(&format!("needs {name}"));
            }
        }
        if let Some(ps) = schema.get("properties").and_then(Value::as_object) {
            for (k, x) in o {
                if let Some(s) = ps.get(k) {
                    matches(x, s, &format!("{at}.{k}"))?;
                }
            }
        }
    }
    if let (Some(items), Some(s)) = (v.as_array(), schema.get("items")) {
        for (i, x) in items.iter().enumerate() {
            matches(x, s, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

fn of_type(v: &Value, t: &str) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    /// The fixture board's schema.
    fn board() -> Value {
        json!({ "type": "object", "properties": {
            "ticket": { "type": "string", "maxLength": 30 },
            "page": { "type": "integer", "minimum": 1, "maximum": 99 } } })
    }

    #[test]
    fn app_05_props_are_validated_against_the_apps_schema() {
        let s = board();
        validate(&obj(json!({"ticket": "tkt_x", "page": 3})), Some(&s)).unwrap();
        validate(&obj(json!({})), Some(&s)).unwrap();
        for bad in [
            json!({"page": 0}),
            json!({"page": 100}),
            json!({"page": 1.5}),
            json!({"page": "3"}),
            json!({"ticket": "x".repeat(31)}),
        ] {
            assert_eq!(validate(&obj(bad.clone()), Some(&s)).unwrap_err().code, "invalid_args", "{bad}");
        }
        let card = json!({ "type": "object", "required": ["ticket"],
            "properties": { "ticket": { "type": "string" }, "mode": { "enum": ["a", "b"] } } });
        assert!(validate(&obj(json!({"label": "x"})), Some(&card)).is_err(), "required");
        assert!(validate(&obj(json!({"ticket": "t", "mode": "c"})), Some(&card)).is_err(), "enum");
        validate(&obj(json!({"ticket": "t", "mode": "a"})), Some(&card)).unwrap();
        // no schema: no props
        validate(&obj(json!({})), None).unwrap();
        assert!(validate(&obj(json!({"a": 1})), None).is_err());
        // a schema outside the subset is refused
        let wide = json!({"type": "object", "pattern": "x"});
        assert!(validate(&obj(json!({})), Some(&wide)).is_err());
        assert!(validate(&obj(json!({})), Some(&json!({"maxLength": 513}))).is_err());
    }

    #[test]
    fn app_06_props_over_8kib_depth_9_or_101_items_are_refused() {
        let open = json!({"type": "object"});
        // 8 KiB is the limit: at it passes, one over is refused
        let pad = MAX_BYTES - r#"{"a":""}"#.len();
        validate(&obj(json!({"a": "x".repeat(pad)})), Some(&open)).unwrap();
        assert!(validate(&obj(json!({"a": "x".repeat(pad + 1)})), Some(&open)).is_err());
        // depth 8 passes, 9 is refused
        let nest = |n: usize| (1..n).fold(json!({}), |inner, _| json!({ "a": inner }));
        validate(&obj(nest(8)), Some(&open)).unwrap();
        assert!(validate(&obj(nest(9)), Some(&open)).is_err());
        // 100 array items pass, 101 are refused
        validate(&obj(json!({"a": vec![0; 100]})), Some(&open)).unwrap();
        assert!(validate(&obj(json!({"a": vec![0; 101]})), Some(&open)).is_err());
        // keys follow the prop-key grammar at every level
        assert!(validate(&obj(json!({"a": {"bad-key": 1}})), Some(&open)).is_err());
    }
}
