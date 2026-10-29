//! Mini App Protocol v1 hosting (plan phase 3), ported from
//! `server/src/v1`: manifests and trust so far. Values arrive as JSON and
//! are judged as Node's v1 code judges them, down to the message text, so
//! a plugin author sees the same refusal from either broker
//! (`server/test/v1-rust.mjs` runs both on every case).

pub mod cli;
pub mod manifest;
pub mod params;
pub mod signing;
pub mod trust;

use rox_v1::ids;
use serde_json::{json, Value};

use crate::js;

/// A refusal with the spec's code (14.1; `v1/errors.mjs`).
#[derive(Debug, Clone, PartialEq)]
pub struct V1Error {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

impl V1Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), details: None }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// `refused` with code `code` and `message`; for `?` in a host call.
    pub fn err<T>(code: impl Into<String>, message: impl Into<String>) -> Result<T, Self> {
        Err(Self::new(code, message))
    }

    /// Whether the spec's table calls the code retryable.
    pub fn retryable(&self) -> bool {
        matches!(self.code.as_str(), "rate_limited" | "timeout" | "plugin_unavailable" | "plugin_restarted" | "internal")
    }

    /// The wire form: `{ok: false, code, message, retryable}`.
    pub fn body(&self) -> Value {
        let mut body = json!({"ok": false, "code": self.code, "message": js::slice_utf16(&self.message, 500), "retryable": self.retryable()});
        if let Some(d) = &self.details {
            body["details"] = d.clone();
        }
        body
    }
}


/// Whether `s` is a whole `kind` name of the v1 grammar table
/// (`grammar.mjs` `V1`, spec 3).
pub fn grammar_ok(kind: &str, s: &str) -> bool {
    let tail = |s: &str, first: fn(char) -> bool, rest: fn(char) -> bool, max: usize| {
        let mut cs = s.chars();
        s.len() <= max && cs.next().is_some_and(first) && cs.all(rest)
    };
    let lower_digit_dash = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    match kind {
        "pluginId" => ids::plugin_id_ok(s),
        "short" => ids::short_ok(s),
        "appId" | "routineId" => ids::app_id_ok(s),
        "command" | "scope" => ids::scope_ok(s),
        "tool" => ids::tool_ok(s),
        "propKey" => ids::prop_key_ok(s),
        "ticket" => ids::ticket_ok(s),
        "reservedTool" => s.strip_prefix("rox.").is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_lowercase() || c == '.')),
        // ^[a-z][a-z0-9-]{1,62}(\.[a-z0-9-]{1,62})*$
        "publisherId" => {
            let mut labels = s.split('.');
            let first = labels.next().unwrap_or("");
            first.len() >= 2
                && tail(first, |c| c.is_ascii_lowercase(), lower_digit_dash, 63)
                && labels.all(|l| (1..=62).contains(&l.len()) && l.chars().all(lower_digit_dash))
        }
        // ^[a-z0-9][a-z0-9-]{0,63}$
        "keyId" => tail(s, |c| c.is_ascii_lowercase() || c.is_ascii_digit(), lower_digit_dash, 64),
        _ => false,
    }
}

/// `isV1(kind, v)`: the grammar over `String(v ?? "")`, as Node checks it.
pub fn is_v1(kind: &str, v: Option<&Value>) -> bool {
    let s = match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => js::string_of(v),
    };
    grammar_ok(kind, &s) && (kind != "pluginId" || s.encode_utf16().count() <= 255)
}

/// `v[key]` of an object; `None` (undefined) for anything else.
pub fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(key))
}

/// `${v}` in a template literal.
pub fn tpl(v: Option<&Value>) -> String {
    v.map_or_else(|| "undefined".to_owned(), js::string_of)
}

/// `JSON.stringify(v)?.slice(0, n)`.
pub fn shown(v: Option<&Value>, n: usize) -> String {
    v.map_or_else(|| "undefined".to_owned(), |v| js::slice_utf16(&js::stringify(v), n))
}

/// `kindOf(v)` (`v1/jcs.mjs`).
pub fn kind_of(v: Option<&Value>) -> &'static str {
    match v {
        None => "other",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

/// The value when `Number.isSafeInteger(v)`.
pub fn safe_int(v: Option<&Value>) -> Option<f64> {
    let f = v?.as_f64()?;
    (f.is_finite() && f.fract() == 0.0 && f.abs() <= MAX_SAFE).then_some(f)
}

/// `a === b` for values from parsed JSON: primitives by value, arrays and
/// objects never (each parse makes its own), `undefined === undefined`.
pub fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Array(_) | Value::Object(_)), _) | (_, Some(Value::Array(_) | Value::Object(_))) => false,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// `fromB64u(s, len)` (`v1/ed.mjs`): base64url of exactly `len` bytes in
/// its one canonical form.
pub fn from_b64u(v: Option<&Value>, len: usize) -> Option<Vec<u8>> {
    let Some(Value::String(s)) = v else { return None };
    let bytes = rox_v1::keys::unb64(s).ok()?;
    (bytes.len() == len && rox_v1::keys::b64(&bytes) == *s).then_some(bytes)
}

/// `canonicalize(v)` (`v1/jcs.mjs`): RFC 8785 text, any finite number.
pub fn canonicalize(v: &Value) -> String {
    match v {
        Value::Array(items) => format!("[{}]", items.iter().map(canonicalize).collect::<Vec<_>>().join(",")),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            format!("{{{}}}", keys.iter().map(|k| format!("{}:{}", js::quote(k), canonicalize(&o[*k]))).collect::<Vec<_>>().join(","))
        }
        v => js::stringify(v),
    }
}

/// `signedBytes(v)` (`v1/jcs.mjs`): JCS bytes of a value holding safe
/// integers only, refused with Node's message at the first other number.
pub fn signed_bytes(v: &Value) -> Result<Vec<u8>, String> {
    fn first_unsafe(v: &Value) -> Option<f64> {
        match v {
            Value::Number(n) => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                safe_int(Some(v)).is_none().then_some(f)
            }
            Value::Array(items) => items.iter().find_map(first_unsafe),
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                keys.into_iter().find_map(|k| first_unsafe(&o[k]))
            }
            _ => None,
        }
    }
    if let Some(n) = first_unsafe(v) {
        return Err(format!("signed objects carry safe integers only, not {}", js::number(n)));
    }
    rox_v1::jcs::signed_bytes(v)
}

/// `verify(pub, bytes, sig)` (`v1/ed.mjs`): malformed keys or signatures
/// are a plain false.
pub fn ed_verify(public: &str, bytes: &[u8], sig: &str) -> bool {
    use ed25519_dalek::Verifier as _;
    let (Some(public), Some(sig)) = (from_b64u(Some(&json!(public)), 32), from_b64u(Some(&json!(sig)), 64)) else { return false };
    let (Ok(public), Ok(sig)) = (<[u8; 32]>::try_from(public), <[u8; 64]>::try_from(sig)) else { return false };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&public) else { return false };
    key.verify(bytes, &ed25519_dalek::Signature::from_bytes(&sig)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // grammar.mjs's table: one good and one bad value per kind (MAN-07)
    #[test]
    fn every_grammar_takes_its_good_value_and_refuses_its_bad_one() {
        for (kind, good, bad) in [
            ("pluginId", "dev.goldcoders.bir", "dev.goldcoders"),
            ("short", "bir", "b"),
            ("appId", "tax-profile", "Tax"),
            ("command", "taxform", "tax_form"),
            ("scope", "branchcode", "-x"),
            ("routineId", "bir-confirmations", "9lives"),
            ("tool", "bir_resolve", "bir-resolve"),
            ("propKey", "ticket", "_x"),
            ("ticket", "tkt_0123456789ABCDEFGHJKMNPQRS", "tkt_0123456789ABCDEFGHIJKLMN"),
            ("reservedTool", "rox.session.open", "rox_session"),
            ("publisherId", "dev.goldcoders", "Dev"),
            ("keyId", "gc-2026-01", "-gc"),
        ] {
            assert!(grammar_ok(kind, good), "{kind} {good}");
            assert!(!grammar_ok(kind, bad), "{kind} {bad}");
        }
        assert!(grammar_ok("publisherId", "ab.c"));
        assert!(!grammar_ok("publisherId", "a.cd"));
        assert!(grammar_ok("keyId", &"k".repeat(64)) && !grammar_ok("keyId", &"k".repeat(65)));
        assert!(!grammar_ok("reservedTool", "rox."));
    }

    #[test]
    fn names_are_checked_on_their_string_form_as_node_does() {
        assert!(is_v1("command", Some(&json!(["board"]))));
        assert!(!is_v1("command", None));
        assert!(!is_v1("command", Some(&Value::Null)));
        assert!(!is_v1("pluginId", Some(&json!(format!("{}.b.c", "a".repeat(254))))));
    }

    #[test]
    fn signed_bytes_refuse_the_first_unsafe_number_in_jcs_order() {
        assert_eq!(signed_bytes(&json!({"b": 1.5, "a": 2.5})).unwrap_err(), "signed objects carry safe integers only, not 2.5");
        assert_eq!(signed_bytes(&json!({"a": 1e21})).unwrap_err(), "signed objects carry safe integers only, not 1e+21");
        assert_eq!(signed_bytes(&json!({"a": 1.0})).unwrap(), b"{\"a\":1}");
        assert_eq!(canonicalize(&json!({"b": 0.5, "a": [1, "x"]})), "{\"a\":[1,\"x\"],\"b\":0.5}");
    }
}
